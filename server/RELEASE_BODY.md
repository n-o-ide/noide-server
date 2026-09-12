## v0.1.0 — first release

**noide-server** is the self-hosted backend for the NoIDE app: a single native
binary that serves file operations, PTY terminals, Git, and AI coding agents
(kilo / opencode) over one WebSocket connection. The NoIDE app connects to it
from anywhere (browser, PWA, iPad, Android tablet) — run the server where your
code lives, and edit it from every other screen you own.

### Highlights

- **Pairing auth by default.** On startup the server prints a fresh
  `XXXX-XXXX` code plus a scannable ASCII QR; every client must present the
  code to connect. No accounts, no config — restart to rotate.
- **One binary, zero runtime deps.** No Rust or Node.js needed to install or
  run (see Requirements in the README). `git` is only required on the host if
  you use the Source Control features.
- **Terminal (PTY) sessions, file tree / read / write / search, and full Git**
  (status, diffs, stage/commit/push/pull, branches, stash, merge conflict
  resolution) over the WebSocket.
- **Chat AI agents** (kilo / opencode) with streaming responses — run against
  any model endpoint, seeded with an API key from the app.
- **Self-host friendly**: plain `ws://` for LANs, `wss://` behind Caddy/nginx
  or Codespaces/Cloudflare tunnels for the internet.

### Install

```bash
curl -fsSL https://raw.githubusercontent.com/n-o-ide/noide-server/main/install.sh | bash
```

Verifies the SHA-256 checksum, installs to `~/.local/bin`, and doubles as the
upgrader (re-run to update). Pin a version with `VERSION=0.1.0`. Read it first
instead? Download and run it by hand — or grab a binary + `SHA256SUMS` below.

### Run

```bash
noide-server
```

Enter the printed pairing code in the NoIDE app (Settings → Server) and
you're connected. Full walkthroughs (systemd, Codespaces, Termux/AndroNix,
wss via Caddy/nginx) are in the [README](https://github.com/n-o-ide/noide-server#readme).

### Assets

| File | Platform |
|------|----------|
| `noide-server-linux-x86_64` | Linux (Intel/AMD) |
| `noide-server-linux-aarch64` | Linux (ARM64 — Raspberry Pi, ARM servers) |
| `noide-server-darwin-x86_64` | macOS (Intel) |
| `noide-server-darwin-aarch64` | macOS (Apple Silicon) |
| `SHA256SUMS` | Checksums for all binaries |

Install a downloaded binary manually:

```bash
curl -fLO https://github.com/n-o-ide/noide-server/releases/download/v0.1.0/noide-server-linux-x86_64
curl -fLO https://github.com/n-o-ide/noide-server/releases/download/v0.1.0/SHA256SUMS
sha256sum -c SHA256SUMS --ignore-missing
chmod +x noide-server-linux-x86_64 && sudo mv noide-server-linux-x86_64 /usr/local/bin/
```

### Security

- Pairing is **on by default** — the code only exists on the server's console.
- Use `wss://` whenever the connection leaves your trusted LAN; the token
  travels in the URL query string, so don't reuse long-lived credentials.
- `--no-auth` exists for localhost development only.

See the [README](https://github.com/n-o-ide/noide-server#readme) and
[PROTOCOL.md](https://github.com/n-o-ide/noide-server/blob/main/PROTOCOL.md)
for details.
