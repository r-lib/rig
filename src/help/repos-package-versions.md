List all versions of a package in the repositories

## Description

List all versions of a package known to the CRAN metadata database,
oldest first. For each version rig shows its publication date, its R
version requirement and its number of hard dependencies (`Depends`,
`Imports` and `LinkingTo`, excluding R and the base packages); the
latest version is marked.

Use `--json` to print the full crandb record for every version,
including the complete metadata and dependency lists. See
[`rig repos package-info`](#rig-repos-package-info) for a detailed view
of a single version.
