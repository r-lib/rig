List the published builds of a package

## Description

List every source and binary artifact Posit Package Manager has published
for one R package. Use `--version` to restrict the listing to a single
package version.

The date is extracted from Posit Public Package Manager and it is not
affected by the `PACKAGEMANAGER_ADDRESS` environment variable.

## The columns

* `version` — the package version, as published.

* `platform` — `source` for the CRAN source tarball, otherwise the build
  target: `macos`, `windows`, or a Linux target name such as `jammy`.
  [`rig ppm platforms`](ppm.qmd#rig-ppm-platforms) lists the target names.

* `arch`, `r_version` — the architecture and minor R version the binary is for.
  Both are `*` on a source row, which is architecture- and
  version-independent.

* `linkingto` — the package versions the binary was compiled against, for
  packages with a `LinkingTo:` field. This column is what tells otherwise
  identical rows apart. PPM republishes a binary when a compiled-against
  dependency changes, so the same version, platform, architecture and R
  version can legitimately have several builds; `linkingto` is the only
  difference between them.

* `url` — where to download that artifact. The date in the URL is the CRAN
  snapshot the build was published against.

`--json` output adds a `sha256` for each row, and for each `linkingto` entry: it
is the hash of the original CRAN source tarball, and not the hash what the
URL serves.

## Examples

```sh
# Every build of a package, latest version last
rig ppm builds cli

# Just one version
rig ppm builds dplyr --version 1.1.4

# The builds for one R version and platform
rig ppm builds dplyr --json |
  jq '.[] | select(.r_version == "4.5" and .platform == "jammy")'
```
