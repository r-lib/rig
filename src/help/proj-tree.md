Dependency tree of a project

## Description

Show everything an R project needs, directly or indirectly, as a tree, so you
can see *how* each package is pulled in and not only *that* it is.

This is the same set of packages that
[`rig proj deps --recursive`](#rig-proj-deps) lists in a flat table, laid out by
the shape of the dependency graph instead. Use `--json` for machine readable
output, as one nested object.

```
myproject 0.1.0 — 3 direct, 24 total
├── R (>= 4.1) [D]
├── cli 3.6.4
│   ├── R (>= 3.4) [D]
│   └── utils
└── dplyr 1.1.4 (>= 1.1.0)
    ├── cli 3.6.4 (>= 3.4.0) (*)
    └── vctrs 0.6.5 (>= 0.6.4)
        └── cpp11 0.5.2 [L]
[Suggests]
└── testthat 3.2.3 (>= 3.1.5)
```

The first line names the project and its version, how many dependencies it
declares directly, and how many distinct packages there are in the whole tree.
Each line below it names a package, the version currently in the repositories,
and the version requirement it is needed with, if it has one.

By default rig reads the project manifest (e.g. `DESCRIPTION`) in the current
directory; use `--input` to point to a different file. Unlike the plain
[`rig proj deps`](#rig-proj-deps) listing, the tree needs the package metadata
of the repositories, which rig downloads if it does not have it yet. It does
not need R.

## Reading the tree

A package that several others need is expanded only once, under the first
place it appears; later occurrences are a single line marked `(*)`, meaning
"its dependencies are above". That is also what makes dependency cycles end on
their own.

`--dev` adds the project's development dependencies, `Suggests` and `Enhances`,
in their own `[Suggests]` and `[Enhances]` sections. As in `rig proj deps`,
`--dev` applies to the project only: below a development dependency rig still
follows hard dependencies only. `--no-base` leaves out R and the base packages
altogether, which is much less to read if you only care about what would have
to be installed. A package that is not in the repositories at all is shown with
`?` for its version.

Among the hard dependencies, `Imports` is the common case and is not marked;
`[D]` is a `Depends`, `[L]` a `LinkingTo`, `[DL]` both.
[`rig pkg tree`](pkg.qmd#rig-pkg-tree), which shows the same tree for a package
in the repositories, describes all of this in full.

rig follows the dependencies of the *latest* version of every package in the
tree, so a version requirement that would force an older version, with
different dependencies, is not taken into account. Use
[`rig proj solve`](#rig-proj-solve) for a resolution that is consistent across
versions.
