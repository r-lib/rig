Install the dependencies `rproj.lock` resolved

## Description

Bring an R project's environment in line with its `rproj.lock`: install the
resolved dependencies, and write the rest of the `.rvenv` layout.

rig looks for the project in the current directory and its parents, reads
its `rproj.lock` and installs the packages into the project library. If the
project has no `rproj.lock` yet, rig runs [`rig proj lock`](#rig-proj-lock) with its default
options first. Pass `--frozen` to fail instead, without touching `rproj.toml`
at all: every package `rproj.lock` lists already carries its own download
URL, so nothing but the lock file is needed to install from it. The one
exception is `.rvenv/etc/repositories` (see below), which still comes from
`rproj.toml` when one is present, and is skipped otherwise.

Development dependencies are installed by default. `--no-dev` leaves them
out; the lock file records which packages are dev-only, so this works the
same with or without `--frozen`. `--max-concurrent` limits the number of
simultaneous installations.

By default, sync also removes any package that is in the project library
but not in `rproj.lock`, e.g. one dropped from `rproj.toml`, or a leftover from
before `--no-dev`. Pass `--inexact` to leave those packages alone instead.

## The R version

The lock file records the R version its solve is valid for, and that is the
R rig installs the packages with. It has to be that very version: another
patch release of the same minor version would run the packages, but it is
not the R the project was solved for, so rig does not quietly use it.

If that R version is not installed, rig installs it first, the way [`rig add`](add.qmd)
would. Pass `--no-install-r` to fail instead, e.g. in CI. rig never rewrites
`rproj.lock` to an R version that is already installed. Run [`rig proj lock`](#rig-proj-lock) to
change the R version a project is locked for.

## Several targets in one lock file

If `rproj.lock` has multiple platforms, then `rig proj sync` picks the target
whose platform matches the OS it runs on. A target for a different OS is
simply inert, which is what makes locking for a Linux deployment target
from a macOS laptop work: each machine's `rig proj sync` picks its own entry
from the same file.

If more than one target matches this machine's OS (typically because the
project locks for several R versions), rig picks the highest R version
among them, with no need for extra flags. Pass `--r-version` and/or
`--platform` to pick a different one of the matching targets instead. `rig
proj sync` fails if none of the lock file's targets match this machine at
all.

## What sync writes

Everything below `.rvenv` is machine-specific and is not committed. The
project library is filled in from the lock file, and the rest is rewritten
on every sync:

- `.rvenv/bin/R` and `.rvenv/bin/Rscript`, wrapper scripts that set the
  project's environment and then hand over to the real R. Run them
  directly, or put `.rvenv/bin` on your `PATH`.
- `.rvenv/bin/activate` and its `activate.csh` / `activate.fish` / `activate.bat` /
  `Activate.ps1` siblings, for the shells that prefer to be activated. Source
  the one for your shell, and call `deactivate` when you are done. Activation
  is a convenience, not a requirement: the wrappers work without it, and an
  R session started by an IDE picks the project up through the project's
  `.Renviron`.
- `.rvenv/rvenv.cfg`, which records the R version, the platform and the
  architecture the environment was built for. rig warns when it syncs an
  environment that was built for a different R.
- `.rvenv/etc/repositories`, which the wrappers point `R_REPOSITORIES` at.
  Reposirories to set up for the project.

After a successful sync rig records the lock file it installed from in
`.rvenv/lib/.synced`. The `rvenv` package in `.rvenvlib` compares the two, and
warns in every R session while the project library does not match
`rproj.lock`.

## Workspaces

In a workspace (see [`rig proj lock`](#rig-proj-lock) ) every member shares one `rproj.lock` and
one package library, both at the workspace root. `rig proj sync` from a
member directory therefore syncs the whole workspace, and installs the
union of every member's dependencies into the root's `.rvenv/lib`.
