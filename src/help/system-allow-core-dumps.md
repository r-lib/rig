Allow creating core dumps when R crashes

## Description

Update the entitlements of the R process to allow core dumps on macOS.
This command is similar to `rig system allow-debugger` but it also makes
sure that the `/cores` directory of core dumps is writeable by the
current user. Don't forget to call `ulimit -c unlimited` from the
same shell before starting R.

This command does nothing on Windows and Linux and in user mode.

In admin mode this command probably needs `sudo`:
`sudo rig system allow-core-dumps`, otherwise rig will ask for your
password.
