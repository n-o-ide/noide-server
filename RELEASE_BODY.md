## NioDE Server 0.5.1

Removes the temporary NoIDE compatibility layer. The executable and npm command
are now `nio-de` only. Legacy `noide-server` launchers, release aliases, and npm
bridge packaging have been removed.

Use `NIO_DE_TOKEN`, `NIO_DE_WS_ADDR`, `NIO_DE_NO_CLOUDFLARE`,
`NIO_DE_PTY_KEEP_ALIVE`, `NIO_DE_INSTALL_DIR`, and `NIO_DE_SERVER_BIN`.
The former `NOIDE_*` variables and `NOTERM_WS_ADDR` are no longer read.
CLI token/port options continue to override environment configuration.

Existing workspace data, browser settings, database paths and WebSocket messages
are retained. The `nio` AI agent and supporting tool commands are unchanged.

### Install

Linux/macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/nio-labs/nio-de/main/install.sh | bash
nio-de
```

The server prints a pairing code for the NioDE app. A fixed token can be supplied
with `--token` or `NIO_DE_TOKEN`. The native release is published before its npm
package; after npm publication, use `npx nio-de` or `npm install -g nio-de`.

### Native artifacts

Platforms: Linux x86_64/aarch64, macOS x86_64/aarch64, and Windows x86_64.
Each platform provides `nio-de`, `port-forward`, `code-vault`, `http-request`,
`canvas-lab`, and `file-manager`. Windows files end in `.exe`.
`SHA256SUMS` covers all 30 binaries.

### Validation

Backend CI passed all five platform builds, formatting, Rust tests, and clippy.
Protocol smoke checks cover pairing, fixed tokens, NIO_DE configuration, CLI
overrides, and rejection of legacy token settings. App CI passed its frontend
build/tests and embedded backend checks. Native mobile and Docker builds were not
run as part of this cleanup.
