List the sources of repositories

## Description

A Package Manager repository is made of one or more sources: a CRAN mirror,
a Bioconductor mirror, a local source that packages are published to, a Git
source, etc.

With a repository name or id, list the sources of that repository. This
works without a token.

Without a repository, list all sources of the instance. This needs a
Package Manager token with API access, see the [authentication guide](../auth.qmd). The
public Package Manager does not allow it.

Besides the name, id and type of each source, the table shows these
columns, if at least one source has a value for them:

- `order`: the order in which the repository searches its sources.
- `bioc_version`: the Bioconductor version, for Bioconductor sources.
- `snapshot`: the CRAN snapshot a curated CRAN source is pinned to.
- `include`: which dependencies of the requested packages a curated source
  adds.
- `package_requests_enabled`: whether users can request packages for a
  curated source.
- `created`: when the source was created.
- `last_sync`: when the source was last synced.

Use `--json` to see every field that Package Manager reports.

## Examples

```sh
rig ppm sources cran
rig ppm sources
```
