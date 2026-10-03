# nio-de


## NioDE configuration

Canonical repositories: `nio-labs/nio-de-app` (app) and `nio-labs/nio-de` (server).
The server command is `nio-de`.
New configuration uses `NIO_DE_TOKEN`, `NIO_DE_WS_ADDR`,
`NIO_DE_NO_CLOUDFLARE`, `NIO_DE_PTY_KEEP_ALIVE`, and `NIO_DE_INSTALL_DIR`.
The npm launcher accepts `NIO_DE_SERVER_BIN` to select a native executable.
Browser storage keys, `.noide` caches, supporting-tool database paths, mobile
identifier `me.no.ide`, WebSocket messages, and ports are retained.
Keep the existing web origin to retain browser data.

The self-hosted backend for **NioDE** — a browser/mobile code editor and
terminal. It is a single native binary that serves PTY terminals, file
operations, git, and Nio AI coding chat (`nio` CLI) over one WebSocket
connection.

The NioDE app never runs on the same machine as this server: you point it at a
`ws://` or `wss://` URL, enter a pairing code or token, and everything (file tree,
editor, terminal, git, chat) runs on the host where `nio-de` runs.

This repo also includes companion binaries:
- **port-forward** — expose local ports via trycloudflare, localhost.run, or localtunnel
- **code-vault** — save and manage code snippets with syntax highlighting
- **http-request** — test APIs with folder organization and OpenAPI import
- **canvas-lab** — create and manage Canvas Lab documents in the NioDE app
- **file-manager** — browse, upload, download, and manage files via a local HTTP server

**Contents**

- [Install](#install)
  - [1-Click Deploy to Railway](#-1-click-deploy-to-railway)
  - [Deploy to Fly.io](#deploy-to-flyio)
  - [Deploy to Koyeb](#deploy-to-koyeb)
  - [1-Click GitHub Codespaces](#1-click-github-codespaces)
  - [Quickstart with npx (Zero-install)](#quickstart-with-npx-zero-install)
  - [Install script (curl)](#recommended-install-script)
- [Run](#run)
- [Port Forward](#port-forward)
- [Code Vault](#code-vault)
- [HTTP Request](#http-request)
- [Canvas Lab](#canvas-lab)
- [File Manager](#file-manager)
- [Pair the NioDE app](#pair-the-noide-app)
- [Connect over the internet (wss)](#connect-over-the-internet-wss)
- [Security](#security)
- [Upgrading](#upgrading)
- [Build from source](#build-from-source)
- [Protocol compatibility](#protocol-compatibility)

---

## Install

Supported platforms:

| OS | Architecture | Notes |
|----|--------------|-------|
| Linux | x86_64, aarch64 | aarch64 targets Raspberry Pi / ARM servers |
| macOS | x86_64 (Intel), aarch64 (Apple Silicon) | see [macOS note](#macos-note) |
| Windows | x86_64 | see [Windows note](#windows-note) |

### ☁️ 1-Click Deploy to Railway

Deploy your personal NioDE cloud development server in one click:

[![Deploy on Railway](https://railway.app/button.svg)](https://railway.app/template/new?template=https%3A%2F%2Fgithub.com%2Fnio-labs%2Fnio-de)

- **Persistent storage:** Attach a Railway volume at `/workspace` so code and git repos survive redeploys; the Dockerfile and `railway.json` do not create a volume automatically.
- **Nio AI chat:** Ready to use with the `nio` CLI (`@nio-labs/nio-ai`).
- **Free Automatic SSL:** Connect your NioDE client directly via `wss://<your-project>.up.railway.app/?token=<your-token>`.
- **Security:** Set `NIO_DE_TOKEN` as your connection password during deployment.

### Deploy to Fly.io

Fly.io can run the server using this repository's Dockerfile. Install the
[Fly CLI](https://fly.io/docs/flyctl/install/), sign in with `fly auth login`,
and run these commands from your `nio-de` checkout:

```bash
fly launch --no-deploy
```

Choose a unique app name and a region. Keep the generated `app` and
`primary_region` values in `fly.toml`, and configure the following sections
(replace any generated service or health-check sections):

```toml
[build]
  dockerfile = "Dockerfile"

[env]
  NIO_DE_WS_ADDR = "0.0.0.0:1421"
  NIO_DE_NO_CLOUDFLARE = "true"

[http_service]
  internal_port = 1421
  force_https = true
  auto_stop_machines = "off"
  auto_start_machines = true
  min_machines_running = 1

[[mounts]]
  source = "nio_workspace"
  destination = "/workspace"

[[vm]]
  size = "shared-cpu-1x"
  memory = "1gb"
```

Then create storage in the same region as the app, set a private connection
token (generate one with `openssl rand -hex 24` and save it for pairing),
and deploy one Machine:

```bash
fly volumes create nio_workspace --region <your-region> --size 1
fly secrets set NIO_DE_TOKEN="<your-long-random-token>"
fly deploy --ha=false
```

Enter `wss://<your-app-name>.fly.dev` and your token in the NioDE app.
Keep one Machine for this personal server so terminals and workspace files
stay on the same host. Increase memory if your tools need more. Disabling
autostop keeps the server running and incurs usage charges; redeploys and
Machine restarts still end running terminal processes.

Files under `/workspace` persist on the volume; settings elsewhere in the
container do not. Keep backups: [Fly volumes](https://fly.io/docs/volumes/overview/)
are local to a Machine and are not automatically replicated.
See the [Fly configuration reference](https://fly.io/docs/reference/configuration/)
for service and storage options.

### Deploy to Koyeb

[![Deploy to Koyeb](https://www.koyeb.com/static/images/deploy/button.svg)](https://app.koyeb.com/deploy?type=git&repository=github.com%2Fnio-labs%2Fnio-de&branch=main&name=nio-de&builder=dockerfile&dockerfile=Dockerfile&instance_type=free&ports=1421%3Bhttp%3B%2F&env%5BNIO_DE_WS_ADDR%5D=0.0.0.0%3A1421&env%5BNIO_DE_NO_CLOUDFLARE%5D=true)

The button preselects this repository's Dockerfile, the **Free** instance,
and HTTP port **1421** at path `/`. Before deploying, set `NIO_DE_TOKEN` to
a value you generate privately with `openssl rand -hex 24` and save for
pairing. Do not put your token in a deploy-button URL.

After deployment, enter `wss://<your-service-domain>.koyeb.app` and your token
in the NioDE app. Keep the default TCP health check on port 1421; the server
accepts WebSocket upgrades and does not expose an HTTP health endpoint.

Koyeb's [free instance](https://www.koyeb.com/docs/reference/instances) has
512 MB RAM, 0.1 vCPU, and 2 GB temporary storage, with one free instance per
organization. It sleeps after one hour without traffic and cannot attach a
persistent volume. Treat this as a disposable demo: commit or export your
work before stopping or redeploying. Nio chat and development tools share
these limited resources.

### 1-Click GitHub Codespaces

[![Open in GitHub Codespaces](https://github.com/codespaces/badge.svg)](https://codespaces.new/nio-labs/nio-de?quickstart=1)

Click the button and confirm creation of a Codespace. The included
`.devcontainer` configuration installs the released NioDE server and Nio chat
CLI, then starts the server automatically on port **1421** each time the
container starts. It runs from the repository workspace; this launches the
published release rather than building the checked-out Rust sources.

1. Read the latest pairing code in the Codespace terminal:
   ```bash
   tail -n 80 /tmp/nio-de-codespaces/server.log
   ```
2. In the **Ports** panel, set port **1421** to **Public** so the external NioDE
   app can connect without GitHub's browser login. Pairing remains required.
3. Copy the forwarded HTTPS address, replace `https://` with `wss://`, and
   enter that URL and pairing code in the NioDE app.

If you configure a `NIO_DE_TOKEN` Codespaces secret, use that token instead
of a pairing code. Otherwise each server restart creates a fresh code.

[GitHub Codespaces](https://docs.github.com/en/billing/concepts/product-billing/github-codespaces)
includes a monthly compute and storage allowance for personal accounts;
it is not an always-on free host. Idle Codespaces stop, ending terminals.
Workspace files survive a stop/start, but deleting the Codespace deletes its
workspace, so keep your work backed up or pushed to Git.
See [port forwarding](https://docs.github.com/en/codespaces/developing-in-a-codespace/forwarding-ports-in-your-codespace)
for visibility and connection details.

### Quickstart with npx (Zero-install)

If you have Node.js available, run `nio-de` anywhere with a single command without downloading or setting up anything manually:

```bash
npx @nio-labs/nio-de
```

Pass any flags directly:

```bash
# Set a persistent secret token
npx @nio-labs/nio-de --token my-secret-token

# Disable automatic Cloudflare tunnel
npx @nio-labs/nio-de --no-cloudflare
```

To install it globally via npm:

```bash
npm install -g @nio-labs/nio-de
nio-de
```

Under the hood, `npx @nio-labs/nio-de` detects your operating system and CPU architecture (Linux x64/ARM64, macOS Apple Silicon/Intel, Windows x64), verifies the binary's SHA-256 checksum, caches it in `~/.noide/bin`, and starts the WebSocket server in your current directory.

### Requirements

- **No Rust needed.** Prebuilt binaries are downloaded automatically by `npx @nio-labs/nio-de` and `install.sh`. A Rust toolchain is only required to [build from source](#build-from-source).
- **Node.js:** Required for `npx @nio-labs/nio-de` and the Nio chat CLI. The standalone server binary installed via `install.sh` has zero runtime dependencies.
- **`git` is required** for the app's Source Control features — the server
  shells out to the `git` binary on the host. Install it if your system
  doesn't already have it.
- Terminal tabs run your host's login shell — zsh/bash on Linux/macOS and
  PowerShell on Windows.

### Recommended: install script (curl)

```bash
curl -fsSL https://raw.githubusercontent.com/nio-labs/nio-de/main/install.sh | bash
```

What it does: detects your OS/arch, downloads the matching binary from the
latest stable release, verifies its SHA-256 checksum, and installs it to
`~/.local/bin` (falling back to `/usr/local/bin`). Pass `--all` to install every
companion binaries too (port-forward, code-vault, http-request, canvas-lab, file-manager). Re-running the
same command upgrades you to the newest version.

Prefer inspecting scripts before piping them into a shell? That's reasonable:

```bash
curl -fsSL https://raw.githubusercontent.com/nio-labs/nio-de/main/install.sh -o install.sh
less install.sh   # read it
bash install.sh
```

Other install options:

```bash
# Pin a specific version
VERSION=0.4.2 bash install.sh

# Dry run: print what would happen without installing
bash install.sh --dry-run

# Install port-forward only
bash install.sh --port-forward

# Install code-vault only
bash install.sh --code-vault

# Install http-request only
bash install.sh --http-request

# Install Canvas Lab only
bash install.sh --canvas-lab

# Install file-manager only
bash install.sh --file-manager

# Install all binaries
bash install.sh --all
```

### Recommended: GitHub Codespaces

Use the [1-click Codespaces setup](#1-click-github-codespaces) above to install
and start the server automatically. To use an existing Codespace instead,
run `npx @nio-labs/nio-de --no-cloudflare`, forward port **1421** publicly,
and pair with its `wss://` address and the code printed in the terminal.

### Termux (Android)

`nio-de` is a native binary and runs on aarch64 devices — Termux works.
For better performance with Nio chat, run it inside an
[AndroNix](https://andronix.app) proot (Ubuntu CLI only) instead (a full Linux distro with a
real glibc + Node.js toolchain for the Nio CLI); avoid third-party "Proot Distro" installers,
which are slower and less reliable.

### Manual install

Download `nio-de-<os>-<arch>` (`nio-de-windows-x86_64.exe` on
Windows) from the [releases](https://github.com/nio-labs/nio-de/releases)
page and verify it against the published `SHA256SUMS`:

```bash
curl -fLO https://github.com/nio-labs/nio-de/releases/latest/download/nio-de-linux-x86_64
curl -fLO https://github.com/nio-labs/nio-de/releases/latest/download/SHA256SUMS
sha256sum -c SHA256SUMS --ignore-missing   # or: shasum -a 256 -c
chmod +x nio-de-linux-x86_64
sudo mv nio-de-linux-x86_64 /usr/local/bin/nio-de
nio-de --version
```

<a name="macos-note"></a>
**macOS note:** binaries downloaded from the internet are quarantined by
Gatekeeper. If you see "cannot be opened because the developer cannot be
verified", remove the quarantine attribute once:

```bash
xattr -d com.apple.quarantine "$(command -v nio-de)"
xattr -d com.apple.quarantine "$(command -v port-forward)"
xattr -d com.apple.quarantine "$(command -v code-vault)"
xattr -d com.apple.quarantine "$(command -v http-request)"
xattr -d com.apple.quarantine "$(command -v file-manager)"
```

<a name="windows-note"></a>
**Windows note:** the release binary is `nio-de-windows-x86_64.exe`.
Binaries are unsigned, so SmartScreen may warn "Windows protected your PC" —
click **More info → Run anyway**, or launch it from PowerShell and verify it
against the published `SHA256SUMS`:

```powershell
curl.exe -fLO https://github.com/nio-labs/nio-de/releases/latest/download/nio-de-windows-x86_64.exe
curl.exe -fLO https://github.com/nio-labs/nio-de/releases/latest/download/SHA256SUMS
certutil -hashfile nio-de-windows-x86_64.exe SHA256   # compare with SHA256SUMS
.\nio-de-windows-x86_64.exe --version
```

Terminal tabs default to PowerShell on Windows. **git** is required for
Source Control — install [Git for Windows](https://git-scm.com/download/win)
if it isn't already on your PATH.

---

## Run

```bash
# Via npx (zero installation):
npx @nio-labs/nio-de

# Or if installed to PATH:
nio-de
```

That's it. On startup the server:

1. Prints a fresh **pairing code** (`XXXX-XXXX`) plus a scannable ASCII QR
   code to the terminal, and
2. Listens for WebSocket connections on `0.0.0.0:1421`.

Every client must present the code (see [Pair the NioDE app](#pair-the-noide-app)).
The code is ephemeral — it is never stored and changes every time the server
restarts.

### Options

| Flag / env | Effect |
|------------|--------|
| *(none)* | **Pairing (default).** Prints a one-time `XXXX-XXXX` code + QR; clients must enter it. |
| `--token <value>` or `NIO_DE_TOKEN=…` | Require this fixed token instead of pairing. Survives restarts. |
| `--no-auth` | Accept unauthenticated connections. **Development / CI only.** |
| `NIO_DE_WS_ADDR=0.0.0.0:1421` | Bind address and port. |
| `--no-cloudflare` | Disable Cloudflare Quick Tunnel auto-start. |

### Running it properly

Example systemd user unit (`~/.config/systemd/user/nio-de.service`):

```ini
[Unit]
Description=NioDE server
After=network-online.target

[Service]
ExecStart=%h/.local/bin/nio-de
Restart=on-failure
RestartSec=3

[Install]
WantedBy=default.target
```

```bash
systemctl --user daemon-reload
systemctl --user enable --now nio-de
journalctl --user -u nio-de -f   # see the pairing code / QR here
```

---

## Port Forward

**port-forward** is a companion binary that exposes local ports to the internet
via trycloudflare, localhost.run, or localtunnel. It's used by the NioDE app's
Port Forwarding feature but can also be used standalone.

### Install

```bash
# Install port-forward only
bash install.sh --port-forward

# Or install all binaries
bash install.sh --all
```

### Usage

```bash
# Expose port 3000 via Cloudflare Quick Tunnel
port-forward --provider cloudflare --port 3000

# Expose port 8080 via localhost.run
port-forward --provider localhost.run --port 8080

# Start the web UI for managing forwards
port-forward --web 127.0.0.1:7420
```

### Options

| Flag | Effect |
|------|--------|
| `--provider <name>` | Tunnel provider: `cloudflare`, `localhost.run`, or `localtunnel` |
| `--port <port>` | Local port to expose |
| `--web [addr]` | Start the web UI (default: `127.0.0.1:7420`) |

---

## Code Vault

**code-vault** is a companion binary for saving and managing code snippets
with syntax highlighting. It runs as a local HTTP server and is accessed
through the NioDE app's Code Vault feature.

### Install

```bash
# Install code-vault only
bash install.sh --code-vault

# Or install all binaries
bash install.sh --all
```

### Features

- Save and organize code snippets with syntax highlighting
- Support for 20+ programming languages
- JSON import/export for backup and sharing
- Full-text search across all snippets
- Auto-save with debounced updates

### API

Code Vault exposes a REST API on localhost:

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/snippets` | List all snippets |
| POST | `/snippets` | Create a new snippet |
| GET | `/snippets/:id` | Get a snippet by ID |
| PUT | `/snippets/:id` | Update a snippet |
| DELETE | `/snippets/:id` | Delete a snippet |
| GET | `/export` | Export all snippets as JSON |
| POST | `/import` | Import snippets from JSON |

---

## HTTP Request

**http-request** is a companion binary for testing REST APIs. It runs as a
local HTTP server and is accessed through the NioDE app's HTTP Request feature.

### Install

```bash
# Install http-request only
bash install.sh --http-request

# Or install all binaries
bash install.sh --all
```

### Features

- Support for all HTTP methods (GET, POST, PUT, DELETE, PATCH, HEAD, OPTIONS)
- Custom headers and request bodies (JSON/text)
- Request history with folder organization
- OpenAPI/Swagger spec import
- Collection export/import for backup and sharing
- Response time tracking

### API

HTTP Request exposes a REST API on localhost:

| Method | Endpoint | Description |
|--------|----------|-------------|
| POST | `/send` | Send an HTTP request |
| GET | `/collections` | List all saved requests (grouped by folder) |
| POST | `/collections` | Save a request |
| GET | `/collections/:id` | Get a saved request |
| PUT | `/collections/:id` | Update a saved request |
| DELETE | `/collections/:id` | Delete a saved request |
| GET | `/folders` | List all folders |
| POST | `/folders` | Create a folder |
| PUT | `/folders/:id` | Update a folder |
| DELETE | `/folders/:id` | Delete a folder |
| GET | `/export` | Export all collections as JSON |
| POST | `/import` | Import collections from JSON |
| POST | `/import-openapi` | Import from OpenAPI spec |

---

## File Manager

**file-manager** is a companion binary for browsing, uploading, downloading,
and managing files on the server. It runs as a local HTTP server and is
accessed through the NioDE app's File Manager feature.

### Install

```bash
# Install file-manager only
bash install.sh --file-manager

# Or install all binaries
bash install.sh --all
```

### Features

- Browse the server's filesystem with directory listing
- Upload files from the client to the server
- Download files from the server to the client
- Create, rename, and delete files and directories
- MIME type detection for served files
- CORS support for cross-origin access

### API

File Manager exposes a REST API on localhost:

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/list?path=…` | List directory contents |
| GET | `/read?path=…` | Read a file |
| POST | `/write` | Write/create a file |
| POST | `/mkdir` | Create a directory |
| POST | `/rename` | Rename a file or directory |
| POST | `/delete` | Delete a file or directory |
| POST | `/upload` | Upload a file (multipart) |
| GET | `/download?path=…` | Download a file |

---

## Pair the NioDE app

1. Start the server and keep the terminal visible:
   ```bash
   nio-de
   ```
   It prints the code + QR:
   ```
   =====================================================
     NioDE pairing required

     Connect from the app and enter this code:

             WD5D-CGSP
   ...
   ```
2. In the NioDE app's connect screen (or Settings → Server), enter the code
   under **Pairing Code** (e.g. `WD5D-CGSP`). If the server isn't on the same
   machine, also set **Server URL** (`ws://<host>:1421` or the `wss://` URL
   below).
3. Done — the app sends the code as `?token=…` on every connection.

QR scanning from the app's camera is planned for the mobile build; until then
the code is entered manually (any QR reader can decode the printed QR — it
contains the bare code).

---

## Connect over the internet (wss)

`nio-de` speaks plain WebSocket. For anything beyond a trusted LAN, put
a TLS-terminating reverse proxy in front of it and connect with `wss://`.

### Caddy (recommended — automatic Let's Encrypt certificates)

`Caddyfile`:

```
noide.example.com {
    reverse_proxy 127.0.0.1:1421
}
```

Point DNS at your server, open port 443, run `caddy`. WebSocket upgrades are
handled automatically.

### nginx

```nginx
server {
    server_name noide.example.com;
    listen 443 ssl;
    # ssl_certificate /path/fullchain.pem;
    # ssl_certificate_key /path/privkey.pem;

    location / {
        proxy_pass http://127.0.0.1:1421;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_read_timeout 3600s;
    }
}
```

### No public IP / behind CG-NAT?

Use an outbound tunnel — the equivalent of port forwarding when inbound isn't
possible:

```bash
# Cloudflare Tunnel (free, trusted cert, WebSocket supported)
cloudflared tunnel --url http://127.0.0.1:1421
# → connect with wss://<random-name>.trycloudflare.com

# GitHub Codespaces
# Forward port 1421 → connect with wss://<codespace>-1421.app.github.dev
```

Then connect the app to `wss://your-url` and pair as usual.

> Avoid self-signed certificates: browsers and mobile WebViews refuse
> `wss://` endpoints they don't trust, and there is no practical way to
> install your CA on a tablet. Use a real certificate (Caddy / Let's Encrypt /
> a tunnel provider).

---

## Security

- **Pairing is on by default.** Anyone who can reach the port still needs the
  code that is only printed on the server's console. Restarting the server
  rotates it.
- **Use `wss://` outside a trusted LAN.** The token travels in the URL query
  string (`?token=…`), which intermediaries can log — never reuse long-lived
  credentials as a token, and terminate TLS whenever the connection crosses
  untrusted networks.
- **`--no-auth` is for development.** Anyone who can reach the port can read,
  write, and delete files and run shells on the host.
- The token is compared in constant time on the server.

---

## Upgrading

```bash
# Latest
curl -fsSL https://raw.githubusercontent.com/nio-labs/nio-de/main/install.sh | bash

# Specific version
VERSION=0.4.2 bash install.sh

# Upgrade all binaries
bash install.sh --all
```

Restart the server after upgrading. **Check the release notes first** — if a
release changes the WebSocket protocol, older NioDE app versions may need an
update too (see below).

---

## Canvas Lab

**canvas-lab** is a companion binary for Canvas Lab documents in the NioDE app.
Install it by itself with `bash install.sh --canvas-lab`, or install all
companion binaries with `bash install.sh --all`.

---

## Build from source

This repo is a Cargo workspace with six crates:

```
nio-de/
├── server/          # nio-de binary
├── port-forward/    # port-forward binary
├── code-vault/      # code-vault binary
├── http-request/    # http-request binary
├── canvas-lab/      # canvas-lab binary
└── file-manager/    # file-manager binary
```

```bash
git clone https://github.com/nio-labs/nio-de
cd nio-de

# Build all binaries
cargo build --release

# Or build individually
cargo build --release -p nio-de
cargo build --release -p port-forward
cargo build --release -p code-vault
cargo build --release -p http-request
cargo build --release -p canvas-lab
cargo build --release -p file-manager
```

Binaries are in `target/release/`. Requires a Rust toolchain. Release builds
use LTO + stripping (`opt-level=s`, `strip=true`) — see `Cargo.toml`.

---

## Protocol compatibility

The WebSocket contract between this server and the NioDE app is documented in
[PROTOCOL.md](PROTOCOL.md). The server and app can be on different versions;
breaking protocol changes are called out in the release notes.

---

## License

MIT
