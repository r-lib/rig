Run R, an R script or an R project

## Description

Run R, an R script or an R project, using the selected R version.

All of these examples allow an `--r-version` argument, to use a specific
R version.

```sh
rig run                    # start R
rig run -f <script-file>   # run an R script
rig run -e <expression>    # evaluate an R expression
rig run <pkg>::<script>    # run a script from a package's exec directory
rig run <path-to-app>      # run an R app
rig run --cmd <command>    # run `R CMD <command>`
```

## Projects

If the current directory is inside a [project](proj.qmd) (i.e. rig finds an
`rproj.toml`, an `rproj.lock` or an `.rvenv` directory at or above it), then
`rig run` uses the project's own environment instead of the default R
version: it runs `.rvenv/bin/R`, which sets the project's package library
and repositories, and it uses the R version the project's lock file names.

Before running R, rig syncs the environment if it is out of date, i.e. it
runs the equivalent of `rig proj lock` and `rig proj sync` for you, which may
also install the R version the project needs. There is nothing to source and
no shell state to keep, so this also works in a `Makefile` or in CI.

Two things turn this off, and run the default R version instead:

- `--r-version`, because a project environment is tied to the R version its
  lock file names, and
- `--no-project`.

If the project has no `.rvenv` directory at all, then `rig run` fails and
asks you to run `rig proj init`, instead of quietly running an R that is not
the project's.

## Supported apps

Currently supported apps are:

- Plumber APIs,
- Shiny apps,
- Quarto documents embedding Shiny apps,
- Quarto documents,
- Rmd documents,
- Rmd documents embedding Shiny apps,
- Static web sites.
