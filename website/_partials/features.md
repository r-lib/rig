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
* On arm64 macs and Windows, choose between x86_64 and arm64 builds of R,
  or install both.

**Cross-platform and self-contained**

* Works on macOS, Windows and Linux. Has native builds for many
  [Linux distributions](install.qmd#id-supported-linux-distributions), and
  portable builds that run on any glibc- or musl-based Linux.
* A single standalone tool with no system requirements. Easy to install
  and update on every platform.
* Two [installation modes](admin-vs-user-mode.qmd): the default
  *admin mode* installs R system-wide (elevating to root/administrator only
  when needed), while the newer *user mode* installs everything into your
  home directory with no `sudo` or administrator rights.
* On Linux, installs distro-specific builds where available, and otherwise
  falls back to portable glibc/musl builds automatically.

**Package management, set up for you**

* Configures the default CRAN mirror and
  [P3M](https://packagemanager.posit.co/) binary repositories.
* Installs [pak](https://pak.r-lib.org) and enables automatic
  [system requirements installation](https://pak.r-lib.org/dev/reference/sysreqs.html).
* Creates and configures per-user package libraries.
* [`rig repos`](reference/repos.qmd) manages package repositories across
  all your R versions. Add your own CRAN-like repositories, e.g. an
  internal CRAN mirror or an r-universe, and turn repositories on and off
  for each R version. rig remembers the setup.
* [`rig pkg`](pkg-guide.qmd) (experimental) installs, removes and lists
  packages in a library, and looks up package info, dependencies and
  dependency trees from the repositories, all without running R. It also
  searches CRAN, and finds problems in a library, e.g. missing or too old
  dependencies.
* Installs packages from CRAN, Bioconductor (the release that belongs to
  your R version), your own repositories and local paths, with version
  constraints, e.g. `rig pkg install 'cli@=3.6.4'`.
* On Linux, `rig pkg`, `rig proj` and `rig run` install the system
  requirements of R packages, or print the commands to install them if
  rig cannot run them as root.

**Project dependency management (experimental)**

* [`rig proj`](proj-guide.qmd) manages an R project through an
  `rproj.toml` manifest: add or remove dependencies, resolve them with
  rig's built-in solver into an `rproj.lock` lockfile, and sync a project
  library to match, all without running R.
* Lock for several R versions and platforms at once, pin repositories and
  Bioconductor releases, ignore package versions newer than a date with
  `--exclude-newer`, and upgrade only some packages while keeping the
  rest pinned.
* Import a project from, or export it to, a `DESCRIPTION` file or an
  [renv](https://rstudio.github.io/renv/) `renv.lock` file, to interoperate
  with existing R package and project tooling.
* Inspect a project's dependencies and dependency tree before installing.

**Run R, scripts and apps**

* `rig run` starts R, runs a script or expression, or launches an app:
  Shiny apps, Plumber APIs, Quarto and R Markdown documents, and static
  sites, with the R version you choose.
* [Self-contained scripts](scripts.qmd): declare the R version and the
  packages a script needs in a `# /// script` comment block, and
  `rig run` installs them into the script's own environment. Lock the
  versions next to the script with `rig proj lock --script`.
* On Windows, `rig system script-assoc` lets you run `.R` files directly
  from `cmd` and PowerShell.

**Platform niceties**

* A [macOS menu bar app](macos-app.qmd) shows the default R version and
  lets you switch it interactively.
* Installs and configures the right Rtools versions on Windows, and cleans
  up stale R entries from the Windows registry.
* Shell auto-completion for `zsh` and `bash` on macOS and Linux, and for
  PowerShell on Windows.
* On macOS, sets up R for debugging with `lldb` and enables core dumps.
* JSON output for scripting, e.g. from `rig list`, `rig add` and
  `rig rtools add`, and a `rig config` command to manage rig's
  own configuration.
