Manage rig's download cache

## Description

rig caches files it downloads (R binary indexes, CRAN-like package
databases, package DESCRIPTION manifests, downloaded package files, and
packages rig built from source) in an OS-specific cache directory, so it
does not have to re-download them on every run. See [`rig system dirs
--cache`](system.qmd#rig-system-dirs) for the cache directory path. Pass the
`--cache-dir` flag to keep the cache in another directory.

The global `--no-cache` flag makes rig skip this cache entirely for the
current run; it does not affect what `rig cache` reports or deletes. You can
also set the `RIG_NO_CACHE` environment variable to `true` or the `no-cache`
config option to `true`. `--no-cache` takes precedence over `--cache-dir`, but
rig may still write log files there on Linux and Windows.
