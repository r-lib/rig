Print the R installation root

## Description

Print the directory rig installs R versions into, and nothing else, e.g.
for use in a shell script:

```
cd "$(rig system r-dir)"
```

This is the directory that *contains* the directories of the individual R
versions; it is not an R installation itself. Use `rig list --json` for the
paths of the installed R versions.

The R installation root depends on the [mode](../admin-vs-user-mode.qmd)
and can be overridden with the `RIG_R_INSTALL_DIR` environment variable or
the `r-install-dir` configuration entry. This command works even if no R
version is installed yet, and the directory itself need not exist.

On Windows the admin mode R installation root depends on the architecture,
and `--arch` prints the root of another architecture. Elsewhere both
architectures share a root and `--arch` is ignored.

`rig system dirs` prints this directory together with all the other
directories rig uses.
