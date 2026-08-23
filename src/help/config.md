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

- `download-dir` (`RIG_DOWNLOAD_DIR`): the directory rig downloads the R (and
  on Windows the Rtools) installers into, before installing them. Defaults to
  `rig-<uid>` in the system temporary directory, e.g. `/tmp/rig-1000`, and to
  `rig` under `%TEMP%` on Windows. The user id is part of the default name on
  purpose: in [admin mode](../admin-vs-user-mode.qmd) rig downloads as `root`,
  in user mode as you, and a directory shared between them would only be
  writable by whoever created it first. For the same reason rig refuses to use
  the default directory if it is a symbolic link, or if it is owned by another
  user, or if other users can write into it. A directory you configure here is
  created but not checked.

- `no-cache` (`RIG_NO_CACHE`): set it to `true` to stop rig from using its
  cache, the same as passing `--no-cache`. rig then neither reads nor writes
  the cache directory: it downloads the repository metadata, the binary
  package indices and the package files again, compiles a source package
  instead of unpacking a build it made earlier, and downloads an R or Rtools
  installer again instead of reusing the one in the download directory. What
  it downloads and builds goes into a temporary directory that is removed when
  rig exits, so the cache is left exactly as it was. Defaults to `false`. This
  is meant for debugging and for the occasional run that must not trust
  anything cached; it makes rig considerably slower, so it is a poor thing to
  turn on permanently.

- `positron-setup`: [user mode](../admin-vs-user-mode.qmd) only. Set it to `false` to stop rig from
  updating Positron's settings: adding its R installation root to
  `positron.r.customRootFolders`, and pointing
  `positron.r.interpreters.default` at the [default](default.qmd) R version.
  Any other value, and the default, keep the Positron setup on.

- `userlibrary`: a JSON object that maps R versions to user library paths.
  rig maintains this entry itself, as a cache for the `rig library`
  commands; you don't normally need to edit or set it.

`rig config list` lists the entries that are currently in the configuration
file, which is typically fewer than the entries above, because rig only
writes an entry once you set it.
