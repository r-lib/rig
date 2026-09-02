Install packages from the repositories

## Description

Install one or more R packages, and everything they need, into an R package
library.

```
rig pkg install cli glue
```

```
✓ Solved dependencies
2 of 2 packages to install (R 4.4.1, main: /Users/gaborcsardi/Library/R/arm64/4.4/library)
Package  Version  Type    Action   Reason
cli      3.6.3    binary  install  not installed
glue     1.8.0    binary  install  not installed
✓ Installed 2 packages (R 4.4.1, main: /Users/gaborcsardi/Library/R/arm64/4.4/library)
```

rig resolves the whole dependency tree first, the same way
[`rig proj lock`](proj.qmd) does for a project, so a package is only
installed if every package it needs can be installed with it, at versions
that work together. `--dry-run` runs the resolution and reports what it
would install, without installing anything.

Package names are case sensitive, as they are in R. Naming the same package
twice is not an error, it is installed once.

## Dev dependencies

By default rig installs the hard dependencies only: `Depends`, `Imports`
and `LinkingTo`, i.e. the packages that need to be installed to use the
package. `--dev` also installs the soft dependencies, `Suggests` and
`Enhances`, which are typically only needed to run the tests, build the
vignettes or use some optional feature.

`--dev` applies to the packages named on the command line only. A dev
dependency is installed with everything *it* needs to be installed, but not
with its own dev dependencies, so `--dev` does not grow without bounds.

A package often suggests packages that are not in the repositories rig
installs from, e.g. Bioconductor packages. Those cannot be installed, and
by default rig reports them and installs nothing.
`--ignore-unavailable` installs the rest of the dev dependencies instead,
and names the ones it skipped. It only applies to dev dependencies: a hard
dependency that is not available is always an error, and so is a dev
dependency that exists but has no version that fits.

## Binary and source packages

A binary package is a package that has already been built for your platform
and R version. Installing one is unpacking it into the library, so rig does
that itself and never starts R.

A package with no binary build is installed from its source tarball, with
`R CMD INSTALL`, which does start R, and needs whatever that package needs
to compile. The output of the compilation goes into a log file per package,
in a `_logs` directory inside the library, and rig points at the log when an
installation fails.

`--platform` installs for a platform other than this machine's, and
`--platform source` installs source packages only. `--prefer-binary` trades
a newer version for an older one that has a binary build, which is useful
when compiling is expensive; it takes the number of versions to look back
through, e.g. `--prefer-binary=5`, and defaults to 3.

## Packages rig builds itself

Compiling a package produces exactly what a repository would have served as
a binary package, so rig keeps it: a source install is archived into the
cache, with the same file name `R CMD INSTALL --build` would have given it,
and installing that package again unpacks the archive instead of compiling
it a second time. That covers another library, another project, and
`--reinstall`, which reinstalls a package but does not recompile it.

A cache entry belongs to one platform, one R minor version, one source
tarball, one set of versions of the packages it is compiled against, and one
set of `~/.R/Makevars` files. Change any of those and the package is
compiled again. What it does *not* cover is your compiler, the system
libraries the package found when it was configured, and the arguments it was
configured with; a machine whose toolchain changed under it can hold an entry
that no longer matches, and the way out is to delete it. `rig system dirs
--cache` says where the cache is.

`--no-cache` turns all of this off for one run: rig then downloads the
repository metadata and the package files again, compiles every source
package rather than unpacking one it built earlier, and adds nothing to the
cache. See [`rig config`](config.qmd).

## What gets skipped

rig does not install a package that is already installed and up to date, so
running the same command twice does nothing the second time.

Being up to date is more than having the right version number. A repository
can publish several builds of one version, and a package with compiled code
only works with the versions of the packages it was compiled against — an R
that loads a package built against a different one can crash rather than
complain. So rig keeps track of which build each package it installs came
from, and what that build was compiled against, and reinstalls a package
whose build is no longer the one the resolution picked.

That check cascades: replacing a package also replaces the packages that
were compiled against it, and the packages compiled against those.

rig only knows this about packages it installed itself, so a package that R,
pak or renv installed is always reinstalled rather than assumed to match.
`--reinstall` installs everything in the resolution regardless.

## Which library

By default rig installs into the default library of the default R version,
i.e. the library that [`rig library default`](library.qmd) reports, and the
one R installs packages into.

`--library` (`-l`) selects another library. It takes either the name of a
library of the R version, as [`rig library list`](library.qmd) prints them,
or the path of a library directory:

```
rig pkg install --library myproject cli
rig pkg install --library /usr/lib/R/site-library cli
```

A path is used as it is, and is created if it does not exist yet, so it does
not need to belong to an R version rig manages.

`--r-version` (`-r`) selects the library of another R version, instead of
the default one, as it does for the [`rig library`](library.qmd) commands.
It has no effect on which library `--library` names when that is a path, but
it still decides which binary packages fit, and which `R` installs a source
package.

In [admin mode](../admin-vs-user-mode.qmd) the site and system libraries of
an R installation belong to the administrator, so installing into them needs
`sudo` (an administrator account on Windows). Your own user library never
does.
