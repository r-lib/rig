Interoperate with renv, R's package management tool

## Description

Convert between rig's own project format ([`rproj.toml`](proj.qmd)) and
`renv.lock`, the lockfile [renv](https://rstudio.github.io/renv/) uses.

`rig proj renv export` solves the current project's dependencies and writes
the result as `renv.lock`, for interop with renv or a service that consumes
it (e.g. Posit Connect).

`rig proj renv import` reads an existing `renv.lock` and creates (or merges
dependencies into) `rproj.toml`, to bring a project that uses renv into
[`rig proj`](proj.qmd).
