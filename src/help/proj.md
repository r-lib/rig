Manage R projects (experimental)

## Description

Manage R projects. `rig proj` is currently experimental, and might change
in future versions. Feedback is appreciated.

A project is a directory with an `rproj.toml` manifest file, that declares
the R packages the project depends on.

## Workflow

- `rig proj init` creates a new project. `rig proj import` creates a new
  project from an existing `DESCRIPTION` file. Alternatively, `rig proj renv
  import` creates a new project from an existing `renv.lock` file.

- `rig proj lock` resolves the dependencies of the project.

- `rig proj sync` installs the dependencies of the project.

- `rig run` starts R, configured for the project. It calls `rig proj lock` and
  `rig proj sync` as needed.

- `rig proj add` and `rig proj remove` adds and removes dependencies to/from
  the project.

## The 'tools' package library

Projects treat the package library named `tools` specially. This library
is automatically added to the library path for every project.

Use this for `devtools`, `usethis`, `roxygen2`, and other R packages that
you use for R development. You can use `rig pkg install -l tool <pkgs>` to
install packages into this library.
