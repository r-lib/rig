Resolve project dependencies and write rproj.lock

## Description

Resolve the dependencies of an R project to a concrete set of package
versions, and write the result to `rproj.lock`.

rig reads the project manifest, `rproj.toml`, in the current directory, and
uses its built-in solver to find a compatible set of package versions from
the configured repositories, without running R.

Use `--r-version` to solve for a specific R version, `--dev` to include
development dependencies, and `--renv` to also write the result as an
`renv.lock` file.

`rproj.lock` currently records a single `(R version, platform)` target;
[`rig proj sync`](#rig-proj-sync) installs that target. Solving for several
targets in one lockfile is planned but not implemented yet.

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

By default rig solves for the machine it runs on. Use `--platform` to solve
for a different one, e.g. to write a lockfile on macOS for a Linux
deployment:

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
