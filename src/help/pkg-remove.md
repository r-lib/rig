Remove packages from a library

## Description

Delete one or more installed packages from an R package library, without
starting R.

```
rig pkg remove cli glue
```

```
▶ Removing cli 3.6.3 from /Users/gaborcsardi/Library/R/arm64/4.4/library/cli...
▶ Removing glue 1.8.0 from /Users/gaborcsardi/Library/R/arm64/4.4/library/glue...
✓ Removed 2 packages (R 4.4.1, main: /Users/gaborcsardi/Library/R/arm64/4.4/library)
```

Removing a package deletes its directory in the library, which is what
`R CMD REMOVE` and `remove.packages()` do as well.

Package names are case sensitive, as they are in R, and every package named
must be installed in the library: if one of them is not, then rig removes
none of them. Naming the same package twice is not an error, it is removed
once.

rig does not check whether another installed package needs the one being
removed. Use [`rig pkg list`](#rig-pkg-list) to see what is installed, and
`--json` for machine readable output about what was removed.

The base packages that ship with R (`base`, `stats`, `utils`, ...) are part
of the R installation, and R does not work without them, so rig refuses to
remove them unless `--force` is also given.

## Which library

By default rig removes the packages from the default library of the default R
version, i.e. the library that [`rig library default`](library.qmd) reports,
and the one R installs packages into.

`--library` (`-l`) selects another library. It takes either the name of a
library of the R version, as [`rig library list`](library.qmd) prints them, or
the path of a library directory:

```
rig pkg remove --library myproject cli
rig pkg remove --library /usr/lib/R/site-library cli
```

A path is used as it is, so it does not need to belong to an R version rig
manages, and rig does not need an R version at all to remove packages from
it.

`--r-version` (`-r`) selects the library of another R version, instead of the
default one, as it does for the [`rig library`](library.qmd) commands. It has
no effect when `--library` is a path.

In [admin mode](../admin-vs-user-mode.qmd) the site and system libraries of
an R installation belong to the administrator, so removing a package from
them needs `sudo` (an administrator account on Windows). Your own user
library never does. To remove a whole library, with all the packages in it,
use [`rig library rm`](library.qmd) instead.
