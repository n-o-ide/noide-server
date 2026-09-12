//! Manages one long-lived `serve` process per supported agent (kilo / opencode).
//!
//! Both CLIs share the same client-server architecture:
//!   * `<agent> serve`            - a headless, heavy server (Node runtime,
//!     providers, model/gateway init). Booting it is the expensive part of a
//!     fresh `run` (~seconds).
//!   * `<agent> run --attach URL` - a thin client that forwards a single
//!     prompt to a running server and streams NDJSON events back.
//!
//! Previously every chat message spawned a brand-new `<agent> run`, which
//! re-paid the full server boot each turn (~20s+ total with the gateway wait).
//! By keeping one server alive per agent and turning every turn into a thin
//! `run --attach`, the boot cost is paid once and reused across all messages
//! (measured steady-state time-to-first-event drops from ~23s to ~5s).
//!
//! "New chat" in the frontend clears the session id but NOT the process: a new
//! session is created on the already-running server, so a new chat only pays
//! the cheap session creation, never a process boot.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, BufReader};

use crate::ws_server::ChatProcessTracker;

/// A running `<agent> serve` process we own so subsequent turns can attach to
/// it instead of booting a fresh server.
struct ServerHandle {
    port: u16,
    pid: u32,
}

impl ServerHandle {
    /// True if the process is still running. On Unix we probe with a zero
    /// signal; on other platforms we optimistically assume it's alive (the
    /// per-message `run --attach` will surface any real failure quickly).
    fn alive(&self) -> bool {
        #[cfg(unix)]
        {
            // kill(pid, 0) returns 0 if the process exists (no signal sent).
            unsafe { libc::kill(self.pid as i32, 0) == 0 }
        }
        #[cfg(not(unix))]
        {
            true
        }
    }
}

pub struct AgentServerManager {
    servers: Mutex<HashMap<String, ServerHandle>>,
    tracker: Arc<ChatProcessTracker>,
}

impl AgentServerManager {
    pub fn new(tracker: Arc<ChatProcessTracker>) -> Self {
        Self {
            servers: Mutex::new(HashMap::new()),
            tracker,
        }
    }

    /// Return the port of a running `<agent> serve`, starting one on first use.
    ///
    /// Once started for an agent, the same server is reused for every later
    /// message of that agent (persistent session). `api_key` seeds the server
    /// environment on first boot only; later calls ignore it (first boot wins,
    /// which matches the serialized single-user chat panel).
    pub async fn ensure_server(&self, agent: &str, api_key: &str) -> Result<u16, String> {
        {
            let guard = self.servers.lock().unwrap();
            if let Some(h) = guard.get(agent) {
                if h.alive() {
                    return Ok(h.port);
                }
                // Server died since we started it — drop it and respawn below.
                eprintln!("[NoIDE] {} server no longer alive; respawning", agent);
            }
        }
        self.servers.lock().unwrap().remove(agent);

        let exe = crate::commands::resolve_agent_bin(agent)
            .ok_or_else(|| crate::commands::cli_missing_message(agent))?;

        let mut cmd = tokio::process::Command::new(&exe);
        // --port 0 lets the OS pick a free port; we discover it by parsing the
        // server's "listening on http://127.0.0.1:<port>" banner line.
        cmd.arg("serve")
            .arg("--port")
            .arg("0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // Own process group so a later kill can tear down the whole server tree.
        #[cfg(unix)]
        {
            cmd.process_group(0);
        }
        if !api_key.trim().is_empty() {
            cmd.env("OPENROUTER_API_KEY", api_key);
        }
        // Run the agent server non-interactively and color-free so captured
        // tool output is clean, deterministic and can never hang:
        //   * NO_COLOR/CLICOLOR/TERM=dumb – no ANSI escapes (incl. termcolor)
        //   * CI=1                        – terse output (no spinners/banners)
        //   * C.UTF-8 + PYTHONIOENCODING   – deterministic UTF-8, no Python
        //                                    UnicodeEncodeError crashes
        //   * PAGER=cat                    – an agent command can never block
        //                                    on an interactive pager
        // The `serve` process executes the agent's tools, so these propagate
        // to every command it spawns. FORCE_COLOR is removed so nothing can
        // re-enable color.
        cmd.env("NO_COLOR", "1");
        cmd.env("CLICOLOR", "0");
        cmd.env("TERM", "dumb");
        cmd.env("CI", "1");
        cmd.env("LANG", "C.UTF-8");
        cmd.env("LC_ALL", "C.UTF-8");
        cmd.env("PYTHONIOENCODING", "utf-8");
        cmd.env("GIT_PAGER", "cat");
        cmd.env("PAGER", "cat");
        cmd.env_remove("FORCE_COLOR");
        cmd.env_remove("CLICOLOR_FORCE");

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to start {} server: {}", agent, e))?;
        let pid = child
            .id()
            .ok_or_else(|| format!("{} server spawned without a pid", agent))?;
        // Track so the ctrlc handler (via ChatProcessTracker::kill_all) reaps
        // the server on shutdown instead of leaking it as an orphan.
        self.tracker.register(pid);

        // Read the server banner to discover its port, then keep draining the
        // pipe so the server never gets SIGPIPE if it logs more lines.
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| format!("{} server stdout unavailable", agent))?;
        let mut reader = BufReader::new(stdout);
        let mut port: Option<u16> = None;
        let mut buf = String::new();
        // Give the server a generous window to print its listening banner.
        for _ in 0..40 {
            buf.clear();
            let n = reader
                .read_line(&mut buf)
                .await
                .map_err(|e| format!("reading {} server output: {}", agent, e))?;
            if n == 0 {
                break;
            }
            if let Some(p) = extract_port(&buf) {
                port = Some(p);
                break;
            }
        }

        let port = port.ok_or_else(|| {
            // Reap the server we just spawned since we can't use it.
            self.tracker.unregister(pid);
            let _ = tokio::process::Command::new("kill")
                .arg("-9")
                .arg(pid.to_string())
                .spawn();
            format!("could not determine {} server port from its output", agent)
        })?;

        // Keep the pipe open + drained so the server doesn't SIGPIPE later.
        tokio::spawn(async move {
            let mut reader = reader;
            let mut line = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                    break;
                }
            }
        });

        self.servers
            .lock()
            .unwrap()
            .insert(agent.to_string(), ServerHandle { port, pid });
        Ok(port)
    }
}

/// Extract the port from a `<agent> serve` banner line such as
/// `kilo server listening on http://127.0.0.1:4096`.
fn extract_port(line: &str) -> Option<u16> {
    let marker = "127.0.0.1:";
    let start = line.find(marker)? + marker.len();
    let rest = &line[start..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}
