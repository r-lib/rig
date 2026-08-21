Check the configured R package repositories

## Description

Check the package repositories configured for an R version: how quickly
each one responds, whether it responds at all, what kinds of packages it
serves, and how fresh its package index is.

rig checks the same repositories that [`rig repos list`](repos.qmd#rig-repos-list)
shows, in the same order. By default these are the repositories of the
default R version; use `--r-version` to pick another one. Add `--all` to
include repositories that are not enabled by default.

## The report

For each repository rig prints:

* `ping` — how long the repository took to answer a request for the
  package index. rig asks for the very index R would use on this machine,
  for the selected R version, so this is the latency you pay when
  installing a package. It is a `HEAD` request: the index itself is not
  downloaded, and the time does not depend on how large it is.

* `status` — `ok` if the index is there. `source only` means the
  repository has no index for this platform and R version, but it does
  have source packages. Anything else is the HTTP status code the server
  returned, or why it returned nothing at all, e.g. `timeout` or
  `cannot connect`.

* `types` — the package types the R installation's `repositories` file
  declares for the repository: `source`, `win` (Windows binaries) and
  `mac` (macOS binaries). These are declarations, not measurements; the
  `status` column is what says whether the index for *this* platform is
  really there.

* `updated` — the `Last-Modified` date of the package index, i.e. how
  fresh the repository's view of the packages is.

* `url` — the repository URL, with the Bioconductor `%v` and `%bm`
  variables resolved. Use `--raw` to print the URLs unresolved; rig still
  resolves them internally, since an unresolved URL cannot be checked.

A repository whose `types` column ends in `*` serves prebuilt **Linux
binaries** as source packages. Linux binaries have no package type of
their own in R, so a Posit Package Manager repository offers them from a
distribution-specific URL instead, and rig recognizes those URLs.

The repositories are checked in parallel, so the command takes about as
long as the slowest repository, not as long as all of them together. An
unreachable repository is reported in its row; it does not fail the
command.

## Examples

```sh
# Check the repositories of the default R version
rig repos status

# Include repositories that are not enabled by default
rig repos status --all

# Check the repositories of another R version
rig repos status --r-version 4.4.1

# Machine readable output, with the exact URLs, statuses and timings
rig repos status --json
```
