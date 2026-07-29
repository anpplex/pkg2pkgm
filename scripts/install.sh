#!/usr/bin/env bash
# Install the latest (or a pinned) pkg2mpkg release binary for this machine.
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/anpplex/pkg2pkgm/main/scripts/install.sh | bash
#   VERSION=v0.1.0 bash install.sh
#   INSTALL_DIR=~/.local/bin bash install.sh
set -euo pipefail

REPO="${REPO:-anpplex/pkg2pkgm}"
INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"
VERSION="${VERSION:-latest}"

need() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "error: required command not found: $1" >&2
    exit 1
  }
}

need curl
need tar
need uname

OS="$(uname -s)"
ARCH="$(uname -m)"

case "${OS}" in
  Linux)
    case "${ARCH}" in
      x86_64 | amd64) TARGET="x86_64-unknown-linux-gnu" ;;
      aarch64 | arm64) TARGET="aarch64-unknown-linux-gnu" ;;
      *)
        echo "error: unsupported Linux arch: ${ARCH}" >&2
        exit 1
        ;;
    esac
    EXT="tar.gz"
    ;;
  Darwin)
    case "${ARCH}" in
      x86_64) TARGET="x86_64-apple-darwin" ;;
      arm64) TARGET="aarch64-apple-darwin" ;;
      *)
        echo "error: unsupported macOS arch: ${ARCH}" >&2
        exit 1
        ;;
    esac
    EXT="tar.gz"
    ;;
  *)
    echo "error: unsupported OS: ${OS} (Windows: download the .zip from GitHub Releases)" >&2
    exit 1
    ;;
esac

if [[ "${VERSION}" == "latest" ]]; then
  API="https://api.github.com/repos/${REPO}/releases/latest"
  TAG="$(curl -fsSL "${API}" | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)"
  if [[ -z "${TAG}" ]]; then
    echo "error: could not resolve latest release tag for ${REPO}" >&2
    exit 1
  fi
else
  TAG="${VERSION}"
  case "${TAG}" in
    v*) ;;
    *) TAG="v${TAG}" ;;
  esac
fi

VER="${TAG#v}"
ASSET="pkg2mpkg-v${VER}-${TARGET}.${EXT}"
URL="https://github.com/${REPO}/releases/download/${TAG}/${ASSET}"

TMP="$(mktemp -d)"
trap 'rm -rf "${TMP}"' EXIT

echo "Downloading ${URL}"
curl -fsSL -o "${TMP}/${ASSET}" "${URL}"

tar -xzf "${TMP}/${ASSET}" -C "${TMP}"
BIN="${TMP}/pkg2mpkg-v${VER}-${TARGET}/pkg2mpkg"
test -x "${BIN}"

mkdir -p "${INSTALL_DIR}"
if [[ -w "${INSTALL_DIR}" ]]; then
  install -m 755 "${BIN}" "${INSTALL_DIR}/pkg2mpkg"
else
  echo "Installing to ${INSTALL_DIR} (may require sudo)..."
  sudo install -m 755 "${BIN}" "${INSTALL_DIR}/pkg2mpkg"
fi

echo "Installed: ${INSTALL_DIR}/pkg2mpkg"
"${INSTALL_DIR}/pkg2mpkg" --help | head -20 || true
echo "Done."
