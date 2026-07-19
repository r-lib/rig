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

printf 'version: "%s"\n' "$version" > _variables.yml
