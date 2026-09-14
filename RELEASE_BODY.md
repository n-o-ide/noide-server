## v0.3.8 — File Manager

Adds a new standalone binary: **file-manager** for browsing, uploading,
downloading, and managing files on the server via a local HTTP server.

### Highlights

- **File Manager.** A new `file-manager` binary for browsing, uploading,
  downloading, and managing files on the server. Features include directory
  listing, file read/write, rename, delete, and MIME type detection.
  Install with `bash install.sh --file-manager` or `bash install.sh --all`.
- **Improved folder browser.** The Open Folder dialog now defaults to `/` on
  first launch and handles directory listing errors gracefully (e.g.
  permission-denied subdirectories no longer break the entire listing).
- **Go-to-definition fix.** Ctrl+click navigation to definitions in other
  files now scrolls to the correct line.
- **Git panel.** Stage/Discard/Diff action buttons are now always visible
  on each changed file row (no hover required).

### Install (upgraded)

```bash
curl -fsSL https://raw.githubusercontent.com/n-o-ide/noide-server/main/install.sh | bash
```

Re-running upgrades you to the newest release. Pin a version with
`VERSION=0.3.8`.

To install all binaries:

```bash
bash install.sh --all
```

Or install individually:

```bash
bash install.sh --port-forward
bash install.sh --code-vault
bash install.sh --http-request
bash install.sh --file-manager
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
xattr -d com.apple.quarantine "$(command -v code-vault)"
xattr -d com.apple.quarantine "$(command -v http-request)"
xattr -d com.apple.quarantine "$(command -v file-manager)"
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
| `code-vault-linux-x86_64` | Linux (Intel/AMD) |
| `code-vault-linux-aarch64` | Linux (ARM64) |
| `code-vault-darwin-x86_64` | macOS (Intel) |
| `code-vault-darwin-aarch64` | macOS (Apple Silicon) |
| `code-vault-windows-x86_64.exe` | Windows (x86_64) |
| `http-request-linux-x86_64` | Linux (Intel/AMD) |
| `http-request-linux-aarch64` | Linux (ARM64) |
| `http-request-darwin-x86_64` | macOS (Intel) |
| `http-request-darwin-aarch64` | macOS (Apple Silicon) |
| `http-request-windows-x86_64.exe` | Windows (x86_64) |
| `file-manager-linux-x86_64` | Linux (Intel/AMD) |
| `file-manager-linux-aarch64` | Linux (ARM64) |
| `file-manager-darwin-x86_64` | macOS (Intel) |
| `file-manager-darwin-aarch64` | macOS (Apple Silicon) |
| `file-manager-windows-x86_64.exe` | Windows (x86_64) |
| `SHA256SUMS` | Checksums for all binaries |

Install a downloaded binary manually:

```bash
curl -fLO https://github.com/n-o-ide/noide-server/releases/download/v0.3.8/noide-server-linux-x86_64
curl -fLO https://github.com/n-o-ide/noide-server/releases/download/v0.3.8/SHA256SUMS
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
