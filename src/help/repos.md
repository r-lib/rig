Manage package repositories

## Description

Manage the R package repositories that rig configures for your R
installations.

rig sets up the repositories R uses to install packages (the `repos` option in R),
typically a CRAN mirror and the Posit Public Package Manager (P3M). These are
configured per R version, and you can control them when installing R (see the
`--with-repos` and `--without-repos` options of `rig add`) or afterwards with the
subcommands here: [`rig repos enable`](#rig-repos-enable) and [`rig repos disable`](#rig-repos-disable) turn repositories on
and off for an R version.

Besides the repositories built into rig, you can add your own CRAN-like
repositories with [`rig repos add`](#rig-repos-add), and enable them for the R versions that need
them.

Changing the repositories of an R version updates the files of the R
installation. In [admin mode](../admin-vs-user-mode.qmd) on Linux and Windows these belong to the
administrator, so rig asks for administrator rights; in user mode, and usually
for admin users on macOS, it does not.

To look up the packages the repositories offer, see [`rig pkg`](pkg.qmd) .
