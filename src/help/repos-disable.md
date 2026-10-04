Disable R package repositories for an R version

## Description

Disable one or more package repositories for R versions, so R does not use them
to install packages. This works both for the repositories built into rig, e.g.
`P3M` or `CRAN`, and for the ones you added with [`rig repos add`](#rig-repos-add). Repository names are
matched case insensitively. Disabling a custom repository keeps it in rig, use
[`rig repos rm`](#rig-repos-rm) to remove it completely.

By default rig disables the repositories for the default R version. Use
`--r-version` (possibly more than once) to pick other R versions, or
`--all-versions` for all of them.

rig remembers which repositories you enabled and disabled for each R version,
and keeps them when it sets up the repositories again, e.g. with
[`rig repos setup`](#rig-repos-setup).

rig updates the files of the R installation. If you cannot write them, which is
typical in [admin mode](../admin-vs-user-mode.qmd) on Linux and Windows, rig asks for administrator rights,
e.g. your password for `sudo`.

## Examples

```sh
# Stop using P3M for the default R version
rig repos disable p3m

# Stop using a custom repository for R 4.4
rig repos disable acme -r 4.4
```
