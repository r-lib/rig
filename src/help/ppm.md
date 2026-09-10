Query Posit Package Manager (experimental)

## Description

Posit Package Manager information.

## PPM server

By default rig reports on the public PPM instance at
`https://packagemanager.posit.co`. Set the `PACKAGEMANAGER_ADDRESS` environment
variable to the base URL of your own PPM instance to report on that
instead. `rig ppm url` prints whichever one is in effect.

For now `rig ppm builds` ignores this environment variable and reads a
package build index derived from the public instance.
