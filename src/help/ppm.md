Query Posit Package Manager (experimental)

## Description

Posit Package Manager information.

## PPM server

By default rig reports on the public PPM instance at
`https://packagemanager.posit.co`. Set the `PACKAGEMANAGER_ADDRESS` environment
variable to the base URL of your own PPM instance to report on that
instead. `rig ppm url` prints whichever one is in effect.

If your PPM instance needs authentication, log in with [`rig auth login`](auth.qmd#rig-auth-login), see
the [authentication guide](../auth.qmd).

For now `rig ppm builds` ignores this environment variable and reads a
package build index derived from the public instance.
