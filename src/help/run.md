Run R, an R script or an R project

## Description

Run R, an R script or an R project, using the selected R version.

All of these examples allow an `--r-version` argument, to use a specific R
version.

```sh
rig run                    # start R
rig run <script.R>         # run an R script
rig run -f <script-file>   # run an R script
rig run -e <expression>    # evaluate an R expression
rig run <pkg>::<script>    # run a script from a package's exec directory
rig run <name>             # run a script the project declares
rig run --list             # list the scripts the project declares
rig run <path-to-app>      # run an R app
rig run --cmd <command>    # run `R CMD <command>`
rig run --activate         # start R with the selected version on PATH
rig run --shell            # start a shell with the selected version on PATH
rig run --rscript ...      # run `Rscript` instead of `R`
rig run -- --vanilla       # pass flags to R/Rscript
```

## Supported apps

Currently supported apps are:

- Plumber APIs,
- Shiny apps,
- Quarto documents embedding Shiny apps,
- Quarto documents,
- Rmd documents,
- Rmd documents embedding Shiny apps,
- Static web sites.

## R arguments

Anything after a literal `--` is passed straight to the R (or `Rscript`)
process as its own command-line flags, e.g. `rig run -- --vanilla`.

Plain `rig run` (no `-e`/`-f`/app/`--cmd`) defaults to
`--no-save --no-restore`, so it never shows the "Save workspace image?"
prompt on exit. Override with `rig run -- --save --restore`.

## Projects

If the current directory is inside a [project](proj.qmd), then `rig run` uses
the project's own environment instead of the default R version: it runs
`.rvenv/bin/R`. It also runs `rig proj lock` and `rig proj sync` as needed.

Use `--no-project` to ignore a project, or `--r-version` to select an R
version directly.

## Scripts with inline dependencies

`rig run -f script.R` runs an R script (the `-f` can be omitted). A
script can declare the packages and the R version it needs in a comment
block at its top, so it runs anywhere without a project:

```r
# /// script
# [dependencies]
# R = ">= 4.4"
# cli = "*"
# dplyr = ">= 1.1"
#
# [tool.rig]
# exclude-newer = "2026-06-01"
# ///
library(dplyr)
```

The block starts with a `# /// script` line and ends with a `# ///` line.
In between, every line is a comment, and without the leading `#` they form
a TOML document. The block takes the `[dependencies]`, `[[repository]]` and
`[tool.rig]` tables of `rproj.toml`, with the same meaning.

rig creates an environment for the script in its cache directory, and runs
the script there, isolated from your own package library. It picks an
installed R version that fits the `R` requirement, or installs one, then
locks and installs the packages, the same way `rig proj sync` does for a
project. Later runs reuse the environment. Scripts with the same block
share one environment, and changing the block creates a new one. A script
with a block always uses its own environment, even inside a project.
`--r-version` selects the R version, which must fit the `R` requirement.

`rig proj init --script script.R` adds a block with an `R` requirement to a
script (or creates the script), and `rig proj add --script script.R <package>`
and `rig proj remove --script script.R <package>` edit its `[dependencies]`, so
you do not have to write the block by hand.

`rig cache clean --category scripts` deletes all script environments. A
script can start with `#!/usr/bin/env -S rig run`, to run it directly.

## Project scripts

A project can give its own scripts a name, in the `[[bin]]` tables of its
`rproj.toml`:

```toml
[[bin]]
name = "report"
path = "scripts/report.R"
description = "Build the report"
```

`rig run report --format pdf` then runs `scripts/report.R` in the project's
environment, and passes `--format pdf` on to the script, where
`commandArgs(TRUE)` picks it up. `path` is relative to the project directory,
so a declared script works the same from anywhere within the project.
`rig run --list` lists the declared scripts, and `--json` prints them as JSON.

A declared name wins over a directory of the same name. Arguments that look
like a path rather than a name are never script names: anything containing
a slash or `::`, anything ending in `.R`, `.r`, `.Rmd` or `.qmd`, and `.` and `..`. Use
`./name` to run an app in a directory whose name a script has taken.

## Activation

`rig run --activate` puts the selected R version's `bin` directory on
`PATH` for the R process it starts. `--shell` starts a shell instead of R,
with the same `PATH` change, so `R` and its subprocesses start the
activated R version.

Inside a [project](proj.qmd), both flags put the project's `.rvenv/bin` on
`PATH` rather than the raw R installation, so a nested `R` (started from
the shell `--shell` opens, or from R itself) picks up the project's package
library and repositories too, not just its version.

## Rscript

`rig run --rscript` runs the selected R version's `Rscript` instead of `R`.
Unlike `R -e`/`R -f`, `Rscript` never echoes back the code it runs, which
fits scripts and pipelines better than `R`'s interactive-style echo.

`--rscript` works with `-e`/`-f`, scripts, project scripts, apps and `--activate`,
but not with `--cmd` (`R CMD` only exists as part of the `R` front-end) or
`--shell` (which never runs the R binary at all).
