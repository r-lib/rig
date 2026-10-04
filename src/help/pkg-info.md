Information about a package in the repositories

## Description

Show information about a package in the repositories configured for the R
version, from its `DESCRIPTION` file.

By default the latest available version is shown; use `--version` to select a
specific one, including versions that CRAN has archived. Use `--json` to
print all `DESCRIPTION` fields.

## Repositories

rig uses the repositories configured for the R version, i.e. the
[`rig repos list`](repos.qmd#rig-repos-list) output for the default R version, or for
the one selected with `--r-version`. If two repositories have the same version
of a package, the one listed first wins.

For P3M and for Bioconductor's software repository rig reads their full
package history, from <https://ppm.r-pkg.org> and <https://ppm-bioc.r-pkg.org>,
so older and archived versions are available, too. Any other repository, e.g.
CRAN itself, an r-universe, or one added with [`rig repos add`](repos.qmd#rig-repos-add), is read
from its `PACKAGES` files, so only its current packages are available.

rig shows the full `DESCRIPTION` file only for a package version that comes
from P3M. For a package version from any other repository, including
Bioconductor, rig only knows the fields of the repository's index: its
version, its dependencies and where to download it.

## README of a package

`--readme` prints the README of the package, instead of its metadata, exactly
as the repository stores it, i.e. not rendered and not paged. It works
together with `--version`, to get the README of an older version, but not
with `--versions`.

`--readme --json` prints an object with the `package` and `version` the README
belongs to, the `readme` itself, and the `format` it is written in. The format
is the one the repository reports, e.g. `md` for markdown or `txt` for plain
text.

A package without a README is not an error. `--readme` then prints nothing,
and `--readme --json` prints `null` for both `readme` and `format`.

## All versions of a package

`--versions` lists all versions of the package ever published on CRAN, oldest
first, and the current versions in the other repositories. It cannot be
combined with `--version`.

`--versions --json` prints the full `DESCRIPTION` of every version, each with
an extra `Archived` field for an archived package.
