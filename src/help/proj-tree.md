Dependency tree of a project

## Description

Show the dependency tree of a project. Example:

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

rig reads the project manifest, `rproj.toml`, in the current directory.

## Reading the tree

A package that several others need is expanded only once, under its first
occurrence; later occurrences are marked `(*)`, meaning "its dependencies are
above". `--dev` adds the project's development dependencies, in their own
`[Suggests]` and `[Enhances]` sections; `--no-base` leaves out R and the base
packages. Among the hard dependencies, `Imports` is not marked, `[D]` is a
`Depends`, `[L]` a `LinkingTo`, `[DL]` both.

`--why <package>` (alias `--explain`) inverts the tree, so that the named
package is the root and the tree grows towards the packages that need it,
down to the project itself. Each line then says how *that* package needs
the one above it, hence `needs`.

rig follows the dependencies of the *latest* version of every package in
the tree, so a version requirement that would force an older version, with
different dependencies, is not taken into account.
