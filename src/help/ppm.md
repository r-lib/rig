Query Posit Package Manager (experimental)

## Description

Query a Posit Package Manager instance: its status, the platforms and R
versions it builds for, its repositories, snapshots and sources.
[`rig ppm pkg`](#rig-ppm-pkg) has the commands about packages: listing,
searching, versions, builds, vulnerabilities and download counts, and
publishing a package.

## PPM server

By default rig reports on the public PPM instance at `https://p3m.dev`
(also known as `https://packagemanager.posit.co`). Set the
`PACKAGEMANAGER_ADDRESS` environment variable, or the `ppm-address`
configuration entry, to the base URL of your own PPM instance to report on
that instead. This also makes new R installations use the repositories of
that instance, see ["Your own Package Manager" in `rig repos
--help`](repos.qmd#rig-repos). `rig ppm url` prints whichever one is in
effect.

## Authentication

Most commands work without a token. Some need one on every instance:
listing all sources and publishing. Private instances may need one for
everything. rig sends a Package Manager token to the API from these, in
this order:

1. The `PACKAGEMANAGER_TOKEN` environment variable, for
   `PACKAGEMANAGER_ADDRESS`.
2. A single sign-on token from [`rig auth login`](auth.qmd#rig-auth-login),
   or from pak.
3. An identity token in the file `PACKAGEMANAGER_IDENTITY_TOKEN_FILE`, for
   `PACKAGEMANAGER_ADDRESS`, e.g. on CI.

[`rig ppm whoami`](#rig-ppm-whoami) shows which token rig uses. See the
[authentication guide](../auth.qmd) for details.

For now `rig ppm pkg builds` ignores `PACKAGEMANAGER_ADDRESS` and reads a
package build index derived from the public instance.
