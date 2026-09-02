Create a new project manifest

## Description

Write a new `rproj.toml` manifest in the current directory. This is rig's
modern project and package file: it describes the project's metadata and its
R and package dependencies, and can do everything a `DESCRIPTION` file can,
plus dependency groups, optional dependencies, workspaces and declared
scripts.

`rig proj init` writes a minimal skeleton — a `[project]` table with the
name (taken from the current directory) and version, and a `[dependencies]`
table with a single R requirement — that you then fill in.

rig refuses to overwrite an existing `rproj.toml`; pass `--force` to replace
it.
