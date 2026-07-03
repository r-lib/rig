## Description

Manage Rtools installations (Windows only).

Rtools is the collection of build tools (compilers, `make`, etc.) needed
to build R packages from source on Windows. Each R version needs a
matching Rtools version.

`rig rtools list` lists the installed Rtools versions.
`rig rtools add` installs Rtools, by default every version needed by the
currently installed R versions.
`rig rtools rm` removes Rtools versions.

On non-Windows platforms this command does nothing and is hidden.
