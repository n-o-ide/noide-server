use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// Max bytes of recent output retained per session so a reconnecting client
/// can replay the tail of its scrollback without the producer ever stalling.
pub const RING_CAP: usize = 256 * 1024;
/// Chunks queued inside a session before the reader thread blocks (which in
/// turn lets the OS pty buffer fill and the child process block — real
/// backpressure instead of unbounded buffering).
const FEED_CAP: usize = 256;
/// Frames queued to a subscriber before the stream task (and ultimately the
/// pty reader) blocks.
const SUB_CAP: usize = 128;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PtySession {
    pub id: String,
    pub pid: u32,
}

/// Information about an attach, returned to the caller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachInfo {
    pub pid: u32,
    /// Absolute byte offset the client should resume from (may be clamped up
    /// to the ring start when the client asked for bytes we no longer hold).
    pub from: u64,
    /// Absolute offset just past the newest byte produced so far.
    pub end: u64,
    /// True when `from` predates the ring buffer (some scrollback lost).
    pub truncated: bool,
}

/// Messages the per-session stream task delivers to its subscriber.
#[derive(Debug)]
pub enum SubMsg {
    /// Raw output bytes starting at the absolute stream offset `from`.
    Out { from: u64, data: Vec<u8> },
    /// The shell exited (or was killed). Always sent after that session's
    /// remaining output so the client never sees exit overtake output.
    Exit { code: Option<i32> },
}

/// Commands sent to a session's stream task.
enum Ctrl {
    Subscribe {
        tx: mpsc::Sender<SubMsg>,
        /// Replay starts here (absolute offset); bytes before it are skipped.
        from: u64,
    },
    Unsubscribe,
}

/// Items pushed by the reader thread into the session's bounded feed channel.
enum Out {
    Data(Vec<u8>),
    /// EOF from the pty, carrying the child's exit code.
    Eof(Option<i32>),
}

/// Shared per-session bookkeeping used by the stream task and the manager.
pub struct SessionState {
    pub exited: bool,
    pub exit_code: Option<i32>,
    /// Total bytes ever produced (== offset of the next incoming byte).
    pub total: u64,
    /// When the session lost its last subscriber (or None while attached).
    pub detached_at: Option<std::time::Instant>,
    ring: VecDeque<u8>,
    ring_start: u64,
}

impl SessionState {
    fn new() -> Self {
        Self {
            exited: false,
            exit_code: None,
            total: 0,
            detached_at: None,
            ring: VecDeque::new(),
            ring_start: 0,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        self.ring.extend(bytes.iter().copied());
        while self.ring.len() > RING_CAP {
            self.ring.pop_front();
            self.ring_start += 1;
        }
        self.total += bytes.len() as u64;
    }

    /// Absolute offset of the oldest byte retained in the ring.
    pub fn ring_start(&self) -> u64 {
        self.ring_start
    }

    /// Copy the ring's bytes from absolute offset `from` (clamped to what is
    /// retained) to the current end, returning the offset they start at.
    fn replay_from(&self, from: u64) -> (u64, Vec<u8>) {
        let start = self.ring_start.max(from);
        let skip = (start - self.ring_start) as usize;
        (start, self.ring.iter().skip(skip).copied().collect())
    }
}

struct Session {
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    pid: u32,
    /// Held so the manager can close the feed when the session dies.
    feed: mpsc::Sender<Out>,
    /// Unbounded: control messages are rare and never a throughput path.
    ctrl: mpsc::UnboundedSender<Ctrl>,
    state: Arc<Mutex<SessionState>>,
    task: tokio::task::JoinHandle<()>,
}

pub struct PtyManager {
    sessions: HashMap<String, Session>,
}

impl PtyManager {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    /// Spawn a shell session. Output is queued per session and retained in a
    /// ring buffer; nothing is sent anywhere until a subscriber attaches.
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
        cmd.env("SHELL", &shell_path);
        cmd.env("SHLVL", "1");

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

        let state = Arc::new(Mutex::new(SessionState::new()));
        let (feed_tx, mut feed_rx) = mpsc::channel::<Out>(FEED_CAP);
        let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel::<Ctrl>();

        // Reader thread: pushes raw bytes into the bounded feed channel. When
        // every downstream consumer is slow the channel fills and this thread
        // blocks — the pty master is then not read, its OS buffer fills, and
        // the child process blocks on write. That is the whole backpressure
        // story; no unbounded buffering anywhere in the chain.
        {
            let feed = feed_tx.clone();
            let child_ref = child.clone();
            std::thread::spawn(move || {
                let mut reader = reader;
                let mut buf = [0u8; 4096];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            if feed.blocking_send(Out::Data(buf[..n].to_vec())).is_err() {
                                // Stream task gone (session killed). The pty
                                // master handle drops with it, unblocking us.
                                return;
                            }
                        }
                        Err(_) => break,
                    }
                }
                let code = {
                    let mut c = child_ref.lock().unwrap();
                    c.wait().ok().and_then(|s| s.exit_code().try_into().ok())
                };
                let _ = feed.blocking_send(Out::Eof(code));
            });
        }

        // Stream task: drains the feed into the ring buffer, keeps at most one
        // subscriber, replays on subscribe and forwards live chunks to it.
        let task = {
            let state = state.clone();
            tokio::spawn(async move {
                let mut sub: Option<mpsc::Sender<SubMsg>> = None;
                loop {
                    tokio::select! {
                        biased;
                        c = ctrl_rx.recv() => match c {
                            Some(Ctrl::Subscribe { tx, from }) => {
                                // Replay first so nothing the subscriber
                                // missed while detached is lost, then go live.
                                let (start, bytes) = {
                                    let st = state.lock().unwrap();
                                    st.replay_from(from)
                                };
                                if !bytes.is_empty()
                                    && tx.send(SubMsg::Out { from: start, data: bytes })
                                        .await
                                        .is_err()
                                {
                                    continue;
                                }
                                state.lock().unwrap().detached_at = None;
                                sub = Some(tx);
                            }
                            Some(Ctrl::Unsubscribe) => {
                                sub = None;
                                state.lock().unwrap().detached_at = Some(std::time::Instant::now());
                            }
                            None => break, // manager dropped the session
                        },
                        o = feed_rx.recv() => match o {
                            Some(Out::Data(bytes)) => {
                                let from = {
                                    let mut st = state.lock().unwrap();
                                    st.push(&bytes);
                                    // total was just advanced by push(); the
                                    // offset of these bytes is total - len.
                                    st.total - bytes.len() as u64
                                };
                                if let Some(tx) = &sub {
                                    if tx.send(SubMsg::Out { from, data: bytes })
                                        .await
                                        .is_err()
                                    {
                                        sub = None;
                                    }
                                }
                            }
                            Some(Out::Eof(code)) => {
                                {
                                    let mut st = state.lock().unwrap();
                                    st.exited = true;
                                    st.exit_code = code;
                                    st.detached_at = None;
                                }
                                if let Some(tx) = sub.take() {
                                    let _ = tx.send(SubMsg::Exit { code }).await;
                                }
                            }
                            None => break, // feed closed
                        },
                    }
                }
                // Session over or dropped: make sure a lingering subscriber is
                // told if the channel just vanished without an Exit.
                let exited = state.lock().unwrap().exited;
                if !exited {
                    if let Some(tx) = sub {
                        let _ = tx.send(SubMsg::Exit { code: None }).await;
                    }
                }
            })
        };

        self.sessions.insert(
            session_id.clone(),
            Session {
                writer,
                master: pair.master,
                child,
                pid,
                feed: feed_tx,
                ctrl: ctrl_tx,
                state,
                task,
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

    /// Attach a subscriber to a live session. Replay starts at `from` when
    /// given (clamped to the ring), otherwise from the oldest retained byte.
    pub fn subscribe(
        &mut self,
        session_id: &str,
        from: Option<u64>,
    ) -> Result<(mpsc::Receiver<SubMsg>, AttachInfo), String> {
        let session = self
            .sessions
            .get(session_id)
            .ok_or_else(|| format!("Session {} not found", session_id))?;
        let st = session.state.lock().unwrap();
        if st.exited {
            return Err(format!("Session {} has exited", session_id));
        }
        let info = AttachInfo {
            pid: session.pid,
            from: from
                .map(|f| f.max(st.ring_start()))
                .unwrap_or(st.ring_start()),
            end: st.total,
            truncated: from.map(|f| f < st.ring_start()).unwrap_or(false),
        };
        let from = info.from;
        drop(st);
        let (tx, rx) = mpsc::channel::<SubMsg>(SUB_CAP);
        session
            .ctrl
            .send(Ctrl::Subscribe { tx, from })
            .map_err(|_| format!("Session {} is shutting down", session_id))?;
        Ok((rx, info))
    }

    /// Detach the current subscriber of a session (called when a connection
    /// drops). The session keeps running and buffering into its ring.
    pub fn unsubscribe(&mut self, session_id: &str) {
        if let Some(session) = self.sessions.get(session_id) {
            let _ = session.ctrl.send(Ctrl::Unsubscribe);
        }
    }

    /// Kill a session: terminates its process group and drops all handles so
    /// the reader thread and stream task wind down. Subscribers are not sent
    /// an Exit (an explicit kill is always client-initiated teardown).
    pub fn kill(&mut self, session_id: &str) -> Result<(), String> {
        if let Some(session) = self.sessions.remove(session_id) {
            kill_session(session).map_err(|e| format!("Kill error: {}", e))
        } else {
            Err(format!("Session {} not found", session_id))
        }
    }

    /// Reap finished or abandoned sessions: exited ones are removed outright;
    /// live ones that have had no subscriber for longer than `max_detached`
    /// are killed so shells never leak as orphans on the host.
    pub fn sweep(&mut self, max_detached: std::time::Duration) {
        let ids: Vec<String> = self.sessions.keys().cloned().collect();
        for id in ids {
            let (exited, detached_for) = {
                let Some(session) = self.sessions.get(&id) else {
                    continue;
                };
                let st = session.state.lock().unwrap();
                let detached_for = st
                    .detached_at
                    .map(|t| t.elapsed())
                    .unwrap_or(std::time::Duration::ZERO);
                (st.exited, detached_for)
            };
            if exited {
                if let Some(session) = self.sessions.remove(&id) {
                    session.task.abort();
                }
            } else if detached_for > max_detached {
                if let Some(session) = self.sessions.remove(&id) {
                    let _ = kill_session(session);
                }
            }
        }
    }

    /// Kill every live session. Used on shutdown so shells (and any jobs
    /// running inside them) never leak as orphaned processes.
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
    // Stop the session's stream task; dropping the session's feed sender
    // below also unblocks the reader thread if it was mid-send.
    drop(session.feed);
    session.task.abort();
    Ok(())
}

impl Default for PtyManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PtyManager {
    fn drop(&mut self) {
        self.kill_all();
    }
}
