Print the quick link directory

## Description

Print the directory rig creates the quick links (`R-4.5.1`, `R-release`,
etc.) in, and nothing else, for use in a shell script.

This directory needs to be on the path for the quick links to work; see
`rig system make-links`. It depends on the
[mode](../admin-vs-user-mode.qmd) and can be overridden with the
`RIG_BINARY_DIR` environment variable or the `binary-dir` configuration
entry.

This command works even if no R version is installed yet, and the directory
itself need not exist.

`rig system dirs` prints this directory together with all the other
directories rig uses.
