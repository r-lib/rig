Dependencies of a package in the repositories

## Description

Show what a package needs, in a table: every package it depends on, the
latest version of that package in the repositories, the dependency type (`Depends`,
`Imports`, `LinkingTo`) and the version requirement, if it has one.

By default the dependencies of the latest version of the package are shown;
use `--version` to ask about a specific one, including versions that CRAN has
archived. Use `--json` for machine readable output.

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
Bioconductor packages are included if the Bioconductor repositories are
enabled for the R version. `--no-bioc` leaves them out.

## Dependency types

By default rig lists the hard dependencies only: `Depends`, `Imports` and
`LinkingTo`, i.e. the packages that need to be installed to use the package.
`--dev` adds the soft dependencies, `Suggests` and `Enhances`, which are
typically only needed to run the tests, build the vignettes or use some
optional feature.

R itself and the base packages are listed if the package depends on them,
with their version requirement, but without a version of their own, as they
are part of R.

`--recursive` (`-r`) shows the whole dependency closure. Each package appears
once, with the `Depth` column giving its distance from the queried package,
and the `Needed by` column naming the packages that pull it in.

See [`rig pkg tree`](#rig-pkg-tree) for a visual dependency tree.

rig follows the dependencies of the *latest* version of every package in
the tree, so a version requirement that would force an older version, with
different dependencies, is not taken into account. Use [`rig proj lock`](proj.qmd) for a
resolution that is consistent across versions.
