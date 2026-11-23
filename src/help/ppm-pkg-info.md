Show a package of a repository

## Description

Show the current version of a package in a repository: its title, license,
dependencies, system requirements, size, known vulnerabilities and archived
versions.

Without `--repo`, rig tries the R and Bioconductor repositories of the
instance in the order [`rig ppm repos`](#rig-ppm-repos) lists them, and
shows the package from the first one that has it. Hidden repositories are
skipped. The output shows the repository used.

For a Bioconductor repository rig uses the newest Bioconductor version, select
another one with `--bioc-version`.

`--json` prints the full record Package Manager sends, except for the
`links` and `readme_data` fields, which are very large.

See [`rig ppm pkg history`](#rig-ppm-pkg-history) for the versions of the package in
past snapshots.

## Examples

```sh
rig ppm pkg info dplyr
rig ppm pkg info dplyr --json
rig ppm pkg info limma --repo bioconductor
rig ppm pkg info limma --repo bioconductor --bioc-version 3.21
rig ppm pkg info mypkg --repo internal
```
