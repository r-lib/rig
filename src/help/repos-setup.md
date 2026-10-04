Set up R package repositories

## Description

Set up the package repositories for installed R versions.

By default rig configures the repositories for all installed R versions;
use `--r-version` to restrict it to one. Use `--with-repos` and `--without-repos`
to control which repositories are enabled, the same way as for `rig add`.

rig remembers the repositories enabled and disabled for each R version, with
`--with-repos`, `--without-repos`, [`rig repos enable`](#rig-repos-enable) and [`rig repos disable`](#rig-repos-disable), and
applies them again every time. So `rig repos setup` without options keeps your
choices, `--with-repos=<names>` and `--without-repos=<names>` add to them, and
`--without-repos` without names starts over with no repositories (plus the ones in
`--with-repos`).

rig updates the files of the R installation. If you cannot write them, which is
typical in [admin mode](../admin-vs-user-mode.qmd) on Linux and Windows, rig asks for administrator rights,
e.g. your password for `sudo`.
