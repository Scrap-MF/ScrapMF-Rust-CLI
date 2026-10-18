#!/bin/sh
# scrapmf installer — one-liner:
#   curl -fsSL https://raw.githubusercontent.com/Scrap-MF/ScrapMF-Rust-CLI/main/install.sh | sh
#
# Usage: install.sh [VERSION] [INSTALL_DIR]
#   VERSION      release tag ("latest" by default), e.g. v1.0.0
#   INSTALL_DIR  target dir (default: ~/.local/bin)
# Flags:
#   --gnu        download the dynamically-linked gnu build instead of musl
# Environment:
#   SCRAPMF_INSTALL_DIR   same as INSTALL_DIR argument (arg wins)
set -eu

REPO="Scrap-MF/ScrapMF-Rust-CLI"
BINARY="scrapmf"
VERSION="${1:-latest}"
case "${1:-}" in
  --gnu) VERSION="${2:-latest}"; LIBC="gnu" ;;
  *) LIBC="musl" ;;
esac
INSTALL_DIR="${SCRAPMF_INSTALL_DIR:-${2:-$HOME/.local/bin}}"

# --- architecture / target detection ---------------------------------------
OS=$(uname -s)
ARCH=$(uname -m)

# Linux targets; LIBC defaults to musl (static) and can be switched with --gnu.
# Termux (Android) is aarch64/armv7 too — the static musl builds run as-is.
case "$OS" in
  Darwin)
    echo "✖ macOS builds are not published yet." >&2
    echo "  Install from source instead:" >&2
    echo "    git clone https://github.com/${REPO}.git && cd ScrapMF-Rust-CLI && cargo install --path ." >&2
    exit 1
    ;;
  MINGW*|MSYS*|CYGWIN*)
    echo "✖ Windows: this installer covers Linux and macOS." >&2
    echo "" >&2
    echo "  Download scrapmf.exe from" >&2
    echo "  https://github.com/${REPO}/releases/latest" >&2
    echo "  put it anywhere in your PATH, then run: scrapmf setup" >&2
    exit 1
    ;;
esac

case "$ARCH" in
  x86_64|amd64)
    ARCH_PART="x86_64"
    case "$LIBC" in gnu) ;; *) LIBC="musl" ;; esac
    ;;
  aarch64|arm64) ARCH_PART="aarch64" ;;
  armv7l|armv8l|armv7)
    ARCH_PART="armv7"
    TARGET_SUFFIX="musleabihf"
    if [ "$LIBC" = "gnu" ]; then TARGET_SUFFIX="gnueabihf"; fi
    ;;
  riscv64|riscv64gc)
    ARCH_PART="riscv64gc"
    # Only the gnu build is published for riscv64.
    LIBC="gnu"; TARGET_SUFFIX="gnu"
    ;;
  ppc64le|powerpc64le)
    ARCH_PART="powerpc64le"
    # Only the gnu build is published for powerpc64le.
    LIBC="gnu"; TARGET_SUFFIX="gnu"
    ;;
  *)
    echo "✖ unsupported architecture: $ARCH" >&2
    echo "  published builds: linux x86_64 · aarch64 · armv7 · riscv64 · powerpc64le, windows x86_64" >&2
    exit 1
    ;;
esac

if [ -n "${TARGET_SUFFIX:-}" ]; then
  TARGET="${ARCH_PART}-unknown-linux-${TARGET_SUFFIX}"
else
  TARGET="${ARCH_PART}-unknown-linux-${LIBC}"
fi

ASSET="${BINARY}-${TARGET}.tar.gz"
if [ "$VERSION" = "latest" ]; then
  BASE_URL="https://github.com/${REPO}/releases/latest/download"
else
  BASE_URL="https://github.com/${REPO}/releases/download/${VERSION}"
fi
# Overridable for mirrors/testing: SCRAPMF_RELEASE_URL_BASE
BASE_URL="${SCRAPMF_RELEASE_URL_BASE:-$BASE_URL}"
URL="${BASE_URL}/${ASSET}"

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

fetch() {
  # fetch <output> <url>
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL -o "$1" "$2"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$1" "$2"
  else
    echo "✖ curl or wget required" >&2
    exit 1
  fi
}

echo "→ Downloading $URL"
# Keep the asset's original filename so the published .sha256 matches.
fetch "$TMPDIR/$ASSET" "$URL"

echo "→ Verifying checksum"
fetch "$TMPDIR/$ASSET.sha256" "${URL}.sha256"
(cd "$TMPDIR" && sha256sum -c "$ASSET.sha256") || {
  echo "✖ checksum verification FAILED — the download is corrupt or tampered with." >&2
  echo "  Aborting. Re-run later or download manually from https://github.com/${REPO}/releases" >&2
  exit 1
}

echo "→ Extracting"
tar xzf "$TMPDIR/$ASSET" -C "$TMPDIR"

BIN_SRC=$(find "$TMPDIR" -name "$BINARY" -type f | head -n1)
if [ -z "$BIN_SRC" ]; then
  echo "✖ binary not found in archive" >&2
  exit 1
fi

mkdir -p "$INSTALL_DIR"
install -Dm755 "$BIN_SRC" "$INSTALL_DIR/$BINARY"
echo "✔ Installed $BINARY to $INSTALL_DIR/$BINARY"

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    echo "⚠ $INSTALL_DIR is not in PATH — add it:"
    echo "  export PATH=\"$INSTALL_DIR:\$PATH\"   # add to ~/.bashrc or ~/.zshrc"
    ;;
esac

"$INSTALL_DIR/$BINARY" --version
echo "→ Run '$BINARY --help' to get started."
