Remove dependencies from `rproj.toml`

## Description

Remove one or more R packages from `rproj.toml`, rig's project and package
manifest, then update `rproj.lock` and the project library to match. A
package is removed wherever it is listed: `[dependencies]`,
`[linking-dependencies]`, or any `[dependency-groups.*]` table.

Naming a package that is not a dependency in `rproj.toml` is an error, and
none of the named packages are removed if any of them is not found, so a
typo cannot silently remove the wrong set of packages.

## Options

`--no-sync` updates `rproj.toml` and `rproj.lock`, but does not touch the project
library.

`--no-lock` only updates `rproj.toml`. Nothing is resolved or installed, so
this also works offline.

## Files

Only `rproj.toml` is edited, and it is rewritten in full, so any comments or
custom formatting in it are not preserved.

If resolving the remaining dependencies fails, `rproj.toml` is restored to
what it was, so a failed `rig proj remove` does not leave the project with a
manifest that cannot be locked.
