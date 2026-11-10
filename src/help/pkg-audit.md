Check the packages of a library for known vulnerabilities and blocked versions

## Description

`rig pkg audit` checks the packages installed in an R package library
against the policy of their repositories, the same way as
[`rig audit`](audit.qmd) does. It reports:

- **vulnerable** versions: the repository knows about security
  vulnerabilities in them,
- **blocked** versions: the repository refuses to serve them, e.g. because
  of a package, license or vulnerability rule, and
- **archived** packages: CRAN removed them. These are only reported, they
  do not make the audit fail.

For each vulnerable or blocked version it shows the nearest newer version
without known problems, if there is one.

Unlike `rig audit`, it always checks a library, also in a project. To check
a project's lock file and library, use [`rig audit`](audit.qmd).

Packages installed from GitHub, git, a URL or a local directory are not in
any repository, so they are not checked. The CRAN package history at
<https://ppm.r-pkg.org> reports known vulnerabilities. A CRAN repository of
Posit Package Manager that serves its own extended metadata may also report
blocked versions, see [`rig audit`](audit.qmd#where-the-data-comes-from).

`rig pkg audit` does not check whether the packages work together, use
[`rig pkg doctor`](#rig-pkg-doctor) for that.

## Which library

Without options `rig pkg audit` checks the default library of the default R
version. `--r-version` (`-r`) checks the library of another R version.
`--library` (`-l`) takes either the name of a library of the R version, see
[`rig library list`](library.qmd), or the path of a library directory. In
[user mode](../admin-vs-user-mode.qmd) the libraries are in your home
directory, in admin mode they may be system-wide.

## Fixing problems

`--fix` upgrades every vulnerable or blocked package to the nearest newer
version without known problems, with
[`rig pkg install`](#rig-pkg-install). It only upgrades the packages that
need an upgrade for that. `--fix --dry-run` shows the upgrades without
making them.

`--exclude-newer` only upgrades to versions published by a date, and passes
the date to `rig pkg install`. Without it, `rig pkg audit` uses the
`exclude-newer` [config](config.qmd) entry, if set, the same way as `rig pkg
install`. If no version without known problems is published by the cutoff,
the package is not upgraded.

## Exit status

`rig pkg audit` exits with status 1 if it finds vulnerable or blocked
packages, also after `--fix` if some are left, and 0 otherwise, so it can be
used in CI.
