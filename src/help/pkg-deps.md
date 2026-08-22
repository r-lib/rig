Dependencies of a package in the repositories

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

R itself and the base packages, e.g. `utils` or `stats`, are listed if
the package depends on them, with their version requirement, but without
a version of their own, as they are part of R.

## Recursive dependencies

`--recursive` (`-r`) shows the whole dependency tree: not only the
packages the package needs directly, but also the packages *those* need,
and so on. Each package appears once, with the `Depth` column giving its
distance from the queried package, and the `Needed by` column naming the
packages that pull it in.

A recursive listing only ever follows hard dependencies, also below a
soft dependency added by `--dev`, so `--dev --recursive` means the
package's own dev dependencies plus everything they need to be
installed.

rig follows the dependencies of the *latest* version of every package in
the tree, so a version requirement that would force an older version,
with different dependencies, is not taken into account. Use
[`rig proj solve`](proj.qmd) for a resolution that is consistent across
versions.
