## v0.3.0 — Port Forwarding, Binary PTY, and Graceful Shutdown

Adds a built-in port-forwarding subsystem with Cloudflare/localhost.run/localtunnel
support, a binary PTY fast path for lower-latency keystrokes, and a graceful
shutdown that tears down runtimes cleanly instead of panicking. Also includes
Windows support, a Cloudflare Quick Tunnel, and a round of stability fixes.

### Highlights

- **Port Forwarding.** A new `port-forward` binary ships alongside `noide-server`.
  Expose local ports via Cloudflare Quick Tunnel, localhost.run, or localtunnel —
  managed from the NoIDE app's Ports panel or used standalone from the CLI.
  Install with `bash install.sh --port-forward` or `bash install.sh --all`.
- **Binary PTY fast path.** Terminal keystrokes can now be sent as raw binary
  frames (type `0x01`) instead of JSON, skipping UTF-8 validation and reducing
  frame overhead for interactive use.
- **Graceful shutdown.** Ctrl+C now signals the async runtime to shut down
  cleanly instead of calling `process::exit` from a signal handler, preventing
  "Cannot drop a runtime in a context where blocking is not allowed" panics.
- **Windows support.** Release builds now produce
  `noide-server-windows-x86_64.exe`; terminal tabs default to PowerShell and
  `git` is required for Source Control (install [Git for Windows](https://git-scm.com/download/win) if it is missing).
- **Cloudflare Quick Tunnel.** With no flags the server downloads
  `cloudflared` into `~/.noide` (or `%LOCALAPPDATA%\noide` on Windows) and
  starts a `trycloudflare.com` tunnel, printing a public `wss://`-friendly URL
  to stderr. Drop `--no-cloudflare` if you bring your own reverse proxy.
- **Lower terminal latency.** WebSocket batch window reduced from 20ms to 5ms
  — fast enough that interactive echoes never feel delayed while still
  coalescing rapid micro-bursts into fewer frames.
- **LAN IP detection + QR.** In `--no-auth` mode the server detects its LAN IP
  and prints a scannable QR code containing the server URL, making phone/tablet
  setup easier.
- **New commands.** `chat_install` (install agent CLIs from the app),
  `chat_refresh_models`, `is_file_git_ignored`, `install_port_forward`,
  `uninstall_port_forward`.

### Install (upgraded)

```bash
curl -fsSL https://raw.githubusercontent.com/n-o-ide/noide-server/main/install.sh | bash
```

Re-running upgrades you to the newest release. Pin a version with
`VERSION=0.3.0`.

To install port-forward as well:

```bash
bash install.sh --all
```

#### Windows note

SmartScreen may warn about an unsigned binary — click **More info → Run anyway**,
or verify against the published `SHA256SUMS` before running.

#### macOS note

Downloaded binaries are quarantined by Gatekeeper. If you see
"cannot be opened because the developer cannot be verified", remove the
quarantine attribute once:

```bash
xattr -d com.apple.quarantine "$(command -v noide-server)"
xattr -d com.apple.quarantine "$(command -v port-forward)"
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
| `port-forward-linux-x86_64` | Linux (Intel/AMD) |
| `port-forward-linux-aarch64` | Linux (ARM64) |
| `port-forward-darwin-x86_64` | macOS (Intel) |
| `port-forward-darwin-aarch64` | macOS (Apple Silicon) |
| `port-forward-windows-x86_64.exe` | Windows (x86_64) |
| `SHA256SUMS` | Checksums for all binaries |

Install a downloaded binary manually:

```bash
curl -fLO https://github.com/n-o-ide/noide-server/releases/download/v0.3.0/noide-server-linux-x86_64
curl -fLO https://github.com/n-o-ide/noide-server/releases/download/v0.3.0/SHA256SUMS
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
