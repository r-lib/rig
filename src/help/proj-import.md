Create rproj.toml from a DESCRIPTION file

## Description

Read a `DESCRIPTION` file and create `rproj.toml`, rig's project and package
manifest, from it. Fails if `rproj.toml` already exists, use `--dependencies`
to merge dependencies into an existing file instead.

| `DESCRIPTION` field.  | `rproj.toml` field.            |
|---------------------|------------------------------|
| `Package:`            | `[project].name`               |
| `Version:`            | `[project].version`            |
| `Title:`              | `[project].title`              |
| `Description:`        | `[project].description`        |
| `License:`            | `[project].license`            |
| `Type:`               | `[project].type`               |
| `URL:`                | `[project.urls].homepage`      |
| `URL:`                | `[project.urls].source`        |
| `BugReports:`         | `[project.urls].bugreports`    |
| `Authors@R`           | `[project].authors`            |
| `Maintainer:`         | `[project].authors`            |
| `Depends`             | `[dependencies]`               |
| `Imports`             | `[dependencies]`               |
| `LinkingTo`           | `[linking-dependencies]`       |
| `Suggests`            | `[dependency-groups.test]`     |
| `Enhances`            | `[dependency-groups.enhances]` |
| `Config/Needs/<name>` | `[dependency-groups.<name>]`   |

`Authors@R` is parsed into `[project].authors`, one entry per `person()` call, in
order; this is a best-effort parser for common `person()` usage (name, `email`,
`role`, and an `ORCID`/`ROR` `comment`), not a full R parser, so unusual calls are
skipped with a warning. If `Authors@R` is absent, the simpler `Maintainer:
Name <email>` field is used instead, as a single author with role `cre`.

`Config/Needs/<name>` depepdencies are parsed specially, an entry that is not
a plain package name (with an optional version requirement) is kept
verbatim as `ref = "..."`. E.g. `tidyverse/tidytemplate` becomes `tidytemplate =
{ ref = "tidyverse/tidytemplate" }`. `rig proj export` writes these back
unchanged. Note that only the `test` and `enhances` groups are installed by
rig, so a `Config/Needs/*` group is carried in the manifest, but not solved
or installed by `rig proj lock` and `rig proj sync`. This behavior will be
improved in the future.

## Files

A full import sets up a whole project, not just its manifest, so it creates
the same files as [`rig proj init`](#rig-proj-init) .

`--r-version` sets the R version the project is set up for. It does not have
to be installed, and it does not change what is written: the manifest's R
requirement always comes from the `DESCRIPTION` file, and the `.rvenvlib/rvenv`
shim package works with every R. The default is the current default R
version, or the current R release if there is no default.
