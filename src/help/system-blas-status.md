Show which BLAS/LAPACK library R is using

## Description

For each installed R version (or the ones given), report whether it is
using the `reference` BLAS that R ships by default, Apple's `accelerate`
(vecLib) BLAS, or an `unknown` library it cannot recognize.

This command only works on macOS.
