Dependency tree of a package in the repositories

## Description

Show everything a package needs, directly or indirectly, as a tree, so you can
see *how* each package is pulled in and not only *that* it is.

This is the same set of packages that
[`rig pkg deps --recursive`](#rig-pkg-deps) lists in a flat table, laid out by
the shape of the dependency graph instead. Use `--json` for machine readable
output, as one nested object.

```
dplyr 1.1.4 — 13 direct, 30 total
├── R (>= 3.5.0) [D]
├── cli 3.6.4 (>= 3.4.0)
│   ├── R (>= 3.4) [D]
│   └── utils
├── lifecycle 1.0.4 (>= 1.0.3)
│   ├── cli 3.6.4 (>= 3.4.0) (*)
│   └── rlang 1.1.6 (>= 1.1.0)
│       └── R (>= 3.5.0) [D]
└── vctrs 0.6.5 (>= 0.6.4)
    └── cpp11 0.5.2 [L]
[Suggests]
├── testthat 3.2.3 (>= 3.1.5)
└── tidyr 1.3.1 (>= 1.3.0)
    └── cpp11 0.5.2 (>= 0.4.0) [L] (*)
```

The first line names the package version the tree is for, how many
dependencies it has directly, and how many distinct packages there are in the
whole tree. Each line below it names a package, the version currently on CRAN,
and the version requirement it is needed with, if it has one.

By default the tree of the latest version of the package is shown; use
`--version` to ask about a specific one, including versions that CRAN has
archived. [`rig proj tree`](proj.qmd#rig-proj-tree) shows the same tree for the
dependencies a project declares, instead of a package's.

## Repeated packages

A package that several others need is expanded only once, under the first place
it appears. Later occurrences are shown as a single line marked `(*)`, meaning
"its dependencies are above". Without this a real dependency tree would be
thousands of lines long, most of it repetition.

The same rule makes dependency cycles — which do occur on CRAN, usually through
`Suggests` — end on their own, and it is why the number of *lines* is larger
than the `total` count on the first line: R and the base packages have no
dependencies to elide, so they are printed wherever they are needed.

## Dependency types

By default rig follows the hard dependencies only: `Depends`, `Imports` and
`LinkingTo`, i.e. the packages that need to be installed to use the package.
`--dev` adds the soft dependencies, `Suggests` and `Enhances`, which are
typically only needed to run the tests, build the vignettes or use some
optional feature. As in `rig pkg deps`, `--dev` applies to the queried package
only: below a soft dependency rig still follows hard dependencies only, so
`--dev` means the package's own dev dependencies plus everything they need to
be installed.

The soft dependencies are set apart in their own `[Suggests]` and `[Enhances]`
sections, each numbering its own lines, the way `cargo tree` sets
`[dev-dependencies]` apart. Because `--dev` only applies to the queried
package, these sections only ever appear at the top of the tree.

Among the hard dependencies, `Imports` is the common case and is not marked.
The other two are, at the end of the line:

* `[D]` — a `Depends`, so the package is *attached* when this one is loaded,
  not merely loaded itself.
* `[L]` — a `LinkingTo`, so this package is compiled against it; it is needed
  to build the package, not to run it.
* `[DL]` — both.

Within a section the dependencies are listed with R first, then grouped by
dependency type, in the order R lists the fields in, and by name within a
type.

R itself and the base packages, e.g. `utils` or `stats`, are shown if a package
depends on them, with their version requirement, but without a version of
their own, as they are part of R. `--no-base` leaves out R and the base
packages altogether, which is much less to read if you only care about what
would have to be installed. A package that is not in the repositories at all
is shown with `?` for its version.

rig follows the dependencies of the *latest* version of every package in the
tree, so a version requirement that would force an older version, with
different dependencies, is not taken into account. Use
[`rig proj solve`](proj.qmd) for a resolution that is consistent across
versions.
