List packages available in the R package repositories

## Description

List the packages available from the repositories configured for the R
version, ordered by name, using the latest version of each package. For each
package rig shows its version and its number of hard dependencies
(`Depends`, `Imports` and `LinkingTo`, excluding R and the base packages).

## Repositories

rig uses the repositories configured for the R version, i.e. the
[`rig repos list`](repos.qmd#rig-repos-list) output for the default R version, or for
the one selected with `--r-version`. If two repositories have the same version
of a package, the one listed first wins.

`--with-repos` (or `--index`) and `--without-repos` (or `--no-index`) change
the repositories for this command only, the setup of the R version stays the
same. Both take a comma-separated list and can be repeated. `--with-repos`
adds repositories, by name, like [`rig repos enable`](repos.qmd#rig-repos-enable),
by URL, or as `name=URL`, e.g. `--index=rlib=https://r-lib.r-universe.dev`.
A repository given by URL is a CRAN-like repository and comes before the
others. `--without-repos=<names>` leaves out repositories, and
`--without-repos` without names leaves out all configured repositories, so
only the ones in `--with-repos` are used, e.g.
`--no-index --index=https://r-lib.r-universe.dev`. URLs cannot contain commas.

For P3M and for Bioconductor's software repository rig reads their full
package history, from <https://ppm.r-pkg.org> and <https://ppm-bioc.r-pkg.org>,
so older and archived versions are available, too. Any other repository, e.g.
CRAN itself, an r-universe, or one added with [`rig repos add`](repos.qmd#rig-repos-add), is read
from its `PACKAGES` files, so only its current packages are available.
If the Bioconductor repositories are enabled, the list includes the packages
of the Bioconductor release of the R version, e.g. Bioconductor 3.23 for
R 4.6. Without any R version rig lists
CRAN, and the newest Bioconductor release.

Packages CRAN or Bioconductor has archived are omitted by default; pass
`--include-archived` to list them as well.

Use `--json` to print the full listing as JSON, including the complete
dependency lists for every package. See [`rig pkg info`](#rig-pkg-info) for a detailed view of
a single package, and `rig pkg info --versions` to list all versions of a
package.
