List configured R package repositories

## Description

List the package repositories configured for an R version.

By default rig shows the repositories of the default R version; use
`--r-version` to pick another. Add `--all` to include repositories that are not
enabled by default, and `--raw` to show repository URLs without resolving the
`%` variables in them.

The list includes the custom repositories (see [`rig repos add`](#rig-repos-add)) that are enabled
for the R version. Use [`rig repos enable`](#rig-repos-enable) and [`rig repos disable`](#rig-repos-disable) to change it.
