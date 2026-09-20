Show cache size and contents

## Description

Prints the total size of rig's cache directory, broken down into
categories: packages rig built from source, downloaded package files,
package metadata (binary package indexes, CRAN-like package databases,
package manifests, and repository data), the cached PPM status document,
the persistent git mirrors kept for git/GitHub dependency resolution
(`rig proj lock`), and the downloaded archives kept for `url` dependency
resolution.

Use `--json` for machine-readable output.
