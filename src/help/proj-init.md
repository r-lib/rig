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
then fill in. Use [`rig proj import`](#rig-proj-import) instead to set up the
same project from an existing `DESCRIPTION` file.

## Files

`rig proj init` creates these, and nothing else. All of them are meant to be
committed, so that a fresh clone of the project works right away:

- `rproj.toml` — the manifest. Its R requirement is `>= <major>.<minor>` of
  the project's R version.
- `.Renviron` — loads the `rvenv` package below in every R session started in
  the project. This is what makes the project work in an editor (RStudio,
  Positron, VS Code), which starts R itself.
- `.gitignore` — a marked `# rig rvenv start` / `# rig rvenv end` block that
  ignores all of `.rvenv`. An existing `.gitignore` is *not* replaced: rig
  only adds or refreshes its own block, and leaves the rest of the file
  alone.
- `.rvenvlib/rvenv` — a small R package that rig writes and manages. It
  is not a dependency of your project, and it lives in rig's own library
  rather than in the project library, which holds only your project's
  packages. `.Renviron` loads it in every R session started in the project,
  where it points R at the project library, `.rvenv/lib`, as an absolute path
  — so that R processes started from a subdirectory still use it — and warns
  while the project is out of sync with `rproj.lock`.

`.rvenv` itself, including the project library `.rvenv/lib`, is
machine-specific and is created by [`rig proj sync`](#rig-proj-sync), which
installs the project's dependencies into it. It can be deleted and rebuilt at
any time; `.rvenvlib` is the only part of the environment that is committed.

Note that `R --vanilla` ignores `.Renviron`, and so does not use the project
library.

## Options

`--r-version` sets the R version the project is for. It does not have to be
installed. Defaults to the current default R version, or the current R
release if there is no default.

rig refuses to overwrite any of the files above; pass `--force` to replace
them. `--force` still does not rewrite the whole `.gitignore`, only rig's
block in it.
