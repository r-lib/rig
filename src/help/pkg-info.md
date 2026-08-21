Information about a package in the repositories

## Description

Show information about a package on CRAN, from its `DESCRIPTION` file.

By default the latest available version is shown; use `--version` to
select a specific one, including versions that CRAN has archived. Use
`--json` to print all `DESCRIPTION` fields.

If CRAN has archived the package, i.e. removed it from the current
repository, rig shows the date it was archived, next to the publication
date of the version. `--json` reports it as an extra `Archived` field.

## README of a package

`--readme` prints the README of the package, instead of its metadata,
exactly as the repository stores it, i.e. not rendered and not paged. It
works together with `--version`, to get the README of an older version,
but not with `--versions`.

`--readme --json` prints an object with the `package` and `version` the
README belongs to, the `readme` itself, and the `format` it is written
in. The format is the one the repository reports, e.g. `md` for markdown
or `txt` for plain text.

A package without a README is not an error. `--readme` then prints
nothing, and `--readme --json` prints `null` for both `readme` and
`format`.

## All versions of a package

`--versions` lists all versions of the package ever published on CRAN,
oldest first, instead of the details of a single version. For each version
rig shows its publication date, its R version requirement and its number
of hard dependencies (`Depends`, `Imports` and `LinkingTo`, excluding R
and the base packages); the latest version is marked. It cannot be
combined with `--version`.

For a package CRAN has archived, i.e. removed from the current
repository, the header also shows the date it was archived. This applies
to the package as a whole, so all of its versions are archived.

`--versions --json` prints the full `DESCRIPTION` of every version, each
with an extra `Archived` field for an archived package.
