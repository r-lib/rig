Check a package with `R CMD check`

## Description

Build the package in the current directory, or in the nearest directory
above it that has an `rproj.toml` or a `DESCRIPTION` file, with `R CMD build`,
and then check it with `R CMD check`.

```sh
rig proj check                     # check the package
rig proj check --as-cran           # check it the way CRAN does
rig proj check -- --no-tests       # pass options to R CMD check
```

## Which R

In a [project](../proj-guide.qmd) the check runs in the project environment,
the same way as [`rig run`](run.qmd): rig locks and syncs the project first,
if needed. A sync installs the `dev` dependency group by default, which has
the `Suggests` packages the check needs.

A package with only a `DESCRIPTION` file and no `rproj.toml` uses the default
R version and its package library.

Use `--r-version` to check with another R version, with the R version's own
package library.

## What rig builds

rig copies the package to a temporary directory and builds it from there,
without `rproj.toml`, `rproj.lock`, `.rvenv`, `.rvenvlib` and the project's
`.Renviron`, so you do not need to list these in `.Rbuildignore`. For a
project whose `DESCRIPTION` file rig writes from `rproj.toml`, the copy gets
a `DESCRIPTION` written from the current `rproj.toml`.

The package tarball and the `<package>.Rcheck` directory go into
`.rvenv/check` in a project, and into a new temporary directory for a
package without `rproj.toml`. Use `--output` to choose another directory.

By default rig checks without the PDF manual, which needs LaTeX. Use
`--manual` to check the manual as well. `--no-vignettes` skips building and
checking the vignettes. Arguments after `--` go to `R CMD check`.

The exit status is the exit status of `R CMD check`. Note that
`R CMD check` only fails on errors, not on warnings or notes.

## Your own check script

A project can run its check its own way, with a script it declares as a
`[[bin]]` named `check` in `rproj.toml`, the same way as for
[`rig proj test`](#rig-proj-test). `--builtin` runs rig's own check
instead of the script.
