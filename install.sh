#!/bin/sh
#
# rig user-mode installer for macOS and Linux.
#
# Installs the `rig` binary and its shell completions into your home
# directory, with no administrator privileges required. See
# <https://github.com/r-lib/rig> for details.
#
# Usage:
#
#   curl -LsSf https://r-lib.github.io/rig/install.sh | sh
#
# To pass options when piping, use `sh -s --`, e.g.:
#
#   curl -LsSf https://r-lib.github.io/rig/install.sh | sh -s -- --no-modify-path
#
# Options (also settable via environment variables):
#
#   --prefix=DIR       Install prefix              (RIG_PREFIX, default $HOME/.local)
#   --version=X.Y.Z    Version to install          (RIG_VERSION, default: 0.10.0-alpha)
#   --no-modify-path   Do not edit shell rc files  (RIG_NO_MODIFY_PATH=1)
#   -h, --help         Show this help and exit

set -eu

REPO="r-lib/rig"
PREFIX="${RIG_PREFIX:-$HOME/.local}"
VERSION="${RIG_VERSION:-0.10.0-alpha}"
MODIFY_PATH=1
if [ "${RIG_NO_MODIFY_PATH:-0}" != "0" ]; then MODIFY_PATH=0; fi

err() { echo "rig install: $*" >&2; exit 1; }
info() { echo "$*"; }

usage() {
  sed -n '3,22p' "$0" 2>/dev/null | sed 's/^# \{0,1\}//'
}

for arg in "$@"; do
  case "$arg" in
    --prefix=*)       PREFIX="${arg#*=}" ;;
    --version=*)      VERSION="${arg#*=}" ;;
    --no-modify-path) MODIFY_PATH=0 ;;
    -h|--help)        usage; exit 0 ;;
    *)                err "unknown option: $arg (try --help)" ;;
  esac
done

# --- Detect platform and architecture -----------------------------------

os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
  Darwin) plat="macos" ;;
  Linux)  plat="linux" ;;
  *)      err "unsupported operating system: $os (only macOS and Linux are supported; on Windows use install.ps1)" ;;
esac

case "$arch" in
  arm64|aarch64)
    # macOS reports arm64, Linux reports aarch64; use each platform's asset name.
    [ "$plat" = "macos" ] && arch="arm64" || arch="aarch64" ;;
  x86_64|amd64) arch="x86_64" ;;
  *) err "unsupported architecture: $arch" ;;
esac

# --- Work out the release asset URL -------------------------------------
#
# The moving `latest` release serves `...-latest.*` assets under the
# `latest` tag; versioned releases serve `...-X.Y.Z.*` assets under the
# `vX.Y.Z` tag.
if [ "$VERSION" = "latest" ]; then
  tag="latest"; vtoken="latest"
else
  tag="v${VERSION}"; vtoken="${VERSION}"
fi

asset="rig-${plat}-${arch}-${vtoken}.tar.gz"
url="https://github.com/${REPO}/releases/download/${tag}/${asset}"

# --- Download and extract -----------------------------------------------

tmp="$(mktemp -d "${TMPDIR:-/tmp}/rig-install.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT INT TERM

info "Downloading $asset ..."
if command -v curl >/dev/null 2>&1; then
  curl -fLsS "$url" -o "$tmp/rig.tar.gz" || err "download failed: $url"
elif command -v wget >/dev/null 2>&1; then
  wget -q "$url" -O "$tmp/rig.tar.gz" || err "download failed: $url"
else
  err "neither curl nor wget is available"
fi

info "Installing into $PREFIX ..."
mkdir -p "$PREFIX"
tar xzf "$tmp/rig.tar.gz" -C "$PREFIX"

bindir="$PREFIX/bin"
[ -x "$bindir/rig" ] || err "installation failed: $bindir/rig not found"

# --- Optionally add the bin directory to PATH ---------------------------

on_path=0
case ":$PATH:" in *":$bindir:"*) on_path=1 ;; esac

if [ "$MODIFY_PATH" -eq 1 ] && [ "$on_path" -eq 0 ]; then
  # Pick an rc file for the user's login shell.
  shellname="$(basename "${SHELL:-sh}")"
  case "$shellname" in
    zsh)  rc="$HOME/.zshrc" ;;
    bash) rc="$HOME/.bashrc" ;;
    *)    rc="$HOME/.profile" ;;
  esac
  line="export PATH=\"$bindir:\$PATH\""
  if [ ! -f "$rc" ] || ! grep -qF "$line" "$rc" 2>/dev/null; then
    {
      echo ""
      echo "# Added by the rig installer"
      echo "$line"
    } >> "$rc"
    info "Added $bindir to your PATH in $rc"
    info "Restart your shell (or 'source $rc') for it to take effect."
  fi
fi

# --- Report next steps --------------------------------------------------

info ""
info "rig has been installed to $bindir/rig"
if [ "$on_path" -eq 0 ] && [ "$MODIFY_PATH" -eq 0 ]; then
  info "Add it to your PATH:  export PATH=\"$bindir:\$PATH\""
fi
info ""
info "To use rig in user mode (no admin needed), run:"
info "    rig system user-mode"
info "    rig add release"
info ""
info "Shell completions were installed under $PREFIX/share:"
info "  zsh:  add '$PREFIX/share/zsh/site-functions' to your fpath"
info "  bash: install bash-completion and make sure it loads"
info "        '$PREFIX/share/bash-completion/completions'"
