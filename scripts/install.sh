#!/bin/sh
# Constant installer. Downloads a prebuilt binary from GitHub Releases, verifies
# its sha256, and installs it to ~/.local/bin (no sudo).
#
#   curl -fsSL https://raw.githubusercontent.com/kennykankush/constant/main/scripts/install.sh | sh
#
# Env:
#   CONSTANT_VERSION       version tag to install (default: latest, e.g. v0.2.0)
#   CONSTANT_INSTALL_DIR   install directory (default: $HOME/.local/bin)
set -eu

REPO="kennykankush/constant"
BIN="constant"
INSTALL_DIR="${CONSTANT_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${CONSTANT_VERSION:-latest}"

say() { printf '%s\n' "$*"; }
err() { printf 'error: %s\n' "$*" >&2; }
fallback() {
  err "$1"
  err "build from source instead:"
  err "  cargo install --git https://github.com/$REPO --locked"
  exit 1
}

need() { command -v "$1" >/dev/null 2>&1 || fallback "required tool not found: $1"; }
need curl
need tar

# --- detect platform ------------------------------------------------------
os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Darwin) os_t="apple-darwin" ;;
  Linux)  os_t="unknown-linux-gnu" ;;
  *) fallback "unsupported OS: $os" ;;
esac
case "$arch" in
  arm64 | aarch64) arch_t="aarch64" ;;
  x86_64 | amd64)  arch_t="x86_64" ;;
  *) fallback "unsupported architecture: $arch" ;;
esac
if [ "$os_t" = "unknown-linux-gnu" ] && [ "$arch_t" = "aarch64" ]; then
  fallback "no prebuilt linux arm64 binary is published"
fi
target="${arch_t}-${os_t}"

# --- resolve version ------------------------------------------------------
if [ "$VERSION" = "latest" ]; then
  tag="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" 2>/dev/null \
    | grep '"tag_name"' | head -1 | cut -d '"' -f4 || true)"
  [ -n "$tag" ] || fallback "no published release found yet"
else
  tag="$VERSION"
fi

asset="constant-${tag}-${target}.tar.gz"
base="https://github.com/$REPO/releases/download/${tag}"
tmp="$(mktemp -d)"
staged=""
trap 'rm -rf "$tmp"; [ -n "$staged" ] && rm -f "$staged" 2>/dev/null || true' EXIT

say "downloading $asset ..."
curl -fsSL "$base/$asset" -o "$tmp/$asset" || fallback "download failed: $base/$asset"
curl -fsSL "$base/$asset.sha256" -o "$tmp/$asset.sha256" || fallback "checksum download failed"

# --- verify checksum ------------------------------------------------------
say "verifying checksum ..."
( cd "$tmp" &&
  if command -v shasum >/dev/null 2>&1; then shasum -a 256 -c "$asset.sha256"
  elif command -v sha256sum >/dev/null 2>&1; then sha256sum -c "$asset.sha256"
  else err "no sha256 tool available"; exit 1
  fi ) >/dev/null || fallback "checksum verification FAILED"

# --- install --------------------------------------------------------------
tar -C "$tmp" -xzf "$tmp/$asset"
mkdir -p "$INSTALL_DIR"

# Stage into the install dir, smoke-test, then atomically swap into place — so a
# failed/incompatible download never clobbers an existing working install. The
# staged temp sits in the same dir so the rename is atomic on the same filesystem.
staged="$INSTALL_DIR/.$BIN.tmp.$$"
cp "$tmp/constant-${tag}-${target}/$BIN" "$staged"
chmod +x "$staged"
if ! ver="$("$staged" --version 2>&1)"; then
  rm -f "$staged"
  staged=""
  err "downloaded binary does not run:"
  err "  $ver"
  fallback "the artifact appears incompatible with this platform"
fi
mv -f "$staged" "$INSTALL_DIR/$BIN"
staged=""

say "installed $BIN $tag ($ver) -> $INSTALL_DIR/$BIN"

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    say ""
    say "note: $INSTALL_DIR is not on your PATH. Add this to your shell profile:"
    say "  export PATH=\"$INSTALL_DIR:\$PATH\""
    ;;
esac
