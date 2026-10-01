# nio-de


## NioDE rename compatibility

Canonical repositories: `nio-labs/nio-de-app` (app) and `nio-labs/nio-de` (server).
The server command is `nio-de`; `noide-server` remains a compatibility command.
New configuration uses `NIO_DE_TOKEN`, `NIO_DE_WS_ADDR`,
`NIO_DE_NO_CLOUDFLARE`, `NIO_DE_PTY_KEEP_ALIVE`, and `NIO_DE_INSTALL_DIR`.
Existing `NOIDE_*` variables and `NOTERM_WS_ADDR` remain supported; new names take precedence.
The npm launcher also accepts `NIO_DE_SERVER_BIN` with `NOIDE_SERVER_BIN` fallback.
Browser storage keys, `.noide` caches, supporting-tool database paths, mobile
identifier `me.no.ide`, WebSocket messages, and ports are retained.
Keep the existing web origin to retain browser data.

The self-hosted backend for **NioDE** — a browser/mobile code editor and
terminal. It is a single native binary that serves PTY terminals, file
operations, git, and AI coding agents (kilo/opencode) over one WebSocket
connection.

The NioDE app never runs on the same machine as this server: you point it at a
`ws://` or `wss://` URL, enter a pairing code, and everything (file tree,
editor, terminal, git, chat) runs on the host where `nio-de` runs.

**Contents**

- [Install](#install)
- [Run](#run)
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

### Requirements

- **No Rust needed.** `install.sh` downloads a prebuilt binary. A Rust
  toolchain is only required to [build from source](#build-from-source).
- **No Node.js needed** for the core server (files, terminal, git, pairing).
  Node is only required by the **Chat AI agents** (kilo / opencode): those
  CLIs are Node-based, so install them on the host yourself (`kilo` /
  `opencode` must be on your PATH) if you want chat.
- **`git` is required** for the app's Source Control features — the server
  shells out to the `git` binary on the host. Install it if your system
  doesn't already have it.
- Terminal tabs run your host's login shell — zsh/bash on Linux/macOS and
  PowerShell on Windows.

### Recommended: install script

```bash
curl -fsSL https://raw.githubusercontent.com/nio-labs/nio-de/main/install.sh | bash
```

What it does: detects your OS/arch, downloads the matching binary from the
latest stable release, verifies its SHA-256 checksum, and installs it to
`~/.local/bin` (falling back to `/usr/local/bin`). Re-running the same command
upgrades you to the newest version.

Prefer inspecting scripts before piping them into a shell? That's reasonable:

```bash
curl -fsSL https://raw.githubusercontent.com/nio-labs/nio-de/main/install.sh -o install.sh
less install.sh   # read it
bash install.sh
```

Other install options:

```bash
# Pin a specific version
VERSION=0.1.0 bash install.sh

# Dry run: print what would happen without installing
bash install.sh --dry-run
```

### Recommended: GitHub Codespaces

For the best experience, **run `nio-de` in a GitHub Codespace** — it's
free within your monthly quota, gives the agents a full Linux environment
(glibc, Node.js for the kilo/opencode CLIs), and its port forwarding hands you
a `wss://` URL automatically:

1. Create a Codespace (any repo works — even a blank one) and open its terminal.
2. Install and start the server:
   ```bash
   curl -fsSL https://raw.githubusercontent.com/nio-labs/nio-de/main/install.sh | bash
   nio-de
   ```
3. Codespaces will suggest forwarding port **1421**. Open the forwarded port
   and copy the `wss://…app.github.dev` URL.
4. In the NioDE app: set **Server URL** to that `wss://` URL and enter the
   **Pairing Code** printed by the server.

Notes:

- Codespace machines pause when idle or closed — restart the server after the
  machine wakes (`nio-de` again; it prints a fresh pairing code).
- Re-run the install command any time to upgrade to the latest release.

### Termux (Android)

`nio-de` is a native binary and runs on aarch64 devices — Termux works.
For better performance with the Chat AI agents, run it inside an
[AndroNix](https://andronix.app) proot (Ubuntu CLI only) instead (a full Linux distro with a
real glibc + Node.js toolchain); avoid third-party "Proot Distro" installers,
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
VERSION=0.2.0 bash install.sh
```

Restart the server after upgrading. **Check the release notes first** — if a
release changes the WebSocket protocol, older NioDE app versions may need an
update too (see below).

---

## Build from source

```bash
git clone https://github.com/nio-labs/nio-de
cd nio-de
cargo build --release
./target/release/nio-de
```

Requires a Rust toolchain. Release builds use LTO + stripping (`opt-level=s`,
`strip=true`) — see `Cargo.toml`.

---

## Protocol compatibility

The WebSocket contract between this server and the NioDE app is documented in
[PROTOCOL.md](../PROTOCOL.md) (copy it into this repo when splitting out). The
server and app can be on different versions; breaking protocol changes are
called out in the release notes.

---

## License

MIT
