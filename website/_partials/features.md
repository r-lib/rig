* Works on macOS, Windows and Linux.
* Supports many Linux distributions, see
  [list below](install.qmd#id-supported-linux-distributions).
* Easy installation and update, no system requirements on any platform.
* Install multiple R versions.
* Select the default R version, for the terminal and RStudio.
* Select R version to install using symbolic names: `devel`, `next`,
  `release`, `oldrel`, etc.
* List R versions available to install.
* Run multiple versions _at the same_ time using quick links.
  E.g. `R-4.1` or `R-4.1.2` starts R 4.1.x. Quick links are automatically
  added to the user's path.
* On macOS it comes with a menu bar app that shows the default R
  version and lets you select it interactively.
  [See more](macos-app.qmd).
* On arm64 macs select between x86_64 and arm64 versions or R, or install both.
* Sets up the default CRAN mirror and [PPM](https://packagemanager.posit.co/).
  (Only if you are not using RStudio or Positron!)
* Installs [pak](https://pak.r-lib.org) and set up automatic
  [system requirements installation](https://pak.r-lib.org/dev/reference/sysreqs.html).
* Creates and configures user level package libraries.
* Restricts permissions to the system library.
  (On macOS, not needed on Windows and Linux).
* Includes auto-complete for `zsh` and `bash`, on macOS and Linux.
* Updates R installations to allow debugging with `lldb`, and to allow
  core dumps, on macOS.
* Installs the appropriate Rtools versions on Windows and sets them up.
* Cleans up stale R-related entries from the Windows registry.
* Optional *user mode* installs R entirely into your home directory, with
  no `sudo` or administrator rights needed. In the default *admin mode* rig
  installs R system-wide and switches to the root/administrator user as
  needed. See the [FAQ](faq.qmd).
* Supports JSON output for scripting.
