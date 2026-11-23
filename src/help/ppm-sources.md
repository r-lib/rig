List the sources of repositories

## Description

A Package Manager repository is made of one or more sources: a CRAN mirror,
a Bioconductor mirror, a local source that packages are published to, a Git
source, etc.

With a repository name or id, list the sources of that repository. This
works without a token.

Without a repository, list all sources of the instance. This needs a
Package Manager token with API access, see the [authentication
guide](../auth.qmd). The public Package Manager does not allow it.

## Examples

```sh
rig ppm sources cran
rig ppm sources
```
