## Description

Make the current R installations orthogonal on macOS. This allows
running multiple R versions at the same time.

This command does nothing on Windows and Linux, where R installations
are automatically orthogonal. It also does nothing in user mode, as user
mode installations are always orthogonal.

`rig add` runs `rig system make-orthogonal` automatically, you do not
need to run it manually.

In admin mode this command probably needs `sudo`:
`sudo rig system make-orthogonal`, otherwise rig will ask for your
password.
