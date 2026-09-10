Dependencies of a package in the repositories

## Note

This command currently only uses PPM (Posit Public Package Manager)
and ignores the configured repositories.

## Description

Show what a package needs, in a table: every package it depends on, the
version of that package currently on CRAN, the dependency type
(`Depends`, `Imports`, `LinkingTo`) and the version requirement, if it
has one.

By default the dependencies of the latest version of the package are
shown; use `--version` to ask about a specific one, including versions
that CRAN has archived. Use `--json` for machine readable output.

## Dependency types

By default rig lists the hard dependencies only: `Depends`, `Imports`
and `LinkingTo`, i.e. the packages that need to be installed to use the
package. `--dev` adds the soft dependencies, `Suggests` and `Enhances`,
which are typically only needed to run the tests, build the vignettes or
use some optional feature.

R itself and the base packages are listed if the package depends on them,
with their version requirement, but without a version of their own, as
they are part of R.

`--recursive` (`-r`) shows the whole dependency closure. Each package
appears once, with the `Depth` column giving its distance from the queried
package, and the `Needed by` column naming the packages that pull it in.

See [`rig pkg tree`](#rig-pkg-tree) for a visual dependency tree.

rig follows the dependencies of the *latest* version of every package in
the tree, so a version requirement that would force an older version,
with different dependencies, is not taken into account. Use
[`rig proj lock`](proj.qmd) for a resolution that is consistent across
versions.
