## Description

Resolve R versions. Checks the version number of an R version (e.g.
release, devel, etc.), and looks up the URL of the installer for it,
if an installer is available.

It prints the R version number, and after a space the URL of the
installer. If no installer is available for this R version and the
current platform, the URL is `NA`.

An R version can be specified in various ways:

- `rig resolve devel` is the latest available development version,
- `rig resolve next` is the next (patched, alpha, beta, etc.) version,
- `rig resolve release` is the latest release.
- `rig resolve x.y.z` is a specific version.
- `rig resolve x.y` is the latest release within the `x.y` minor branch.
- `rig resolve oldrel/n` is the latest release within the `n`th previous
  minor branch (`oldrel` is the same as `oldrel/1`).

## Examples

```sh
# Latest development snapshot
rig resolve devel

# Latest release (that has an installer available)
rig resolve release

# URL for a specific version
rig resolve 4.1.2

# Latest version within a minor branch
rig resolve 4.1
```
