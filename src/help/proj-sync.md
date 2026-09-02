Install the dependencies rproj.lock resolved

## Description

Install the resolved dependencies of an R project into a package library.

rig reads `rproj.lock` (written by [`rig proj lock`](#rig-proj-lock)) and
installs its packages into the library given by `--library` (default:
`.rvenv/lib`). Use `--r-binary` to select which R to build against
(default: `R`) and `--max-concurrent` to limit the number of simultaneous
installations (default: 4).
