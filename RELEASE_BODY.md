## v0.3.6 — Ship code-vault & http-request binaries

v0.3.5 documented the new `code-vault` and `http-request` binaries, but the
release workflow only packaged `noide-server` and `port-forward` — the two new
companion binaries were never actually built or attached to the release. v0.3.6
fixes that: `code-vault` and `http-request` are now Cargo workspace members and
are built, packaged, checksummed, and verified in the release for every supported
platform.

### Highlights

- **Code Vault.** A `code-vault` binary for saving, organizing, and editing code
  snippets with syntax highlighting (via CodeMirror). JSON import/export, folder
  organization, full-text search, and auto-save. Install with
  `bash install.sh --code-vault` or `bash install.sh --all`.
- **HTTP Request.** An `http-request` binary for testing REST APIs. All HTTP
  methods, custom headers, JSON/text bodies, request history with folder
  organization, and OpenAPI import. Install with
  `bash install.sh --http-request` or `bash install.sh --all`.
- **OpenAPI Import.** Import API endpoints from OpenAPI/Swagger specs directly
  into http-request; endpoints are organized by tags into folders, with sample
  request bodies generated from schemas.
- **Collection Export/Import.** Both code-vault and http-request support
  exporting and importing collections as JSON files for backup and sharing.
- **Release artifacts.** All four binaries — `noide-server`, `port-forward`,
  `code-vault`, and `http-request` — are now built and attached to every release,
  with a `SHA256SUMS` manifest covering all of them.
- **Cloudflare Quick Tunnel URL.** Now reported as `wss://` (the tunnel speaks
  WebSocket over the tunnel), so clients connect correctly.
- **New commands.** `check_code_vault`, `install_code_vault`,
  `start_code_vault`, `stop_code_vault`, `code_vault_request`,
  `check_http_request`, `install_http_request`, `start_http_request`,
  `stop_http_request`, `http_request_request`.

### Install

```bash
curl -fsSL https://raw.githubusercontent.com/n-o-ide/noide-server/main/install.sh | bash
```

Re-running upgrades you to the newest release. Pin a version with
`VERSION=0.3.6`.

To install all binaries:

```bash
bash install.sh --all
```

Or install individually:

```bash
bash install.sh --port-forward
bash install.sh --code-vault
bash install.sh --http-request
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
```

### Run

```bash
noide-server
```

On startup you get the usual pairing code + QR **plus** a Cloudflare tunnel URL
if the download succeeds. Enter the pairing code in the NoIDE app
(Settings → Server) and connect.

```bash
noide-server --no-cloudflare
```

Use `--no-cloudflare` (or set up Caddy/nginx as in the README) when you already
have a `wss://` path.

### Assets

This release ships pre-built binaries for all four components — `noide-server`,
`port-forward`, `code-vault`, and `http-request` — plus a `SHA256SUMS` manifest.
`install.sh` downloads and verifies the ones you request.

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
| `code-vault-linux-aarch64` | Linux (ARM64 — Raspberry Pi, ARM servers) |
| `code-vault-darwin-x86_64` | macOS (Intel) |
| `code-vault-darwin-aarch64` | macOS (Apple Silicon) |
| `code-vault-windows-x86_64.exe` | Windows (x86_64) |
| `http-request-linux-x86_64` | Linux (Intel/AMD) |
| `http-request-linux-aarch64` | Linux (ARM64 — Raspberry Pi, ARM servers) |
| `http-request-darwin-x86_64` | macOS (Intel) |
| `http-request-darwin-aarch64` | macOS (Apple Silicon) |
| `http-request-windows-x86_64.exe` | Windows (x86_64) |
| `SHA256SUMS` | Checksums for all binaries |

Install a downloaded binary manually:

```bash
curl -fLO https://github.com/n-o-ide/noide-server/releases/download/v0.3.6/noide-server-linux-x86_64
curl -fLO https://github.com/n-o-ide/noide-server/releases/download/v0.3.6/SHA256SUMS
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
