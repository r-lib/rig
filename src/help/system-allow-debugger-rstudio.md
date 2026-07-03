## Description

Update the entitlements of RStudio R process to allow debuggers on macOS.
It adds the `get-task-allow` entitlement to the `rsession` (and
`rsession-arm64` on arm64 hardware) binary.

See also `rig system allow-debugger`, which does the same for
R running outside of RStudio.

This command does nothing on Windows and Linux.

This command probably needs `sudo`:
`sudo rig system allow-debugger-rstudio`, otherwise rig will ask for your
password.
