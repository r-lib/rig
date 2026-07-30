#!/usr/bin/env bash
#
# Collect rig release artifacts from CI, upload them to a versioned GitHub
# release, and refresh the moving `latest` snapshot release.
#
# It pulls:
#   * Linux artifacts from a test.yaml run:
#       - rig-linux-x86_64   (rig-linux-x86_64-<ver>.tar.gz, *_amd64.deb, *.x86_64.rpm)
#       - rig-linux-aarch64  (rig-linux-aarch64-<ver>.tar.gz, *_arm64.deb, *.aarch64.rpm)
#   * Signed Windows artifacts from a sign-windows.yaml run:
#       - rig-windows-x86_64-signed   (rig-<ver>.exe -> rig-windows-<ver>.exe)
#       - rig-windows-aarch64-signed  (rig-<ver>.exe -> rig-windows-arm64-<ver>.exe)
#   * Optionally, macOS packages/tarballs from a local directory (built and
#     notarized locally):
#       - rig-<ver>-macOS-{arm64,x86_64}.pkg
#       - rig-macos-{arm64,x86_64}-<ver>.tar.gz
#
# The versioned release is uploaded WITHOUT overwriting existing assets
# (already-present files are skipped). The `latest` release is always
# clobbered, and additionally receives the legacy-named aliases that older
# docs/URLs still use (rig-linux-latest.tar.gz, rig-linux-arm64-latest.tar.gz).
#
# Usage:
#   tools/release-assets.sh \
#       --tag v0.10.0 \
#       --build-run <RUN_ID> \
#       --win-run <RUN_ID> \
#       [--macos-dir ./0.10.0] \
#       [--no-latest] \
#       [--dry-run]

set -euo pipefail

REPO="r-lib/rig"
TAG=""
BUILD_RUN=""
WIN_RUN=""
MACOS_DIR=""
DO_LATEST=1
DRY_RUN=0

err()  { printf 'error: %s\n' "$*" >&2; exit 1; }
warn() { printf 'warning: %s\n' "$*" >&2; }
info() { printf '==> %s\n' "$*"; }

usage() {
    sed -n '3,30p' "$0" | sed 's/^# \{0,1\}//'
    exit "${1:-0}"
}

while [ $# -gt 0 ]; do
    case "$1" in
        --tag)        TAG="${2:?}"; shift 2 ;;
        --build-run)  BUILD_RUN="${2:?}"; shift 2 ;;
        --win-run)    WIN_RUN="${2:?}"; shift 2 ;;
        --macos-dir)  MACOS_DIR="${2:?}"; shift 2 ;;
        --no-latest)  DO_LATEST=0; shift ;;
        --dry-run)    DRY_RUN=1; shift ;;
        -h|--help)    usage 0 ;;
        *)            err "unknown argument: $1 (see --help)" ;;
    esac
done

[ -n "$TAG" ]       || err "--tag is required (e.g. --tag v0.10.0)"
[ -n "$BUILD_RUN" ] || err "--build-run is required (a test.yaml run id)"
[ -n "$WIN_RUN" ]   || err "--win-run is required (a sign-windows.yaml run id)"
command -v gh >/dev/null || err "the GitHub CLI ('gh') is required"

VERSION="${TAG#v}"
info "repo=$REPO tag=$TAG version=$VERSION"

WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/rig-release.XXXXXX")"
trap 'rm -rf "$WORKDIR"' EXIT
DL="$WORKDIR/dl"; STAGE="$WORKDIR/stage"; LATEST="$WORKDIR/latest"
mkdir -p "$DL" "$STAGE" "$LATEST"

# ---------------------------------------------------------------------------
# Download the CI artifacts.
# ---------------------------------------------------------------------------
download_artifact() { # <run-id> <artifact-name> <dest-subdir>
    info "downloading artifact '$2' from run $1"
    gh run download "$1" --repo "$REPO" -n "$2" -D "$DL/$3" \
        || err "could not download artifact '$2' from run $1"
}

download_artifact "$BUILD_RUN" rig-linux-x86_64          linux-x86_64
download_artifact "$BUILD_RUN" rig-linux-aarch64         linux-aarch64
download_artifact "$WIN_RUN"   rig-windows-x86_64-signed win-x86_64
download_artifact "$WIN_RUN"   rig-windows-aarch64-signed win-aarch64

# ---------------------------------------------------------------------------
# Stage the canonical release-named files into $STAGE.
# ---------------------------------------------------------------------------
# Find exactly one file matching <pattern> under <dir>, copy it into $STAGE
# under <dstname> ("" keeps the source name).
stage() { # <dir> <pattern> [dstname]
    local dir="$1" pat="$2" dst="${3:-}"
    local matches=()
    while IFS= read -r -d '' f; do matches+=("$f"); done \
        < <(find "$dir" -type f -name "$pat" -print0 2>/dev/null)
    [ "${#matches[@]}" -gt 0 ] || err "no file matching '$pat' in $dir"
    [ "${#matches[@]}" -eq 1 ] || err "multiple files matching '$pat' in $dir: ${matches[*]}"
    local src="${matches[0]}"
    [ -n "$dst" ] || dst="$(basename "$src")"
    cp -p "$src" "$STAGE/$dst"
    printf '    staged %-38s <- %s\n' "$dst" "$(basename "$src")"
}

info "staging Linux x86_64 assets"
stage "$DL/linux-x86_64" 'rig-linux-x86_64-*.tar.gz'
stage "$DL/linux-x86_64" 'r-rig_*_amd64.deb'
stage "$DL/linux-x86_64" 'r-rig-*.x86_64.rpm'

info "staging Linux aarch64 assets"
stage "$DL/linux-aarch64" 'rig-linux-aarch64-*.tar.gz'
stage "$DL/linux-aarch64" 'r-rig_*_arm64.deb'
stage "$DL/linux-aarch64" 'r-rig-*.aarch64.rpm'

info "staging signed Windows assets"
stage "$DL/win-x86_64"  '*.exe' "rig-windows-${VERSION}.exe"
stage "$DL/win-aarch64" '*.exe' "rig-windows-arm64-${VERSION}.exe"

if [ -n "$MACOS_DIR" ]; then
    [ -d "$MACOS_DIR" ] || err "--macos-dir '$MACOS_DIR' is not a directory"
    info "staging macOS assets from $MACOS_DIR"
    stage "$MACOS_DIR" "rig-${VERSION}-macOS-arm64.pkg"
    stage "$MACOS_DIR" "rig-${VERSION}-macOS-x86_64.pkg"
    stage "$MACOS_DIR" "rig-macos-arm64-${VERSION}.tar.gz"
    stage "$MACOS_DIR" "rig-macos-x86_64-${VERSION}.tar.gz"
else
    warn "no --macos-dir given; macOS packages/tarballs will not be uploaded"
fi

# ---------------------------------------------------------------------------
# Upload to the versioned release (never overwrite existing assets).
# ---------------------------------------------------------------------------
gh release view "$TAG" --repo "$REPO" >/dev/null 2>&1 \
    || err "release '$TAG' does not exist on $REPO; create it first"

existing="$(gh release view "$TAG" --repo "$REPO" --json assets --jq '.assets[].name')"

to_upload=()
for f in "$STAGE"/*; do
    name="$(basename "$f")"
    if grep -qxF "$name" <<<"$existing"; then
        warn "skip (already on $TAG): $name"
    else
        to_upload+=("$f")
    fi
done

if [ "${#to_upload[@]}" -eq 0 ]; then
    info "nothing new to upload to $TAG"
else
    info "uploading ${#to_upload[@]} asset(s) to $TAG (no overwrite)"
    for f in "${to_upload[@]}"; do printf '    %s\n' "$(basename "$f")"; done
    if [ "$DRY_RUN" -eq 1 ]; then
        info "[dry-run] gh release upload $TAG ..."
    else
        gh release upload "$TAG" "${to_upload[@]}" --repo "$REPO"
    fi
fi

# ---------------------------------------------------------------------------
# Refresh the moving `latest` release (clobber, plus legacy aliases).
# ---------------------------------------------------------------------------
# Map a canonical release filename to its `latest` snapshot name(s).
latest_names() { # <canonical-basename>
    case "$1" in
        rig-linux-x86_64-*.tar.gz)  echo rig-linux-x86_64-latest.tar.gz rig-linux-latest.tar.gz ;;
        rig-linux-aarch64-*.tar.gz) echo rig-linux-aarch64-latest.tar.gz rig-linux-arm64-latest.tar.gz ;;
        r-rig_*_amd64.deb)          echo r-rig_latest-1_amd64.deb ;;
        r-rig_*_arm64.deb)          echo r-rig_latest-1_arm64.deb ;;
        r-rig-*.x86_64.rpm)         echo r-rig-latest-1.x86_64.rpm ;;
        r-rig-*.aarch64.rpm)        echo r-rig-latest-1.aarch64.rpm ;;
        rig-windows-arm64-*.exe)    echo rig-windows-arm64-latest.exe ;;
        rig-windows-*.exe)          echo rig-windows-latest.exe ;;
        rig-*-macOS-arm64.pkg)      echo rig-latest-macOS-arm64.pkg ;;
        rig-*-macOS-x86_64.pkg)     echo rig-latest-macOS-x86_64.pkg ;;
        rig-macos-arm64-*.tar.gz)   echo rig-macos-arm64-latest.tar.gz ;;
        rig-macos-x86_64-*.tar.gz)  echo rig-macos-x86_64-latest.tar.gz ;;
        *) warn "no 'latest' mapping for $1" ;;
    esac
}

if [ "$DO_LATEST" -eq 1 ]; then
    info "preparing 'latest' snapshot assets"
    for f in "$STAGE"/*; do
        name="$(basename "$f")"
        for ln in $(latest_names "$name"); do
            cp -p "$f" "$LATEST/$ln"
            printf '    %-34s <- %s\n' "$ln" "$name"
        done
    done
    info "uploading $(find "$LATEST" -type f | wc -l | tr -d ' ') asset(s) to 'latest' (clobber)"
    if [ "$DRY_RUN" -eq 1 ]; then
        info "[dry-run] gh release upload latest ... --clobber"
    else
        gh release upload latest "$LATEST"/* --repo "$REPO" --clobber
    fi
else
    info "skipping 'latest' snapshot (--no-latest)"
fi

info "done"
