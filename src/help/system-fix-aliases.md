Fix symbolic R-* quick links

## Description

Check the symbolic quick links `R-release`, `R-oldrel`, `R-devel` and
`R-next` (plus the `-x86_64` variants on Arm machines) and make sure that
each one points to the R version it currently should, i.e. the same version
[`rig resolve`](resolve.qmd) would return for its name today.

In [user mode](../admin-vs-user-mode.qmd) `R-devel`/`R-next` install under a
fixed directory name, so they are always already correct there. In admin
mode the installation directory is named after the version number, so
`R-devel`/`R-next` can go stale when devel/next branches to a new minor
version; rig detects this the same way it names the directory in the first
place.

If an alias points to the wrong version, rig re-points it to the correct
one. If the correct version is not installed, rig removes the stale alias
and prints a warning; it does not install R versions.

In user mode no administrator rights are needed. In admin mode you need an
administrator account to run this command or use `sudo` on Unix, otherwise
rig will ask for your password.
