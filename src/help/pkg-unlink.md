Remove a package linked with `rig pkg link`

## Description

Remove one or more packages [`rig pkg link`](pkg.qmd) linked into a library,
deleting the linked package's entry. This does not touch the source
directory it pointed at, only rig's link to it. This command does not start R.

A name that is not currently linked -- either not installed at all, or a
normal, non-linked install -- is an error; use [`rig pkg remove`](pkg.qmd) for
a normal install instead. There is nothing to restore after unlinking: a
link never replaces a real install (`rig pkg link` refuses to overwrite one),
so the name is simply available again afterwards.

## Which library

`--library` (`-l`) selects a non-default library. It takes either the name of a
library of the R version, see [`rig library list`](library.qmd), or the path of a
library directory. A library name is tried first, so prefix a relative path
with `./` (or use an absolute path) if it happens to share a name with a
library.

`--r-version` (`-r`) selects the library of another R version, instead of the
default one.

In [admin mode](../admin-vs-user-mode.qmd) the site and system libraries of an
R installation belong to the administrator, so unlinking a package from them
needs `sudo` (an administrator account on Windows). Your own user library
never does.
