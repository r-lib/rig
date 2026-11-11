Create rproj.toml from an renv.lock file

## Description

Read an `renv.lock` file and create `rproj.toml`, rig's project manifest, from
it: one dependency per locked package (with a `^`-pinned version requirement),
an `R` requirement from the lock file's R version, and the package
repositories. Then create `rproj.lock` with the same package versions as
`renv.lock`. Fails if `rproj.toml` already exists; use `--dependencies` to
merge into an existing file instead.

`renv.lock` has no project metadata (name, title, authors, ...), so a new
manifest is named after the current directory.

By default rig reads `renv.lock` in the current directory; use `--input` to
point to a different file.

After importing, run [`rig proj sync`](proj.qmd#rig-proj-sync) to install the
packages of `rproj.lock` into the project library.

## Sources

renv enumerates required repositories listed in the lock file, so rig attempts
to do the same. It detects the type of each repository of the lock file, like
[`rig repos add`](repos.qmd#rig-repos-add) does, and uses the built-in repositories for the ones they
stand for:

- a public CRAN mirror (e.g. `cloud.r-project.org`), or the CRAN repository
  of the public Posit Package Manager (PPM, `p3m.dev`), is the built-in CRAN;
- a public Bioconductor repository of the lock file's Bioconductor release
  is the built-in Bioconductor;
- a public PPM snapshot is the built-in CRAN, or the built-in Bioconductor
  for a snapshot of Bioconductor.

Every other repository, e.g. a private or internal one, gets an explicit
`[[repository]]` entry. A snapshot of a private PPM is replaced by the same
PPM repository at `latest`. Use `--keep-ppm-snapshots` to keep all PPM
snapshots, public and private, as they are. Built-in repositories are
disabled if the lock file does not use them.

After the import, rig prints the repositories of `rproj.toml`. For each
repository with a URL it shows the type (e.g. `ppm-snapshot` or `r-universe`)
and the protocol it will use for it, and how many packages are pinned to it.

When a lock file uses Bioconductor, rig will also constrain the R version to one
compatible with that Bioconductor release.

A package that renv installed from GitHub, GitLab, Bitbucket or a git URL
becomes a `git` dependency, at the branch or tag renv installed it from (or the
default branch). rig asks the git repository whether the ref is a branch or a
tag. If it cannot tell, e.g. because the repository cannot be reached, it uses
a branch and prints a warning. `rproj.lock` gets the commit of `renv.lock`.

Packages from any other source (e.g. a URL or a local file) are currently
imported as ordinary version requirements. rig prints a warning that they may
need manual adjustment.

## Versions

renv restores exactly the versions in `renv.lock`. rig keeps these versions in
`rproj.lock`, so `rig proj sync` installs the same packages as
`renv::restore()`. In `rproj.toml` it imports them as `^` requirements instead,
e.g. `cli = "^3.6.1"`. So later `rig proj lock --upgrade`,
`--upgrade-package` and `rig proj add` can move packages to newer, compatible
versions, using rig's resolution and compatibility checks.

`rproj.lock` is for the R version of `renv.lock` (or `--r-version`), and for the
default platforms of [`rig proj lock`](proj.qmd#rig-proj-lock). rig also sets
`r-versions` in the `[tool.rig]` table of `rproj.toml` to this R version, so
later `rig proj lock` runs keep using it, instead of the default R version.
Edit or remove `r-versions` to move the project to another R version.

Creating `rproj.lock` can fail if `renv.lock` is not internally consistent,
e.g. if it has a package together with an older version of its dependency
than the package needs. Then the import fails, but it keeps the new
`rproj.toml`: run `rig proj lock` to lock compatible versions instead.

Use `--no-lock-versions` to skip creating `rproj.lock`. Then run
`rig proj lock` to solve the dependencies and write `rproj.lock`.

## Files

A full import sets up a whole project, not just its manifest, so it creates
the same files as [`rig proj init`](proj.qmd#rig-proj-init) , and `rproj.lock`
(unless `--no-lock-versions` is used).

rig refuses to overwrite any of the `.rvenv` files above; pass `--force` to
replace them.

## The `--dependencies` option

`--dependencies` does not require `rproj.toml` to be missing: if it exists, its
dependencies are merged into it (importing a package already listed
overwrites its entry with the version requirement from `renv.lock`),
preserving comments and custom formatting where possible; if it does not
exist, a minimal manifest is created first. An existing `rproj.lock` is
updated: the packages of `renv.lock` get its versions, the other
packages keep the versions they have.
