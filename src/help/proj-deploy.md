Deploy project dependencies

## Description

Install the resolved dependencies of an R project into a package library.

rig solves the project dependencies and installs them into the library
given by `--library`. Use `--r-binary` to select which R to build against
(default: `R`) and `--max-concurrent` to limit the number of simultaneous
installations (default: 4).
