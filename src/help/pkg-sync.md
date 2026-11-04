Install the packages of a lock file into a library

## Description

Install the packages of a lock file, written by
[`rig proj lock`](proj.qmd#rig-proj-lock), into a library. The lock file
defaults to `rproj.lock` in the current directory, but it can have any name:

```
rig pkg sync deploy.lock --library ./lib
```

Unlike [`rig proj sync`](proj.qmd#rig-proj-sync), it installs from the lock
file as it is, never removes a package from the library, and does not touch
the project environment. Use [`rig proj sync`](proj.qmd#rig-proj-sync) for
projects, and [`rig pkg install`](#rig-pkg-install) to install packages.

It uses the default R version, or the one given with `--r-version`, if the
lock file has a target for it, and otherwise the highest R version in the
lock file. `--library` works as for [`rig pkg install`](#rig-pkg-install),
the other options as for [`rig proj sync`](proj.qmd#rig-proj-sync).
