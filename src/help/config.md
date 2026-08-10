Manage rig configuration

## Description

Manage the rig configuration file.

rig reads a number of settings from a configuration file. The configuration
file is a JSON file, `rig config config-file-path` prints its path, and
`rig system dirs` shows it together with the other directories rig uses.

Most settings can also be overridden with an environment variable. The
environment variable takes precedence over the configuration file, and the
configuration file takes precedence over rig's built-in default.

## Configuration entries

- `mode` (`RIG_MODE`): the installation mode, either `user` or `admin`, see
  [user and admin mode](../admin-vs-user-mode.qmd). Defaults to `admin`.

- `binary-dir` (`RIG_BINARY_DIR`): the directory rig puts the quick links
  (`R-4.5.1`, `R-release`, etc.) into. Defaults to `/usr/local/bin` in admin
  mode and `~/.local/bin` in user mode. On Windows the defaults are
  `C:\Program Files\R\bin` and `%USERPROFILE%\.local\bin`.

- `r-install-dir` (`RIG_R_INSTALL_DIR`): the root directory of the R
  installations, i.e. the directory that holds the directories of the
  individual R versions. Defaults to the platform's system-wide location in
  admin mode (`/opt/R` on Linux, `/Library/Frameworks/R.framework` on macOS,
  `C:\Program Files\R` on Windows), and to `~/.local/share/rig/r`
  (`%APPDATA%\rig\data\r` on Windows) in user mode. On Windows this entry
  only applies in user mode; the admin-mode root is fixed, because it also
  depends on the architecture.

- `rtools-install-dir` (`RIG_RTOOLS_INSTALL_DIR`): Windows only, the
  directory that holds the Rtools installations. Defaults to `C:\` in admin
  mode (so Rtools 4.5 goes into `C:\rtools45`) and to
  `%APPDATA%\rig\data\rtools` in user mode.

- `positron-setup`: macOS only. Set it to `false` to stop rig from adding its
  R installation root to Positron's `positron.r.customRootFolders` setting.
  Any other value, and the default, keep the Positron setup on.

- `userlibrary`: a JSON object that maps R versions to user library paths.
  rig maintains this entry itself, as a cache for the `rig library`
  commands; you don't normally need to edit or set it.

`rig config list` lists the entries that are currently in the configuration
file, which is typically fewer than the entries above, because rig only
writes an entry once you set it.
