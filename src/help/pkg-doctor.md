Find problems with the packages in a library

## Description

Check the packages installed in an R package library, and report the ones
that may not work: missing dependencies, dependencies that are too old, and
packages compiled against another version of a `LinkingTo` dependency than
the one installed now. This command does not start R.

Example output:
```
3 problems (2 errors, 1 warning) in 312 packages (R 4.4.1, main: /Users/johndoe/Library/R/arm64/4.4/library)

Package   Version   Severity   Problem   Details
---------------------------------------------------------------------------
foo       1.0.0     error      missing   Imports bar, not installed
glue      1.8.0     error      version   Imports cli (>= 3.6.0), 3.4.1 is installed
qux       2.1.0     warning    built-r   built for R 4.3.3, R 4.4.1 is used

To fix the errors, run:
  rig pkg install bar cli

To fix the warnings, run:
  rig pkg install qux
```

Give package names to report on those packages only. They are still checked
against the whole library.

After the table rig prints the commands that fix the problems it can fix:
[`rig pkg install`](#rig-pkg-install) for missing and too old dependencies
and for packages that need a reinstall, and the commands that remove leftover
lock directories and broken packages. The `r-version` and `name` problems
need to be fixed by hand.

rig exits with a non-zero status if it finds an error. Warnings alone do not
change the exit status. Use `--json` for machine readable output: an array
with one object per problem.

## Checks

Errors:

* `missing`: a `Depends` or `Imports` dependency is not installed.
* `version`: a `Depends` or `Imports` dependency is installed, but its
  version does not satisfy the requirement of the package.
* `r-version`: the package needs another R version, according to its
  `Depends: R (...)` requirement.
* `abi`: the package was compiled against another version (or another build)
  of a `LinkingTo` dependency than the one installed now. rig can only tell
  this for packages it installed itself, because
  [`rig pkg install`](#rig-pkg-install) records what a package was compiled
  against.

Warnings:

* `stale`: only with `--stale`, see below.
* `built-r`: the package was built for another R minor version, e.g. for
  R 4.3.x in an R 4.4.x library.
* `platform`: the package was built for another architecture, e.g. `x86_64`
  instead of `aarch64`.
* `name`: the `Package` field of the `DESCRIPTION` file does not match the
  directory the package is installed in.
* `lock`: a `00LOCK*` directory, left behind by an interrupted installation.
* `broken`: a directory without a readable `DESCRIPTION` file, or a package
  whose dependencies cannot be parsed.

`--dev` also checks the `Suggests` and `Enhances` dependencies. A missing or
too old `Suggests` or `Enhances` dependency is a warning. `Suggests` are
typically only needed for tests, vignettes or optional features. An
`Enhances` dependency that is too old may break the parts of the package that
work with it.

`--stale` also checks `LinkingTo` ABI compatibility for the packages that
rig did not install, so it has no record of what they were compiled against.
For these rig compares the `Built` timestamps, and warns if a `LinkingTo`
dependency was built after the package itself, because then the package may
need a reinstall. This is off by default, because it flags many packages
that are fine: binary packages from CRAN are not rebuilt when a `LinkingTo`
dependency changes on some platforms.

`LinkingTo` dependencies are only needed to install a package, so a
`LinkingTo` dependency that is not installed is not a problem, unless the
package also needs it in `Depends` or `Imports`.

Dependencies are looked up in the library first, then in the system library
of the R version, which holds the base and recommended packages. The base
packages always count as installed.

## Which library

`--r-version` (`-r`) checks the library of another R version, instead of the
default one.

`--library` (`-l`) selects a non-default library. It takes either the name of a
library of the R version, see [`rig library list`](library.qmd) , or the path of a library
directory. A library name is tried first, so prefix a relative path with `./`
(or use an absolute path) if it happens to share a name with a library.

A library given as a path does not belong to an R version, so rig checks it
against the R version `--r-version` names, or else the default R version, and
tells you which one it uses. Without `--r-version` and a default R version,
rig skips the checks that need an R version (`r-version`, `built-r`,
`platform`) and the system library.
