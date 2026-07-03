Restrict system library permissions to admin

## Description

Update the permissions of the current R versions on macOS, so only the
administrator can install R packages into the system library.
Together with `rig system create-lib` this facilitates keeping
additional packages in a user library, instead of the system library.

This command does nothing on Windows and Linux and in user mode.

`rig add` runs `rig system fix-permissions`, so if you only use rig to
install R, then you do not need to run it manually.

In admin mode this command probably needs `sudo`:
`sudo rig system fix-permissions`, otherwise rig will ask for your
password.
