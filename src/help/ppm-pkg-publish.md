Publish a package to Package Manager

## Description

Upload a package to a local source of a Package Manager instance. This
needs a token with the `sources:write` scope, see the [authentication
guide](../auth.qmd).

The package is a source package file (`.tar.gz`), or a package directory,
the current directory by default. A directory is built with `R CMD build`,
with the default R version.

To upload a binary package, add `--binary` and `--distro`, the platform the
binary was built for, e.g. `jammy`.

`--overwrite` replaces the package if the same version is already in the
source.

## Token

Needed, with the `sources:write` scope.

## Examples

```sh
export PACKAGEMANAGER_TOKEN=...
rig ppm pkg publish --source internal
rig ppm pkg publish mypkg_1.0.0.tar.gz --source internal --overwrite
rig ppm pkg publish mypkg_1.0.0_R_x86_64-pc-linux-gnu.tar.gz --source internal --binary --distro jammy
```
