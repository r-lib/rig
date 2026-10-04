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

For P3M and for Bioconductor's software repository rig reads their full
package history, from <https://ppm.r-pkg.org> and <https://ppm-bioc.r-pkg.org>,
so older and archived versions are available, too. Any other repository, e.g.
CRAN itself, an r-universe, or one added with [`rig repos add`](repos.qmd#rig-repos-add), is read
from its `PACKAGES` files, so only its current packages are available.
If the Bioconductor repositories are enabled, the list includes the packages
of the Bioconductor release of the R version, e.g. Bioconductor 3.23 for
R 4.6. Use `--no-bioc` to leave them out. Without any R version rig lists
CRAN, and the newest Bioconductor release.

Packages CRAN or Bioconductor has archived are omitted by default; pass
`--include-archived` to list them as well.

Use `--json` to print the full listing as JSON, including the complete
dependency lists for every package. See [`rig pkg info`](#rig-pkg-info) for a detailed view of
a single package, and `rig pkg info --versions` to list all versions of a
package.
