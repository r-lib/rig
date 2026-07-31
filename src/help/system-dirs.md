Print the directories rig uses

## Description

Print the directories rig uses: where R (and on Windows Rtools) is
installed, where the quick links are created, and where rig keeps its own
configuration, data, cache and log files.

This works even if no R version is installed yet, so you can use it to see
where `rig add` will put things. Use `--json` for machine-readable output.

`rig system dirs` prints the directories rig will *actually* use, after
applying the [mode](../admin-vs-user-mode.qmd), the environment variables
and the configuration file. `rig config get` in contrast prints only an
explicit override from the configuration file, and prints nothing when the
default applies.

| Reported      | Environment variable       | Config entry         |
| ------------- | -------------------------- | -------------------- |
| `mode`        | `RIG_MODE`                 | `mode`               |
| `r_root`      | `RIG_R_INSTALL_DIR`        | `r-install-dir`      |
| `rtools_root` | `RIG_RTOOLS_INSTALL_DIR`   | `rtools-install-dir` |
| `binary_dir`  | `RIG_BINARY_DIR`           | `binary-dir`         |

`--user` and `--admin` also override the mode, for a single invocation.

The reported architecture is the architecture of the machine, spelled the
way `rig add --arch` spells it on this platform. (This is not always the
spelling `rig system detect-platform` uses, which reports `aarch64` on
Apple silicon.) On Windows the R installation root depends on the
architecture in admin mode, and `--arch` reports the root of another
architecture. Elsewhere both architectures share a root, and `--arch` is
ignored.

`rtools_root` is only reported on Windows. In admin mode it is the drive
root, because Rtools keeps its historical location there, e.g. Rtools 4.5
is installed into `C:\Rtools45`.

See also `rig system r-dir`, `rig system binary-dir` and
`rig system rtools-dir`, which each print a single path, and
`rig config config-file-path`.
