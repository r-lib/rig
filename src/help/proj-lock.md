Resolve project dependencies and write `rproj.lock`

## Description

Resolve the dependencies of an R project to a concrete set of package
versions, and write the result to `rproj.lock`.

rig reads the project manifest, `rproj.toml`, in the current directory, and
uses its built-in solver to find a compatible set of package versions from
the configured repositories.

`rig proj solve` does not run R.

Use `--r-version` to solve for a specific R version.

## Optional dependencies and dependency groups

Every `[dependency-groups.*]` table (`dev`, `enhances`, or any other name)
and every `[optional-dependencies.*]` extra are optional dependencies:
packages the project suggests or can take advantage of, but does not need to
run. `rig proj lock` always solves all of them together with the project's
hard dependencies, in one solve, so `rproj.lock` is complete -- a version
picked for a shared package is the same whether it got pulled in as a hard
or an optional dependency, and every group and extra is available to install
without a new solve. A `[dependency-groups.*]` table can also `include-groups
= [...]` other groups, pulling in their packages too; `rig proj lock` follows
this when solving, and rejects a cycle (a group that includes itself,
directly or through others).

[`rig proj sync`](#rig-proj-sync) is where a subset of this is picked for
installation -- by default `main` plus the `dev` group, more with
`--group`/`--all-groups`/`--extra`/`--all-extras`.

## The R version

Without `--r-version` rig solves for the default R version, provided the
manifest's own `R` requirement allows it. If it does not, rig takes the
newest installed R version that does, and failing that the current R
release. The version it picks does not have to be installed: `rig proj lock`
never runs R, and [`rig proj sync`](#rig-proj-sync) installs the R version the lock file names.

## Bioconductor packages

rig solves from CRAN and from the Bioconductor release that belongs to the
target's R version, e.g. Bioconductor 3.23 for R 4.6. You do not need to set
up anything: `rig proj add limma` works like `rig proj add dplyr`. Some R
versions have two Bioconductor releases, e.g. 3.21 and 3.22 for R 4.5, and
rig uses the newer one. With `--exclude-newer` it uses the release that was
current on the cutoff day, see below.

A `bioc` entry in `[[repository]]` pins the Bioconductor release, e.g. to use
the development version, or turns Bioconductor off:

```toml
[[repository]]
name = "bioc"
version = "3.24"
```

```toml
[[repository]]
name = "bioc"
enabled = false
```

The `bioc` entry has no `url`. The `R_BIOC_VERSION` environment variable
also pins the release, and `RIG_BIOCONDUCTOR=false` turns Bioconductor off
for every project.

`rproj.lock` records the Bioconductor release of each target as
`bioc-version`, and `repository = "bioc/<version>"` for each Bioconductor
package. A target solved with another release is solved again.
[`rig proj sync`](#rig-proj-sync) also adds the Bioconductor repositories of
that release to the project environment, so `install.packages()` finds
Bioconductor packages, too.

## Source and binary packages

The solver considers binary packages as well as source packages, and
prefers a binary build when one is available for the same version. By
default a binary build never changes *which version* rig picks: the newest
suitable version wins, and a binary of it is used if there is one. Use
`--prefer-binary` to let an older version win instead, when the newest one
has no binary but an older one does, typically because a version was
released so recently that it has not been built yet. Only the three newest
versions of a package are considered; `--prefer-binary=5` considers five.
Versions held back this way are marked in the output.

To make it part of the project, set it in `rproj.toml`. `true` means the
default of three versions, a number gives the number of versions:

```toml
[tool.rig]
prefer-binary = true
```

`--prefer-binary` overrides it, and `--prefer-binary=0` turns it off.
`rproj.lock` records the setting in its `[tool.rig]` table, and a lock solved
with a different one is solved again, keeping the pinned versions where they
fit, see "Sticky lock files" below.

By default rig solves for this machine plus the three other common
platforms (macOS arm64, Windows x86_64 and GNU Linux x86_64), and for
source packages only, so the lock file also works on platforms without
binary packages. `rig proj sync` only uses the source target if no other
target matches the machine. Use
`--platform` to solve for a different set instead, e.g. a single specific
distro:

```sh
rig proj lock --platform ubuntu-24.04
```

Use `--add-platform` instead to add a platform to that default set rather
than replacing it, e.g. to also solve for one extra distro on top of the
usual five. `--add-platform` can be repeated:

```sh
rig proj lock --add-platform ubuntu-24.04 --add-platform linux-fedora-42
```

`--platform`/`--add-platform` accept:

- `macos-arm64`, `windows-x86_64` -- an OS plus arch, for the two
  non-Linux platforms.
- `ubuntu-24.04`, `fedora-42`, `opensuse-15.6` -- a Linux distro and version,
  matched against P3M's build list.
- `manylinux_2_28-arm64`, `jammy-x86_64` -- a P3M platform name and arch
  directly, e.g. copied from another `rproj.lock`'s `platform` field.
- `source` -- source packages only, for any platform.

A Linux distro/version or platform name P3M has no specific build for falls
back to its generic manylinux build for the given arch, rather than failing.

rig also falls back to source packages when there are no binaries for a
platform at all.

rig keeps the repository metadata and the binary package indices it solves
from in its cache, and refreshes them once a day. Use `--no-cache` to ignore
the cache, or clean the cache with `rig cached clean`.

## Sticky lock files

A `rig proj lock` run that finds an existing `rproj.lock` already satisfying
`rproj.toml` reuses it as-is, for every package, instead of re-resolving
anything. This applies to ordinary dependencies, an existing pin that still
satisfies the manifest's version requirement is kept, even if a newer
version has since been published, as well as to git/GitHub dependencies (see
below).

If `rproj.toml` changed, e.g. after `rig proj add`, rig solves the
dependencies again, but it still keeps the versions `rproj.lock` pins, as
long as they fit. So adding a package only changes the versions it needs
changed, and does not upgrade the rest of the project. A platform or R
version that is new to the lock file gets the same versions as the other
platforms of the same R version, where possible.

Use `rig proj lock --upgrade` to ignore the existing lock file and
re-resolve every dependency instead, picking the latest version that still
satisfies `rproj.toml`.

Use `--upgrade-package` to upgrade only some packages, and keep the rest:

```sh
rig proj lock --upgrade-package cli
rig proj lock -P cli,glue
```

The packages they depend on are only upgraded if they need to be. You can
also give a version requirement, with the same `<package>@<version>` syntax
as `rig proj add`, e.g. to upgrade or downgrade to a specific version:

```sh
rig proj lock -P 'cli@>= 3.6.4'
rig proj lock -P 'cli@=3.6.2'
```

The requirement only applies to this run, it is not written to
`rproj.toml`. Later `rig proj lock` runs keep the version it picked, as
long as it fits `rproj.toml`. A git/GitHub dependency named in
`--upgrade-package` is checked against its remote again, like with
`--upgrade`.

## Excluding newer package versions

`--exclude-newer` makes the solver ignore CRAN and Bioconductor package
versions published after a date, so you can lock the project as the
repositories were on that day, or skip versions released in the last few
days:

```sh
rig proj lock --exclude-newer 2025-06-01
rig proj lock --exclude-newer "7 days"
```

It takes a date (`2025-06-01`), an RFC 3339 timestamp (only its UTC date is
used), or a span back from today, e.g. `7 days`, `2 weeks` or `P1M`. A
version's publication date is the day it first appeared in a Posit Package
Manager snapshot, so the cutoff is a day, and it cannot be earlier than
2017-10-10, the first snapshot. It only applies to CRAN and Bioconductor
packages, not to git, GitHub, URL or local dependencies, and binary builds
of a version are used no matter when they were built. If the target's R
version has two Bioconductor releases, the cutoff also chooses between them:
the newer one if it was released by the cutoff day, otherwise the older one.

To make it part of the project, set it in `rproj.toml`; `--exclude-newer`
overrides it:

```toml
[tool.rig]
exclude-newer = "2025-06-01"
```

`rproj.lock` records the cutoff in its own `[tool.rig]` table, and a lock
solved with a different one is solved again. This keeps the pinned versions
that are not newer than the new cutoff, see "Sticky lock files" above. For a span, the lock records the
span as well, and the lock is reused as long as the span stays the same, so
the cutoff does not move every day. Use `--upgrade` to solve again with a
fresh cutoff.

## Git, GitHub and URL dependencies

A `git::`/`github::` dependency pinned to a branch, a pull request, or no ref
at all (the default branch's tip) is only resolved against its remote the
first time it's locked. Once `rproj.lock` records a commit for it, later
`rig proj lock` runs reuse that commit as-is rather than re-checking whether
the branch moved on every lock. `rig proj lock --upgrade` re-checks every
git/GitHub dependency's ref and moves the pin forward if it changed. A
`rev`/`tag` pins an exact commit already, so there's nothing for `--upgrade`
to move. `release = true` is sticky the same way: once locked, later runs
keep the release it pinned instead of asking GitHub which release is latest
every time. `rig proj lock --upgrade` re-checks and moves the pin forward if
a newer release exists.

A `url::` dependency names one exact archive rather than a movable ref, so
there's nothing for `--upgrade` to move either: every `rig proj lock` run
downloads it (the download itself is cached) and records its sha256 in
`rproj.lock`, which then changes only if the archive's contents change. Set
`hash` on a `url` dependency to pin the expected sha256 and catch that.

The `rproj.lock` file records, for every package, whether it is a source or a
binary package and the URL it is downloaded from. It also records where the
file is cached, which is per *build* rather than per version: a repository
can offer several binaries of one version for one platform and R version,
and they are cached side by side.

## Scripts

`rig proj lock --script script.R` locks the dependencies in the
`# /// script` block of an R script, instead of a project, and writes them
to `script.R.lock`, next to the script. It takes the same options as for a
project, and an existing `script.R.lock` is sticky in the same way.
`rig run script.R` then installs the versions the lock file names, see
[`rig run`](run.qmd).

## Workspaces

A manifest with a `[workspace]` table is the root of a workspace: a monorepo
of several projects or packages, listed as path patterns in `members`, that
share one `rproj.lock` and one package library. `exclude` drops directories a
`members` pattern would otherwise match, and the root manifest is always a
member of its own workspace.

`rig proj lock` in a workspace (from the root or from any member directory)
reads every member and resolves them all in one solve, so that every member
ends up with the same version of every shared dependency, and writes one
`rproj.lock` at the workspace root. A member that depends on a sibling member
is resolved against that sibling's own dependencies. The members themselves
are directories rather than packages to download, so they are not recorded
in the lock file.

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

An entry no member inherits has no effect on the solve; it is a
declaration, not a request. Whether a member attaches a package (`attach`) is
still the member's own business, and is kept when the rest of the entry is
inherited.

