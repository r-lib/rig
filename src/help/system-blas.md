Manage the BLAS/LAPACK library used by R

## Description

Commands to check or switch which BLAS/LAPACK library an installed R
version uses. See `rig system blas status --help` and
`rig system blas set --help` for details.

This is only supported on macOS. See the
[R for macOS FAQ](https://cran.r-project.org/bin/macosx/RMacOSX-FAQ.html#Which-BLAS-is-used-and-how-can-it-be-changed_003f)
for background on the reference BLAS vs. Apple's Accelerate/vecLib BLAS.

This command does nothing on Windows and Linux.
