Create rproj.toml from an renv.lock file

## Description

Read an `renv.lock` file and create `rproj.toml`, rig's project manifest,
from it: one dependency per locked package (with a `^`-pinned version
requirement), and an `R` requirement from the lockfile's R version. Fails
if `rproj.toml` already exists; use `--dependencies` to merge into an
existing file instead.

`renv.lock` has no project metadata (name, title, authors, ...), so a new
manifest is named after the current directory.

By default rig reads `renv.lock` in the current directory; use `--input`
to point to a different file.

After importing, run [`rig proj lock`](proj.qmd#rig-proj-lock) to solve the
dependencies and write `rproj.lock`.

## Files

A full import sets up a whole project, not just its manifest, so it creates
the same files as [`rig proj init`](proj.qmd#rig-proj-init).

rig refuses to overwrite any of the `.rvenv` files above; pass `--force` to
replace them.

## The `--dependencies` option

`--dependencies` does not require `rproj.toml` to be missing: if it exists,
its dependencies are merged into it (importing a package already listed
overwrites its entry with the version requirement from `renv.lock`); if it
does not exist, a minimal manifest is created first. Because `rproj.toml`
is rewritten in full, any comments or custom formatting in an existing file
are not preserved.
