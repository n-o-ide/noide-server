use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use flate2::write::GzEncoder;
use flate2::Compression;
use futures_util::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::http;
use tokio_tungstenite::tungstenite::Message;

use crate::agent_servers::AgentServerManager;
use crate::commands;
use crate::pty::{AttachInfo, PtyManager, SubMsg};

/// How many outbound WS messages may be queued per connection before senders
/// block. Combined with the per-session queues in pty.rs this bounds memory:
/// a slow client eventually stalls the pty reader, and the OS pty buffer makes
/// the child process block — real end-to-end backpressure.
const OUT_CAP: usize = 512;
/// Replay/live batches are split into frames at most this large.
const FRAME_MAX: usize = 128 * 1024;
/// gzip payloads at or above this size; smaller frames go out raw.
const GZIP_MIN: usize = 128;
/// How long an unattached (detached) session is kept alive before it is
/// reaped. Detaches now happen on layout moves / pane pops / mode switches
/// and on closing a panel (but not a tab), so 60s was far too short — the
/// shell should survive a long detour (working in another pane, switching
/// layouts, a long build) and reattach when the terminal comes back. Still
/// bounded so shells the user genuinely abandoned get reaped. Override with
/// NOIDE_PTY_KEEP_ALIVE (seconds).
const REAP_UNATTACHED: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Tracks every chat-stream child process PID across all connections so
/// the ctrlc handler can reap them on server shutdown — without this,
/// CLI processes (kilo / opencode) survive as orphans when the server
/// exits, wasting RAM/CPU on low-end devices.
pub struct ChatProcessTracker {
    pids: Mutex<HashSet<u32>>,
}

impl ChatProcessTracker {
    pub fn new() -> Self {
        Self {
            pids: Mutex::new(HashSet::new()),
        }
    }

    pub fn register(&self, pid: u32) {
        self.pids.lock().unwrap().insert(pid);
    }

    pub fn unregister(&self, pid: u32) {
        self.pids.lock().unwrap().remove(&pid);
    }

    /// Kill every tracked chat process (and its process group on Unix).
    /// Called from the ctrlc handler so CLI children never leak as orphans.
    pub fn kill_all(&self) {
        let pids: Vec<u32> = self.pids.lock().unwrap().drain().collect();
        for pid in pids {
            #[cfg(unix)]
            unsafe {
                // Negative pid targets the whole process group (the CLI and
                // any grandchildren it spawned for tool execution).
                libc::kill(-(pid as i32), libc::SIGTERM);
            }
            #[cfg(windows)]
            unsafe {
                use windows_sys::Win32::Foundation::CloseHandle;
                use windows_sys::Win32::System::Threading::{
                    OpenProcess, TerminateProcess, PROCESS_TERMINATE,
                };
                // Windows has no process groups; open the process with
                // terminate rights and kill the direct child. Grandchildren
                // are short-lived and will be reaped by the OS once the
                // parent exits.
                let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
                if !handle.is_null() {
                    TerminateProcess(handle, 1);
                    CloseHandle(handle);
                }
            }
        }
    }
}

/// Per-connection transfer statistics, for diagnosing slow remote links.
#[derive(Debug)]
pub struct ConnStats {
    pub started: Instant,
    pub out_frames: u64,
    pub out_binary: u64,
    pub out_text: u64,
    /// PTY payload bytes before compression (what the user actually typed).
    pub out_raw_bytes: u64,
    /// Bytes actually placed on the wire (post-gzip).
    pub out_wire_bytes: u64,
    pub pty_exits: u64,
    pub in_msgs: u64,
    pub in_bytes: u64,
}

impl ConnStats {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            out_frames: 0,
            out_binary: 0,
            out_text: 0,
            out_raw_bytes: 0,
            out_wire_bytes: 0,
            pty_exits: 0,
            in_msgs: 0,
            in_bytes: 0,
        }
    }

    fn to_json(&self) -> Value {
        let elapsed = self.started.elapsed();
        let secs = elapsed.as_secs_f64().max(0.001);
        json!({
            "elapsedMs": elapsed.as_millis() as u64,
            "outFrames": self.out_frames,
            "outFramesPerSec": (self.out_frames as f64 / secs).round() as u64,
            "outBinary": self.out_binary,
            "outText": self.out_text,
            "rawBytes": self.out_raw_bytes,
            "wireBytes": self.out_wire_bytes,
            "compressionRatio": if self.out_raw_bytes > 0 {
                (self.out_raw_bytes as f64 / self.out_wire_bytes.max(1) as f64 * 10.0).round() / 10.0
            } else { 0.0 },
            "ptyExits": self.pty_exits,
            "inMsgs": self.in_msgs,
            "inBytes": self.in_bytes,
        })
    }
}

/// Compare two byte strings in constant time (no early exit on the first
/// mismatching byte) so token comparison doesn't leak timing information.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[derive(serde::Deserialize)]
struct Request {
    id: u64,
    command: String,
    #[serde(default)]
    args: Value,
}

fn arg<T: DeserializeOwned>(args: &Value, key: &str) -> Result<T, String> {
    let v = args.get(key).cloned().unwrap_or(Value::Null);
    serde_json::from_value(v).map_err(|e| format!("invalid arg '{}': {}", key, e))
}

fn opt_arg<T: DeserializeOwned>(args: &Value, key: &str) -> Option<T> {
    let v = args.get(key)?;
    serde_json::from_value(v.clone()).ok()
}

/// Encode one PTY output run as a WS frame. Layout (all little-endian):
///   u32 session_id length | session_id bytes | u64 `from` offset | payload
/// The payload is the raw terminal bytes; when it is >= GZIP_MIN it is
/// gzip-compressed first (the client sniffs the gzip magic to decide).
/// Raw bytes in — no JSON escaping, no lossy UTF-8 — so arbitrary terminal
/// output survives intact.
fn encode_output_frame(session_id: &str, from: u64, data: &[u8]) -> Message {
    let mut frame = Vec::with_capacity(4 + session_id.len() + 8 + data.len());
    frame.extend_from_slice(&(session_id.len() as u32).to_le_bytes());
    frame.extend_from_slice(session_id.as_bytes());
    frame.extend_from_slice(&from.to_le_bytes());
    if data.len() >= GZIP_MIN {
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        if gz.write_all(data).is_ok() {
            if let Ok(compressed) = gz.finish() {
                frame.extend_from_slice(&compressed);
                return Message::Binary(frame);
            }
        }
        // Fall through to raw on any compression hiccup.
    }
    frame.extend_from_slice(data);
    Message::Binary(frame)
}

/// Wire bytes of a Message (what actually travels).
fn msg_len(m: &Message) -> usize {
    match m {
        Message::Text(s) => s.len(),
        Message::Binary(b) => b.len(),
        _ => 0,
    }
}

async fn push(
    out_tx: &mpsc::Sender<Message>,
    m: Message,
    stats: &Arc<Mutex<ConnStats>>,
    binary: bool,
) -> bool {
    {
        let mut s = stats.lock().unwrap();
        s.out_frames += 1;
        if binary {
            s.out_binary += 1;
        } else {
            s.out_text += 1;
        }
        s.out_wire_bytes += msg_len(&m) as u64;
    }
    out_tx.send(m).await.is_ok()
}

/// Stream one session's `SubMsg`s to a connection: coalesces output into
/// short batches (capped at FRAME_MAX) while output streams continuously,
/// but sends a chunk immediately when the link has been idle so a lone
/// interactive echo (keystroke, prompt, single command result) is not held
/// for the batch window. Frames are binary; exit is JSON. Runs until the
/// session detaches, exits, or the connection goes away.
fn spawn_subscriber_task(
    session_id: String,
    mut rx: mpsc::Receiver<SubMsg>,
    out_tx: mpsc::Sender<Message>,
    stats: Arc<Mutex<ConnStats>>,
) {
    tokio::spawn(async move {
        const BATCH_WINDOW: std::time::Duration = std::time::Duration::from_millis(20);
        // When nothing has been flushed for at least this long the link is
        // considered idle: the next chunk is sent immediately instead of
        // waiting out BATCH_WINDOW. Batching only pays off while output is
        // streaming continuously.
        const IDLE_RESET: std::time::Duration = std::time::Duration::from_millis(100);

        let mut pending: Vec<u8> = Vec::new();
        let mut pending_from: u64 = 0;
        let mut flush_at: Option<tokio::time::Instant> = None;
        // Initialised to "long ago" so the very first chunk of a session
        // (e.g. the shell prompt right after attach) is also sent right away.
        let mut last_flush = tokio::time::Instant::now()
            .checked_sub(IDLE_RESET)
            .unwrap_or_else(tokio::time::Instant::now);

        // Drain the pending batch, returning it ready to encode (or None).
        let take_batch = |pending: &mut Vec<u8>,
                          pending_from: &mut u64,
                          flush_at: &mut Option<tokio::time::Instant>,
                          last_flush: &mut tokio::time::Instant|
         -> Option<(u64, Vec<u8>)> {
            *flush_at = None;
            if pending.is_empty() {
                return None;
            }
            let from = *pending_from;
            let bytes = std::mem::take(pending);
            *last_flush = tokio::time::Instant::now();
            stats.lock().unwrap().out_raw_bytes += bytes.len() as u64;
            Some((from, bytes))
        };

        // Send the pending batch (if any) as one encoded frame.
        macro_rules! flush_batch {
            () => {{
                if let Some((from, bytes)) = take_batch(
                    &mut pending,
                    &mut pending_from,
                    &mut flush_at,
                    &mut last_flush,
                ) {
                    let m = encode_output_frame(&session_id, from, &bytes);
                    if !push(&out_tx, m, &stats, true).await {
                        break;
                    }
                }
            }};
        }

        loop {
            let item = match flush_at {
                Some(deadline) => {
                    let remaining =
                        deadline.saturating_duration_since(tokio::time::Instant::now());
                    match tokio::time::timeout(remaining, rx.recv()).await {
                        Err(_elapsed) => {
                            // Quiet for the whole window — send what we have.
                            flush_batch!();
                            continue;
                        }
                        Ok(Some(item)) => item,
                        Ok(None) => {
                            flush_batch!();
                            break;
                        }
                    }
                }
                None => match rx.recv().await {
                    Some(item) => item,
                    None => break,
                },
            };

            match item {
                SubMsg::Out { from, data } => {
                    // A chunk arriving with nothing pending while the link has
                    // been quiet is likely a lone interactive echo — send it
                    // now instead of holding it for the batch window. Once
                    // output streams continuously the window applies again.
                    let idle = pending.is_empty() && last_flush.elapsed() >= IDLE_RESET;
                    if pending.is_empty() {
                        pending_from = from;
                    }
                    pending.extend_from_slice(&data);
                    if pending.len() >= FRAME_MAX {
                        flush_batch!();
                    } else if idle {
                        flush_batch!();
                    } else if flush_at.is_none() {
                        flush_at = Some(tokio::time::Instant::now() + BATCH_WINDOW);
                    }
                }
                SubMsg::Exit { code } => {
                    // Never let the exit overtake this session's trailing
                    // output.
                    flush_batch!();
                    stats.lock().unwrap().pty_exits += 1;
                    let text = json!({
                        "event": "pty-exit",
                        "payload": { "session_id": session_id, "code": code }
                    })
                    .to_string();
                    if !push(&out_tx, Message::Text(text), &stats, false).await {
                        break;
                    }
                    break;
                }
            }
        }
    });
}

pub async fn start(
    pty: Arc<Mutex<PtyManager>>,
    addr: &str,
    chat_tracker: Arc<ChatProcessTracker>,
    agent_servers: Arc<AgentServerManager>,
    token: Option<String>,
) -> std::io::Result<()> {
    // Reap exited or long-abandoned sessions so shells never leak as
    // orphans on the host (a session with no subscriber is kept alive for
    // REAP_UNATTACHED so reconnects and reattaches keep working).
    let reap_unattached = std::env::var("NOIDE_PTY_KEEP_ALIVE")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(std::time::Duration::from_secs)
        .unwrap_or(REAP_UNATTACHED);
    {
        let pty = pty.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(15));
            loop {
                tick.tick().await;
                if let Ok(mut mgr) = pty.lock() {
                    mgr.sweep(reap_unattached);
                }
            }
        });
    }

    let listener = TcpListener::bind(addr).await?;
    eprintln!("[NoIDE] WebSocket PTY server listening on ws://{}", addr);
    loop {
        let (stream, _) = listener.accept().await?;
        let pty = pty.clone();
        let chat_tracker = chat_tracker.clone();
        let agent_servers = agent_servers.clone();
        let token = token.clone();
        tokio::spawn(async move {
            if let Err(e) =
                handle_connection(stream, pty, chat_tracker, agent_servers, token).await
            {
                eprintln!("[NoIDE] WS connection error: {}", e);
            }
        });
    }
}

// The auth callback must return tungstenite's `Result<Response, ErrorResponse>`
// shape, whose error type is a large HTTP response — not ours to change, so
// the lint is allowed rather than boxing what the handshake API requires.
#[allow(clippy::result_large_err)]
async fn handle_connection(
    stream: tokio::net::TcpStream,
    pty: Arc<Mutex<PtyManager>>,
    chat_tracker: Arc<ChatProcessTracker>,
    agent_servers: Arc<AgentServerManager>,
    expected_token: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Interactive terminal traffic is a stream of small messages; without
    // TCP_NODELAY the Nagle algorithm can hold a small write until earlier
    // data is ACKed (up to ~40ms on a healthy link, worse over a high-RTT
    // WAN), which reads as intermittent echo lag on remote connections.
    stream.set_nodelay(true)?;

    // Token auth: when a token is configured the client must present it as
    // `?token=<value>` on the WebSocket URL during the handshake. Rejecting
    // the upgrade with HTTP 401 lets browsers surface a clean onerror instead
    // of hanging.
    let ws = match &expected_token {
        Some(expected) => {
            let expected = expected.clone();
            accept_hdr_async(
                stream,
                move |req: &http::Request<()>, response: http::Response<()>| -> Result<
                    http::Response<()>,
                    http::Response<Option<String>>,
                > {
                // `req` is the HTTP upgrade request; reject with HTTP 401 by
                // returning an error response whose body carries the reason.
                let query = req.uri().query().unwrap_or("");
                let provided = query.split('&').find_map(|pair| {
                    let mut it = pair.splitn(2, '=');
                    if it.next() == Some("token") {
                        it.next().map(|v| v.to_string())
                    } else {
                        None
                    }
                });
                let ok = match &provided {
                    Some(p) => constant_time_eq(p.as_bytes(), expected.as_bytes()),
                    None => false,
                };
                if ok {
                    Ok(response)
                } else {
                    Err(http::Response::builder()
                        .status(401)
                        .body(Some("missing or invalid token".to_string()))
                        .unwrap())
                }
                },
            )
            .await
        }
        None => accept_async(stream).await,
    }?;
    let (mut writer, mut reader) = ws.split();

    // Bounded outbound queue: when it fills, producers (session subscriber
    // tasks, RPC responses) block, and that pressure propagates all the way
    // back to the pty reader and the child process.
    let (out_tx, mut out_rx) = mpsc::channel::<Message>(OUT_CAP);

    // Tracks running chat-stream child PIDs so they can be cancelled.
    let processes = Arc::new(tokio::sync::Mutex::new(HashMap::<
        String,
        u32,
    >::new()));

    // Sessions this connection is currently subscribed to (spawned or
    // attached). On close we detach from them — they keep running and their
    // output keeps buffering into the ring so a reconnect can reattach.
    let subscribed: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    let stats: Arc<Mutex<ConnStats>> = Arc::new(Mutex::new(ConnStats::new()));

    // Idle timeout: if the client goes silent (e.g. browser killed without a
    // close frame, network drop), drop the connection. The clock resets on
    // ANY incoming message (incl. protocol pongs) and on every outbound data
    // write, so a live terminal is never dropped — even one that just streams
    // output or sits idle while the user reads.
    let idle_limit = std::time::Duration::from_secs(60);
    let last_activity = Arc::new(Mutex::new(Instant::now()));

    // Task that drains outgoing messages to the websocket. A periodic Ping
    // keeps pong traffic flowing from browsers (they answer pings at the
    // protocol level), so idle-but-live terminals keep resetting the idle
    // clock even in background tabs where JS timers are throttled. Pings
    // don't touch last_activity — only the pongs they elicit and real data
    // count as proof of life.
    let last_activity_write = last_activity.clone();
    tokio::spawn(async move {
        let mut ping = tokio::time::interval(std::time::Duration::from_secs(20));
        ping.tick().await; // first tick completes immediately; skip it
        loop {
            tokio::select! {
                msg = out_rx.recv() => {
                    match msg {
                        Some(m) => {
                            if writer.send(m).await.is_err() { break; }
                            // Outbound data = the client is receiving bytes,
                            // so the connection is demonstrably alive.
                            *last_activity_write.lock().unwrap() = Instant::now();
                        }
                        None => break,
                    }
                }
                _ = ping.tick() => {
                    if writer.send(Message::Ping(Vec::new())).await.is_err() { break; }
                }
            }
        }
    });

    loop {
        let since = last_activity.lock().unwrap().elapsed();
        if since >= idle_limit {
            eprintln!("[NoIDE] idle timeout; closing connection");
            break;
        }
        let timeout = tokio::time::sleep(idle_limit - since);
        let next = tokio::select! {
            msg = reader.next() => msg,
            _ = timeout => { eprintln!("[NoIDE] idle timeout; closing connection"); break; }
        };
        *last_activity.lock().unwrap() = Instant::now();
        let msg = match next {
            Some(Ok(m)) => m,
            Some(Err(_)) => break,
            None => break,
        };
        match msg {
            Message::Text(text) => {
                {
                    let mut s = stats.lock().unwrap();
                    s.in_msgs += 1;
                    s.in_bytes += text.len() as u64;
                }
                // Application-level protocol events from the client (no id/command).
                // Route these before the RPC path so a heartbeat ping is replied to
                // directly instead of being rejected as an invalid request.
                {
                    let v: Value = match serde_json::from_str(&text) {
                        Ok(v) => v,
                        Err(_) => Value::Null,
                    };
                    if let Some(event) = v.get("event").and_then(|e| e.as_str()) {
                        if event == "__ping" {
                            let _ = out_tx
                                .send(Message::Text(
                                    json!({ "event": "__pong" }).to_string(),
                                ))
                                .await;
                            // Reset the idle clock on the pong we just sent — the
                            // connection is demonstrably alive.
                            *last_activity.lock().unwrap() = Instant::now();
                            continue;
                        }
                    }
                }

                let req: Request = match serde_json::from_str(&text) {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = out_tx
                            .send(Message::Text(
                                json!({ "id": 0, "ok": false, "error": format!("invalid request: {}", e) })
                                    .to_string(),
                            ))
                            .await;
                        continue;
                    }
                };
                // Handle each command in its own task so a slow command
                // (e.g. a full-tree `list_files` walk) can't block faster
                // ones (e.g. `read_directory`) on this single connection.
                // Responses are tagged by id and matched client-side, so
                // out-of-order replies are harmless.
                let pty_t = pty.clone();
                let out_tx_t = out_tx.clone();
                let processes_t = processes.clone();
                let subscribed_t = subscribed.clone();
                let stats_t = stats.clone();
                let chat_tracker_t = chat_tracker.clone();
                let agent_servers_t = agent_servers.clone();
                tokio::spawn(async move {
                    let res = handle(
                        &req,
                        &pty_t,
                        out_tx_t.clone(),
                        processes_t,
                        subscribed_t,
                        stats_t,
                        chat_tracker_t,
                        agent_servers_t,
                    )
                    .await;
                    let resp = match res {
                        Ok(v) => json!({ "id": req.id, "ok": true, "result": v }),
                        Err(e) => json!({ "id": req.id, "ok": false, "error": e }),
                    };
                    let _ = out_tx_t.send(Message::Text(resp.to_string())).await;
                });
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    // Connection dropped: detach from every session we subscribed to so it
    // keeps running (and keeps its ring) for a reconnect, and kill any
    // chat-stream subprocesses this client owned.
    {
        let subs = subscribed.lock().unwrap();
        if !subs.is_empty() {
            if let Ok(mut mgr) = pty.lock() {
                for sid in subs.iter() {
                    mgr.unsubscribe(sid);
                }
            }
        }
    }
    {
        let s = stats.lock().unwrap();
        eprintln!(
            "[NoIDE] connection closed: {} frames ({} bin / {} txt), {} raw -> {} wire bytes ({:.1}x), {} pty-exits, {} msgs in",
            s.out_frames,
            s.out_binary,
            s.out_text,
            s.out_raw_bytes,
            s.out_wire_bytes,
            if s.out_raw_bytes > 0 {
                s.out_raw_bytes as f64 / s.out_wire_bytes.max(1) as f64
            } else {
                0.0
            },
            s.pty_exits,
            s.in_msgs,
        );
    }

    // Kill any chat-stream subprocesses still running for this client.
    let pids: Vec<u32> = processes.lock().await.drain().map(|(_, pid)| pid).collect();
    for pid in pids {
        #[cfg(unix)]
        {
            chat_tracker.unregister(pid);
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        #[cfg(not(unix))]
        {
            use windows_sys::Win32::Foundation::CloseHandle;
            use windows_sys::Win32::System::Threading::{
                OpenProcess, TerminateProcess, PROCESS_TERMINATE,
            };
            let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
            if !handle.is_null() {
                unsafe { TerminateProcess(handle, 1); }
                unsafe { CloseHandle(handle); }
            }
        }
    }

    Ok(())
}

/// Attach or spawn-side subscription helper: register this connection as the
/// subscriber of `session_id` and start its forwarding task. Used by both
/// `pty_spawn` (fresh session, no replay) and `pty_attach` (existing session,
/// replay from `from`).
fn attach_and_stream(
    session_id: &str,
    pty: &Arc<Mutex<PtyManager>>,
    from: Option<u64>,
    out_tx: mpsc::Sender<Message>,
    stats: Arc<Mutex<ConnStats>>,
    subscribed: &Arc<Mutex<HashSet<String>>>,
) -> Result<AttachInfo, String> {
    let mut mgr = pty.lock().map_err(|e| e.to_string())?;
    let (rx, info) = mgr.subscribe(session_id, from)?;
    drop(mgr);
    subscribed.lock().unwrap().insert(session_id.to_string());
    spawn_subscriber_task(session_id.to_string(), rx, out_tx, stats);
    Ok(info)
}

// Central command dispatch: every arg is a distinct context handle threaded
// through the match arms. Grouping them would churn the whole dispatch.
#[allow(clippy::too_many_arguments)]
async fn handle(
    req: &Request,
    pty: &Arc<Mutex<PtyManager>>,
    out_tx: mpsc::Sender<Message>,
    processes: Arc<tokio::sync::Mutex<HashMap<String, u32>>>,
    subscribed: Arc<Mutex<HashSet<String>>>,
    stats: Arc<Mutex<ConnStats>>,
    chat_tracker: Arc<ChatProcessTracker>,
    agent_servers: Arc<AgentServerManager>,
) -> Result<Value, String> {
    let args = &req.args;
    match req.command.as_str() {
        "pty_spawn" => {
            let session_id: String = arg(args, "sessionId")?;
            let shell: Option<String> = opt_arg(args, "shell");
            let cwd: Option<String> = opt_arg(args, "cwd");
            let cols: Option<u32> = opt_arg(args, "cols");
            let rows: Option<u32> = opt_arg(args, "rows");
            let spawned = {
                let mut mgr = pty.lock().map_err(|e| e.to_string())?;
                mgr.spawn(
                    session_id.clone(),
                    shell,
                    cwd,
                    cols.unwrap_or(80),
                    rows.unwrap_or(24),
                )?
            };
            // Subscribe immediately so the very first prompt is delivered.
            // Replay from the ring covers any bytes produced in the window
            // between spawn and subscribe.
            let info =
                attach_and_stream(&session_id, pty, None, out_tx, stats, &subscribed)?;
            Ok(json!({
                "id": spawned.id,
                "pid": spawned.pid,
                "from": info.from,
                "end": info.end,
            }))
        }
        "pty_attach" => {
            let session_id: String = arg(args, "sessionId")?;
            let from: Option<u64> = opt_arg(args, "from");
            let info = attach_and_stream(&session_id, pty, from, out_tx, stats, &subscribed)?;
            Ok(serde_json::to_value(info).unwrap_or(Value::Null))
        }
        "pty_write" => {
            let session_id: String = arg(args, "sessionId")?;
            let data: String = arg(args, "data")?;
            let mut mgr = pty.lock().map_err(|e| e.to_string())?;
            mgr.write(&session_id, &data)?;
            Ok(Value::Null)
        }
        "pty_resize" => {
            let session_id: String = arg(args, "sessionId")?;
            let cols: u32 = arg(args, "cols")?;
            let rows: u32 = arg(args, "rows")?;
            let mut mgr = pty.lock().map_err(|e| e.to_string())?;
            mgr.resize(&session_id, cols, rows)?;
            Ok(Value::Null)
        }
        "pty_kill" => {
            let session_id: String = arg(args, "sessionId")?;
            subscribed.lock().unwrap().remove(&session_id);
            let mut mgr = pty.lock().map_err(|e| e.to_string())?;
            mgr.kill(&session_id)?;
            Ok(Value::Null)
        }
        "pty_detach" => {
            let session_id: String = arg(args, "sessionId")?;
            // Stop streaming to this connection WITHOUT ending the session:
            // the shell keeps running and buffering into its ring, so a
            // remount (layout move / maximize / pane drag / mode switch)
            // can reattach to the same shell instead of spawning a new one.
            subscribed.lock().unwrap().remove(&session_id);
            {
                let mut mgr = pty.lock().map_err(|e| e.to_string())?;
                mgr.unsubscribe(&session_id);
            }
            Ok(Value::Null)
        }
        "ping" => Ok(json!({ "pong": true })),
        "connection_stats" => {
            let s = stats.lock().unwrap();
            let mut v = s.to_json();
            let subs: Vec<String> = subscribed.lock().unwrap().iter().cloned().collect();
            v["subscribed"] = json!(subs);
            drop(s);
            Ok(v)
        }
        "run_command" => {
            let command: String = arg(args, "command")?;
            Ok(serde_json::to_value(commands::run_command(command)?).unwrap_or(Value::Null))
        }
        "get_git_status" => {
            let path: String = arg(args, "path")?;
            Ok(serde_json::to_value(commands::get_git_status(path)?).unwrap_or(Value::Null))
        }
        "get_git_diff" => {
            let path: String = arg(args, "path")?;
            let file: String = arg(args, "file")?;
            Ok(serde_json::to_value(commands::get_git_diff(path, file)?).unwrap_or(Value::Null))
        }
        "get_git_staged_diff" => {
            let path: String = arg(args, "path")?;
            let file: String = arg(args, "file")?;
            Ok(
                serde_json::to_value(commands::get_git_staged_diff(path, file)?)
                    .unwrap_or(Value::Null),
            )
        }
        "get_git_head_file" => {
            let path: String = arg(args, "path")?;
            let file: String = arg(args, "file")?;
            Ok(
                serde_json::to_value(commands::get_git_head_file(path, file)?)
                    .unwrap_or(Value::Null),
            )
        }
        "git_show_index" => {
            let path: String = arg(args, "path")?;
            let file: String = arg(args, "file")?;
            Ok(serde_json::to_value(commands::git_show_index(path, file)?).unwrap_or(Value::Null))
        }
        "git_add" => {
            let path: String = arg(args, "path")?;
            let file: String = arg(args, "file")?;
            Ok(serde_json::to_value(commands::git_add(path, file)?).unwrap_or(Value::Null))
        }
        "git_commit" => {
            let path: String = arg(args, "path")?;
            let message: String = arg(args, "message")?;
            let amend: Option<bool> = opt_arg(args, "amend");
            Ok(
                serde_json::to_value(commands::git_commit(path, message, amend)?)
                    .unwrap_or(Value::Null),
            )
        }
        "git_undo_commit" => {
            let path: String = arg(args, "path")?;
            Ok(serde_json::to_value(commands::git_undo_commit(path)?).unwrap_or(Value::Null))
        }
        "git_log" => {
            let path: String = arg(args, "path")?;
            let limit: Option<usize> = opt_arg(args, "limit");
            Ok(serde_json::to_value(commands::git_log(path, limit)?).unwrap_or(Value::Null))
        }
        "git_commit_diff" => {
            let path: String = arg(args, "path")?;
            let commit: String = arg(args, "commit")?;
            Ok(
                serde_json::to_value(commands::git_commit_diff(path, commit)?)
                    .unwrap_or(Value::Null),
            )
        }
        "git_stash_list" => {
            let path: String = arg(args, "path")?;
            Ok(serde_json::to_value(commands::git_stash_list(path)?).unwrap_or(Value::Null))
        }
        "git_stash" => {
            let path: String = arg(args, "path")?;
            Ok(serde_json::to_value(commands::git_stash(path)?).unwrap_or(Value::Null))
        }
        "git_stash_pop" => {
            let path: String = arg(args, "path")?;
            Ok(serde_json::to_value(commands::git_stash_pop(path)?).unwrap_or(Value::Null))
        }
        "git_push" => {
            let path: String = arg(args, "path")?;
            Ok(serde_json::to_value(commands::git_push(path)?).unwrap_or(Value::Null))
        }
        "git_fetch" => {
            let path: String = arg(args, "path")?;
            Ok(serde_json::to_value(commands::git_fetch(path)?).unwrap_or(Value::Null))
        }
        "git_pull" => {
            let path: String = arg(args, "path")?;
            Ok(serde_json::to_value(commands::git_pull(path)?).unwrap_or(Value::Null))
        }
        "git_revert" => {
            let path: String = arg(args, "path")?;
            let commit: String = arg(args, "commit")?;
            Ok(serde_json::to_value(commands::git_revert(path, commit)?).unwrap_or(Value::Null))
        }
        "git_init" => {
            let path: String = arg(args, "path")?;
            Ok(serde_json::to_value(commands::git_init(path)?).unwrap_or(Value::Null))
        }
        "git_stage_all" => {
            let path: String = arg(args, "path")?;
            Ok(serde_json::to_value(commands::git_stage_all(path)?).unwrap_or(Value::Null))
        }
        "git_unstage" => {
            let path: String = arg(args, "path")?;
            let file: String = arg(args, "file")?;
            Ok(serde_json::to_value(commands::git_unstage(path, file)?).unwrap_or(Value::Null))
        }
        "git_unstage_all" => {
            let path: String = arg(args, "path")?;
            Ok(serde_json::to_value(commands::git_unstage_all(path)?).unwrap_or(Value::Null))
        }
        "git_discard" => {
            let path: String = arg(args, "path")?;
            let file: String = arg(args, "file")?;
            Ok(serde_json::to_value(commands::git_discard(path, file)?).unwrap_or(Value::Null))
        }
        "git_discard_all" => {
            let path: String = arg(args, "path")?;
            Ok(serde_json::to_value(commands::git_discard_all(path)?).unwrap_or(Value::Null))
        }
        "git_branch_list" => {
            let path: String = arg(args, "path")?;
            Ok(serde_json::to_value(commands::git_branch_list(path)?).unwrap_or(Value::Null))
        }
        "git_checkout_branch" => {
            let path: String = arg(args, "path")?;
            let name: String = arg(args, "name")?;
            let create: Option<bool> = opt_arg(args, "create");
            Ok(
                serde_json::to_value(commands::git_checkout_branch(path, name, create)?)
                    .unwrap_or(Value::Null),
            )
        }
        "git_merge" => {
            let path: String = arg(args, "path")?;
            let from_ref: String = arg(args, "fromRef")?;
            Ok(serde_json::to_value(commands::git_merge(path, from_ref)?).unwrap_or(Value::Null))
        }
        "git_merge_abort" => {
            let path: String = arg(args, "path")?;
            Ok(serde_json::to_value(commands::git_merge_abort(path)?).unwrap_or(Value::Null))
        }
        "git_resolve_conflict" => {
            let path: String = arg(args, "path")?;
            let file: String = arg(args, "file")?;
            let side: String = arg(args, "side")?;
            Ok(
                serde_json::to_value(commands::git_resolve_conflict(path, file, side)?)
                    .unwrap_or(Value::Null),
            )
        }
        "get_cwd" => Ok(serde_json::to_value(commands::get_cwd()?).unwrap_or(Value::Null)),
        "read_file" => {
            let path: String = arg(args, "path")?;
            Ok(serde_json::to_value(commands::read_file(path)?).unwrap_or(Value::Null))
        }
        "read_file_base64" => {
            let path: String = arg(args, "path")?;
            Ok(serde_json::to_value(commands::read_file_base64(path)?).unwrap_or(Value::Null))
        }
        "write_file" => {
            let path: String = arg(args, "path")?;
            let content: String = arg(args, "content")?;
            commands::write_file(path, content)?;
            Ok(Value::Null)
        }
        "read_directory" => {
            let path: String = arg(args, "path")?;
            let max_depth: Option<u32> = opt_arg(args, "maxDepth");
            Ok(
                serde_json::to_value(commands::read_directory(path, max_depth)?)
                    .unwrap_or(Value::Null),
            )
        }
        "file_exists" => {
            let path: String = arg(args, "path")?;
            Ok(serde_json::to_value(commands::file_exists(path)?).unwrap_or(Value::Null))
        }
        "create_file" => {
            let path: String = arg(args, "path")?;
            let content: String = opt_arg(args, "content").unwrap_or_default();
            commands::create_file(path, content)?;
            Ok(Value::Null)
        }
        "create_directory" => {
            let path: String = arg(args, "path")?;
            commands::create_directory(path)?;
            Ok(Value::Null)
        }
        "rename_entry" => {
            let from: String = arg(args, "from")?;
            let to: String = arg(args, "to")?;
            commands::rename_entry(from, to)?;
            Ok(Value::Null)
        }
        "copy_entry" => {
            let from: String = arg(args, "from")?;
            let to: String = arg(args, "to")?;
            commands::copy_entry(from, to)?;
            Ok(Value::Null)
        }
        "delete_entry" => {
            let path: String = arg(args, "path")?;
            commands::delete_entry(path)?;
            Ok(Value::Null)
        }
        "upload_file" => {
            let path: String = arg(args, "path")?;
            let content: String = arg(args, "content")?;
            commands::upload_file(path, content)?;
            Ok(Value::Null)
        }
        "download_file" => {
            let path: String = arg(args, "path")?;
            Ok(serde_json::to_value(commands::download_file(path)?).unwrap_or(Value::Null))
        }
        "extract_zip" => {
            let zip_content_b64: String = arg(args, "zipContent")?;
            let dest_dir: String = arg(args, "destDir")?;
            Ok(
                serde_json::to_value(commands::extract_zip(zip_content_b64, dest_dir)?)
                    .unwrap_or(Value::Null),
            )
        }
        "list_files" => {
            let path: String = arg(args, "path")?;
            Ok(serde_json::to_value(commands::list_files(path)?).unwrap_or(Value::Null))
        }
        "search_in_files" => {
            let path: String = arg(args, "path")?;
            let query: String = arg(args, "query")?;
            let cs: Option<bool> = opt_arg(args, "caseSensitive");
            let ww: Option<bool> = opt_arg(args, "wholeWord");
            let rx: Option<bool> = opt_arg(args, "regex");
            Ok(
                serde_json::to_value(commands::search_in_files(path, query, cs, ww, rx)?)
                    .unwrap_or(Value::Null),
            )
        }
        "chat_stream" => {
            let command: String = arg(args, "command")?;
            let cmd_args: Vec<String> = opt_arg(args, "args").unwrap_or_default();
            let cwd: Option<String> = opt_arg(args, "cwd");
            let api_key: String = opt_arg(args, "apiKey").unwrap_or_default();
            let mode: Option<String> = opt_arg(args, "mode");
            let attachments: Vec<commands::ChatAttachmentInput> =
                opt_arg(args, "attachments").unwrap_or_default();
            // Frontend-provided requestId (assistant UUID) for event matching.
            let rid: String = opt_arg(args, "requestId").unwrap_or_else(|| req.id.to_string());
            let event_tx = out_tx;
            let procs = processes.clone();
            let agent_servers = agent_servers.clone();
            tokio::spawn(async move {
                use std::process::Stdio;
                use tokio::io::{AsyncBufReadExt, BufReader};

                // The frontend sends cwd = focused/working directory so the
                // agent runs in the folder the user has open. If that path
                // doesn't exist on THIS server (stale session from another
                // backend, deleted folder, etc.), `cmd.current_dir(dir)`
                // makes spawn() fail with ENOENT, which surfaces as
                // ErrorKind::NotFound and used to be misreported as "CLI not
                // on your PATH". Validate it first and say what's actually
                // wrong.
                if let Some(dir) = cwd.as_deref() {
                    let p = std::path::Path::new(dir);
                    if !p.exists() {
                        let error = format!(
                            "Working directory '{}' does not exist on this server. \
                             Open a valid folder (or clear the saved session) and try again.",
                            dir
                        );
                        let msg = json!({"event": "chat-stream-done", "payload": {"id": &rid, "error": error}});
                        let _ = event_tx.send(Message::Text(msg.to_string())).await;
                        return;
                    }
                    if !p.is_dir() {
                        let error = format!(
                            "Working directory '{}' is not a directory. \
                             Open a valid folder and try again.",
                            dir
                        );
                        let msg = json!({"event": "chat-stream-done", "payload": {"id": &rid, "error": error}});
                        let _ = event_tx.send(Message::Text(msg.to_string())).await;
                        return;
                    }
                }

                // Resolve the actual executable (handles PATH and common install
                // locations like nvm) so a GUI-launched process can still find it.
                // We also reject shadowing binaries here — a stale `opencode` or
                // `kilo` in `~/.local/bin` would otherwise be picked before the
                // real one in `~/.opencode/bin` / `~/.kilo/bin`, ignore our
                // `--format json` / `--pure` flags, and produce a confusing
                // silent failure. `resolve_agent_bin` already does the
                // identity check, so a returned `Some` that fails the stricter
                // `chat_cli_check` here means it's a wrong binary shadowing
                // the real one.
                let exe = match commands::resolve_agent_bin(&command) {
                    Some(e) => e,
                    None => {
                        let error = commands::cli_missing_message(&command);
                        let msg = json!({"event": "chat-stream-done", "payload": {"id": &rid, "error": error}});
                        let _ = event_tx.send(Message::Text(msg.to_string())).await;
                        return;
                    }
                };
                if !matches!(
                    commands::chat_cli_check(&command),
                    commands::CliStatus::Available
                ) {
// resolve_agent_bin returned a path, but the binary at that
                // path doesn't look like the agent we asked for. Surface a
                // clear "wrong binary" error instead of letting the run
                // proceed and produce a silent failure.
                let error = commands::cli_wrong_binary_message(&command);
                let msg = json!({"event": "chat-stream-done", "payload": {"id": &rid, "error": error}});
                let _ = event_tx.send(Message::Text(msg.to_string())).await;
                return;
                }

                // Materialize attachment contents to temp files and pass them via
                // the CLI's native `-f/--file` flag instead of inlining huge
                // base64 blobs into the (size-limited) prompt argument.
                let attachment_paths = match commands::write_attachment_files(&attachments) {
                    Ok(p) => p,
                    Err(e) => {
                        let msg = json!({"event": "chat-stream-done", "payload": {"id": &rid, "error": e}});
                        let _ = event_tx.send(Message::Text(msg.to_string())).await;
                        return;
                    }
                };
                let mut cmd_args = cmd_args;
                commands::inject_attachment_args(&mut cmd_args, &attachment_paths);

                // Persistent server (#2): ensure the agent's long-lived `serve`
                // process is up, then turn this turn into a thin `run --attach`
                // so we reuse the warm server instead of rebooting the full CLI.
                match agent_servers.ensure_server(&command, &api_key).await {
                    Ok(port) => {
                        let attach = format!("http://127.0.0.1:{}", port);
                        // cmd_args[0] is the "run" subcommand; insert --attach
                        // right after it so it applies to the run invocation.
                        if cmd_args.first().map(|s| s.as_str()) == Some("run") {
                            cmd_args.insert(1, "--attach".to_string());
                            cmd_args.insert(2, attach);
                        } else {
                            cmd_args.insert(0, "--attach".to_string());
                            cmd_args.insert(1, attach);
                        }
                    }
                    Err(e) => {
                        let msg = json!({"event": "chat-stream-done",
                          "payload": {"id": &rid, "error": e}});
                        let _ = event_tx.send(Message::Text(msg.to_string())).await;
                        return;
                    }
                }

                let mut cmd = tokio::process::Command::new(&exe);
                cmd.args(&cmd_args)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                // Put the CLI in its own process group so we can kill the
                // entire tree (CLI + any grandchildren from tool execution)
                // with a single killpg call.  Without this, child.kill()
                // only reaps the direct child and orphaned grandchildren
                // waste RAM/CPU on low-end devices.
                #[cfg(unix)]
                {
                    cmd.process_group(0);
                }
                if let Some(dir) = cwd {
                    cmd.current_dir(dir);
                }
                // Pass the API key as an env var so the CLI (e.g. opencode)
                // can authenticate with providers (OpenRouter, etc.) even when
                // its own credential store (~/.local/share/opencode/auth.json)
                // is empty.
                if !api_key.trim().is_empty() {
                    cmd.env("OPENROUTER_API_KEY", &api_key);
                }
                // Run the agent CLI non-interactively and color-free so
                // captured tool output is clean, deterministic and can never
                // hang: NO_COLOR/CLICOLOR/TERM=dumb kill ANSI (incl.
                // termcolor tools), CI=1 silences spinners/banners, C.UTF-8 +
                // PYTHONIOENCODING keep tool output deterministic, and
                // PAGER=cat means no command can block on an interactive
                // pager. FORCE_COLOR is removed so nothing re-enables color.
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
                // Switch kilo/opencode's real mode via an inline config. Prompt
                // seeded instructions are not enough — kilo's mode system
                // overrides them; KILO_CONFIG_CONTENT is deep-merged with highest
                // precedence, so this actually switches Code/Ask/Plan. Only kilo
                // reads this env var; opencode switches mode via the
                // `--agent`/`--permissions` CLI flags (set in the frontend args).
                if command == "kilo" {
                    if let Some(m) = &mode {
                        if !m.trim().is_empty() {
                            let kilo_mode = match m.as_str() {
                                "ask" => "ask",
                                "plan" => "plan",
                                _ => "code",
                            };
                            cmd.env(
                                "KILO_CONFIG_CONTENT",
                                format!("{{\"mode\":\"{}\"}}", kilo_mode),
                            );
                        }
                    }
                }

                let child = match cmd.spawn() {
                    Ok(c) => {
                        // Register PID with the global tracker so the ctrlc
                        // handler can reap it on server shutdown.
                        if let Some(pid) = c.id() {
                            chat_tracker.register(pid);
                        }
                        c
                    }
                    Err(e) => {
                        // By the time we get here, resolve_agent_bin + the
                        // chat_cli_check above already verified the exe exists
                        // and identifies as the right agent, and cwd was
                        // validated to be a real directory. A NotFound from
                        // spawn at this point is therefore NOT "CLI not on
                        // PATH" (the old message actively misled users whose
                        // working directory was stale) — surface the real
                        // error verbatim instead.
                        commands::cleanup_attachment_files(&attachment_paths);
                        let error = format!("Failed to run {}: {}", command, e);
                        let msg = json!({"event": "chat-stream-done", "payload": {"id": &rid, "error": error}});
                        let _ = event_tx.send(Message::Text(msg.to_string())).await;
                        return;
                    }
                };

                // Stream stdout/stderr before waiting so lines flow to the frontend.
                let mut child = child;
                let stdout = child.stdout.take();
                let stderr = child.stderr.take();

                // Store the PID (not the Child) so chat_cancel can kill by PID
                // without needing mutable access to the Child handle.
                let child_pid = child.id();
                if let Some(pid) = child_pid {
                    procs.lock().await.insert(rid.clone(), pid);
                }

                // Spawn line readers (no lock held).
                if let Some(stdout) = stdout {
                    let tx = event_tx.clone();
                    let id = rid.clone();
                    tokio::spawn(async move {
                        let mut lines = BufReader::new(stdout).lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            let msg = json!({"event": "chat-stream-chunk", "payload": {"id": &id, "stream": "stdout", "text": line}});
                            if tx.send(Message::Text(msg.to_string())).await.is_err() {
                                break;
                            }
                        }
                    });
                }
                if let Some(stderr) = stderr {
                    let tx = event_tx.clone();
                    let id = rid.clone();
                    tokio::spawn(async move {
                        let mut lines = BufReader::new(stderr).lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            let msg = json!({"event": "chat-stream-chunk", "payload": {"id": &id, "stream": "stderr", "text": line}});
                            if tx.send(Message::Text(msg.to_string())).await.is_err() {
                                break;
                            }
                        }
                    });
                }

                // Unregister from global tracker.
                if let Some(pid) = child_pid {
                    chat_tracker.unregister(pid);
                }
                // Wait on the local Child handle (not via procs).
                // chat_cancel kills by PID so it doesn't need to remove from procs.
                let wait_result = child.wait().await;

                // Remove PID from procs (cleanup).
                procs.lock().await.remove(&rid);

                // The CLI has read the attachment files by now — reclaim the
                // temp space regardless of how the run ended.
                commands::cleanup_attachment_files(&attachment_paths);

                match wait_result {
                    Ok(status) if status.success() => {
                        let msg = json!({"event": "chat-stream-done", "payload": {"id": &rid}});
                        let _ = event_tx.send(Message::Text(msg.to_string())).await;
                    }
                    Ok(status) => {
                        let code = status.code().unwrap_or(-1);
                        let msg = json!({"event": "chat-stream-done", "payload": {"id": &rid, "error": format!("{} exited with code {}", command, code)}});
                        let _ = event_tx.send(Message::Text(msg.to_string())).await;
                    }
                    Err(e) => {
                        let msg = json!({"event": "chat-stream-done", "payload": {"id": &rid, "error": format!("Process error: {}", e)}});
                        let _ = event_tx.send(Message::Text(msg.to_string())).await;
                    }
                }
            });
            Ok(Value::Null)
        }
        "chat_cancel" => {
            let cancel_id: String = arg(args, "id")?;
            let pid = {
                let guard = processes.lock().await;
                guard.get(&cancel_id).copied()
            };
            if let Some(pid) = pid {
                // Kill the entire process group (CLI + grandchildren) not
                // just the direct child.
                #[cfg(unix)]
                {
                    chat_tracker.unregister(pid);
                    unsafe {
                        libc::kill(-(pid as i32), libc::SIGKILL);
                    }
                }
                #[cfg(not(unix))]
                {
                    use windows_sys::Win32::Foundation::CloseHandle;
                    use windows_sys::Win32::System::Threading::{
                        OpenProcess, TerminateProcess, PROCESS_TERMINATE,
                    };
                    let handle = unsafe {
                        OpenProcess(PROCESS_TERMINATE, 0, pid)
                    };
                    if !handle.is_null() {
                        unsafe { TerminateProcess(handle, 1); }
                        unsafe { CloseHandle(handle); }
                    }
                }
                Ok(json!({ "cancelled": true }))
            } else {
                Ok(json!({ "cancelled": false, "reason": "Process not found" }))
            }
        }
        "chat_models" => {
            let agent: String = arg(args, "agent")?;
            let api_key: String = opt_arg(args, "apiKey").unwrap_or_default();
            Ok(
                serde_json::to_value(commands::chat_models(agent, api_key).await?)
                    .unwrap_or(Value::Null),
            )
        }
        "chat_check_install" => {
            let command: String = arg(args, "command")?;
            // Return the discrete status string ("available" | "missing" |
            // "wrong") so the frontend can distinguish a shadowing binary
            // (WrongBinary) from a real install. Previously this returned a
            // plain boolean, which collapsed Missing and WrongBinary into
            // "not installed" and was the reason a stale `~/.local/bin`
            // shadow was silently used as the agent CLI.
            Ok(serde_json::to_value(commands::chat_cli_check(&command).as_str()).unwrap_or(Value::Null))
        }
        other => Err(format!("unknown command: {}", other)),
    }
}
