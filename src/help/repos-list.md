List configured R package repositories

## Description

List the package repositories configured for an R version.

By default rig shows the repositories of the default R version; use
`--r-version` to pick another. Add `--all` to include repositories that are not
enabled, and `--raw` to show repository URLs without resolving the
`%` variables in them.

The `name` column is the name that [`rig repos enable`](#rig-repos-enable), [`rig repos disable`](#rig-repos-disable) and
[`rig repos available`](#rig-repos-available) use. Some of these, e.g. `Bioconductor`, have several entries
in R's `repositories` file; then the `repo` column shows the name of the entry. The
`E` column marks the enabled repositories. The `M` column marks the repositories
that have extended metadata: the full history of their packages, including
archived versions, and binary package indices. rig uses these to resolve
package versions and to find binary packages. In `--json` output `name` is the
entry name, the `group` field is the name of the rig repository, or `null`, and the
`metadata` field holds the base URL of the extended metadata, or `null`.

The list includes the custom repositories (see [`rig repos add`](#rig-repos-add)) that are enabled
for the R version. Use [`rig repos enable`](#rig-repos-enable) and [`rig repos disable`](#rig-repos-disable) to change it.
