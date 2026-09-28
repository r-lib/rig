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
concrete version rig picks is recorded in `rproj.lock`, so a project does not
need a version requirement for every dependency; use one when the project
really needs a particular version.

Because `@` and the comparison operators are meaningful to most shells, quote
a specification that contains a space or a `>` character, as in the examples
above.

## Git, GitHub, GitLab and URL sources

A package can also be added straight from a git repository, [pak's package
reference](https://pak.r-lib.org/reference/pak_package_sources.html) syntax:

```
rig proj add r-lib/crayon
rig proj add r-lib/crayon@84be6207
rig proj add r-lib/crayon@some-branch
rig proj add r-lib/crayon#41
rig proj add r-lib/crayon@*release
rig proj add gitlab::group/project@main
rig proj add 'git::https://gitlab.com/example/pkg.git@main'
rig proj add https://cran.rstudio.com/src/contrib/processx_3.9.0.tar.gz
```

A bare `<owner>/<repo>` (optionally `github::<owner>/<repo>`) is a GitHub
reference; `<owner>/<repo>/<subdir>` points at a package in a subdirectory of
the repository. After the path, `@<ref>` pins a branch, tag or commit,
`#<pr>` a pull request, and `@*release` the latest release.

`gitlab::<group>/<project>` is the same idea for GitLab, including nested
subgroups (`gitlab::<group>/<subgroup>/<project>`); `@<ref>` pins a branch,
tag or commit, and `/-/<subdir>` points at a subdirectory. Merge requests and
`@*release` are not supported for GitLab. A self-hosted instance is
`gitlab::<https-url-of-group-and-project>`, e.g.
`gitlab::https://gitlab.example.com/group/project`.

A `git::<url>` reference works with any git host, not only GitHub or GitLab.

`url::<https-url>` (or a bare `<https-url>`) points straight at a
package source archive.

The package name is read from the fetched repository's (or archive's) own
`DESCRIPTION` (which may differ from the repository name), and the
dependency is pinned by commit, or by the archive's sha256 for a `url::`
source, not by version range: `rig proj add` resolves the reference right
away, and `rproj.lock` records it.

rig fetches a git/GitHub/GitLab source with the system `git`, which must be
installed and on `PATH`. A private repository authenticates exactly the way
a plain `git clone` would on your machine: a configured credential helper
(Keychain, Windows Credential Manager, `git credential-store`, etc.),
`.netrc`, an SSH agent, or credentials already embedded in the URL. There is
no separate rig-specific token setting. A `url::` source is a plain HTTP(S)
download and needs no such authentication.

## Local sources

A package that is already on this machine, in a source directory or a
package file, can be added straight from there:

```
rig proj add ../mypkg
rig proj add ~/works/mypkg
rig proj add mypkg_1.0.0.tar.gz
rig proj add local::mypkg
```

The path is recorded in `rproj.toml` relative to the project's own directory
(not the directory `rig proj add` was run from), so the manifest stays
correct after the project is moved or checked out elsewhere, as long as the
local package stays in the same place relative to it.

## Version requirements

* `^1.2.3` is *compatible with* 1.2.3, i.e. `>= 1.2.3, < 2.0.0`.
* `1.2.3`, a bare version, means the same as `^1.2.3`.
* `~1.2.3` is `>= 1.2.3, < 1.3.0`.
* `>= 1.2`, `> 1.2`, `<= 2.0`, `< 2.0` and `= 1.2.3` are a single bound.
* `>= 1.0, < 2.0`: a comma means *and*, so both bounds hold.
* `*` is any version.

A bare version is written into the manifest in its explicit `^` spelling, so
the file reads the same way whether or not you know that a bare version
means *compatible with*.

The caret and tilde forms bump one component of the version and zero the
ones after it: the leftmost non-zero component for `^` (`^0.2.3` is `>= 0.2.3, <
0.3.0`), the second component for `~`. R versions can have any number of
components, so `^1.1.0.9000` is `>= 1.1.0.9000, < 2.0.0.0`.

## Options

`--dev` adds the packages as development dependencies, into the
`[dependency-groups.dev]` table instead of `[dependencies]`. These are
installed by default, and left out by `rig proj sync --no-dev`.

`--no-sync` updates `rproj.toml` and `rproj.lock`, but does not install anything.

`--no-lock` only updates `rproj.toml`. Nothing is resolved or installed, so
this also works offline.

## Files

Only `rproj.toml` is edited. Changes are merged into the existing file, so
comments and custom formatting are preserved where possible.

If resolving the dependencies fails (most often because a package name is
misspelled, and no repository has such a package) `rproj.toml` is restored to
what it was, so a failed `rig proj add` does not leave the project with a
dependency that cannot be installed.
