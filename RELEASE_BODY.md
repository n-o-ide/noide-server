## v0.4.2 — 1-Click Railway Deployment & Mandatory NioAI Agent Bundling

Bumps version to `v0.4.2`, introduces 1-Click Railway cloud deployment with persistent storage volume support, auto-detects dynamic `$PORT` environments, and bundles the `nio-ai` agent into both `install.sh` and `npx noide-server` with reliable native curl fallbacks.

### Highlights

- **☁️ 1-Click Deploy to Railway:** Deploy your personal NoIDE cloud development server with a single click:
  - Multi-stage Docker container with essential developer tooling (`git`, `bash`, `python3`, `nodejs`, `npm`, `openssh`).
  - Persistent storage volume support at `/workspace` so cloned repositories and project files survive redeploys.
  - Automatic dynamic `$PORT` binding and cloud platform detection (`RAILWAY_ENVIRONMENT`, `NOIDE_NO_CLOUDFLARE`).
  - Pre-installed AI agent CLIs: `nio-ai`, `@kilocode/cli` (`kilo`), and `opencode-ai` (`opencode`).
  - Single connection secret via `NOIDE_TOKEN` with automatic SSL termination (`wss://`).
- **🤖 Mandatory NioAI Agent Bundling (`nio-ai`):**
  - **`install.sh`:** Automatically checks and installs `nio-ai`. Tries `npm install -g nio-ai` first, then falls back directly to the native installer (`curl -fsSL https://raw.githubusercontent.com/nio-labs/nio/main/install.sh | bash`).
  - **`npx noide-server`:** The zero-install npm runner verifies `nio` availability and provisions it into `~/.nio/bin/nio` with curl / PowerShell fallback if missing.
  - **Backend Agent Search:** Added `~/.nio/bin` and `~/.cargo/bin` to `noide-server`'s native agent search directories, and added `install_agent` support for `nio-ai`.
- **Crate & Monorepo Synchronization:** Synchronized and bumped all companion tools (`canvas-lab`, `code-vault`, `file-manager`, `http-request`, `port-forward`) to `v0.4.2`.

---

### Install & Run

#### Option 1: 1-Click Deploy on Railway

[![Deploy on Railway](https://railway.app/button.svg)](https://railway.app/template/new?template=https%3A%2F%2Fgithub.com%2Fnio-labs%2Fnoide-server)

#### Option 2: Via npx (Instant / Zero-install)

```bash
npx noide-server
```

Pass any flags directly:

```bash
# Persistent custom token
npx noide-server --token my-secret-code

# Custom port
npx noide-server --port 8080
```

To install globally via npm:

```bash
npm install -g noide-server
noide-server
```

#### Option 3: Via curl install script

```bash
curl -fsSL https://raw.githubusercontent.com/nio-labs/noide-server/main/install.sh | bash
```

Pin a specific version:
```bash
VERSION=0.4.2 bash install.sh
```

To install all companion binaries:
```bash
bash install.sh --all
```

---

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
curl -fLO https://github.com/nio-labs/noide-server/releases/download/v0.4.2/noide-server-linux-x86_64
curl -fLO https://github.com/nio-labs/noide-server/releases/download/v0.4.2/SHA256SUMS
sha256sum -c SHA256SUMS --ignore-missing
chmod +x noide-server-linux-x86_64 && sudo mv noide-server-linux-x86_64 /usr/local/bin/noide-server
```
