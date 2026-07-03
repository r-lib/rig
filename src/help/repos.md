## Description

Manage the R package repositories that rig configures for your R
installations.

rig sets up the repositories R uses to install packages (the `repos`
option in R), typically a CRAN mirror and the Posit Public Package
Manager (P3M). These are configured per R version, and you can control
them when installing R (see the `--with-repos` and `--without-repos`
options of `rig add`) or afterwards with the subcommands here.

`rig repos setup` (re)configures the repositories for one or all R
versions.
`rig repos list` shows the repositories currently configured for an R
version.
`rig repos available` lists the repositories rig knows about and can set
up.
`rig repos package-list` lists the packages available from the
configured repositories.
