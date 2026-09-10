Information about a package in the repositories

## Note

This command currently only uses PPM (Posit Public Package Manager)
and ignores the configured repositories.

## Description

Show information about a package on PPM's CRAN repository, from its
`DESCRIPTION` file.

By default the latest available version is shown; use `--version` to
select a specific one, including versions that CRAN has archived. Use
`--json` to print all `DESCRIPTION` fields.

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
oldest first. For each version It cannot be combined with `--version`.

`--versions --json` prints the full `DESCRIPTION` of every version, each
with an extra `Archived` field for an archived package.
