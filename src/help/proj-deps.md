Show project dependencies

## Description

Show the dependencies of an R project, in a table: every package the
project depends on, the dependency type (`Depends`, `Imports`,
`LinkingTo`) and the version requirement, if it has one.

By default rig reads the project manifest (e.g. `DESCRIPTION`) in the
current directory; use `--input` to point to a different file. Add `--dev`
to include development dependencies. Use `--json` for machine readable
output.

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

[`rig proj tree`](#rig-proj-tree) shows the same closure as a tree, laid
out by the shape of the dependency graph, so you can see *how* each
package is pulled in and not only *that* it is.

A recursive listing only ever follows hard dependencies, also below a
development dependency added by `--dev`, so `--dev --recursive` means the
project's own dev dependencies plus everything they need to be installed.

rig follows the dependencies of the *latest* version of every package in
the closure, so a version requirement that would force an older version,
with different dependencies, is not taken into account. Use
[`rig proj lock`](#rig-proj-lock) for a resolution that is consistent
across versions.
