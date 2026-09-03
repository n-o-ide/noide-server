use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PtySession {
    pub id: String,
    pub pid: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PtyOutput {
    pub session_id: String,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PtyExit {
    pub session_id: String,
    pub code: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PtyEvent {
    Output(PtyOutput),
    Exit(PtyExit),
}

struct Session {
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
}

pub struct PtyManager {
    sessions: HashMap<String, Session>,
    tx: broadcast::Sender<PtyEvent>,
}

impl PtyManager {
    pub fn new(tx: broadcast::Sender<PtyEvent>) -> Self {
        Self {
            sessions: HashMap::new(),
            tx,
        }
    }

    pub fn spawn(
        &mut self,
        session_id: String,
        shell: Option<String>,
        cwd: Option<String>,
        cols: u32,
        rows: u32,
    ) -> Result<PtySession, String> {
        let shell_path = shell.unwrap_or_else(|| {
            if cfg!(target_os = "windows") {
                "powershell.exe".to_string()
            } else {
                for candidate in &["/bin/zsh", "/bin/bash", "/bin/sh"] {
                    if std::path::Path::new(candidate).exists() {
                        return candidate.to_string();
                    }
                }
                "/bin/sh".to_string()
            }
        });

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: rows.max(1) as u16,
                cols: cols.max(1) as u16,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("Failed to open pty: {}", e))?;

        let mut cmd = CommandBuilder::new(&shell_path);

        // Only use the cwd if it is actually a valid directory; a bad
        // cwd makes spawn() fail with ENOENT even if the shell exists.
        if let Some(dir) = cwd.as_deref() {
            if std::path::Path::new(dir).is_dir() {
                cmd.cwd(dir);
            }
        }

        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "NoIDE");

        let child = pair.slave.spawn_command(cmd).map_err(|e| {
            format!(
                "Failed to spawn shell '{}' (cwd: {:?}): {}",
                shell_path, cwd, e
            )
        })?;
        let pid = child.process_id().unwrap_or(0);
        let child = Arc::new(Mutex::new(child));

        let writer = pair
            .master
            .take_writer()
            .map_err(|e| format!("Failed to get pty writer: {}", e))?;
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("Failed to get pty reader: {}", e))?;

        let tx = self.tx.clone();
        let sid = session_id.clone();

        // Reader thread: raw bytes so prompts flush immediately.
        {
            let child_ref = child.clone();
            std::thread::spawn(move || {
                let mut reader = reader;
                let mut buf = [0u8; 4096];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            let _ = tx.send(PtyEvent::Output(PtyOutput {
                                session_id: sid.clone(),
                                data: String::from_utf8_lossy(&buf[..n]).into_owned(),
                            }));
                        }
                        Err(_) => break,
                    }
                }
                let code = {
                    let mut c = child_ref.lock().unwrap();
                    c.wait().ok().and_then(|s| s.exit_code().try_into().ok())
                };
                let _ = tx.send(PtyEvent::Exit(PtyExit {
                    session_id: sid,
                    code,
                }));
            });
        }

        self.sessions.insert(
            session_id.clone(),
            Session {
                writer,
                master: pair.master,
                child,
            },
        );

        Ok(PtySession {
            id: session_id,
            pid,
        })
    }

    pub fn write(&mut self, session_id: &str, data: &str) -> Result<(), String> {
        if let Some(session) = self.sessions.get_mut(session_id) {
            session
                .writer
                .write_all(data.as_bytes())
                .map_err(|e| format!("Write error: {}", e))?;
            session
                .writer
                .flush()
                .map_err(|e| format!("Flush error: {}", e))?;
            Ok(())
        } else {
            Err(format!("Session {} not found", session_id))
        }
    }

    pub fn resize(&mut self, session_id: &str, cols: u32, rows: u32) -> Result<(), String> {
        if let Some(session) = self.sessions.get_mut(session_id) {
            session
                .master
                .resize(PtySize {
                    rows: rows.max(1) as u16,
                    cols: cols.max(1) as u16,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|e| format!("Resize error: {}", e))
        } else {
            Err(format!("Session {} not found", session_id))
        }
    }

    pub fn kill(&mut self, session_id: &str) -> Result<(), String> {
        if let Some(session) = self.sessions.remove(session_id) {
            kill_session(session).map_err(|e| format!("Kill error: {}", e))
        } else {
            Err(format!("Session {} not found", session_id))
        }
    }

    /// Kill every live session. Used on shutdown / connection loss so shells
    /// (and any jobs running inside them) never leak as orphaned processes —
    /// important on low-end devices where every leftover process costs RAM/CPU.
    pub fn kill_all(&mut self) {
        let ids: Vec<String> = self.sessions.keys().cloned().collect();
        for id in ids {
            if let Some(session) = self.sessions.remove(&id) {
                let _ = kill_session(session);
            }
        }
    }
}

/// Terminate a session's child. We kill the *process group* (not just the
/// direct child) so foreground/background jobs launched inside the shell are
/// reaped too. Falls back to killing the child directly on non-unix targets.
///
/// Uses SIGKILL instead of SIGTERM so processes that ignore the latter are
/// still reaped — important when the server itself is about to shut down and
/// we cannot afford lingering orphans.
fn kill_session(session: Session) -> Result<(), Box<dyn std::error::Error>> {
    let child = session.child.lock().map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        if let Some(pid) = child.process_id() {
            let pgid = pid as i32;
            // Negative pid targets the whole process group.
            unsafe {
                libc::killpg(pgid, libc::SIGKILL);
            }
        }
    }
    #[cfg(not(unix))]
    {
        // portable-pty's kill() takes &mut self; rebind here so the unix
        // build (which only takes shared borrows above) does not trip an
        // unused-mut lint.
        let mut child = child;
        let _ = child.kill();
    }
    Ok(())
}

impl Drop for PtyManager {
    fn drop(&mut self) {
        self.kill_all();
    }
}
