Resolve project dependencies and write rproj.lock

## Description

Resolve the dependencies of an R project to a concrete set of package
versions, and write the result to `rproj.lock`.

rig reads the project manifest, `rproj.toml`, in the current directory, and
uses its built-in solver to find a compatible set of package versions from
the configured repositories, without running R.

Development dependencies are included by default. Use `--r-version` to solve
for a specific R version and `--no-dev` to leave out development
dependencies. See [`rig proj renv export`](proj.qmd#rig-proj-renv-export) to also
write an `renv.lock` file.

`--r-version` and `--platform` each take a comma-separated list, to solve for
several R versions and/or platforms in one `rproj.lock` file — rig solves the
cross product of every version given against every platform given, and
writes one target per combination. For example:

```sh
rig proj lock --r-version 4.5,4.6
rig proj lock --platform macos,ubuntu-24.04
```

Without `--platform`, rig locks for four platforms at once: this machine,
Windows, a generic glibc Linux build (P3M's distro-independent "manylinux"
build, which covers any glibc-based x86_64 distro P3M has no specific build
for), and macOS on arm64 — the common set of platforms a project needs to
run on beyond the machine it was locked on. Pass `--platform` to lock for a
different set instead, e.g. a single platform.

[`rig proj sync`](#rig-proj-sync) then picks the target whose platform
matches the OS it runs on (the highest R version among them if more than one
matches), so this default already covers deploying to a Linux server or CI
from a macOS or Windows laptop: `rig proj sync` on each machine picks its own
entry from the same file.

## Workspaces

A manifest with a `[workspace]` table is the root of a workspace: a monorepo
of several projects or packages, listed as path patterns in `members`, that
share one `rproj.lock` and one package library. `exclude` drops directories a
`members` pattern would otherwise match, and the root manifest is always a
member of its own workspace.

`rig proj lock` in a workspace — from the root or from any member directory —
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

## The R version

Without `--r-version` rig solves for the default R version, provided the
manifest's own `R` requirement allows it. If it does not, rig takes the newest
installed R version that does, and failing that the current R release. The
version it picks does not have to be installed: `rig proj lock` never runs R,
and [`rig proj sync`](#rig-proj-sync) installs the R version the lock file
names.

## Source and binary packages

The solver considers binary packages as well as source packages, and
prefers a binary build when one is available for the same version. Which
artifact each package is installed from is part of what the solve decides,
because a binary is only usable together with the exact versions of its
`LinkingTo` dependencies that it was compiled against. If those versions
conflict with the rest of the project, rig picks another build of that
package, or falls back to its source tarball.

By default a binary build never changes *which version* rig picks: the
newest suitable version wins, and a binary of it is used if there is one.
Pass `--prefer-binary` to let an older version win instead, when the newest
one has no binary but an older one does — typically because a version was
released so recently that it has not been built yet. Only the three newest
versions of a package are considered; `--prefer-binary=5` considers five.
Versions held back this way are marked in the output.

Trading a version away for a binary is not free: the binary pins its
`LinkingTo` dependencies to the versions it was compiled against, and those
dependencies then prefer their own binaries in turn, so a whole project can
end up on older versions.

By default rig solves for this machine plus three other platforms (see
above). Use `--platform` to solve for a different set instead, e.g. a single
specific distro:

```sh
rig proj lock --platform ubuntu-24.04
```

`--platform source` solves for source packages only, and does not download
any binary package metadata. rig also falls back to source packages when
there are no binaries for a platform at all. There is then nothing for
`--prefer-binary` to prefer, and rig ignores it.

rig keeps the repository metadata and the binary package indices it solves
from in its cache, and refreshes them once a day. `--no-cache` downloads
them again instead, and writes nothing to the cache, which is the way to
solve against a package that was published minutes ago. It is a good deal
slower, because the metadata it re-downloads is large. See
[`rig config`](config.qmd).

The `rproj.lock` file records, for every package, whether it is a source or a
binary package and the URL it is downloaded from. It also records where the
file is cached, which is per *build* rather than per version: a repository
can offer several binaries of one version for one platform and R version,
and they are cached side by side.
