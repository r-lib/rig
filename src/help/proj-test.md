Run the tests of a package or project

## Description

Run the test suite of the package or project in the current directory, or
in the nearest directory above it that has an `rproj.toml` or a
`DESCRIPTION` file. The tests run in that directory, so `rig proj test`
works from any subdirectory.

```sh
rig proj test                # run all tests
rig proj test utils          # only tests/testthat/test-utils.R
rig proj test -r 4.4         # run the tests with R 4.4
```

## Which R

In a [project](../proj-guide.qmd) the tests run in the project environment,
the same way as [`rig run`](run.qmd): rig locks and syncs the project first,
if needed. A sync installs the `dev` dependency group by default, which has
the test packages of a package project (its `Suggests`).

A package with only a `DESCRIPTION` file and no `rproj.toml` uses the default
R version and its package library.

Use `--r-version` to run the tests with another R version, with the R
version's own package library.

## Test frameworks

rig looks for:

- `tests/testthat`: for a package it calls `testthat::test_local()`, which
  loads the package from source, like `devtools::test()`. For a project
  that is not a package (`type = "project"`) it calls
  `testthat::test_dir("tests/testthat")`. The optional `FILTER` argument
  selects test files, see the `filter` argument of `testthat::test_dir()`.
- `inst/tinytest`: for a package it calls `tinytest::build_install_test()`,
  otherwise `tinytest::run_test_dir()`.
- `.R` files in `tests`: it runs them the same way `R CMD check` does, with
  `tools:::.runPackageTests()`. Each file runs in its own R process, and
  if there is a `.Rout.save` file next to it, rig compares the output with
  it. For a package, rig installs the package into a temporary library
  first, because the test files load the installed package. The tests run
  in a temporary copy of `tests`. If a test fails, rig keeps this copy and
  prints where it is, so you can look at the `.Rout.fail` files.

The `FILTER` argument only works for testthat tests.

The exit status is not zero if a test fails, so `rig proj test` works in CI.

## Your own test script

A project can run its tests its own way, with a script it declares as a
`[[bin]]` named `test` in `rproj.toml`:

```toml
[[bin]]
name = "test"
path = "tools/run-tests.R"
```

`rig proj test` then runs this script instead, in the project environment,
from the project directory, and passes all of its arguments on to it. Use
`--` before arguments that start with a dash: `rig proj test -- --fast`.
`--builtin` runs rig's own test task instead of the script.
