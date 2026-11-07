Create rproj.toml from an renv.lock file

## Description

Read an `renv.lock` file and create `rproj.toml`, rig's project manifest, from
it: one dependency per locked package (with a `^`-pinned version
requirement), an `R` requirement from the lock file's R version (limited to that
R minor version if the lock file pins a Bioconductor release, which is built for
one R minor version), and the package repositories. Fails if `rproj.toml`
already exists; use `--dependencies` to merge into an existing file instead.

`renv.lock` has no project metadata (name, title, authors, ...), so a new
manifest is named after the current directory.

By default rig reads `renv.lock` in the current directory; use `--input` to
point to a different file.

After importing, run [`rig proj lock`](proj.qmd#rig-proj-lock) to solve the dependencies and write
`rproj.lock`.

## Sources

renv enumerates required repositories listed in the lock file, so rig attempts
to do the same, mapping known CRAN and Bioconductor aliases to the built-in
repositories when possible and writing explicit `[[repository]]` entries
otherwise. Built-in repositories are disabled if the lock file does not use
them.

A package that renv installed from GitHub, GitLab, Bitbucket or a git URL
becomes a `git` dependency.

Packages from any other source (e.g. a URL or a local file) are currently
imported as ordinary version requirements. rig prints a warning that they may
need manual adjustment.

## Files

A full import sets up a whole project, not just its manifest, so it creates
the same files as [`rig proj init`](proj.qmd#rig-proj-init) .

rig refuses to overwrite any of the `.rvenv` files above; pass `--force` to
replace them.

## The `--dependencies` option

`--dependencies` does not require `rproj.toml` to be missing: if it exists, its
dependencies are merged into it (importing a package already listed
overwrites its entry with the version requirement from `renv.lock`),
preserving comments and custom formatting where possible; if it does not
exist, a minimal manifest is created first.
