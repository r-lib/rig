Show the sync log of the sources of a repository

## Description

Show the most recent changes of the sources of the R and Bioconductor
repositories of the instance, or of the repository given as an argument:
when they were synced, and how many packages were added, updated, archived
and removed.

The changes of all sources are shown together, newest first, with the source in the `source` column. Select
a single source with `--source`, by name or id, see [`rig ppm
sources`](#rig-ppm-sources). The newest 20 changes are shown, use `--limit`
for more.

A Bioconductor repository has a separate log for each Bioconductor version.
rig shows the newest version by default, select another one with
`--bioc-version`. The `bioc_version` column shows the version, it is empty
for other sources.

## Examples

```sh
rig ppm transactions
rig ppm transactions cran --limit 5
rig ppm transactions all --source github
rig ppm transactions bioconductor --bioc-version 3.21
```
