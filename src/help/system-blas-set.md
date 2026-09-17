Switch the BLAS/LAPACK library R uses

## Description

Switch one or more installed R versions (default: all) to use either the
`reference` BLAS that R ships by default, or `accelerate`, Apple's
Accelerate/vecLib BLAS, which is usually much faster for linear algebra.

```
rig system blas set accelerate
rig system blas set reference 4.4.1
```

This command only works on macOS. It skips (with a warning) any R version
that does not ship the requested library.

In admin mode this command probably needs `sudo`:
`sudo rig system blas set accelerate`, otherwise rig will ask for your
password.
