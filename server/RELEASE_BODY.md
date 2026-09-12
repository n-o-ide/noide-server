## v0.2.2 — Windows, Cloudflare Quick Tunnel, and stability

Adds Windows support and a one-shot Cloudflare tunnel, widens file-tree/search
ignore lists for bigger workspaces, and includes a round of stability fixes
around PTY teardown, WebSocket keep-alive, and agent lifecycle.

### Highlights

- **Windows support.** Release builds now produce
  `noide-server-windows-x86_64.exe`; terminal tabs default to PowerShell and
  `git` is required for Source Control (install [Git for Windows](https://git-scm.com/download/win) if it is missing).
- **Cloudflare Quick Tunnel.** With no flags the server downloads
  `cloudflared` into `~/.noide` (or `%LOCALAPPDATA%\noide` on Windows) and
  starts a `trycloudflare.com` tunnel, printing a public `wss://`-friendly URL
  to stderr. Drop `--no-cloudflare` if you bring your own reverse proxy.
- **File tree / search ignore lists widened.** More project noise is skipped by
  default (intermediate build/output folders for common stacks), so large repos
  browse and search more cleanly.
- **Chat agent detection is stricter.** `chat_check_install` now distinguishes
  "missing" from "wrong binary on PATH", and streaming startup surfaces a
  clear error instead of proceeding silently when a shadowed CLI doesn't match
  the requested agent.
- **Cleaner PTY shutdown and ring-buffer attach path.** The PTY reader now
  fails fast on a full outbound channel instead of blocking inside the reader
  thread, and terminal attach/replay code is less twiddly around ring state.
- **Smarter WebSocket keep-alive.** Idle connections are still probed, but the
  server now resets the idle timer on the **pong it just sent** (not on the
  incoming ping), so a half-closed client can't keep the server convinced the
  connection is alive.
- **Windows cleanup hardened.** `chat_cancel` and server shutdown both kill
  Windows agent processes with separate `OpenProcess(...)` / `TerminateProcess(...)`
  / `CloseHandle(...)` calls, and Cloudflare's Windows binary path/fallbacks
  are a bit more robust.

### Install (upgraded)

```bash
curl -fsSL https://raw.githubusercontent.com/n-o-ide/noide-server/main/install.sh | bash
```

Re-running upgrades you to the newest release. Pin a version with
`VERSION=0.2.2`. The Windows binary is `noide-server-windows-x86_64.exe`.

#### Windows note

SmartScreen may warn about an unsigned binary — click **More info → Run anyway**,
or verify against the published `SHA256SUMS` before running.

#### macOS note

Downloaded binaries are quarantined by Gatekeeper. If you see
"cannot be opened because the developer cannot be verified", remove the
quarantine attribute once:

```bash
xattr -d com.apple.quarantine "$(command -v noide-server)"
```

### Run

```bash
noide-server
```

On startup you get the usual pairing code + QR **plus** a Cloudflare tunnel URL
if the download succeeds. Enter the pairing code in the NoIDE app
(Settings → Server) and connect.

```
noide-server --no-cloudflare
```

Use `--no-cloudflare` (or set up Caddy/nginx as in the README) when you
already have a `wss://` path.

### Assets

| File | Platform |
|------|----------|
| `noide-server-linux-x86_64` | Linux (Intel/AMD) |
| `noide-server-linux-aarch64` | Linux (ARM64 — Raspberry Pi, ARM servers) |
| `noide-server-darwin-x86_64` | macOS (Intel) |
| `noide-server-darwin-aarch64` | macOS (Apple Silicon) |
| `noide-server-windows-x86_64.exe` | Windows (x86_64) |
| `SHA256SUMS` | Checksums for all binaries |

Install a downloaded binary manually:

```bash
curl -fLO https://github.com/n-o-ide/noide-server/releases/download/v0.2.2/noide-server-linux-x86_64
curl -fLO https://github.com/n-o-ide/noide-server/releases/download/v0.2.2/SHA256SUMS
sha256sum -c SHA256SUMS --ignore-missing
chmod +x noide-server-linux-x86_64 && sudo mv noide-server-linux-x86_64 /usr/local/bin/
```

### Security

- Pairing is **on by default** — the code only exists on the server's console.
- Use `wss://` whenever the connection leaves your trusted LAN; the token
  travels in the URL query string, so don't reuse long-lived credentials.
- `--no-auth` exists for localhost development only.
- Cloudflare Quick Tunnel is opt-out at startup via `--no-cloudflare`; the
  tunnel URL is visible only on the server's console (not exposed in the
  WebSocket protocol itself).

See the [README](https://github.com/n-o-ide/noide-server#readme) and
[PROTOCOL.md](https://github.com/n-o-ide/noide-server/blob/main/PROTOCOL.md)
for details.
