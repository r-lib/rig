Print the directories rig uses

## Description

Print the directories rig uses: where R (and on Windows Rtools) is
installed, where the quick links are created, and where rig keeps its own
configuration, data, cache and log files.

This works even if no R version is installed yet, so you can use it to see
where `rig add` will put things. The directories themselves need not exist.
Use `--json` for machine-readable output.

`rig system dirs` prints the directories rig will *actually* use, after
applying the [mode](../admin-vs-user-mode.qmd), the environment variables
and the configuration file. `rig config get` in contrast prints only an
explicit override from the configuration file, and prints nothing when the
default applies.

| Reported      | Flag       | Environment variable     | Config entry         |
| ------------- | ---------- | ------------------------ | -------------------- |
| `mode`        |            | `RIG_MODE`               | `mode`               |
| `r_root`      | `--r`      | `RIG_R_INSTALL_DIR`      | `r-install-dir`      |
| `rtools_root` | `--rtools` | `RIG_RTOOLS_INSTALL_DIR` | `rtools-install-dir` |
| `binary_dir`  | `--binary` | `RIG_BINARY_DIR`         | `binary-dir`         |
| `data_dir`    | `--data`   |                          |                      |
| `cache_dir`   | `--cache`  |                          |                      |
| `logs_dir`    | `--log`    |                          |                      |

`--user` and `--admin` also override the mode, for a single invocation.

## Printing a single directory

With one of the flags above rig prints that single directory as a bare
path, and nothing else, for use in a shell script:

```
cd "$(rig system dirs --r)"
```

The flags are mutually exclusive, and cannot be combined with `--json`.
The path of the configuration file is `rig config config-file-path`.

`--r` is the directory that *contains* the directories of the individual R
versions; it is not an R installation itself. Similarly `--rtools` is the
directory that contains the Rtools directories. Use `rig list --json` and
`rig rtools list` for the paths of the installed versions.

## Architecture

The reported architecture is the architecture of the machine, spelled the
way `rig add --arch` spells it on this platform. (This is not always the
spelling `rig system detect-platform` uses, which reports `aarch64` on
Apple silicon.) On Windows the R installation root depends on the
architecture in admin mode, and `--arch` reports the root of another
architecture. Elsewhere both architectures share a root, and `--arch` is
ignored.

## Rtools

The Rtools installation root is only reported on Windows. In admin mode it
is the drive root, because Rtools keeps its historical location there, e.g.
Rtools 4.5 is installed into `C:\Rtools45`, and its aarch64 build into
`C:\Rtools44-aarch64`.

On non-Windows platforms `--rtools` prints nothing and is hidden.
