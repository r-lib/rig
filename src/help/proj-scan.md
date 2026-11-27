Find the packages that the project's code uses

## Description

Scan the R code of a project for the R packages it uses, and show them.
With `--check`, compare them with the dependencies in `rproj.toml`. With `--fix`,
add the missing ones to `rproj.toml`.

```
rig proj scan
rig proj scan --files
rig proj scan --check
rig proj scan --fix
```

The scan is static: rig reads the code, it does not run it. It finds calls
like `library(pkg)`, `require(pkg)`, `requireNamespace("pkg")` and `pkg::fn()`. It
cannot know about a package that is loaded dynamically, e.g. `library(name,
character.only = TRUE)` where `name` is a variable; it reports these instead
of guessing. So treat the result as a good first draft, not as the final
word.

`rig proj scan` scans the current project, the nearest directory at or above
the current one that has an `rproj.toml` file. Give a directory to scan
another one. It works in a directory without `rproj.toml`, too, e.g. to see
what a project would need before running `rig proj init --scan`.

`--files` shows every use of every package, with the file and line. Use
`--json` for machine readable output.

## What is scanned

- R scripts: `.R` files, and files without an extension that start with an
  `Rscript` `#!` line.
- R Markdown (`.Rmd`), Quarto (`.qmd`) and Sweave (`.Rnw`) documents: the R code
  chunks and inline R code. Chunks with `eval = FALSE` (or `#| eval: false`)
  are skipped. Packages named in the YAML header, e.g. `output:
  bookdown::html_document2`, are included. An R Markdown document needs
  rmarkdown, a Shiny document needs shiny, and a Python chunk in a knitr
  document needs reticulate.
- A few special files: `_bookdown.yml` (bookdown), `_pkgdown.yml` (pkgdown),
  `.lintr` (lintr), an `.Rproj` file that uses devtools (devtools and
  roxygen2), and an `rsconnect` directory (rsconnect).

These calls load a package, besides `pkg::fn` and `pkg:::fn`: `library()`,
`require()`, `requireNamespace()`, `loadNamespace()`, `use()`, `pacman::p_load()`,
`box::use()`, `import::from()`, `import::here()`, `import::into()`,
`modules::import()`, `rlang::check_installed()`, `rlang::is_installed()`,
`xfun::pkg_attach()` and `targets::tar_option_set(packages = ...)`.

Base R packages, e.g. utils and stats, are never reported. The project's
own package is not reported either.

## Skipped files

rig skips:

- files and directories that `.gitignore` or `.renvignore` files ignore,
- hidden files and directories, e.g. `.git`,
- `renv`, `packrat`, `revdep`, `vendor`, `node_modules` and `rsconnect` directories,
- subdirectories with their own `rproj.toml` file, these are separate
  projects,
- files that match an `exclude` pattern, see below.

To skip a single use of a package, add a `# rig: ignore` comment to its line:

```r
library(mytestpkg) # rig: ignore
```

## Dev dependencies

A package that is only used in `tests/`, `test/`, `vignettes/`, `inst/doc/` or
`man/roxygen/` is a dev dependency. `--fix` adds it to the
`[dependency-groups.dev]` table, all other packages go to `[dependencies]`.

## Checking the dependencies

`--check` reports:

- missing packages: used, but not in `rproj.toml`,
- misplaced packages: used outside of the dev directories, but only a dev
  dependency in `rproj.toml`,
- unused packages: in `[dependencies]`, but not used anywhere.

It exits with status 1 if there are missing or misplaced packages, so it
works as a CI check. Unused packages are only reported, add `--strict` to
fail for them, too. Other errors give exit status 2.

Like renv, rig counts a package as used even if it is only loaded
conditionally, e.g. in `if (requireNamespace("pkg", quietly = TRUE))`. Use
`ignore-packages` or a `# rig: ignore` comment to leave out such packages.

## Updating the dependencies

`--fix` adds the missing packages to `rproj.toml`, as `"*"`, i.e. any version. A
misplaced package is added to `[dependencies]`. The rest of the file,
including comments, is kept as it is. Then rig updates `rproj.lock` and
installs the new packages, like [`rig proj add`](#rig-proj-add) does. `--no-lock` only updates
`rproj.toml`, `--no-sync` does not install the packages. If `rproj.lock` cannot
be updated, e.g. because no repository has one of the packages, rig
restores `rproj.toml`.

`--fix` never removes a dependency, because the scan cannot see every use of
a package. Add `--prune` to also remove the unused packages from
`[dependencies]`. `--dry-run` shows what `--fix` would change, without changing
anything.

## Settings

The `[tool.rig.scan]` table of `rproj.toml` configures the scan:

```toml
[tool.rig.scan]
# Glob patterns of files and directories to skip, relative to the
# project directory. `*` does not match `/`, use `**` for that.
exclude = ["data-raw/**", "scratch/*.R"]
# Packages to leave out of the scan.
ignore-packages = ["mytestpkg"]
```

## Compared to renv

The scan follows `renv::dependencies()`, so it finds mostly the same
packages. The main differences:

- rig parses the code with tree-sitter, so a syntax error in a file does
  not hide the other packages of the file.
- rig reads both `.gitignore` and `.renvignore` files, renv reads `.renvignore`
  instead of `.gitignore` if there is one.
- rig does not read `DESCRIPTION` files. Use [`rig proj import`](#rig-proj-import) to import the
  dependencies of a package.
- Some of renv's rarer rules are not implemented yet, e.g. packages used in
  `glue()` strings, or implied by testthat functions.
