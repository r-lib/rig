
# rig 0.5.0

* rig can now open renv projects in RStudio, with the correct R version.
  Pass the renv lock file to rig: `rig rstudio .../renv.lock` (#74).

* `rig list` now prints the result as a table, and it prints the R version
  number as well. `rig list --json` includes the version number, the path
  to the installation, and the path to the R binary.

* rig is now more robust when setting up the user library. In
  particular R will not fail to start in renv projects (#81, @krlmlr).

* On macOS and Linux `rig add` now creates the user library with the right
  permissions, if it does not exist and pak is installed (#84).

* `rig add` now correctly installs pak into the user library, instead of
  the system library, even if the user library did not exist before.

## Windows

* On Windows, `rig add ... --without-translations` installs R without
  message translations. This is useful if you prefer using R in
  English on a non-English system (#88).

* On Windows `rig add` does not add a Desktop icon now by default.
  If want an icon, use the new `--with-desktop-icon` switch (#89).

* On Windows, if the default version is deleted, rig updates the
  registry accordingly, and removes the default from there as well (#86).

* New subcommand `rig system update-rtools40` updates MSYS2 packages
  of Rtools40 on Windows (#14).

## macOS

* On macOS `rig add` now does not fail if it is started from an x86_64
  shell, when adding an arm64 R version on M1 Macs (#79).

* On macOS, `rig rstudio <ver>` now errors if R `<ver>` is not orthogonal,
  and the menu bar app errors as well (#90).

# rig 0.4.1

* `rig rstudio <version>` and `rig rstudio <version> <project>` work properly
  again (#77).

* On Windows, `rig rstudio` now properly restores the default version after
  starting RStudio, if a non-default version was specified.

# rig 0.4.0

## NEW NAME

`rim` is now known as `rig`.

## New features

* Experimental multi-library support. See `rig library --help` for the
  details.

* On macOS rig now includes a menu bar app that show the default R version,
  lets you choose between R versions and libraries, and lets you start
  RStudio with a specific R version, and/or a recent RStudio project.
  Start the app with `open -a Rig`.

* New subcommand `rig system setup-user-lib` to update the R config to create
  the user package library when R starts up, if it does not exist. The old
  `rig system create-lib` subcommand is now an alias of this.

* Better messages. rig has a `-v` and `-vv` flag now, for extra debug and
  trace messages.

* On arm64 macOS, `rig add` installs arm64 R by default now.
  (This is also true for the x86_64 build of `rig`.

* On macOS `rig add` does not change the default R version any more (#2).

* rig now supports Linux distros based on Ubuntu Bionic 18.04, Focal 20.04
  and Jammy 22.04. They need to have a proper `UBUNTU_CODENAME` entry in
  `/etc/os-release` (#34).

* rig now sets up automated system requirements installation with pak on Linux
  distributions that support it: Ubuntu 18.04, 20.04 and 22.04 (on distros
  based on these), on both x86_64 and aarch64. (This currently needs
  passwordless `sudo` or a root account.)

* All OSes create an `Rscript` link now that runs the default R
  version (#20).

# rim 0.3.0

* New `rim rstudio` command to open a project in RStudio with the specified
  R version.

* `rim add` now performs additonal tasks, to give you an R installation that
  is ready to use:

  - It installs pak for the newly added R version, it is wasn't
    installed before. You can opt out of this with the `--without-pak` option.
    You can select the pak version to install with `--pak-version`.

  - Sets the default R version after installation to the newly
    installed version, if no default is set.

  - Sets the default CRAN mirror to the cloud mirror in the
    system profile, after installation (#11).

  - Sets up RStudio Package Manager (RSPM) as the default repository, if
    your system is supported. See
    https://packagemanager.rstudio.com for more about RSPM. The systems that
    are supported by both RSPM and rim are Windows, and Ubuntu Linux
    18.04, 20.04 and 22.04, all of them on x86_64 architectures only (#15).

* `rim add` now only caches downloaded files for a day.

* `rim system add-pak` now has a new option `--pak-version` to specify the
  pak version to install (stable, rc or devel). Its `--devel` option is
  now deprecated.

* `rim list` now marks the default R version (if any) with `(default)` (#38).

## Windows

* rim has a Chocolatey package now, so on Windows you can install it with
  `choco install rim` and upgrade it with `choco upgrade rim`.

* On Windows `rim default <version>` now sets the default R version in the
  Windows Registry as well, which changes the default for RStudio.
  (Make sure you set the R version in RStudio to the machine's default
  version in Tools -> Global Options -> Basic -> General -> R version.)

## macOS

* On macOS `rim system fix-permissions` now sets the permissions and group
  of the `Current` link. Also, `rim add` now calls `rim system fix-permissions`
  for all installed R versions, because the R installed changes the
  permissions of all of them.

# rim 0.2.3

* `rim system allow-debugger` and `rim system allow-core-dumps` now work on
  macOS Big Sur.

# rim 0.2.2

* rim now supports the next version of R:
  ```
  rim resolve next
  rim add next
  ```
  The next version of R is R-alpha, R-beta, R-rc or R-prerelease if there
  is an active R release process, and it is R-patched otherwise.

# rim 0.2.1

## Linux

* rim now supports arm64 Linux, currently the following distributions:
  Ubuntu 18.04, 20.04 & 22.04 and Debian 9, 10 & 11.

* rim now supports Debian 11, on arm64 and x86_64.

## macOS

* New macOS `rim system` commands to allow debugging:
  `rim system allow-debugger`; and to allow core dumps:
  `rim system allow-core-dumps`.

* rim now uses better R-devel URLs on macOS, that do not fail if the daily
  build failed on mac.r-project.org.

## Windows

* rim now supports Rtools42 on Windows: `rim add rtools42`.

* Fix resolution of older Windows installers, they were moved to
  another URL.

* rim can now delete Rtools on Windows, e.g.: `rim rm rtools42`.

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
