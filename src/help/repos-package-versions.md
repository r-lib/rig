List all versions of a package in the repositories

## Description

List all versions of a package ever published on CRAN, oldest first. For
each version rig shows its publication date, its R version requirement
and its number of hard dependencies (`Depends`, `Imports` and
`LinkingTo`, excluding R and the base packages); the latest version is
marked.

For a package CRAN has archived, i.e. removed from the current
repository, the header also shows the date it was archived. This applies
to the package as a whole, so all of its versions are archived.

Use `--json` to print the full `DESCRIPTION` of every version, each with
an extra `Archived` field for an archived package. See
[`rig repos package-info`](#rig-repos-package-info) for a detailed view
of a single version.
