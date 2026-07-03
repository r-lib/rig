## Description

Tell macOS to forget about the currently installed R versions.
This is needed to have multiple R installations at the same time.

This command does nothing on Windows and Linux and in user mode.

`rig add` runs `rig system forget` before and after the installation,
so if you only use rig to install R, then you don't need to run this
command manually.

In admin mode this command probably needs `sudo`:
`sudo rig system forget`, otherwise rig will ask for your password.
