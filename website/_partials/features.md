**Multiple R versions, side by side**

* Install as many R versions as you like, and switch the default for the
  terminal, RStudio and Positron.
* Select versions by symbolic name: `release`, `devel`, `next`, `oldrel`,
  or by exact version.
* Run several versions _at the same time_ via quick links: `R-4.5` or
  `R-4.5.1` starts the matching R. Quick links are added to your path
  automatically.
* List the versions you have installed, and the versions available to
  install.
* On arm64 macs, choose between x86_64 and arm64 builds of R, or install
  both.
* On arm64 Windows, install x86_64 and aarch64 builds of R.

**Cross-platform and self-contained**

* Works on macOS, Windows and Linux — with native builds for many
  [Linux distributions](install.qmd#id-supported-linux-distributions), and
  portable builds that run on any glibc- or musl-based Linux.
* A single standalone tool with no system requirements — easy to install
  and update on every platform.
* Two installation modes: the default *admin mode* installs R system-wide
  (elevating to root/administrator only when needed), while *user mode*
  installs everything into your home directory with no `sudo` or
  administrator rights. See [admin vs. user mode](admin-vs-user-mode.qmd).
* On Linux, installs distro-specific builds where available, and otherwise
  falls back to portable glibc/musl builds automatically — so rig works
  even on distributions without a dedicated build.

**Package management, set up for you**

* Configures the default CRAN mirror and
  [PPM](https://packagemanager.posit.co/) binary repositories.
* Installs [pak](https://pak.r-lib.org) and enables automatic
  [system requirements installation](https://pak.r-lib.org/dev/reference/sysreqs.html).
* Creates and configures per-user package libraries.
* `rig repos` manages package repositories across all your R versions.

**Run R, scripts and apps**

* `rig run` starts R, runs a script or expression, or launches an app —
  Shiny apps, Plumber APIs, Quarto and R Markdown documents, and static
  sites — with the R version you choose.
* `rig proj` (experimental) resolves and installs R project dependencies
  with a built-in solver, and can write an `renv.lock`, without needing R
  to be running.

**Platform niceties**

* A macOS menu bar app shows the default R version and lets you switch it
  interactively. [See more](macos-app.qmd).
* Installs and configures the right Rtools versions on Windows, and cleans
  up stale R entries from the Windows registry.
* Shell auto-completion for `zsh` and `bash` on macOS and Linux, and for
  PowerShell on Windows.
* On macOS, sets up R for debugging with `lldb` and enables core dumps.
* JSON output for scripting, and a `rig config` command to manage rig's
  own configuration.
