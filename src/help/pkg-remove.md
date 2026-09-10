Remove packages from a library

## Description

Delete one or more installed packages from an R package library.
This command does not start R.

rig does not check whether another installed package needs the one being
removed.

The base packages that ship with R (`base`, `stats`, `utils`, ...) are part
of the R installation, and R does not work without them, so rig refuses to
remove them unless `--force` is also given.

## Which library

`--library` (`-l`) selects a non-default library. It takes either the name
of a library of the R version, see [`rig library list`](library.qmd), or
the path of a library directory.

`--r-version` (`-r`) lists the library of another R version, instead of the
default one.

In [admin mode](../admin-vs-user-mode.qmd) the site and system libraries of
an R installation belong to the administrator, so removing a package from
them needs `sudo` (an administrator account on Windows). Your own user
library never does. To remove a whole library, with all the packages in it,
use [`rig library rm`](library.qmd) instead.
