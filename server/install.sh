#!/bin/sh
# NoIDE installer
#
# Downloads noide-server and/or port-forward binaries for your OS/arch from the
# latest GitHub release, verifies their SHA-256 checksum, and installs them.
# Re-running the script upgrades to the newest version.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/n-o-ide/noide-server/main/install.sh | bash
#
# Options:
#   VERSION=x.y.z bash install.sh          Install a specific version (default: latest)
#   bash install.sh --dry-run              Print what would happen, change nothing
#   bash install.sh --force                Reinstall even when the version matches
#   bash install.sh --port-forward         Install port-forward only
#   bash install.sh --all                  Install both noide-server and port-forward
#
# Environment:
#   NOIDE_INSTALL_DIR                Install directory (default ~/.local/bin, or
#                                    /usr/local/bin when run as root)

set -eu

REPO="n-o-ide/noide-server"
DRY_RUN=0
FORCE=0
INSTALL_SERVER=1
INSTALL_PORT_FORWARD=0

usage() {
  echo "Usage: bash install.sh [--dry-run] [--force] [--port-forward] [--all]"
  echo "       VERSION=x.y.z bash install.sh [--dry-run] [--force] [--port-forward] [--all]"
}

for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=1 ;;
    --force) FORCE=1 ;;
    --port-forward) INSTALL_SERVER=0; INSTALL_PORT_FORWARD=1 ;;
    --all) INSTALL_SERVER=1; INSTALL_PORT_FORWARD=1 ;;
    -h | --help) usage; exit 0 ;;
    *) echo "unknown option: $arg" >&2; usage >&2; exit 1 ;;
  esac
done

# --- Detect OS / arch ----------------------------------------------------------

OS="$(uname -s 2>/dev/null || echo unknown)"
ARCH="$(uname -m 2>/dev/null || echo unknown)"

case "$OS" in
  Linux) OS_SUFFIX="linux" ;;
  Darwin) OS_SUFFIX="darwin" ;;
  *)
    echo "error: unsupported OS '$OS'. NoIDE supports Linux and macOS." >&2
    exit 1
    ;;
esac

case "$ARCH" in
  x86_64 | amd64) ARCH_SUFFIX="x86_64" ;;
  aarch64 | arm64) ARCH_SUFFIX="aarch64" ;;
  *)
    echo "error: unsupported architecture '$ARCH'. Supported: x86_64, aarch64." >&2
    exit 1
    ;;
esac

SUFFIX="${OS_SUFFIX}-${ARCH_SUFFIX}"

if ! command -v curl >/dev/null 2>&1; then
  echo "error: curl is required to install NoIDE" >&2
  exit 1
fi

# --- Resolve version -----------------------------------------------------------

if [ -z "${VERSION:-}" ]; then
  echo "Fetching the latest NoIDE release…" >&2
  TAG="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" |
    sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)"
  if [ -z "$TAG" ]; then
    echo "error: could not determine the latest release from GitHub." >&2
    echo "       Set VERSION=x.y.z to install a specific version." >&2
    exit 1
  fi
else
  case "$VERSION" in
    v*) TAG="$VERSION" ;;
    *) TAG="v${VERSION}" ;;
  esac
fi

BASE_URL="https://github.com/${REPO}/releases/download/${TAG}"

# --- Install directory ----------------------------------------------------------

if [ "$(id -u)" -eq 0 ]; then
  INSTALL_DIR="${NOIDE_INSTALL_DIR:-/usr/local/bin}"
else
  INSTALL_DIR="${NOIDE_INSTALL_DIR:-${HOME}/.local/bin}"
fi

# --- Download + verify + install helper ----------------------------------------

install_binary() {
  local BIN_NAME="$1"
  local ASSET="${BIN_NAME}-${SUFFIX}"
  local DEST="${INSTALL_DIR}/${BIN_NAME}"

  echo "noide-server ${TAG} (${SUFFIX}) → ${DEST}" >&2
  echo "  download: ${BASE_URL}/${ASSET}" >&2
  echo "  checksum: ${BASE_URL}/SHA256SUMS" >&2

  if [ "$DRY_RUN" -eq 1 ]; then
    echo "(dry run — nothing installed)" >&2
    return 0
  fi

  mkdir -p "$INSTALL_DIR"

  # No-op upgrade when the installed version already matches (unless --force).
  if [ "$FORCE" -eq 0 ] && [ -x "$DEST" ] && "$DEST" --version 2>/dev/null | grep -q " ${TAG#v}"; then
    echo "${BIN_NAME} ${TAG#v} is already installed at ${DEST}." >&2
    echo "Re-run with --force to reinstall." >&2
    return 0
  fi

  local TMP_DIR
  TMP_DIR="$(mktemp -d)"
  trap 'rm -rf "$TMP_DIR"' EXIT

  echo "Verifying SHA-256 checksum…" >&2
  curl -fsSL "${BASE_URL}/SHA256SUMS" -o "${TMP_DIR}/SHA256SUMS"
  local EXPECTED
  EXPECTED="$(awk -v a="${ASSET}" '$2 == a || $2 == ("*" a) { print $1; exit }' "${TMP_DIR}/SHA256SUMS")"
  if [ -z "$EXPECTED" ]; then
    echo "error: no checksum found for ${ASSET} in the release's SHA256SUMS." >&2
    exit 1
  fi

  curl -fsSL "${BASE_URL}/${ASSET}" -o "${TMP_DIR}/${ASSET}"

  local ACTUAL
  if command -v sha256sum >/dev/null 2>&1; then
    ACTUAL="$(sha256sum "${TMP_DIR}/${ASSET}" | awk '{print $1}')"
  else
    ACTUAL="$(shasum -a 256 "${TMP_DIR}/${ASSET}" | awk '{print $1}')"
  fi

  if [ "$ACTUAL" != "$EXPECTED" ]; then
    echo "error: checksum mismatch for ${ASSET}." >&2
    echo "  expected: ${EXPECTED}" >&2
    echo "  actual:   ${ACTUAL}" >&2
    echo "  Refusing to install. Re-run to retry (may be a transient error)." >&2
    exit 1
  fi

  install -m 0755 "${TMP_DIR}/${ASSET}" "$DEST"
  echo "Installed ${BIN_NAME} ${TAG#v} to ${DEST}" >&2
}

# --- Install requested binaries ------------------------------------------------

if [ "$INSTALL_SERVER" -eq 1 ]; then
  install_binary "noide-server"
fi

if [ "$INSTALL_PORT_FORWARD" -eq 1 ]; then
  install_binary "port-forward"
fi

# --- Post-install notes --------------------------------------------------------

case ":$PATH:" in
  *":${INSTALL_DIR}:"*) : ;;
  *) echo "note: ${INSTALL_DIR} is not on your PATH." >&2
     echo "     Add it, e.g.:  export PATH=\"${INSTALL_DIR}:\$PATH\"" >&2 ;;
esac

echo >&2
if [ "$INSTALL_SERVER" -eq 1 ]; then
  echo "Start noide-server with:  ${INSTALL_DIR}/noide-server" >&2
  echo "It prints a pairing code + QR — enter it in the NoIDE app." >&2
fi
if [ "$INSTALL_PORT_FORWARD" -eq 1 ]; then
  echo "Start port-forward with:  ${INSTALL_DIR}/port-forward" >&2
fi
echo "Re-run this installer any time to upgrade." >&2
