use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use futures_util::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio::sync::mpsc::Sender as MpscSender;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::http;
use tokio_tungstenite::tungstenite::Message;

use crate::agent_servers::AgentServerManager;
use crate::commands;
use crate::pty::{PtyEvent, PtyManager};

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

/// Maps a session id to the connection that owns it. When a session is killed
/// (tab close / page unload) we signal that connection's "close" channel so its
/// WebSocket is dropped and `handle_connection` reaps the PTY process even if
/// the client disappeared without a clean close frame.
type Connections = Arc<Mutex<HashMap<String, MpscSender<()>>>>;

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

pub async fn start(
    pty: Arc<Mutex<PtyManager>>,
    tx: broadcast::Sender<PtyEvent>,
    addr: &str,
    chat_tracker: Arc<ChatProcessTracker>,
    agent_servers: Arc<AgentServerManager>,
    token: Option<String>,
) -> std::io::Result<()> {
    let connections: Connections = Arc::new(Mutex::new(HashMap::new()));
    let listener = TcpListener::bind(addr).await?;
    eprintln!("[NoIDE] WebSocket PTY server listening on ws://{}", addr);
    loop {
        let (stream, _) = listener.accept().await?;
        let pty = pty.clone();
        let tx = tx.clone();
        let connections = connections.clone();
        let chat_tracker = chat_tracker.clone();
        let agent_servers = agent_servers.clone();
        let token = token.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(
                stream,
                pty,
                tx,
                connections,
                chat_tracker,
                agent_servers,
                token,
            )
            .await
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
    tx: broadcast::Sender<PtyEvent>,
    connections: Connections,
    chat_tracker: Arc<ChatProcessTracker>,
    agent_servers: Arc<AgentServerManager>,
    expected_token: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
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

    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<Message>();

    // Tracks running chat-stream child processes so they can be cancelled.
    let processes = Arc::new(tokio::sync::Mutex::new(HashMap::<
        String,
        tokio::process::Child,
    >::new()));

    // Sessions spawned over THIS connection. On disconnect we kill only these
    // so other frontends' terminals keep running.
    let mut owned_sessions: Vec<String> = Vec::new();

    // A "close" signal lets another task (e.g. pty_kill) drop this connection
    // so its owned PTY sessions are reaped even if the client vanished. Shared
    // via Arc<Notify> because the channel receiver is single-consumer.
    let (close_tx, _close_rx) = tokio::sync::mpsc::channel::<()>(1);
    let close_notify = std::sync::Arc::new(tokio::sync::Notify::new());
    let close_notify_write = close_notify.clone();

    // Task that drains outgoing messages to the websocket. It also watches the
    // close signal: when a session this connection owns is killed, we stop
    // writing so the socket closes and handle_connection reaps the PTY.
    tokio::spawn(async move {
        loop {
            tokio::select! {
                msg = out_rx.recv() => {
                    match msg {
                        Some(m) => { if writer.send(m).await.is_err() { break; } }
                        None => break,
                    }
                }
                _ = close_notify_write.notified() => break,
            }
        }
    });

    // Task that forwards PTY events to the client.
    let mut event_rx = tx.subscribe();
    let event_tx = out_tx.clone();
    tokio::spawn(async move {
        while let Ok(ev) = event_rx.recv().await {
            let (event, payload) = match ev {
                PtyEvent::Output(o) => (
                    "pty-output".to_string(),
                    serde_json::to_value(o).unwrap_or(Value::Null),
                ),
                PtyEvent::Exit(e) => (
                    "pty-exit".to_string(),
                    serde_json::to_value(e).unwrap_or(Value::Null),
                ),
            };
            let msg = json!({ "event": event, "payload": payload });
            if event_tx.send(Message::Text(msg.to_string())).is_err() {
                break;
            }
        }
    });

    // Idle timeout: if the client goes silent (e.g. browser killed without a
    // close frame, network drop), reap its sessions instead of leaking them.
    // Any incoming message resets the clock. 60s is a generous ceiling; a
    // live terminal sends keystrokes/resize events far sooner.
    let idle_limit = std::time::Duration::from_secs(60);
    let last_activity = std::sync::Arc::new(std::sync::Mutex::new(std::time::Instant::now()));

    loop {
        let since = last_activity.lock().unwrap().elapsed();
        if since >= idle_limit {
            eprintln!("[NoIDE] idle timeout; closing connection");
            break;
        }
        let timeout = tokio::time::sleep(idle_limit - since);
        let next = tokio::select! {
            msg = reader.next() => msg,
            _ = close_notify.notified() => break,
            _ = timeout => { eprintln!("[NoIDE] idle timeout; closing connection"); break; }
        };
        *last_activity.lock().unwrap() = std::time::Instant::now();
        let msg = match next {
            Some(Ok(m)) => m,
            Some(Err(_)) => break,
            None => break,
        };
        match msg {
            Message::Text(text) => {
                let req: Request = match serde_json::from_str(&text) {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = out_tx.send(Message::Text(
                                json!({ "id": 0, "ok": false, "error": format!("invalid request: {}", e) })
                                    .to_string(),
                            ));
                        continue;
                    }
                };
                // Remember sessions this connection owns so we can reap
                // them when the client disappears (close/rerun/project
                // switch) and the frontend never gets a chance to call
                // pty_kill.
                if req.command == "pty_spawn" {
                    if let Ok(sid) = serde_json::from_value::<String>(
                        req.args.get("sessionId").cloned().unwrap_or(Value::Null),
                    ) {
                        if !owned_sessions.contains(&sid) {
                            owned_sessions.push(sid.clone());
                        }
                        connections.lock().unwrap().insert(sid, close_tx.clone());
                    }
                }
                // Handle each command in its own task so a slow command
                // (e.g. a full-tree `list_files` walk) can't block faster
                // ones (e.g. `read_directory`) on this single connection.
                // Responses are tagged by id and matched client-side, so
                // out-of-order replies are harmless.
                let pty_t = pty.clone();
                let out_tx_t = out_tx.clone();
                let processes_t = processes.clone();
                let connections_t = connections.clone();
                let close_notify_t = close_notify.clone();
                let chat_tracker_t = chat_tracker.clone();
                let agent_servers_t = agent_servers.clone();
                tokio::spawn(async move {
                    let res = handle(
                        &req,
                        &pty_t,
                        out_tx_t.clone(),
                        processes_t,
                        connections_t,
                        close_notify_t,
                        chat_tracker_t,
                        agent_servers_t,
                    )
                    .await;
                    let resp = match res {
                        Ok(v) => json!({ "id": req.id, "ok": true, "result": v }),
                        Err(e) => json!({ "id": req.id, "ok": false, "error": e }),
                    };
                    let _ = out_tx_t.send(Message::Text(resp.to_string()));
                });
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    // Drop our ownership records so a future session with the same id (after
    // a reconnect) maps to the new connection.
    {
        let mut guard = connections.lock().unwrap();
        for sid in &owned_sessions {
            guard.remove(sid);
        }
    }

    // Connection dropped: clean up anything this client owned so nothing
    // leaks as an orphaned process on the host.
    eprintln!(
        "[NoIDE] connection closed; owned_sessions={:?}",
        owned_sessions
    );
    if !owned_sessions.is_empty() {
        if let Ok(mut mgr) = pty.lock() {
            for sid in owned_sessions {
                match mgr.kill(&sid) {
                    Ok(_) => eprintln!("[NoIDE] reaped session {}", sid),
                    Err(e) => eprintln!("[NoIDE] failed to reap {}: {}", sid, e),
                }
            }
        }
    }
    // Kill any chat-stream subprocesses still running for this client.
    // Use killpg to reap the entire process tree (CLI + grandchildren)
    // instead of child.kill() which only kills the direct child.
    let mut procs = processes.lock().await;
    for (_id, mut child) in procs.drain() {
        #[cfg(unix)]
        {
            if let Some(pid) = child.id() {
                chat_tracker.unregister(pid);
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            }
        }
        let _ = child.kill().await;
    }

    Ok(())
}

// Central command dispatch: every arg is a distinct context handle threaded
// through the match arms. Grouping them would churn the whole dispatch.
#[allow(clippy::too_many_arguments)]
async fn handle(
    req: &Request,
    pty: &Arc<Mutex<PtyManager>>,
    out_tx: tokio::sync::mpsc::UnboundedSender<Message>,
    processes: Arc<tokio::sync::Mutex<HashMap<String, tokio::process::Child>>>,
    connections: Connections,
    close_notify: std::sync::Arc<tokio::sync::Notify>,
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
            let mut mgr = pty.lock().map_err(|e| e.to_string())?;
            let session = mgr.spawn(
                session_id,
                shell,
                cwd,
                cols.unwrap_or(80),
                rows.unwrap_or(24),
            )?;
            Ok(serde_json::to_value(session).unwrap_or(Value::Null))
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
            // Signal the owning connection to close so its WebSocket drops and
            // handle_connection reaps the PTY (and any jobs inside it). This also
            // covers the case where the client already vanished without a clean
            // close frame.
            let close_tx = connections.lock().unwrap().get(&session_id).cloned();
            if let Some(close_tx) = close_tx {
                let _ = close_tx.send(()).await;
            }
            // Notify both reader and writer tasks to stop.
            close_notify.notify_waiters();
            let mut mgr = pty.lock().map_err(|e| e.to_string())?;
            mgr.kill(&session_id)?;
            Ok(Value::Null)
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
            let event_tx = out_tx.clone();
            let procs = processes.clone();
            let agent_servers = agent_servers.clone();
            tokio::spawn(async move {
                use std::process::Stdio;
                use tokio::io::{AsyncBufReadExt, BufReader};

                // Resolve the actual executable (handles PATH and common install
                // locations like nvm) so a GUI-launched process can still find it.
                let exe = match commands::resolve_agent_bin(&command) {
                    Some(e) => e,
                    None => {
                        let error = commands::cli_missing_message(&command);
                        let msg = json!({"event": "chat-stream-done", "payload": {"id": &rid, "error": error}});
                        let _ = event_tx.send(Message::Text(msg.to_string()));
                        return;
                    }
                };

                // Materialize attachment contents to temp files and pass them via
                // the CLI's native `-f/--file` flag instead of inlining huge
                // base64 blobs into the (size-limited) prompt argument.
                let attachment_paths = match commands::write_attachment_files(&attachments) {
                    Ok(p) => p,
                    Err(e) => {
                        let msg = json!({"event": "chat-stream-done", "payload": {"id": &rid, "error": e}});
                        let _ = event_tx.send(Message::Text(msg.to_string()));
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
                        let _ = event_tx.send(Message::Text(msg.to_string()));
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
                        // Only a genuinely missing binary should be reported as
                        // the "not on your PATH" error. Other failures (e.g.
                        // argument list too long) are real and must be surfaced
                        // verbatim, not masked as a PATH problem.
                        commands::cleanup_attachment_files(&attachment_paths);
                        let error = if e.kind() == std::io::ErrorKind::NotFound {
                            commands::cli_missing_message(&command)
                        } else {
                            format!("Failed to run {}: {}", command, e)
                        };
                        let msg = json!({"event": "chat-stream-done", "payload": {"id": &rid, "error": error}});
                        let _ = event_tx.send(Message::Text(msg.to_string()));
                        return;
                    }
                };

                // Stream stdout/stderr before waiting so lines flow to the frontend.
                let mut child = child;
                let stdout = child.stdout.take();
                let stderr = child.stderr.take();

                // Register the child so chat_cancel can find and kill it.
                procs.lock().await.insert(rid.clone(), child);

                // Spawn line readers (no lock held).
                if let Some(stdout) = stdout {
                    let tx = event_tx.clone();
                    let id = rid.clone();
                    tokio::spawn(async move {
                        let mut lines = BufReader::new(stdout).lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            let msg = json!({"event": "chat-stream-chunk", "payload": {"id": &id, "stream": "stdout", "text": line}});
                            if tx.send(Message::Text(msg.to_string())).is_err() {
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
                            if tx.send(Message::Text(msg.to_string())).is_err() {
                                break;
                            }
                        }
                    });
                }

                // Remove from map and wait WITHOUT holding the lock.
                // This lets chat_cancel acquire the lock and kill the process.
                let mut removed_child = procs.lock().await.remove(&rid);
                // Unregister from global tracker before waiting.
                if let Some(ref c) = removed_child {
                    if let Some(pid) = c.id() {
                        chat_tracker.unregister(pid);
                    }
                }
                let wait_result = if let Some(ref mut c) = removed_child {
                    c.wait().await
                } else {
                    // Process was already cancelled by chat_cancel.
                    commands::cleanup_attachment_files(&attachment_paths);
                    return;
                };
                drop(removed_child);

                // The CLI has read the attachment files by now — reclaim the
                // temp space regardless of how the run ended.
                commands::cleanup_attachment_files(&attachment_paths);

                match wait_result {
                    Ok(status) if status.success() => {
                        let msg = json!({"event": "chat-stream-done", "payload": {"id": &rid}});
                        let _ = event_tx.send(Message::Text(msg.to_string()));
                    }
                    Ok(status) => {
                        let code = status.code().unwrap_or(-1);
                        let msg = json!({"event": "chat-stream-done", "payload": {"id": &rid, "error": format!("{} exited with code {}", command, code)}});
                        let _ = event_tx.send(Message::Text(msg.to_string()));
                    }
                    Err(e) => {
                        let msg = json!({"event": "chat-stream-done", "payload": {"id": &rid, "error": format!("Process error: {}", e)}});
                        let _ = event_tx.send(Message::Text(msg.to_string()));
                    }
                }
            });
            Ok(Value::Null)
        }
        "chat_cancel" => {
            let cancel_id: String = arg(args, "id")?;
            let mut guard = processes.lock().await;
            if let Some(mut child) = guard.remove(&cancel_id) {
                // Kill the entire process group (CLI + grandchildren) not
                // just the direct child.  child.kill() only sends SIGKILL
                // to the one PID; tool-execution subprocesses survive.
                #[cfg(unix)]
                {
                    if let Some(pid) = child.id() {
                        chat_tracker.unregister(pid);
                        unsafe {
                            libc::kill(-(pid as i32), libc::SIGKILL);
                        }
                    }
                    // child.kill().await reaps the zombie so tokio doesn't
                    // leak the waitpid entry.  The process is already dead
                    // from killpg, so this is just the reaping call.
                    let _ = child.kill().await;
                }
                #[cfg(not(unix))]
                {
                    let _ = child.kill().await;
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
            Ok(serde_json::to_value(commands::chat_cli_available(&command)).unwrap_or(Value::Null))
        }
        other => Err(format!("unknown command: {}", other)),
    }
}
