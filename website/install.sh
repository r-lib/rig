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
#   --version=X.Y.Z    Version to install          (RIG_VERSION, default: latest)
#                      Use 'nightly' for the latest development build.
#   --no-modify-path   Do not edit shell rc files  (RIG_NO_MODIFY_PATH=1)
#   -h, --help         Show this help and exit

set -eu

REPO="r-lib/rig"
PREFIX="${RIG_PREFIX:-$HOME/.local}"
VERSION="${RIG_VERSION:-latest}"
MODIFY_PATH=1
if [ "${RIG_NO_MODIFY_PATH:-0}" != "0" ]; then MODIFY_PATH=0; fi

err() { echo "rig install: $*" >&2; exit 1; }
info() { echo "$*"; }

usage() {
  sed -n '3,23p' "$0" 2>/dev/null | sed 's/^# \{0,1\}//'
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
#
# The moving `nightly` release serves `...-nightly.*` assets under the
# `nightly` tag. While a new nightly build is being swapped in, `nightly`
# can briefly return 404 and the new build is under `nightly-staging`
# instead (see .github/workflows/nightly.yml). So we try `nightly`, then
# `nightly-staging`, then `nightly` once more, in case the swap finished
# between the first two tries.
if [ "$VERSION" = "latest" ]; then
  tags="latest"; vtoken="latest"
elif [ "$VERSION" = "nightly" ]; then
  tags="nightly nightly-staging nightly"; vtoken="nightly"
else
  tags="v${VERSION}"; vtoken="${VERSION}"
fi

asset="rig-${plat}-${arch}-${vtoken}.tar.gz"

# --- Download and extract -----------------------------------------------

tmp="$(mktemp -d "${TMPDIR:-/tmp}/rig-install.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT INT TERM

if command -v curl >/dev/null 2>&1; then
  download() { curl -fLsS "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
  download() { wget -q "$1" -O "$2"; }
else
  err "neither curl nor wget is available"
fi

info "Downloading $asset ..."
ntags=$(echo $tags | wc -w)
i=0
for tag in $tags; do
  i=$((i + 1))
  url="https://github.com/${REPO}/releases/download/${tag}/${asset}"
  if [ "$i" -lt "$ntags" ]; then
    # Not the last try: a failure here is expected now and then, stay quiet.
    download "$url" "$tmp/rig.tar.gz" 2>/dev/null && break
  else
    download "$url" "$tmp/rig.tar.gz" || err "download failed: $url"
  fi
done

info "Installing into $PREFIX ..."
mkdir -p "$PREFIX"
tar xzf "$tmp/rig.tar.gz" -C "$PREFIX"

bindir="$PREFIX/bin"
[ -x "$bindir/rig" ] || err "installation failed: $bindir/rig not found"

# --- Write an install receipt --------------------------------------------
#
# `rig self update` only replaces the binary for installs it can prove it
# fully owns, i.e. ones made by this script. It looks for this receipt, and
# refuses to touch a rig installed via a .pkg/.deb/.rpm/Chocolatey/WinGet/
# Homebrew instead.
case "$(uname -s)" in
  Darwin) receipt_dir="$HOME/Library/Application Support/com.gaborcsardi.rig" ;;
  *)      receipt_dir="${XDG_DATA_HOME:-$HOME/.local/share}/rig" ;;
esac
mkdir -p "$receipt_dir"
cat > "$receipt_dir/install-receipt.json" <<EOF
{
  "receipt_version": 1,
  "install_method": "script",
  "rig_version": "${vtoken}",
  "platform": "${plat}",
  "arch": "${arch}",
  "bin_path": "${bindir}/rig",
  "prefix": "${PREFIX}",
  "installed_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
EOF

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
