Print the Rtools installation root

## Description

Print the directory rig installs Rtools versions into, and nothing else,
for use in a shell script (Windows only).

This is the directory that *contains* the Rtools directories. In admin mode
it is the drive root, because Rtools keeps its historical location there:
Rtools 4.5 is installed into `C:\Rtools45`, and its aarch64 build into
`C:\Rtools44-aarch64`. In [user mode](../admin-vs-user-mode.qmd) it is a
directory in the user's home. It can be overridden with the
`RIG_RTOOLS_INSTALL_DIR` environment variable or the `rtools-install-dir`
configuration entry.

This command works even if no Rtools version is installed yet. Use
`rig rtools list` for the paths of the installed Rtools versions.

On non-Windows platforms this command prints nothing and is hidden.
