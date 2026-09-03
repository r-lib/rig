Install the dependencies rproj.lock resolved

## Description

Bring an R project's environment in line with its `rproj.lock`: install the
resolved dependencies, and write the rest of the `.rvenv` layout.

rig looks for the project in the current directory and its parents, reads its
`rproj.lock` (written by [`rig proj lock`](#rig-proj-lock)) and installs the
packages into the project library, `.rvenv/lib`. If the project has no
`rproj.lock` yet, rig runs [`rig proj lock`](#rig-proj-lock) with its default
options first, to create one. That library is created by
[`rig proj init`](#rig-proj-init), together with the `.gitignore` files that
keep it in version control, so `rig proj sync` fails if it is missing. Pass
`--library` to install somewhere else instead.

Development dependencies are installed by default. `--no-dev` leaves them
out. `--max-concurrent` limits the number of simultaneous installations
(default: 8).

## The R version

The lock file records the R version its solve is valid for, and that is the R
rig installs the packages with -- not whatever `R` is on the `PATH`. It has to
be that very version: another patch release of the same minor version would
run the packages, but it is not the R the project was solved for, so rig does
not quietly use it.

If that R version is not installed, rig installs it first, the way
[`rig add`](add.qmd) would; pass `--no-install-r` to fail instead, e.g. in CI.
rig never rewrites `rproj.lock` to an R version that is already installed --
run [`rig proj lock`](#rig-proj-lock) to change the R version a project is
locked for.

## What sync writes

Everything below `.rvenv`, except the library and the shim package in it, is
machine-specific, is not committed, and is rewritten on every sync:

- `.rvenv/bin/R` and `.rvenv/bin/Rscript`, wrapper scripts that set the
  project's environment and then hand over to the real R. Run them directly,
  or put `.rvenv/bin` on your `PATH`. They also pass `R CMD ...` through.
- `.rvenv/bin/activate` and its `activate.csh` / `activate.fish` /
  `activate.bat` / `Activate.ps1` siblings, for the shells that prefer to be
  activated. Source the one for your shell, and `deactivate` when you are
  done. Activation is a convenience, not a requirement: the wrappers work
  without it, and an R session started by an IDE picks the project up through
  the project's `.Renviron`.
- `.rvenv/rvenv.cfg`, which records the R version, the platform and the
  architecture the environment was built for. rig warns when it syncs an
  environment that was built for a different R.
- `.rvenv/etc/repositories`, which the wrappers point `R_REPOSITORIES` at. It
  lists P3M first, at the binary URL of the platform the lock file was solved
  for, so that an `install.packages()` in the environment installs the same
  binary packages `rig proj sync` does. The repositories from `rproj.toml`
  follow it, at lower precedence. A lock file solved for source packages only
  has no P3M entry, and then the file holds the `rproj.toml` repositories
  alone (CRAN, if it names none).

`R --vanilla` ignores the project's `.Renviron`, so it only stays inside the
project when started through the wrappers.

After a successful sync rig records the lock file it installed from in
`.rvenv/lib/.synced`. The `rig` package in the project library compares the
two, and warns in every R session while the project library does not match
`rproj.lock`.
