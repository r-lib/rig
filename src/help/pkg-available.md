List packages available in the R package repositories

## Note

This command currently only uses P3M (Posit Public Package Manager)
and ignores the configured repositories.

## Description

List the packages available from P3M's full package history ordered by
name, using the latest version of each package. For each package rig shows
its version and its number of hard dependencies (`Depends`, `Imports` and
`LinkingTo`, excluding R and the base packages).

Packages CRAN has archived are omitted by default; pass
`--include-archived` to list them as well.

Use `--json` to print the full listing as JSON, including the complete
dependency lists for every package. See
[`rig pkg info`](#rig-pkg-info) for a detailed view of a single package,
and `rig pkg info --versions` to list all versions of a package.
