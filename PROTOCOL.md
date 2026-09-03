# NoIDE WebSocket Protocol

The contract between the frontend (`web/`) and the server (`server/`, the
`noide-server` binary). Every feature — file tree, editor, terminal, git,
chat — speaks only this protocol; the UI never assumes it runs on the same
machine as the server.

Source of truth:
- Server dispatch: `server/src/ws_server.rs`
- Server commands: `server/src/commands.rs`, `server/src/pty.rs`
- Client: `web/src/composables/useBackend.ts`
- TS mirror of payload shapes: `web/src/types/index.ts`

---

## 1. Connection

- Plain WebSocket, text frames, UTF-8 JSON.
- Default endpoint: `ws://0.0.0.0:1421` (bind address override via the
  `NOTERM_WS_ADDR` env var, e.g. `127.0.0.1:1421`).
- Endpoint selection (client): user-configured `serverWsUrl` setting →
  `VITE_WS_URL` env → `ws://<current-host>:1421` (or `ws://127.0.0.1:1421`
  for loopback hosts).

### Authentication

`noide-server` runs in one of three auth modes. In every protected mode the
client presents a token as a query parameter on the WebSocket URL; a missing
or wrong token rejects the upgrade with HTTP `401`, which browsers surface as
a connection error. Tokens are compared in constant time.

```
ws://host:1421/?token=<value>
wss://host:1421/?token=<value>
```

| Mode | How to start | Client behavior |
|---|---|---|
| **Pairing (default)** | no flags | Server prints a fresh `XXXX-XXXX` code plus an ASCII QR at startup. Enter the code in the app (connect screen → *Pairing code*, or Settings → Server (Pairing Code)). The code is ephemeral — valid until the server exits, never stored — so a restart mints a new one. |
| **Fixed token** | `--token <value>` or `NOIDE_TOKEN` | Use that value as the token. Survives restarts (it is your secret). |
| **No auth** | `--no-auth` | Server accepts unauthenticated connections and prints a loud warning. For localhost development / CI only. |

The QR encodes the bare code text (nothing else), so scanning later — e.g. a
native camera plugin in the mobile app — fills in the same value the manual
entry path accepts.

> Send tokens only over `wss://` (or a trusted LAN). The query string can be
> logged by intermediaries, so never reuse a long-lived credential here — a
> per-boot random pairing code or a fresh random `--token` is the right shape.

## 2. Message framing

Three message kinds, all one JSON object per text frame.

### Requests (client → server)

```json
{ "id": 1, "command": "read_file", "args": { "path": "/home/user/a.txt" } }
```

| Field     | Type   | Notes                                            |
|-----------|--------|--------------------------------------------------|
| `id`      | number | Client-chosen correlation id, echoed in the reply. |
| `command` | string | One of the commands in §5.                        |
| `args`    | object | Command arguments (camelCase keys), may be `{}`.  |

### Replies (server → client)

```json
{ "id": 1, "ok": true,  "result": { ... } }
{ "id": 1, "ok": false, "error": "File already exists" }
```

Commands run concurrently on the server and replies may arrive **out of
order** — match them by `id`. A malformed frame gets
`{ "id": 0, "ok": false, "error": "invalid request: …" }`; an unknown command
gets `error: "unknown command: …"`.

### Events (server → client)

```json
{ "event": "pty-output", "payload": { ... } }
```

Events are fire-and-forget broadcasts with no `id`. The client registers a
per-event listener and matches `payload.id` / `payload.session_id` to its own
session/request ids.

## 3. Connection lifecycle

- **Ownership:** PTY sessions spawned over a connection belong to that
  connection. When it closes (clean close frame, network drop, or idle
  timeout) the server reaps every session the connection owned plus any
  in-flight agent chat subprocesses — other clients' terminals keep running.
- **Idle timeout:** a connection that sends nothing for 60 seconds is closed
  and its sessions reaped (prevents leaks when a tab is killed without a close
  frame). Any incoming message resets the clock.
- **Client behavior:** `useBackend.ts` auto-reconnects with exponential
  backoff (400 ms → 1500 ms cap) and resolves a per-command 30-second timeout
  with `error: "Backend timeout"`.
- **Server shutdown:** SIGINT/SIGTERM kills all live PTY sessions and agent
  processes (process groups) so nothing orphans on the host.

## 4. Events

### `pty-output`
```json
{ "session_id": "pty-123", "data": "\u001b[1m$ " }
```
Raw terminal bytes from a PTY session (lossy UTF-8 conversion; ANSI escapes
intact).

### `pty-exit`
```json
{ "session_id": "pty-123", "code": 0 }
```
`code` is `null` when the exit code is unknown.

### `chat-stream-chunk`
```json
{ "id": "assistant-uuid", "stream": "stdout", "text": "<one line>" }
```
One line per chunk from the agent CLI's stdout/stderr. `id` is the
`requestId` passed to `chat_stream`.

### `chat-stream-done`
```json
{ "id": "assistant-uuid" }
{ "id": "assistant-uuid", "error": "kilo exited with code 1" }
```
Terminates a chat stream. `error` present only on failure.

## 5. Commands

Conventions: `path` args are server-side paths. Optional args are marked `?`.
`→ null` commands succeed with an empty result.

### Terminal (PTY)
| Command | Args | Result |
|---|---|---|
| `pty_spawn` | `sessionId`, `shell?`, `cwd?`, `cols?`, `rows?` | `{ id, pid }` |
| `pty_write` | `sessionId`, `data` | `→ null` |
| `pty_resize` | `sessionId`, `cols`, `rows` | `→ null` |
| `pty_kill`  | `sessionId` | `→ null` (closes the owning connection) |

### Shell / misc
| Command | Args | Result |
|---|---|---|
| `run_command` | `command` | combined stdout+stderr string |
| `get_cwd` | — | working directory string |
| `chat_check_install` | `command` (agent id) | `boolean` |

### File system
| Command | Args | Result |
|---|---|---|
| `read_file` | `path` | file text |
| `read_file_base64` | `path` | base64 string (images etc.) |
| `write_file` | `path`, `content` | `→ null` |
| `upload_file` | `path`, `content` (base64) | `→ null` |
| `download_file` | `path` | base64 string |
| `read_directory` | `path`, `maxDepth?` | `FileNode[]` |
| `list_files` | `path` | `string[]` (recursive relative paths) |
| `file_exists` | `path` | `boolean` |
| `create_file` | `path`, `content?` | `→ null` |
| `create_directory` | `path` | `→ null` |
| `rename_entry` | `from`, `to` | `→ null` |
| `copy_entry` | `from`, `to` | `→ null` |
| `delete_entry` | `path` | `→ null` |
| `extract_zip` | `zipContent` (base64), `destDir` | `string[]` extracted paths |
| `search_in_files` | `path`, `query`, `caseSensitive?`, `wholeWord?`, `regex?` | `SearchMatch[]` |

```ts
// FileNode (camelCase)
{ name: string; path: string; isDirectory: boolean;
  children?: FileNode[]; extension?: string; size?: number; modified?: number }

// SearchMatch (camelCase)
{ file: string; lineNumber: number; lineText: string;
  matchStart: number; matchEnd: number }   // matchStart/End are char offsets in lineText
```

### Git
All git commands take `path` (the repo dir) plus command-specific args.
String results are the CLI's trimmed output.

| Command | Extra args | Result |
|---|---|---|
| `get_git_status` | — | `GitStatus` |
| `get_git_diff` | `file` | unified diff string |
| `get_git_staged_diff` | `file` | unified diff string |
| `get_git_head_file` | `file` | file text at HEAD |
| `git_show_index` | `file` | file text from the index |
| `git_add` | `file` | output string |
| `git_stage_all` | — | output string |
| `git_unstage` | `file` | output string |
| `git_unstage_all` | — | output string |
| `git_discard` | `file` | output string |
| `git_discard_all` | — | output string |
| `git_commit` | `message`, `amend?` | output string |
| `git_undo_commit` | — | output string |
| `git_log` | `limit?` | `GitCommitInfo[]` |
| `git_commit_diff` | `commit` | unified diff string |
| `git_stash_list` | — | `GitStashInfo[]` |
| `git_stash` | — | output string |
| `git_stash_pop` | — | output string |
| `git_push` | — | output string |
| `git_fetch` | — | output string |
| `git_pull` | — | output string |
| `git_revert` | `commit` | output string |
| `git_init` | — | output string |
| `git_branch_list` | — | `GitBranches` |
| `git_checkout_branch` | `name`, `create?` | output string |
| `git_merge` | `fromRef` | output string (non-zero exit is *not* fatal — check result) |
| `git_merge_abort` | — | output string |
| `git_resolve_conflict` | `file`, `side` (`"ours"`/`"theirs"`) | output string |

```ts
// GitStatus (camelCase; detail maps are optional)
{ branch: string; ahead?: number; behind?: number;
  modified: string[]; staged: string[]; untracked: string[]; deleted: string[];
  unmerged: string[];
  stagedDetails?: Record<string, string>;    // per-file status letter (A/M/D/R/C/T)
  worktreeDetails?: Record<string, string>;  // per-file status letter (M/D/U)
  worktreeStats?: Record<string, GitFileStat>;
  stagedStats?:   Record<string, GitFileStat>; }

// GitFileStat: { insertions: number; deletions: number }
// GitCommitInfo: { hash, shortHash, message, author, time (unix s),
//                  insertions, deletions }
// GitStashInfo: { ref, message, time (unix s) }
// GitBranches: { current: string, branches: string[], remotes: string[] }
```

### Agent chat (kilo / opencode)
| Command | Args | Result |
|---|---|---|
| `chat_stream` | `command` (agent id), `args?` (CLI args incl. `-m` model), `cwd?`, `apiKey?`, `mode?`, `attachments?`, `requestId?` | fires `chat-stream-chunk` / `chat-stream-done` events; reply `→ null` |
| `chat_cancel` | `id` (the `requestId`) | `{ cancelled: boolean, reason?: string }` |
| `chat_models` | `agent`, `apiKey?` | `{ id, label }[]` |

`chat_stream` runs the agent's thin client (`run --attach`) against a
long-lived `serve` process the server manages per agent. The frontend
subscribes to events **before** invoking so no early chunk is missed, matches
chunks by `requestId`, and reconciles the stream with `chat-stream-done`.
`chat_cancel` kills the CLI's whole process group. `attachments` items:

```ts
{ name: string; mimeType?: string; base64?: string | null; content?: string | null }
```
Text files use `content`; binary/image files use base64 `base64`. The server
materializes them to temp files and passes them to the CLI via `-f/--file`.

## 6. Extending the protocol

1. Add the handler arm in `ws_server.rs` `handle()` (and any helpers in
   `commands.rs`).
2. Call it from `web/src/composables/useBackend.ts` consumers via
   `invokeCommand('new_command', args)`.
3. Mirror new payload shapes in `web/src/types/index.ts` and update this doc.
4. Bump the version note here when behavior changes in a breaking way
   (removed/renamed commands or changed payloads), since old clients may stay
   connected to newer servers and vice versa.
