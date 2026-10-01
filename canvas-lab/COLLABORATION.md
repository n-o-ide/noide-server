# Canvas Lab — Collaboration Implementation Plan

This document describes the plan to add real-time multi-user collaboration to Canvas Lab. It covers the target architecture, backend and frontend changes, persistence strategy, protocol extension, and phased rollout.

---

## 1. Current State

- Canvas Lab is a single-user editor.
- The Rust backend (`canvas-lab/`) exposes a simple HTTP API: `GET /document`, `PUT /document`.
- The frontend holds document state in local Vue refs and syncs debounced writes via `noide-server`'s WebSocket proxy.
- There is no concurrency control, no shared state, and no presence/cursor support.

---

## 2. Architecture Decision: Yjs + y-websocket

**Recommended approach:** use Yjs CRDT bindings on the frontend and a lightweight `y-websocket` signaling server for room-based sync.

**Why Yjs:**
- Mature CRDT with first-class Vue 3 support (`y-vue`, `@vue-yjs/core`).
- Handles concurrent inserts, deletes, moves, and property edits without central coordination.
- Binary sync protocol is compact over WebSocket.
- Built-in awareness (cursors, selection, presence).

**Why not OT:** Operational transforms require a central transform server and are harder to extend for nested document structures. CRDTs are peer-to-peer friendly and simpler in this context.

---

## 3. Backend Changes

### 3.1 Existing tunneling support (reuse)

This project already supports outbound tunneling:

- `noide-server` auto-starts a **Cloudflare Quick Tunnel** (`trycloudflare.com`) on startup unless `--no-cloudflare` is passed.
- A standalone `port-forward/` binary supports `trycloudflare`, `localhost.run`, and `localtunnel`.
- `cloudflared` is downloaded automatically on first run.

### 3.2 Two-link architecture for remote collaboration

Use **two separate tunnels** so Canvas Lab collab is isolated from full IDE access.

| Link | Purpose | Exposes |
|---|---|---|
| **Tunnel A** (existing) | Full IDE | `noide-server` on port 1421 |
| **Tunnel B** (new) | Canvas Lab collab only | Dedicated Yjs room server |

**Why two tunnels:**
- Security: sharing the IDE tunnel gives full repo/terminal access; collab-only tunnel restricts the surface to canvas data.
- Stability: you can restart the IDE without dropping active canvas collaborators.
- Separation: IDE auth stays on Tunnel A; canvas room auth can be lighter.

### 3.3 Add a Yjs document server

- Option A (recommended for speed): run `y-websocket` as a sidecar process.
- Option B (future): embed a minimal Yjs-aware sync handler in `noide-server` via a new WebSocket sub-protocol.

For now, add `y-websocket` as a dev/run dependency managed by the project scripts.

### 3.4 Second Cloudflare tunnel for collab

1. Spawn a lightweight collab server (e.g., `y-websocket` or a minimal Axum WebSocket server) on a dedicated port.
2. From `noide-server` startup, start a second `cloudflared` tunnel pointing at that port.
3. Capture the public URL and surface it as the **Canvas Lab invite link**.
4. The collaborator connects **only** to Tunnel B for canvas sync; they never touch the IDE.

This builds on the existing `cloudflare::start_tunnel(port)` logic in `server/src/cloudflare.rs` and the tunnel plumbing in `port-forward/src/tunnel.rs`.

### 3.5 Document persistence strategy

- Current behavior: full JSON read/write on every change.
- New behavior: backend receives debounced Yjs binary updates and persists the merged document JSON to disk.
- On startup: load existing JSON → import into a temporary Yjs doc → serve as initial state to room clients.

### 3.6 WebSocket protocol extension

Extend `PROTOCOL.md` with a new message family, for example:

```json
{ "type": "canvas_lab_collab", "action": "join", "room": "doc-<id>" }
{ "type": "canvas_lab_collab", "action": "update", "data": "<base64-encoded Yjs update>" }
{ "type": "canvas_lab_collab", "action": "awareness", "data": "<JSON>" }
```

`noide-server` proxies these to the Yjs room server.

---

## 4. Frontend Changes

### 4.1 Dependencies

```
yjs
y-vue
@vue-yjs/core
y-websocket
```

### 4.2 State migration

- Replace local `nodes` / `snapshots` refs with Yjs structures:
  - `nodes` → `Y.Array` of `Y.Map`
  - `snapshots` → `Y.Array` of `Y.Map`
- Bind via Vue reactivity using `@vue-yjs/core` or `y-vue`.
- Keep `localStorage` fallback for offline mode using a Yjs persistence adapter.
- Replace `history.ts` with Yjs undo manager for undo/redo.

### 4.3 Collaboration UI

- Presence bar showing collaborator names and colors.
- Remote cursors/selections rendered in `DesignCanvasNode.vue` using awareness states.
- "User X is editing…" indicators near active nodes.

### 4.4 Networking

- Connect to `y-websocket` through Tunnel B (the collab-only tunnel).
- Handle reconnection: Yjs syncs state automatically; frontend re-establishes the WebSocket.
- The invite link is the public URL of Tunnel B + room ID, e.g. `https://<random>.trycloudflare.com/?room=doc-<id>`.

---

## 5. Conflict Resolution & Consistency

| Scenario | Resolution |
|---|---|
| Two users move the same node | CRDT merge; last writer wins for properties, geometry merges |
| User A deletes node, User B is editing | CRDT tombstone; B sees deletion, no crash |
| Offline edits on reconnect | Yjs syncs missing updates automatically |
| Backend restart | Load persisted JSON, initialize Yjs doc, clients re-sync |

---

## 6. Implementation Phases

### Phase 1: Foundation (Week 1)

1. Add `yjs`, `y-vue`, `@vue-yjs/core`, `y-websocket` to frontend dependencies.
2. Wrap `nodes` and `snapshots` in `Y.Array` / `Y.Map` in `CanvasLab.vue`.
3. Add a local in-memory Yjs doc without networking.
4. Replace `history.ts` mutations with Yjs undo manager.

### Phase 2: Networking (Week 2)

1. Spin up `y-websocket` server.
2. Extend `ws_server.rs` with `canvas_lab_collab` proxy commands.
3. Frontend connects to a room; sync works across two browser tabs.

### Phase 3: Persistence (Week 3)

1. Backend subscribes to Yjs doc updates.
2. Debounced persistence of full document JSON to disk.
3. Load existing JSON into Yjs on server startup.

### Phase 4: Presence & Polish (Week 4)

1. Awareness protocol for names, colors, cursors, and selections.
2. UI for collaborator avatars.
3. Conflict notifications and resolution UX.

---

## 7. Open Questions / Decisions Needed

1. **Room model:** one global room for all canvas docs, or per-document rooms?
   - Recommended: per-document rooms using the document file path or UUID as room ID.
2. **Authentication:** who can join a room?
   - If Canvas Lab is local-only, any local client can join. If networked later, add token-based room access.
3. **Persistence format:** keep JSON, or switch to Yjs binary blobs?
   - Keep JSON for readability; derive Yjs state on load. Optionally store Yjs updates for faster boot.
4. **Sidecar vs embedded:** Node `y-websocket` vs Rust implementation?
   - Sidecar is fastest. Move to an embedded Rust implementation later if needed.
5. **Invite link strategy:** use two separate Cloudflare tunnels?
   - Recommended: yes. Tunnel A for the IDE, Tunnel B for Canvas Lab collab only. This limits exposure and keeps the invite link scoped to canvas data.

---

## 8. Risk Mitigation

- **Rollback flag:** keep `localStorage` + REST path behind a feature flag while Yjs is stabilized.
- **Incremental migration:** introduce Yjs for `nodes` first; `snapshots` can remain local until Phase 3.
- **Testing:** add a vitest worker that simulates a second collaborator making concurrent edits and asserts convergence.
