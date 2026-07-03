Remove OpenMP (-fopenmp) option for Apple compilers

## Description

Remove -fopenmp flags from the R configuration, to make R work with
the Apple compilers, instead of CRAN's custom compilers. This is only
needed for R 3.6.x and before.

`rig add` runs `rig system no-openmp` after the installation, so if
only use rig to install R, then you don't need to run this command
manually.

This command does nothing on Windows and Linux and in user mode.

In admin mode this command probably needs `sudo`:
`sudo rig system no-openmp`, otherwise rig will ask for your password.
