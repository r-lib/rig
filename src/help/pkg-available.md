List packages available in the R package repositories

## Note

This command currently only uses PPM (Posit Public Package Manager) and
ignores the configured repositories.

## Description

List the packages available from PPM's full package history ordered by
name, using the latest version of each package. For each package rig shows
its version and its number of hard dependencies (`Depends`, `Imports` and
`LinkingTo`, excluding R and the base packages).

The list includes the packages of the Bioconductor release of the default R
version, e.g. Bioconductor 3.23 for R 4.6, or of the newest Bioconductor
release if there is no default R version. Use `--no-bioc` to list CRAN
packages only.

Packages CRAN or Bioconductor has archived are omitted by default; pass
`--include-archived` to list them as well.

Use `--json` to print the full listing as JSON, including the complete
dependency lists for every package. See [`rig pkg info`](#rig-pkg-info) for a detailed view of
a single package, and `rig pkg info --versions` to list all versions of a
package.
