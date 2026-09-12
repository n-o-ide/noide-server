mod agent_servers;
mod cloudflare;
mod commands;
mod pairing;
mod pty;
mod ws_server;

use crate::agent_servers::AgentServerManager;
use std::sync::{Arc, Mutex};

/// Best-effort LAN IP detection: bind a UDP socket (no traffic is sent) and
/// read the local address the OS would route through for a public endpoint.
fn detect_lan_ip() -> Option<String> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    sock.local_addr().ok().map(|a| a.ip().to_string())
}

fn main() {
    let addr = std::env::var("NOTERM_WS_ADDR").unwrap_or_else(|_| "0.0.0.0:1421".to_string());

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("noide-server {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!(
            "{}",
            concat!(
                "noide-server ",
                env!("CARGO_PKG_VERSION"),
                " — NoIDE WebSocket backend\n\n",
                "USAGE:\n",
                "    noide-server [OPTIONS]\n\n",
                "OPTIONS:\n",
                "    --token <value>    Require this fixed token (or NOIDE_TOKEN env)\n",
                "    --no-auth          Accept unauthenticated connections (dev only)\n",
                "    --no-cloudflare    Disable automatic Cloudflare tunnel\n",
                "    --version, -V      Print version and exit\n",
                "    --help, -h         Print this help\n\n",
                "ENVIRONMENT:\n",
                "    NOTERM_WS_ADDR      Bind address and port (default 0.0.0.0:1421)\n",
                "    NOIDE_TOKEN         Fixed token (same as --token)\n",
                "    NOIDE_PTY_KEEP_ALIVE  Seconds an unattached terminal session is kept\n",
                "                          before reaping (default 1800)\n",
            )
        );
        return;
    }

    // Authentication modes (highest precedence wins):
    //   1. --no-auth            -> accept unauthenticated connections (dev)
    //   2. --token <value> / NOIDE_TOKEN -> require that fixed token
    //   3. (default)            -> pairing: generate an ephemeral xxxx-xxxx
    //                              code + QR, printed once at startup, valid
    //                              until this process exits.
    let mut token: Option<String> = std::env::var("NOIDE_TOKEN")
        .ok()
        .filter(|t| !t.trim().is_empty());
    let mut no_auth = false;
    let mut no_cloudflare = false;
    {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--token" => {
                    if let Some(v) = args.get(i + 1) {
                        if !v.trim().is_empty() {
                            token = Some(v.clone());
                        }
                        i += 2;
                        continue;
                    }
                }
                "--no-auth" => no_auth = true,
                "--no-cloudflare" => no_cloudflare = true,
                _ => {}
            }
            if let Some(v) = args[i].strip_prefix("--token=") {
                if !v.trim().is_empty() {
                    token = Some(v.to_string());
                }
            }
            i += 1;
        }
    }

    if no_auth {
        if token.is_some() {
            eprintln!("[NoIDE] WARNING: --no-auth overrides the configured token.");
        }
        token = None;
        eprintln!(
            "[NoIDE] WARNING: --no-auth — the server is accepting UNAUTHENTICATED WebSocket connections.\n    Anyone who can reach this port can read/write files and run shells.\n    Remove --no-auth to require pairing (default)."
        );
        // No pairing code exists in this mode, so instead of the code QR,
        // print a QR of the LAN server URL for convenience (scan it with a
        // phone/tablet camera to copy the address into the app).
        let port: u16 = addr
            .rsplit(':')
            .next()
            .and_then(|p| p.parse().ok())
            .unwrap_or(1421);
        if let Some(ip) = detect_lan_ip() {
            let url = format!("ws://{}:{}", ip, port);
            eprintln!();
            eprintln!("=====================================================");
            eprintln!("  NoIDE server (no auth)");
            eprintln!();
            eprintln!("  Server URL for the app:");
            eprintln!();
            eprintln!("          {}", url);
            eprintln!();
            eprintln!("  Or scan this QR code with a camera to copy the URL:");
            eprintln!();
            pairing::print_qr(&url);
            eprintln!();
            eprintln!("=====================================================");
            eprintln!();
        } else {
            eprintln!(
                "[NoIDE] Could not detect a LAN IP — connect to ws://<this-host>:{} manually.",
                port
            );
        }
    } else if token.is_some() {
        eprintln!("[NoIDE] token auth enabled (fixed token from --token / NOIDE_TOKEN).");
    } else {
        let code = pairing::generate_code();
        pairing::print_pairing(&code);
        token = Some(code);
    }

    // Use a signal-driven shutdown so the runtime shuts down cleanly instead of
    // being dropped while worker threads are still active (which would panic with
    // "Cannot drop a runtime in a context where blocking is not allowed").
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");

    rt.block_on(async move {
        let manager = Arc::new(Mutex::new(pty::PtyManager::new()));
        let chat_tracker = Arc::new(ws_server::ChatProcessTracker::new());
        // One long-lived `<agent> serve` per agent, reused across chat messages.
        let agent_servers = Arc::new(AgentServerManager::new(chat_tracker.clone()));
        let port_forward = Arc::new(ws_server::PortForwardState::new());

        // Broadcast channel for graceful shutdown (Ctrl+C, server error, etc.).
        // Broadcast is used instead of oneshot because the sender needs to be
        // cloned for the Ctrl+C signal handler.
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::broadcast::channel::<()>(1);

        // Reap every live PTY session AND chat-stream child processes when
        // the server itself is terminated so shells, CLI tools (kilo/opencode),
        // and any jobs inside them never orphan on the host.
        //
        // We use `unwrap_or_else` instead of `.map()` so that a poisoned
        // mutex (caused by a panic in another thread) does not silently
        // prevent cleanup — we still call kill_all() on the poisoned
        // manager because its inner state is likely still usable, and
        // even a partial cleanup is better than none.
        let shutdown_mgr = manager.clone();
        let shutdown_chat = chat_tracker.clone();
        let shutdown_port_forward = port_forward.clone();
        let shutdown_tx_for_handler = shutdown_tx.clone();
        ctrlc::set_handler(move || {
            // Kill all chat-stream processes first (they are the most
            // likely to have spawned grandchildren via tool execution).
            shutdown_chat.kill_all();
            match shutdown_mgr.lock() {
                Ok(mut m) => m.kill_all(),
                Err(poisoned) => {
                    eprintln!("[NoIDE] mutex poisoned during shutdown — cleaning up anyway");
                    poisoned.into_inner().kill_all();
                }
            }
            // Stop all port-forward tunnels and the web UI if running.
            let pf = shutdown_port_forward.clone();
            tokio::spawn(async move {
                pf.stop_all().await;
            });
            // Signal the async runtime to shut down gracefully instead of
            // calling process::exit from a signal handler.
            let _ = shutdown_tx_for_handler.send(());
        })
        .ok();

        // --- Cloudflare Quick Tunnel (trycloudflare.com) ---
        // Spawn in the background so the server starts immediately.
        // The tunnel prints its public URL once ready.
        // Skip when --no-cloudflare is passed.
        if !no_cloudflare {
            let port: u16 = addr
                .rsplit(':')
                .next()
                .and_then(|p| p.parse().ok())
                .unwrap_or(1421);
            tokio::spawn(async move {
                match cloudflare::start_tunnel(port).await {
                    Ok((url, mut child)) => {
                        eprintln!();
                        eprintln!("=====================================================");
                        eprintln!("  Cloudflare Quick Tunnel active");
                        eprintln!();
                        eprintln!("  {}", url);
                        eprintln!();
                        eprintln!("  Open this URL in any browser to use NoIDE remotely.");
                        eprintln!("  The tunnel stays alive as long as this server runs.");
                        eprintln!("=====================================================");
                        eprintln!();
                        // Wait for the child to exit (i.e. server shutdown).
                        let _ = child.wait().await;
                    }
                    Err(e) => {
                        eprintln!("[NoIDE] Cloudflare tunnel unavailable: {e}");
                        eprintln!("[NoIDE] The server is still reachable on the local network.");
                    }
                }
            });
        }

        // Run the server and wait for shutdown signal.
        let server_fut = ws_server::start(
            manager,
            &addr,
            chat_tracker,
            agent_servers,
            token,
            port_forward.clone(),
        );

        // Subscribe to the shutdown channel and wait for a signal.
        let shutdown_fut = async {
            // The shutdown_rx is a Receiver; we can use it directly to wait.
            // Since we're in an async context, we need to handle the case where
            // the sender is dropped (channel closed).
            while shutdown_rx.recv().await.is_ok() {}
        };

        // Run both concurrently; when shutdown signal is received, the server
        // will be cancelled and we proceed to clean shutdown.
        tokio::select! {
            res = server_fut => {
                if let Err(e) = res {
                    eprintln!("[NoIDE] server error: {}", e);
                }
            }
            _ = shutdown_fut => {
                // Shutdown signal received; server task will be cancelled.
            }
        }
    });
}
