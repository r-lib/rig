Create a new R project

## Description

Set up an R project in the current directory: the `rproj.toml` manifest, plus
the part of the project's virtual environment that belongs in version
control.

`rproj.toml` is rig's project manifest. It describes the project's metadata
and its R and package dependencies.

`rig proj init` writes a minimal skeleton: a `[project]` table with the name
(taken from the current directory) and version, and a `[dependencies]` table
with a single R requirement, that you then fill in.

Use [`rig proj import`](#rig-proj-import) instead to set up the same project from an existing
`DESCRIPTION` file.

## Files

`rig proj init` creates these, and nothing else. All of them are meant to be
committed, so that a fresh clone of the project works right away:

- `rproj.toml` - the manifest.
- `.Renviron`, `.rvenvlib/rvenv` - boilerplate to set up R's libraries for the
  project when started from the project directory, in a terminal, of from
  an editor (RStudio, Positron, VS Code).
- `.gitignore` - to ignore `.rvenv` which contains the project library and
  configuration files, created by `rig proj sync`.

The R virtual environment in `.rvenv`, including the project library
`.rvenv/lib`, is machine-specific and is created by [`rig proj sync`](#rig-proj-sync) . It can be
deleted and rebuilt at any time.

Note that `R --vanilla` ignores `.Renviron`, and so does not use the project
library.

## Scripts

`rig proj init --script script.R` sets up a single script instead of a project:
it adds a `# /// script` block with an R requirement to the top of the script,
or creates the script, if it does not exist yet:

```r
# /// script
# [dependencies]
# R = ">= 4.6"
# ///
```

The block goes after a `#!` line, if the script has one. It does not create any
other file. Add packages to the block with [`rig proj add --script`](#rig-proj-add), and run the
script with [`rig run`](run.qmd), which sets up an environment for it, see [scripts with
inline dependencies](run.qmd#scripts-with-inline-dependencies). rig refuses to replace a block that is already there, pass
`--force` to replace it.

## Options

`--r-version` sets the R version the project is for. It does not have to be
installed. Defaults to the current default R version, or the current R
release if there is no default.

rig refuses to overwrite any of the files above; pass `--force` to replace
them. The `.gitignore` block is the exception: rig never refuses on an
existing `.gitignore`, it just merges its block into it (or adds one),
leaving the rest of the file alone, and `--force` does not change that.
