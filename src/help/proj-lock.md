Resolve project dependencies and write `rproj.lock`

## Description

Resolve the dependencies of an R project to a concrete set of package
versions, and write the result to `rproj.lock`.

rig reads the project manifest, `rproj.toml`, in the current directory, and
uses its built-in solver to find a compatible set of package versions from
the configured repositories.

`rig proj solve` does not run R.

Development dependencies are included by default. Use `--r-version` to solve
for a specific R version and `--no-dev` to leave out development
dependencies.

## The R version

Without `--r-version` rig solves for the default R version, provided the
manifest's own `R` requirement allows it. If it does not, rig takes the
newest installed R version that does, and failing that the current R
release. The version it picks does not have to be installed:
`rig proj lock` never runs R, and [`rig proj sync`](#rig-proj-sync)
installs the R version the lock file names.

## Source and binary packages

The solver considers binary packages as well as source packages, and
prefers a binary build when one is available for the same version.
By default a binary build never changes *which version* rig picks: the
newest suitable version wins, and a binary of it is used if there is one.
Use `--prefer-binary` to let an older version win instead, when the newest
one has no binary but an older one does, typically because a version was
released so recently that it has not been built yet. Only the three newest
versions of a package are considered; `--prefer-binary=5` considers five.
Versions held back this way are marked in the output.

By default rig solves for this machine plus the three other common
platforms (macOS arm64, Windows x86_64 and GNU Linux x86_64).
Use `--platform` to solve for a different set instead, e.g. a single
specific distro:

```sh
rig proj lock --platform ubuntu-24.04
```

`--platform source` solves for source packages only. rig also falls back
to source packages when there are no binaries for a platform at all.

rig keeps the repository metadata and the binary package indices it solves
from in its cache, and refreshes them once a day. Use `--no-cache`
to ignore the cache, or clean the cache with `rig cached clean`.

The `rproj.lock` file records, for every package, whether it is a source or
a binary package and the URL it is downloaded from. It also records where
the file is cached, which is per *build* rather than per version: a
repository can offer several binaries of one version for one platform and R
version, and they are cached side by side.

## Workspaces

A manifest with a `[workspace]` table is the root of a workspace: a monorepo
of several projects or packages, listed as path patterns in `members`, that
share one `rproj.lock` and one package library. `exclude` drops directories a
`members` pattern would otherwise match, and the root manifest is always a
member of its own workspace.

`rig proj lock` in a workspace (from the root or from any member directory)
reads every member and resolves them all in one solve, so that every member
ends up with the same version of every shared dependency, and writes one
`rproj.lock` at the workspace root. A member that depends on a sibling
member is resolved against that sibling's own dependencies. The members
themselves are directories rather than packages to download, so they are not
recorded in the lock file.

The R version rig solves for has to satisfy every member's `R` requirement,
not just the root's.

`[workspace.dependencies]` declares shared version requirements. A member
inherits one by name, instead of spelling out its own requirement:

```toml
# rproj.toml, the workspace root
[workspace]
members = ["packages/*"]

[workspace.dependencies]
cli = ">= 3.6.0"
```

```toml
# packages/mypkg/rproj.toml, a member
[dependencies]
cli = { workspace = true }
```

An entry no member inherits has no effect on the solve; it is a declaration,
not a request. Whether a member attaches a package (`attach`) is still the
member's own business, and is kept when the rest of the entry is inherited.

