Show project dependencies

## Description

Show the dependencies of an R project, in a table.

rig reads the project manifest, `rproj.toml`, in the current directory.

Add `--dev` to include development dependencies.

Use `--json` for machine
readable output.

The plain listing only reads the manifest, so it needs neither R nor the
package repositories.

## Recursive dependencies

`--recursive` (`-r`) shows the whole dependency closure: not only the
packages the project needs directly, but also the packages *those* need,
and so on. Each package appears once, with the version currently on CRAN,
the `Depth` column giving its distance from the project, and the
`Needed by` column naming the packages that pull it in. This needs the
package metadata of the repositories, which rig downloads if it does not
have it yet.

See [`rig proj tree`](#rig-proj-tree) to show the dependency tree of the
project.

rig follows the dependencies of the *latest* version of every package in
the closure, so a version requirement that would force an older version,
with different dependencies, is not taken into account.
