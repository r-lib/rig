Create a new R project

## Description

Set up an R project in the current directory: the `rproj.toml` manifest, plus
the part of the project's virtual environment (`.rvenv`) that belongs in
version control.

`rproj.toml` is rig's modern project and package file. It describes the
project's metadata and its R and package dependencies, and can do everything
a `DESCRIPTION` file can, plus dependency groups, optional dependencies,
workspaces and declared scripts. `rig proj init` writes a minimal skeleton — a
`[project]` table with the name (taken from the current directory) and
version, and a `[dependencies]` table with a single R requirement — that you
then fill in. Use [`rig proj import`](#rig-proj-import) instead to take the
dependencies from an existing `DESCRIPTION`.

## Files

`rig proj init` creates these, and nothing else. All of them are meant to be
committed, so that a fresh clone of the project works right away:

- `rproj.toml` — the manifest. Its R requirement is `>= <major>.<minor>` of
  the project's R version.
- `.Renviron` — points R at the project library, `.rvenv/lib`. This is what
  makes the project work in an editor (RStudio, Positron, VS Code), which
  starts R itself.
- `.gitignore` — a marked `# rig rvenv start` / `# rig rvenv end` block that
  ignores everything in `.rvenv` except the library directory. An existing
  `.gitignore` is *not* replaced: rig only adds or refreshes its own block,
  and leaves the rest of the file alone.
- `.rvenv/lib/.gitignore` — keeps the library directory itself, and the `rig`
  package in it, in version control, and ignores the installed dependencies.
  The directory has to exist in every checkout, because plain R does not
  create a missing library directory.
- `.rvenv/lib/rig` — a small, pre-built R package that rig manages. It is not
  a dependency of your project. `.Renviron` loads it in every R session
  started in the project, where it turns the relative library path into an
  absolute one — so that R processes started from a subdirectory still use the
  project library — and warns while the project is out of sync with
  `rproj.lock`.

The rest of `.rvenv` is machine-specific and is created by
[`rig proj sync`](#rig-proj-sync), which installs the project's dependencies
into `.rvenv/lib`.

Note that `R --vanilla` ignores `.Renviron`, and so does not use the project
library.

## Options

`--r-version` sets the R version the project is for. It defaults to the
current default R version, and does not have to be installed. Both the
manifest's R requirement and the flavor of the pre-built `rig` package depend
on it.

rig refuses to overwrite any of the files above; pass `--force` to replace
them. `--force` still does not rewrite the whole `.gitignore`, only rig's
block in it.
