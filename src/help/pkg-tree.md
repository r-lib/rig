Dependency tree of a package in the repositories

## Description

Show everything a package needs, directly or indirectly, as a tree: the same
closure [`rig pkg deps --recursive`](#rig-pkg-deps) lists in a flat table, laid
out by the shape of the dependency graph.

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
└── tidyr 1.3.1 (>= 1.3.0)
    └── cpp11 0.5.2 (>= 0.4.0) [L] (*)
```

The first line names the package version, how many dependencies it has
directly and how many distinct packages there are in the whole tree. Each line
below names a package, the version currently on CRAN, and the version
requirement it is needed with, if it has one.

`--version` asks about a specific version, including versions CRAN has
archived. `--json` gives machine readable output, as one nested object.
[`rig proj tree`](proj.qmd#rig-proj-tree) shows the same tree for the
dependencies a project declares.

## Reading the tree

A package that several others need is expanded only once, under its first
occurrence; later occurrences are a single line marked `(*)`, meaning "its
dependencies are above". This is also what makes dependency cycles end on
their own.

A mark at the end of a line says how the package is needed; `Imports` is the
common case and is not marked.

* `[D]` — a `Depends`, so the package is *attached*, not merely loaded.
* `[L]` — a `LinkingTo`, so this package is compiled against it.
* `[DL]` — both.

Dependencies are listed with R first, then grouped by dependency type, in the
order R lists the fields in, and by name within a type. R and the base
packages, e.g. `utils`, are shown with their version requirement but without a
version of their own, as they are part of R; `--no-base` leaves them out
altogether. A package that is not in the repositories is shown with `?` for
its version.

By default rig follows the hard dependencies only. `--dev` adds `Suggests` and
`Enhances`, in their own `[Suggests]` and `[Enhances]` sections. As in
`rig pkg deps`, `--dev` applies to the queried package only, so these sections
only ever appear at the top of the tree.

rig follows the dependencies of the *latest* version of every package in the
tree, so a version requirement that would force an older version, with
different dependencies, is not taken into account. Use
[`rig proj lock`](proj.qmd) for a resolution that is consistent across
versions.

## Inverting the tree

`--why <package>` (alias `--explain`) inverts the tree: the named package is
the root and the tree grows towards the packages that need it, down to the
queried package, which becomes a leaf.

```
glue 1.8.1 — 4 direct dependents, 5 total
├── dplyr 1.2.1 (needs >= 1.3.2)
├── pillar 1.11.1
│   └── dplyr 1.2.1 (needs >= 1.9.0)
└── vctrs 0.7.3
    ├── dplyr 1.2.1 (needs >= 0.7.1)
    └── pillar 1.11.1 (needs >= 0.5.0) (*)
```

Each line says how *that* package needs the one **above** it, hence `needs`;
the `[D]`, `[L]`, `[S]` and `[E]` marks describe the same edge. `[S]` and `[E]`
take the place of the `[Suggests]` and `[Enhances]` sections, which in an
inverted tree would be one line deep inside it.

`--why` searches the tree only, not the repositories, so `--version`, `--dev`
and `--no-base` apply as above, and a package that is not in the tree is an
error.
