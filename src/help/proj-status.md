Show the project's status

## Description

Show what rig knows about the project in the current directory, or the
workspace it belongs to: the manifest, the lock file, and whether the local
[project environment](../admin-vs-user-mode.qmd) is in sync with it.

This is read-only. It never solves dependencies, downloads anything, or
changes the project -- see [`rig proj lock`](#rig-proj-lock) and
[`rig proj sync`](#rig-proj-sync) for that.

The report has three parts, each shown only if it applies:

* **Manifest** -- the project's name and version from `rproj.toml`, its
  workspace members if it is a workspace, and its dependency counts by
  group (`main` plus any `[dependency-groups.*]`).
* **Lock file** -- `rproj.lock`'s version, and one row per locked target: the
  R version, the platform, and how many packages it locks.
* **Environment** -- the R version and platform the local `.rvenv` was last
  synced against, and whether it is still up to date with the lock file, or
  why not.

Use `--json` for machine readable output.
