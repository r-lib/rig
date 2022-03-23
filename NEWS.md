
# rim 0.2.1 (unreleased)

* rim now supports arm64 Linux, currently the following distributions:
  Ubuntu 18.04, 20.04 & 22.04 and Debian 9 & 10.

* New macOS `rim system` commands to allow debugging:
  `rim system allow-debugger`; and to allow core dumps:
  `rim system allow-core-dumps`.

* Fix resolution of older Windows installers, they were moved to
  another URL.

* rim now uses better R-devel URLs on macOS, that do not fail if the daily
  build failed on mac.r-project.org.

# rim 0.2.0

* New Linux version.

* On macOS rim now asks for your password for tasks that require admin
  access.

* On Windows rim now automatically elevates to administrator privileges
  as needed, by re-running with gsudo. gsudo is now bundled in the
  Windows distribution.

* New `rim system clean-registry` command to remove leftover Windows
  registry entries

* New `rim system no-openmp` to use the Apple compilers on macOS.

* `rim rm` now cleans the registry on Windows.

* The Windows rim installer adds rim and R to the PATH on GitHub Actions.

* `rim list` does not error any more if no R versions are installed.

* macOS now has `rim system no-openmp` to fix the conpiler configurations
  for the Apple compilers.

# rim 0.1.5

* Experimental Windows version.

# rim 0.1.4

* `resolve` and `add` work again.

# rim 0.1.3

* `bash` and `zsh` completions out of the box, on macOS

* You can now pass URLs `.pkg` installers to `rim add` on macOS.

# rim 0.1.2

* `rim ls` is now a synonym of `rim list`.

* We have macOS installers now, and they are signed and notarized.

# rim 0.1.1

* `rim rm now supports removing multiple versions.

* `rim system` commands `create-lib`, `fix-permissions` and
  `make-orthogonal` can be now restricted to one or more R versions.
  `rim add` now only calls them for the newly installed R version.

* `rim system add-pak` is now implemented.

# rim 0.1.0

First pre-release.
