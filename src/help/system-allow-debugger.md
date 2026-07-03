Allow debugging R with lldb and gdb

## Description

Update the entitlements of the R binary to allow debuggers on macOS. It
adds the `get-task-allow` entitlement to the R binary. This is only
needed for R installers 3.6 and later, previous versions are not signed.
Call `R -d lldb` to start `lldb` on `R`. (Or `R-x.y -d lldb` if you
want a non-default version.)

See also `rig system allow-debugger-rstudio`, which does the same for
R running in RStudio.

This command does nothing on Windows and Linux and in user mode.

In admin mode this command probably needs `sudo`:
`sudo rig system allow-debugger`, otherwise rig will ask for your
password.
