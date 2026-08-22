Manage R packages (experimental)

## Description

Look up R packages, in the package repositories rig configures for your R
installations and in the libraries they are installed into, without
starting R.

[`rig pkg available`](#rig-pkg-available) lists every package the
repositories offer, [`rig pkg info`](#rig-pkg-info) shows the
`DESCRIPTION` of one package, or, with `--versions`, all of its versions,
[`rig pkg deps`](#rig-pkg-deps) lists the packages one package needs,
directly or, with `--recursive`, transitively, and
[`rig pkg tree`](#rig-pkg-tree) shows those transitive dependencies as a
tree instead of a table.

[`rig pkg list`](#rig-pkg-list) is the one subcommand that reads a package
library instead of the repositories: it lists the packages that are
actually installed.

The repositories themselves are managed by [`rig repos`](repos.qmd), the
libraries by [`rig library`](library.qmd).
