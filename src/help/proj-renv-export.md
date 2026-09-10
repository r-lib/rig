Write an renv.lock file from rproj.toml

## Description

Resolve the dependencies of the project in the current directory (its
[`rproj.toml`](proj.qmd) manifest) with rig's built-in solver, for one
`(R version, platform)` target, and write the result as `renv.lock`.

Unlike [`rig proj lock`](proj.qmd#rig-proj-lock), this only ever solves a
single target, since `renv.lock` has no multi-target concept.

Without `--r-version`, rig solves for the default R version, provided the
manifest's own `R` requirement allows it; otherwise the newest installed R
version that does, and failing that the current R release. Without
`--platform`, rig solves for this machine.

## Examples

```sh
rig proj renv export
rig proj renv export --r-version 4.5 --platform ubuntu-24.04
```
