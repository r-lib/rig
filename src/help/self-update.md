Update rig to the latest release

## Description

Downloads the latest rig release and replaces the currently running `rig`
binary with it.

This only works if rig was installed with the install script (`install.sh` on
macOS/Linux, `install.ps1` on Windows), because that is the only install
method where rig fully controls the binary's location. If rig was installed
via the `.pkg`/`.deb`/`.rpm` package, Chocolatey, WinGet, Scoop, or Homebrew, `rig
self update` refuses and tells you which tool to use instead.

This works the same way in [admin mode and user mode](../admin-vs-user-mode.qmd): `rig self update` only
ever replaces the rig binary itself, not any installed R version.

Use `--dry-run` to check whether a new version is available without
installing it, and `--pre-release` to also consider pre-release versions when
looking for the latest one.

Use `--nightly` to update to the latest nightly build. Nightly builds are
made every day from the development version of rig. They are not signed and
are less tested than releases, so they are mainly for trying out new features
and fixes before a release. `rig self update --nightly` updates whenever the
nightly build differs from the running rig. To go back to releases, run
`rig self update` (without `--nightly`) after the next release: a nightly
build is always older than the release that follows it.
