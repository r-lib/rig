#!/bin/sh
# Quarto pre-render script: expose the crate version from the root Cargo.toml
# to the site as the Quarto `version` variable (used by the Get started hero).
# Runs automatically before every `quarto render`/`quarto preview` because it
# is listed under `project: pre-render:` in _quarto.yml. The working directory
# is the Quarto project dir (this `website/` folder), so Cargo.toml is one up.
set -e

version=$(grep -m1 '^version[[:space:]]*=' ../Cargo.toml |
  sed -E 's/^version[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/')

if [ -z "$version" ]; then
  echo "gen-version.sh: could not read version from ../Cargo.toml" >&2
  exit 1
fi

# Only rewrite _variables.yml when the content actually changes. Quarto watches
# _variables.yml during `quarto preview`; rewriting it on every pre-render (this
# script runs before each render) would re-dirty the file and make the preview
# server fire a page `reload` on every navigation, aborting internal link clicks.
new_content=$(printf 'version: "%s"\n' "$version")
if [ ! -f _variables.yml ] || [ "$(cat _variables.yml)" != "$new_content" ]; then
  printf '%s\n' "$new_content" > _variables.yml
fi
