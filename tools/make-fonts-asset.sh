#! /bin/bash

# Build the `rig-fonts-<n>.tar.gz` asset: the fallback fonts that rig installs
# next to the portable Linux R builds.
#
# The portable R builds from https://github.com/rstudio/r-builds bundle
# libfontconfig, but no fontconfig configuration and no fonts, so on a minimal
# system (slim containers, Alpine, bare servers) R has nothing to render text
# with. rig writes a fonts.conf itself and downloads this asset for the fonts.
#
# The asset is published once, to a non-moving GitHub release tag, and is
# deliberately not tied to a rig release: it only changes when the font set
# changes, and then it gets a new number and a new tag. Update
# `RIG_FONTS_URL` and `RIG_FONTS_SHA256` in src/linux.rs afterwards.
#
# Usage: ./tools/make-fonts-asset.sh [output-directory]

set -e

DEJAVU_VERSION=2.37
DEJAVU_URL="https://github.com/dejavu-fonts/dejavu-fonts/releases/download/version_${DEJAVU_VERSION//./_}/dejavu-fonts-ttf-${DEJAVU_VERSION}.tar.bz2"

# Bump this (and the release tag) whenever the contents change.
ASSET_VERSION=1

# The faces rig's fonts.conf aliases sans / serif / mono to, in all four
# styles. The rest of the DejaVu family is left out to keep the asset small.
FACES="
DejaVuSans.ttf
DejaVuSans-Bold.ttf
DejaVuSans-Oblique.ttf
DejaVuSans-BoldOblique.ttf
DejaVuSerif.ttf
DejaVuSerif-Bold.ttf
DejaVuSerif-Italic.ttf
DejaVuSerif-BoldItalic.ttf
DejaVuSansMono.ttf
DejaVuSansMono-Bold.ttf
DejaVuSansMono-Oblique.ttf
DejaVuSansMono-BoldOblique.ttf
"

outdir="${1:-.}"
outdir="$(cd "$outdir" && pwd)"
asset="${outdir}/rig-fonts-${ASSET_VERSION}.tar.gz"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo ">>> Downloading $DEJAVU_URL"
curl -fsSL -o "${tmp}/dejavu.tar.bz2" "$DEJAVU_URL"

echo ">>> Extracting"
mkdir -p "${tmp}/x" "${tmp}/stage/fonts"
tar xjf "${tmp}/dejavu.tar.bz2" -C "${tmp}/x"
src="${tmp}/x/dejavu-fonts-ttf-${DEJAVU_VERSION}"

for face in $FACES; do
    cp "${src}/ttf/${face}" "${tmp}/stage/fonts/${face}"
done
cp "${src}/LICENSE" "${tmp}/stage/fonts/LICENSE"

echo ">>> Packing $asset"
# Reproducible-ish: sorted entries, fixed owner, fixed mtime, no gzip
# timestamp, so rebuilding the same font set gives the same checksum.
( cd "${tmp}/stage" && \
  find fonts -type f | LC_ALL=C sort | \
  tar cf - --no-recursion --owner=0 --group=0 --numeric-owner \
      --mtime='@0' --format=ustar -T - ) | gzip -n -9 > "$asset"

if command -v sha256sum >/dev/null; then
    sha256="$(sha256sum "$asset" | cut -d' ' -f1)"
else
    sha256="$(shasum -a 256 "$asset" | cut -d' ' -f1)"
fi

echo
echo "Asset:  $asset"
echo "Size:   $(wc -c < "$asset") bytes"
echo "SHA256: $sha256"
echo
echo "Publish with:"
echo "  gh release create assets-fonts-v${ASSET_VERSION} \\"
echo "    --repo r-lib/rig --title 'rig fallback fonts ${ASSET_VERSION}' \\"
echo "    --notes 'DejaVu ${DEJAVU_VERSION} subset used by rig for portable Linux R builds.' \\"
echo "    '$asset'"
echo
echo "Then update RIG_FONTS_URL and RIG_FONTS_SHA256 in src/linux.rs."
