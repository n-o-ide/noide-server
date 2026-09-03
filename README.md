# noide-server

The self-hosted server for **NoIDE** — a lightweight code editor and
terminal. It is a single native binary that serves PTY terminals, file
operations, git, and AI coding agents (kilo/opencode) over one WebSocket
connection.

The NoIDE app never runs on the same machine as this server: you point it at a
`ws://` or `wss://` URL, enter a pairing code, and everything (file tree,
editor, terminal, git, chat) runs on the host where `noide-server` runs.

**Contents**

- [Install](#install)
- [Run](#run)
- [Pair the NoIDE app](#pair-the-noide-app)
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
curl -fsSL https://raw.githubusercontent.com/n-o-ide/noide-server/main/install.sh | bash
```

What it does: detects your OS/arch, downloads the matching binary from the
latest stable release, verifies its SHA-256 checksum, and installs it to
`~/.local/bin` (falling back to `/usr/local/bin`). Re-running the same command
upgrades you to the newest version.

Prefer inspecting scripts before piping them into a shell? That's reasonable:

```bash
curl -fsSL https://raw.githubusercontent.com/n-o-ide/noide-server/main/install.sh -o install.sh
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

For the best experience, **run `noide-server` in a GitHub Codespace** — it's
free within your monthly quota, gives the agents a full Linux environment
(glibc, Node.js for the kilo/opencode CLIs), and its port forwarding hands you
a `wss://` URL automatically:

1. Create a Codespace (any repo works — even a blank one) and open its terminal.
2. Install and start the server:
   ```bash
   curl -fsSL https://raw.githubusercontent.com/n-o-ide/noide-server/main/install.sh | bash
   noide-server
   ```
3. Codespaces will suggest forwarding port **1421**. Open the forwarded port
   and copy the `wss://…app.github.dev` URL.
4. In the NoIDE app: set **Server URL** to that `wss://` URL and enter the
   **Pairing Code** printed by the server.

Notes:

- Codespace machines pause when idle or closed — restart the server after the
  machine wakes (`noide-server` again; it prints a fresh pairing code).
- Re-run the install command any time to upgrade to the latest release.

### Manual install

Download `noide-server-<os>-<arch>` (`noide-server-windows-x86_64.exe` on
Windows) from the [releases](https://github.com/n-o-ide/noide-server/releases)
page and verify it against the published `SHA256SUMS`:

```bash
curl -fLO https://github.com/n-o-ide/noide-server/releases/latest/download/noide-server-linux-x86_64
curl -fLO https://github.com/n-o-ide/noide-server/releases/latest/download/SHA256SUMS
sha256sum -c SHA256SUMS --ignore-missing   # or: shasum -a 256 -c
chmod +x noide-server-linux-x86_64
sudo mv noide-server-linux-x86_64 /usr/local/bin/noide-server
noide-server --version
```

<a name="macos-note"></a>
**macOS note:** binaries downloaded from the internet are quarantined by
Gatekeeper. If you see "cannot be opened because the developer cannot be
verified", remove the quarantine attribute once:

```bash
xattr -d com.apple.quarantine "$(command -v noide-server)"
```

<a name="windows-note"></a>
**Windows note:** the release binary is `noide-server-windows-x86_64.exe`.
Binaries are unsigned, so SmartScreen may warn "Windows protected your PC" —
click **More info → Run anyway**, or launch it from PowerShell and verify it
against the published `SHA256SUMS`:

```powershell
curl.exe -fLO https://github.com/n-o-ide/noide-server/releases/latest/download/noide-server-windows-x86_64.exe
curl.exe -fLO https://github.com/n-o-ide/noide-server/releases/latest/download/SHA256SUMS
certutil -hashfile noide-server-windows-x86_64.exe SHA256   # compare with SHA256SUMS
.\noide-server-windows-x86_64.exe --version
```

Terminal tabs default to PowerShell on Windows. **git** is required for
Source Control — install [Git for Windows](https://git-scm.com/download/win)
if it isn't already on your PATH.

### Termux (Android)

`noide-server` is a native binary and runs on aarch64 devices — Termux works.
For better performance with the Chat AI agents, run it inside an
[AndroNix](https://andronix.app) proot instead (a full Linux distro with a
real glibc + Node.js toolchain); avoid third-party "Proot Distro" installers,
which are slower and less reliable.

---

## Run

```bash
noide-server
```

That's it. On startup the server:

1. Prints a fresh **pairing code** (`XXXX-XXXX`) plus a scannable ASCII QR
   code to the terminal, and
2. Listens for WebSocket connections on `0.0.0.0:1421`.

Every client must present the code (see [Pair the NoIDE app](#pair-the-noide-app)).
The code is ephemeral — it is never stored and changes every time the server
restarts.

### Options

| Flag / env | Effect |
|------------|--------|
| *(none)* | **Pairing (default).** Prints a one-time `XXXX-XXXX` code + QR; clients must enter it. |
| `--token <value>` or `NOIDE_TOKEN=…` | Require this fixed token instead of pairing. Survives restarts. |
| `--no-auth` | Accept unauthenticated connections. **Development / CI only.** |
| `NOTERM_WS_ADDR=0.0.0.0:1421` | Bind address and port. |

### Running it properly

Example systemd user unit (`~/.config/systemd/user/noide-server.service`):

```ini
[Unit]
Description=NoIDE server
After=network-online.target

[Service]
ExecStart=%h/.local/bin/noide-server
Restart=on-failure
RestartSec=3

[Install]
WantedBy=default.target
```

```bash
systemctl --user daemon-reload
systemctl --user enable --now noide-server
journalctl --user -u noide-server -f   # see the pairing code / QR here
```

---

## Pair the NoIDE app

1. Start the server and keep the terminal visible:
   ```bash
   noide-server
   ```
   It prints the code + QR:
   ```
   =====================================================
     NoIDE pairing required

     Connect from the app and enter this code:

             WD5D-CGSP
   ...
   ```
2. In the NoIDE app's connect screen (or Settings → Server), enter the code
   under **Pairing Code** (e.g. `WD5D-CGSP`). If the server isn't on the same
   machine, also set **Server URL** (`ws://<host>:1421` or the `wss://` URL
   below).
3. Done — the app sends the code as `?token=…` on every connection.

QR scanning from the app's camera is planned for the mobile build; until then
the code is entered manually (any QR reader can decode the printed QR — it
contains the bare code).

---

## Connect over the internet (wss)

`noide-server` speaks plain WebSocket. For anything beyond a trusted LAN, put
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
curl -fsSL https://raw.githubusercontent.com/n-o-ide/noide-server/main/install.sh | bash

# Specific version
VERSION=0.2.0 bash install.sh
```

Restart the server after upgrading. **Check the release notes first** — if a
release changes the WebSocket protocol, older NoIDE app versions may need an
update too (see below).

---

## Build from source

```bash
git clone https://github.com/n-o-ide/noide-server
cd noide-server
cargo build --release
./target/release/noide-server
```

Requires a Rust toolchain. Release builds use LTO + stripping (`opt-level=s`,
`strip=true`) — see `Cargo.toml`.

---

## Protocol compatibility

The WebSocket contract between this server and the NoIDE app is documented in
[PROTOCOL.md](PROTOCOL.md). The server and app can be on different versions; breaking protocol changes are
called out in the release notes.

---

## License

MIT
