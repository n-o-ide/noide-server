## NioDE Server 0.5.2

NioAI is now installed from `@nio-labs/nio-ai`. The `nio` command is unchanged.
This updates the npm launcher, shell installer, Docker image, and in-app agent
installer so they no longer depend on the removed unscoped `nio-ai` package.

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
