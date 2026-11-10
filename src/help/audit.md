Check packages for known vulnerabilities and blocked versions

## Description

`rig audit` checks the installed and locked packages against the policy of
their repositories. It reports:

- **vulnerable** versions: the repository knows about security
  vulnerabilities in them,
- **blocked** versions: the repository refuses to serve them, e.g. because
  of a package, license or vulnerability rule, and
- **archived** packages: CRAN removed them. These are only reported, they
  do not make the audit fail.

For each vulnerable or blocked version it shows the nearest newer version
without known problems, if there is one.

In a project, `rig audit` checks the packages of `rproj.lock`, for every
target, and the project library. Outside of a project it checks the default
library of the default R version. `--library` and `--r-version` select
another library, also in a project. In [user mode](../admin-vs-user-mode.qmd)
the libraries are in your home directory, in admin mode they may be
system-wide. [`rig pkg audit`](pkg.qmd#rig-pkg-audit) always checks a
library, never a project.

Packages installed from GitHub, git, a URL or a local directory are not in
any repository, so they are not checked.

## Where the data comes from

Only a repositories that serve their own extended metadata with
vulnerabilities and blocked versions, and repositories that rig can expand
(see [`rig proj lock`](proj.qmd#repositories)) are supported.
<https://ppm.r-pkg.org>, if the repository has it.

Independently of `rig audit`, rig never picks a package version blocked
by PPM, version when it installs or locks packages, and it warns if it
picks a version with known vulnerabilities.

## Fixing problems

`--fix` upgrades every vulnerable or blocked package to the nearest newer
version without known problems, and only the packages that need an upgrade
for that. In a project it runs [`rig proj lock`](proj.qmd#rig-proj-lock)
`--upgrade-package` and then [`rig proj sync`](proj.qmd#rig-proj-sync).
Otherwise it runs [`rig pkg install`](pkg.qmd#rig-pkg-install) for the
library. `--fix --dry-run` shows the upgrades without making them.

`--exclude-newer` only upgrades to versions published by a date, and passes
the date to `rig proj lock` or `rig pkg install`. Without it, `rig audit`
uses the same cutoff as these commands: in a project the `exclude-newer`
setting of `rproj.toml`, otherwise the `exclude-newer`
[config](config.qmd) entry, if set. If no version without known problems is
published by the cutoff, the package is not upgraded.

## Exit status

`rig audit` exits with status 1 if it finds vulnerable or blocked packages,
also after `--fix` if some are left, and 0 otherwise, so it can be used in
CI.
