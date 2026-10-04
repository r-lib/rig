Dependency tree of a package in the repositories

## Description

Show everything a package needs, directly or indirectly, as a tree:

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

Each line of the tree below names a package, its latest version in the
repositories, and the version requirement it is needed with, if it has one.

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
Bioconductor packages are included if the Bioconductor repositories are
enabled for the R version.

## Reading the tree

A package that several others need is expanded only once, under its first
occurrence. Later occurrences are a single line marked `(*)`, meaning "its
dependencies are above".

Key for markers:

* `[D]`: a `Depends`, so the package is *attached*, not merely loaded.
* `[L]`: a `LinkingTo`, so this package is compiled against it.
* `[DL]`: both.

(`Imports` is the most common and it is not marked.)

By default rig follows the hard dependencies only. `--dev` adds `Suggests` and
`Enhances`, in their own `[Suggests]` and `[Enhances]` sections.

rig follows the dependencies of the *latest* version of every package in
the tree, so a version requirement that would force an older version, with
different dependencies, is not taken into account.

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

Each line says how *that* package needs the one above it. The `[D]`, `[L]`, `[S]`
and `[E]` marks describe the same edge.

`--why` searches the tree only, not the repositories, so `--version`, `--dev` and
`--no-base` apply, and a package that is not in the tree is an error.
