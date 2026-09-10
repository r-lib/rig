Show the PPM build log for a package

## Description

Fetch Posit Package Manager's build log for one CRAN package.

`--platform` and `--arch` name the build target to look up: `--platform` is
a PPM target name such as `macos`, `windows`, or a Linux codename like
`jammy` (run [`rig ppm platforms`](ppm.qmd#rig-ppm-platforms) or
[`rig ppm builds`](ppm.qmd#rig-ppm-builds) to see the names PPM uses).

`--r-version` defaults to rig's default R version (`rig default`), trimmed
to a minor version (`4.6.1` becomes `4.6`) since that is all PPM accepts;
pass it explicitly to ask about a different one. `--version` restricts the
log to one package version; without it PPM reports the log for the latest
version it knows.

Currently PPM has no way to ask for the log of a specific historical build.
If package version has multiple builds, then the log of the last one is
shown.

## Examples

```sh
# The current platform, architecture and default R version
rig ppm build-log pak

# A specific target and package version
rig ppm build-log dplyr --platform noble --r-version 4.6 --version 1.2.1
```
