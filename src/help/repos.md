Manage package repositories

## Description

Manage the R package repositories that rig configures for your R
installations.

rig sets up the repositories R uses to install packages (the `repos` option
in R), typically a CRAN mirror and the Posit Public Package Manager (P3M).
These are configured per R version, and you can control them when
installing R (see the `--with-repos` and `--without-repos` options of `rig add`)
or afterwards with the subcommands here: [`rig repos enable`](#rig-repos-enable) and [`rig repos
disable`](#rig-repos-disable) turn repositories on and off for an R version.

## Your own Package Manager

If you set the `PACKAGEMANAGER_ADDRESS` environment variable, or the
`ppm-address` key of the rig configuration file (`rig config set
ppm-address=https://ppm.acme.com`), to a Posit Package Manager instance
other than the public P3M, then rig sets up all R and Bioconductor
repositories of that instance for new R installations, and none of the
public repositories. They are in the `PPM` repository group of [`rig repos
available`](#rig-repos-available). The first repository that has a CRAN or
curated CRAN source (possibly among other sources) comes first, and it is named `CRAN`
in R, the other repositories keep their names. On Linux rig
uses the binary URLs of the instance where it builds binaries for your
distribution. Each Bioconductor repository is only used with the R
versions whose Bioconductor version it serves. The public repositories are
still available with `--with-repos`, e.g. `--with-repos=cran`.

rig asks the instance for its repositories, and caches the list for a day.
`https://p3m.dev` and `https://packagemanager.posit.co` are both the public
P3M, setting either of them is the same as setting neither.

Already installed R versions keep their repositories, run [`rig repos
setup`](#rig-repos-setup) to update them. Until then rig warns when it
reads the repositories of an R version that does not use the instance,
e.g. in [`rig repos list`](#rig-repos-list) and [`rig pkg`](pkg.qmd). To
keep an R version on the public repositories on purpose, and silence the
warning, turn off the instance and turn on the public repositories for
it, e.g. `rig repos setup --r-version 4.5.1 --without-repos=ppm
--with-repos=p3m,cran`. `rig repos disable ppm` alone also silences the
warning, but then the R version has no repositories from rig.

## Custom repositories

Besides the repositories built into rig, you can add your own CRAN-like
repositories with [`rig repos add`](#rig-repos-add), and enable them for the R versions that
need them.

Changing the repositories of an R version updates the files of the R
installation. In [admin mode](../admin-vs-user-mode.qmd) on Linux and Windows these belong to the
administrator, so rig asks for administrator rights; in user mode, and
usually for admin users on macOS, it does not.

To look up the packages the repositories offer, see [`rig pkg`](pkg.qmd) .
