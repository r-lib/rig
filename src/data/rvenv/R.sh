#!/bin/sh
# Managed by rig (rig proj sync). Do not edit, `rig proj sync` rewrites it.
#
# A wrapper script, not a symlink: a symlink would resolve R_HOME correctly
# but would carry no environment, so putting .rvenv/bin on PATH would leak
# into the user's own package library. The exec path below is absolute, which
# also pins the R version this project was solved for.
#
# RVENV is derived from this script's own location, so the project directory
# can be moved or checked out anywhere.
RVENV=$(cd "$(dirname "$0")/.." && pwd)
export RVENV
# R_LIBS is empty, so that .libPaths()[1] stays the project library, and
# R_LIBS_SITE points at a path that cannot exist, because an empty one does
# not reliably disable the site library on every R version.
@RVENV_EXPORTS@
exec "@R_BINARY@" "$@"
