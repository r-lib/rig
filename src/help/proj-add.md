Add dependencies to `rproj.toml`

## Description

Add one or more R packages to `rproj.toml`, rig's project and package
manifest, then update `rproj.lock` and install the new packages into the
project library. Adding a package that the manifest already lists updates
its version requirement.

A package is given as `<package>` or `<package>@<version>`:

```
rig proj add dplyr
rig proj add dplyr@1.1.0
rig proj add 'cli@>= 3.6' 'rlang@>= 1.0, < 2.0'
```

Without a version, the package is added as `"*"`, i.e. any version. The
concrete version rig picks is recorded in `rproj.lock`, so a project does
not need a version requirement for every dependency; use one when the
project really needs a particular version.

Because `@` and the comparison operators are meaningful to most shells,
quote a specification that contains a space or a `>` character, as in the
examples above.

## Version requirements

* `^1.2.3` is *compatible with* 1.2.3, i.e. `>= 1.2.3, < 2.0.0`.
* `1.2.3`, a bare version, means the same as `^1.2.3`.
* `~1.2.3` is `>= 1.2.3, < 1.3.0`.
* `>= 1.2`, `> 1.2`, `<= 2.0`, `< 2.0` and `= 1.2.3` are a single bound.
* `>= 1.0, < 2.0`: a comma means *and*, so both bounds hold.
* `*` is any version.

A bare version is written into the manifest in its explicit `^` spelling,
so the file reads the same way whether or not you know that a bare version
means *compatible with*.

The caret and tilde forms bump one component of the version and zero the
ones after it: the leftmost non-zero component for `^` (`^0.2.3` is
`>= 0.2.3, < 0.3.0`), the second component for `~`. R versions can have any
number of components, so `^1.1.0.9000` is `>= 1.1.0.9000, < 2.0.0.0`.

## Options

`--dev` adds the packages as development dependencies, into the
`[dependency-groups.test]` table instead of `[dependencies]`. These are
installed by default, and left out by `rig proj lock --no-dev` and
`rig proj sync --no-dev`.

`--no-sync` updates `rproj.toml` and `rproj.lock`, but does not install
anything.

`--no-lock` only updates `rproj.toml`. Nothing is resolved or installed, so
this also works offline.

## Files

Only `rproj.toml` is edited, and it is rewritten in full, so any comments or
custom formatting in it are not preserved.

If resolving the dependencies fails (most often because a package name is
misspelled, and no repository has such a package) `rproj.toml` is restored
to what it was, so a failed `rig proj add` does not leave the project with a
dependency that cannot be installed.
