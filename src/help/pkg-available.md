List packages available in the R package repositories

## Description

List the packages available from the configured package repositories,
ordered by name. For each package rig shows its version and its number of
hard dependencies (`Depends`, `Imports` and `LinkingTo`, excluding R and
the base packages). A header line reports the total number of packages and
the R version and package type they were resolved for.

By default rig uses the default R version and the current platform;
override these with `--r-version`, `--platform` and `--pkg-type` (e.g.
`source` or `binary`).

Use `--json` to print the full listing as JSON, including the complete
dependency lists for every package. See
[`rig pkg info`](#rig-pkg-info) for a detailed view of a single package,
and `rig pkg info --versions` to list all versions of a package.
