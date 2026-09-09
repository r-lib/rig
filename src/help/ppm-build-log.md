Show the P3M build log for a package

## Description

Fetch Posit Package Manager's build log for one CRAN package: exit code,
timings, system dependencies, and the compiler/build output. It is the
diagnosis tool for "why is there no binary for this package on this
platform" — a non-zero exit code or a missing system dependency in the log
usually explains it.

`--platform` and `--arch` name the build target to look up: `--platform` is
a P3M target name such as `macos`, `windows`, or a Linux codename like
`jammy` (run [`rig ppm platforms`](ppm.qmd#rig-ppm-platforms) or
[`rig ppm builds`](ppm.qmd#rig-ppm-builds) to see the names P3M uses). Left
unset, both default to the machine rig is running on. `--r-version` defaults
to rig's default R version (`rig default`), trimmed to a minor version
(`4.6.1` becomes `4.6`) since that is all P3M accepts; pass it explicitly to
ask about a different one. `--version` restricts the log to one package
version; without it P3M reports the log for the latest version it knows.

P3M has no way to ask for the log of a specific *historical* rebuild: the
log it serves is always for the current dependency snapshot of the
requested distribution/architecture/R version/package version, recomputed
on every request. So the same command run today and in a month can report
different logs for what looks like the same build, if a dependency of the
package has since changed. There is no flag to pin an older snapshot — P3M
itself has no such endpoint.

## Examples

```sh
# The current platform, architecture and default R version
rig ppm build-log pak

# A specific target and package version
rig ppm build-log dplyr --platform jammy --r-version 4.5 --version 1.1.4

# Pick a target straight from `rig ppm builds`
rig ppm builds dplyr --json | jq '.[0] | {platform, arch, r_version}'
```
