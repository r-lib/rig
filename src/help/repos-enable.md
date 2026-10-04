Enable R package repositories for an R version

## Description

Enable one or more package repositories for R versions. This works both for the
repositories built into rig, e.g. `Bioconductor` or `RHUB`, and for the ones you
added with [`rig repos add`](#rig-repos-add). See [`rig repos available`](#rig-repos-available) for the list. Repository names
are matched case insensitively.

By default rig enables the repositories for the default R version. Use
`--r-version` (possibly more than once) to pick other R versions, or
`--all-versions` for all of them. Each R version has its own set of repositories,
so enabling a repository for one R version does not change the others.

rig remembers which repositories you enabled and disabled for each R version,
and keeps them when it sets up the repositories again, e.g. with
[`rig repos setup`](#rig-repos-setup).

Some built-in repositories only have URLs for certain platforms, architectures
or R versions, and rig fails if a repository cannot be used with an R version.

rig updates the files of the R installation. If you cannot write them, which is
typical in [admin mode](../admin-vs-user-mode.qmd) on Linux and Windows, rig asks for administrator rights,
e.g. your password for `sudo`.

## Examples

```sh
# Enable Bioconductor for the default R version
rig repos enable bioconductor

# Enable a custom repository for two R versions
rig repos enable acme -r 4.5 -r 4.4

# Enable a custom repository for every R version
rig repos enable acme --all-versions
```
