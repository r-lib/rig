Packages installed in a library

## Description

List the packages installed in an R package library, without starting R.

```
312 packages (R 4.4.1, main: /Users/gaborcsardi/Library/R/arm64/4.4/library)

Package     Version      Built   Platform                 Source
-----------------------------------------------------------------------------
cli         3.6.3        4.4.0   aarch64-apple-darwin20   CRAN
glue        1.8.0        4.4.1   aarch64-apple-darwin20   CRAN
asciicast   2.3.1.9000   4.4.1   aarch64-apple-darwin20   github::r-lib/asciicast
mypkg       0.0.1        4.4.1   -                        -
```

The first line names the number of packages and the library they were found
in. Each line below it names a package, its version, the R version it was
built for, the platform it was built for, and where it was installed from.

`Platform` is empty for a package installed from source. `Source` is the
repository the package came from, e.g. `CRAN`, and for a package installed
from somewhere else it names that place instead, in the package reference
syntax pak uses: `github::<user>/<repo>` for a GitHub install, `git::<url>`
for a git one, and so on. It is empty for a package installed from a local
directory, as `R CMD INSTALL` and `devtools::install()` do, because such a
package records nothing about where its source was.

A field the package's `DESCRIPTION` does not have is shown as `-`. Use
`--json` for machine readable output, which reports the repository or remote
type as `source` and the remote itself as `remote`, separately.

This subcommand and [`rig pkg remove`](#rig-pkg-remove) read an *installed*
library; the others, e.g. [`rig pkg available`](#rig-pkg-available), read the
package repositories that packages are installed *from*.

## Which library

By default rig lists the default library of the default R version, i.e. the
library that [`rig library default`](library.qmd) reports, and the one R
installs packages into.

`--library` (`-l`) selects another library. It takes either the name of a
library of the R version, as [`rig library list`](library.qmd) prints them, or
the path of a library directory:

```
rig pkg list --library myproject
rig pkg list --library /usr/lib/R/site-library
```

A path is used as it is, so it does not need to belong to an R version rig
manages, and rig does not need an R version at all to list it.

`--r-version` (`-r`) lists the library of another R version, instead of the
default one, as it does for the [`rig library`](library.qmd) commands. It has
no effect when `--library` is a path.

Subdirectories that are not packages are left out: rig's own libraries of a
main library, and the leftovers of an interrupted installation.
