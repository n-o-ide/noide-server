## v0.4.1 — Zero-Install via npx & CI Stabilization

Fixes CI formatting verification (`cargo fmt --check`), bumps version across all workspace crates and npm package, and stabilizes the automated release pipeline.

### Highlights

- **`npx noide-server` (Zero-install launch):** Launch `noide-server` directly on any host, VPS, or desktop with a single command:
  ```bash
  npx noide-server
  ```
  Auto-detects OS & CPU architecture (Linux x86_64/ARM64, macOS Apple Silicon/Intel, Windows x86_64), downloads the verified native binary matching the release SHA-256 checksum, caches it locally, and launches the server.
- **npm Package Distribution:** Published as [`noide-server`](https://www.npmjs.com/package/noide-server) on npm with a lightweight, zero-dependency native launcher.
- **Code Formatting & CI Fix:** Fully formatted according to Rust style guidelines (`cargo fmt --check`) across all workspace crates.
- **Process Group & Agent Lifecycle:** Improved Unix process group termination for background chat agents and companion processes to ensure clean teardown on exit.
- **WebSocket Protocol & Sync Updates:** Core backend stability updates and crate optimizations synced from the primary monorepo.
- **Automated Release Pipeline:** GitHub Actions packages and verifies checksums for all platform binaries (`noide-server`, `port-forward`, `code-vault`, `http-request`, `canvas-lab`, `file-manager`) and publishes to npm automatically.

### Install & Run

#### Option 1: Via npx (Instant / Zero-install)

```bash
npx noide-server
```

Pass any flags directly:

```bash
# Persistent custom token
npx noide-server --token my-secret-code

# Disable automatic Cloudflare tunnel
npx noide-server --no-cloudflare
```

To install globally via npm:

```bash
npm install -g noide-server
noide-server
```

#### Option 2: Via curl install script

```bash
curl -fsSL https://raw.githubusercontent.com/n-o-ide/noide-server/main/install.sh | bash
```

Pin a specific version:
```bash
VERSION=0.4.1 bash install.sh
```

To install all companion binaries:
```bash
bash install.sh --all
```

Or install individually:
```bash
bash install.sh --port-forward
bash install.sh --code-vault
bash install.sh --http-request
bash install.sh --canvas-lab
bash install.sh --file-manager
```

### Assets

This release attaches pre-built native binaries for all components plus the `SHA256SUMS` manifest:

| File | Platform |
|------|----------|
| `noide-server-linux-x86_64` | Linux (Intel/AMD) |
| `noide-server-linux-aarch64` | Linux (ARM64 — Raspberry Pi, ARM servers) |
| `noide-server-darwin-x86_64` | macOS (Intel) |
| `noide-server-darwin-aarch64` | macOS (Apple Silicon) |
| `noide-server-windows-x86_64.exe` | Windows (x86_64) |
| `port-forward-*` | Linux, macOS, Windows |
| `code-vault-*` | Linux, macOS, Windows |
| `http-request-*` | Linux, macOS, Windows |
| `canvas-lab-*` | Linux, macOS, Windows |
| `file-manager-*` | Linux, macOS, Windows |
| `SHA256SUMS` | Checksums for all release assets |

Manual install:

```bash
curl -fLO https://github.com/n-o-ide/noide-server/releases/download/v0.4.1/noide-server-linux-x86_64
curl -fLO https://github.com/n-o-ide/noide-server/releases/download/v0.4.1/SHA256SUMS
sha256sum -c SHA256SUMS --ignore-missing
chmod +x noide-server-linux-x86_64 && sudo mv noide-server-linux-x86_64 /usr/local/bin/noide-server
```
