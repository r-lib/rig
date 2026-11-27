Print the Posit Package Manager URL, or a repository URL

## Description

Print the base URL of the Posit Package Manager instance the other [`rig
ppm`](ppm.qmd) commands report on, and nothing else, so it can be used
directly in a script:

```sh
curl "$(rig ppm url)/__api__/repos"
```

This is `https://p3m.dev` unless the `PACKAGEMANAGER_ADDRESS` environment
variable or the `ppm-address` configuration entry is set, in which case it
is that, with any trailing slash removed. The environment variable takes
precedence.

## Repository URLs

With `--repo`, `--snapshot` or `--platform`, print the URL of a repository
instead, to use as a package repository in R:

* `--repo`: the repository, by default the first R repository that
  [`rig ppm repos`](#rig-ppm-repos) lists.
* `--snapshot`: `latest` (the default), or a date. For a date without a
  snapshot, the latest snapshot before it is used. See [`rig ppm
  snapshots`](#rig-ppm-snapshots). Repositories with several sources, or
  with a curated or Git source, have no dated snapshots.
* `--platform`: on Linux, binary packages have their own URLs. This is a
  build target, e.g. `noble`, a rig platform string, or `current` for the
  current platform. macOS and Windows binaries use the plain repository URL.

## Examples

```sh
rig ppm url --snapshot 2024-01-02
rig ppm url --platform current
rig ppm url --repo cran --snapshot 2024-01-02 --platform noble
```
