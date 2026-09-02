Install the dependencies rproj.lock resolved

## Description

Install the resolved dependencies of an R project into a package library.

rig looks for the project in the current directory and its parents, reads its
`rproj.lock` (written by [`rig proj lock`](#rig-proj-lock)) and installs the
packages into the project library, `.rvenv/lib`. That library is created by
[`rig proj init`](#rig-proj-init), together with the `.gitignore` files that
keep it in version control, so `rig proj sync` fails if it is missing. Pass
`--library` to install somewhere else instead.

Use `--r-binary` to select which R to build against (default: `R`) and
`--max-concurrent` to limit the number of simultaneous installations
(default: 8).

After a successful sync rig records the lock file it installed from in
`.rvenv/lib/.synced`. The `rig` package in the project library compares the
two, and warns in every R session while the project library does not match
`rproj.lock`.
