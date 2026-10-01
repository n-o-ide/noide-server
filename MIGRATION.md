# NioDE migration

## Scope

Product: NioDE. App: nio-labs/nio-de-app. Backend: nio-labs/nio-de.
Local folders: nio-de-app and nio-de. Active branch: main.
The merged rename-niode branches have been deleted locally and remotely.
Backend/native/npm version: 0.5.2; supporting tools retain their versions.
NioAI uses npm package `@nio-labs/nio-ai`; its command remains `nio`.
NioDE installers and OpenGuru now install the scoped package.

Shared backend/tool source is owned by nio-de-app; run from its root:

```sh
node scripts/sync-server.mjs --all --dry-run
node scripts/sync-server.mjs --all --stat
node --test scripts/sync-server.test.mjs
```

`--all` selects all tools while preserving standalone README/release/CI files.
Use `--include-repo-owned` only when intentionally replacing those files.
Destination-only files are reported and retained. Nested paths and owned-file
comparisons now use the correct source/destination paths.

## Current configuration

The server command, Cargo binary and npm executable are `nio-de` only.
Only `NIO_DE_*` configuration names are recognized; CLI token/port options
continue to override the environment. The legacy native/npm launchers,
package bridge and release aliases have been removed in 0.5.1.

Browser keys, PWA id/scope, mobile id me.no.ide, .noide caches, database paths,
wire messages, tool names and ports are retained to preserve existing data.
Retain the existing app origin; a domain migration requires separate handling.
Previously published 0.5.0 artifacts remain historical releases.

## Verified locally

- Frontend production build; 134 canvas assertions; 4 website export tests.
- 3 sync regression tests for nested paths, stale files, dry runs and protected CI.
- Rust workspace build/test, fmt, and clippy with warnings denied.
  Existing Rust unit-test suites contain zero tests; protocol smoke tests provide
  the runtime coverage below.
- Embedded app backend build with --locked after regenerating its stale lock entry.
- Real pairing/no-auth/fixed-token handshakes and NIO_DE_* configuration,
  including CLI overrides and rejection of legacy token settings.
- Real WebSocket file write/read, binary terminal spawn/write/kill, and nio lookup.
- Native and npm nio-de launcher version output.
- npm package dry runs and Linux/macOS/Windows npm artifact-name selection.

## Release gates

Before releasing 0.5.1, require the five-platform CI matrix to pass, verify GitHub
redirects/raw installer URLs and deployment connections, and check npm namespace
availability and token permission for nio-de. Docker is unavailable locally;
the builder now uses Rust 1.98.1, matching local validation and satisfying the
locked dependencies (which require up to Rust 1.88). A container build is still
required before deployment.
Native iOS/Android builds, installed PWA upgrade, real provider chat/model refresh,
and a running OpenGuru instance have not been exercised locally.

Publish native assets/checksums first, then nio-de npm. Deploy the web build
only after backend installation URLs are working. A v* tag triggers the backend release workflow; do not tag until
these gates pass. Local validation does not itself publish packages, merge code or deploy the app.
GitHub repositories have been renamed/transferred to the canonical names.
Push, CI, merge and release follow local validation under the user's instruction.

## Baseline and rollback

App baseline: 7b731586bb2f85afb43e1c1b00e2ca0a148ffcbd
Server baseline: 062232d2c8091b49ebae926795f4bc8b60d1d476
Original local remotes: git@github.com:monynith/NoIDE.git and
 git@github.com:n-o-ide/noide-server.git (already redirects to nio-labs/noide-server).
Nio baseline: aab202dcd66c79aea02879fe0d9ad7405edbb5f0
OpenGuru baseline: 4674b2b1c94d2f4886906bcc60e13a956bf18cf2

Keep previous release artifacts and deployments. Revert migration commits or
redeploy previous builds for a code rollback. Retained data/protocol identities
allow prior versions to keep reading the same state. Rename folders/remotes back
if necessary; repository ownership changes require a separate GitHub transfer.
