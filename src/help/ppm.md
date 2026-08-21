Query Posit Package Manager (experimental)

## Description

Ask Posit Package Manager (P3M) what it offers: which platforms and R
versions it builds binary packages for, and which builds exist for a
package. Nothing here changes anything on your machine.

[`rig ppm platforms`](#rig-ppm-platforms) and
[`rig ppm r-versions`](#rig-ppm-r-versions) list the build targets and R
versions, [`rig ppm status`](#rig-ppm-status) shows P3M's whole status
report, [`rig ppm builds`](#rig-ppm-builds) lists the published builds of
one package, and [`rig ppm url`](#rig-ppm-url) prints the URL rig is
talking to.

This is about P3M itself. To manage the repositories configured for your R
installations, including P3M ones, use [`rig repos`](repos.qmd); to look up
package metadata in those repositories, use [`rig pkg`](pkg.qmd).

## Which server

By default rig reports on the public instance,
`https://packagemanager.posit.co`. Set the `PACKAGEMANAGER_ADDRESS`
environment variable to the base URL of your own P3M instance to report on
that instead. `rig ppm url` prints whichever one is in effect.

One command is different: `rig ppm builds` reads a package build index that
rig publishes itself, derived from P3M, because P3M has no endpoint that
lists a package's builds. That index always comes from rig's own host, and
`PACKAGEMANAGER_ADDRESS` does not redirect it.
