List the snapshots of a repository

## Description

List the snapshots of the R and Bioconductor repositories of the instance,
or of the repository given as an argument, newest first, with their URLs. A
snapshot is the state of the repository on a given day, and its URL can be
used as a repository URL in R to always install the same package versions.

Repositories with several sources, or with a curated or Git source, have
no dated snapshot URLs, their `url` column is empty. Pin these with a
snapshot URL from Package Manager's UI. For Bioconductor repositories rig
uses the newest Bioconductor version, select another one with
`--bioc-version`.

Only the newest 20 snapshots are shown;
use `--limit` to show more, `--all` to show all of them, or `--since` to
show the ones on or after a date.

[`rig ppm url --snapshot`](#rig-ppm-url) prints the repository URL of a
snapshot, also for Linux binaries.

## Examples

```sh
rig ppm snapshots
rig ppm snapshots cran --since 2024-01-01 --all
```
