Solve project dependencies

## Description

Resolve the dependencies of an R project to a concrete set of package
versions.

rig reads the project manifest (e.g. `DESCRIPTION`; override with
`--input`) and uses its built-in solver to find a compatible set of
package versions from the configured repositories, without running R.

Use `--r-version` to solve for a specific R version, `--dev` to include
development dependencies, and `--renv` to write the result as an
`renv.lock` file.

## Source and binary packages

The solver considers binary packages as well as source packages, and
prefers a binary build when one is available for the same version. Which
artifact each package is installed from is part of what the solve decides,
because a binary is only usable together with the exact versions of its
`LinkingTo` dependencies that it was compiled against. If those versions
conflict with the rest of the project, rig picks another build of that
package, or falls back to its source tarball.

By default rig solves for the machine it runs on. Use `--platform` to solve
for a different one, e.g. to write a lockfile on macOS for a Linux
deployment:

```sh
rig proj solve --platform ubuntu-24.04
```

`--platform source` solves for source packages only, and does not download
any binary package metadata. rig also falls back to source packages when
there are no binaries for a platform at all.

The `pkg.lock` file records, for every package, whether it is a source or a
binary package and the URL it is downloaded from.
