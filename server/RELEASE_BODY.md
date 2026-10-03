## NioDE Server 0.5.3

Deploy NioDE with new Koyeb and GitHub Codespaces launch buttons, plus a
Fly.io deployment walkthrough. Codespaces installs the released server and
Nio chat CLI automatically and starts an authenticated server on port 1421.
The startup hook prevents duplicate server processes and protects pairing logs.

The README now documents Koyeb's free-instance limits, Codespaces quotas,
and Railway's explicit persistent-volume setup. CodeSandbox deployment
instructions have been removed.

### Install

```sh
npx @nio-labs/nio-de
```

Or install globally:

```sh
npm install -g @nio-labs/nio-de
nio-de
```

Linux/macOS native installer:

```sh
curl -fsSL https://raw.githubusercontent.com/nio-labs/nio-de/main/install.sh | bash
nio-de
```

The server prints a pairing code for NioDE. Use `--token` or `NIO_DE_TOKEN`
for a fixed token. npm publication follows the native release.

### Native artifacts

Platforms: Linux x86_64/aarch64, macOS x86_64/aarch64, and Windows x86_64.
Each platform provides `nio-de`, `port-forward`, `code-vault`, `http-request`,
`canvas-lab`, and `file-manager`. Windows files end in `.exe`.
`SHA256SUMS` covers all 30 binaries.
