Create rproj.toml from a DESCRIPTION file

## Description

Read a `DESCRIPTION` file and create `rproj.toml`, rig's project and package
manifest, from it: `[project]` (name, version, title, description, license,
authors, urls) as well as dependencies. Fails if `rproj.toml` already
exists, since populating the full `[project]` metadata block is not a
well-defined merge onto an existing, possibly hand-edited, manifest; use
`--dependencies` to merge into an existing file instead (see below).

`Package:`, `Version:`, `Title:`, `Description:` and `License:` map to the
matching `[project]` fields. `Type:` becomes `[project].type` (defaulting to
`package`, since a `DESCRIPTION` always describes one). `URL:` becomes
`[project.urls]` (the first URL as `homepage`, the second as `source`), and
`BugReports:` becomes `[project.urls].bugreports`.

`Authors@R` is parsed into `[project].authors`, one entry per `person()`
call, in order; this is a best-effort parser for common `person()` usage
(name, `email`, `role`, and an `ORCID`/`ROR` `comment`), not a full R
parser, so unusual calls are skipped with a warning. If `Authors@R` is
absent, the simpler `Maintainer: Name <email>` field is used instead, as a
single author with role `cre`.

`Depends` and `Imports` land in the `[dependencies]` table (`Depends`
packages are marked to attach on load); `LinkingTo` also lands in
`[linking-dependencies]`. `Suggests` is imported into
`[dependency-groups.test]` and `Enhances` into
`[dependency-groups.enhances]`.

Every `Config/Needs/<name>` field becomes a dependency group of the same
name, e.g. `Config/Needs/website` becomes `[dependency-groups.website]`.
Unlike a `DESCRIPTION` dependency field, these list package *references*, not
just package names, so an entry that is not a plain package name (with an
optional version requirement) is kept verbatim as `ref = "..."`, under the
package name the reference implies: `tidyverse/tidytemplate` becomes
`tidytemplate = { ref = "tidyverse/tidytemplate" }`. `rig proj export` writes
these back unchanged. Note that only the `test` and `enhances` groups are
installed, so a `Config/Needs/*` group is carried in the manifest, but not
solved or installed by `rig proj lock` and `rig proj sync`.

By default rig reads `DESCRIPTION` in the current directory; use `--input`
to point to a different file.

## Files

A full import sets up a whole project, not just its manifest, so it creates
the same files as [`rig proj init`](#rig-proj-init): `rproj.toml`, plus the
part of the project's virtual environment that belongs in version control —
`.Renviron`, a marked block in `.gitignore` and the `.rvenvlib/rvenv` shim
package. See [`rig proj init`](#rig-proj-init) for what each of them is for.
`.rvenv` itself is machine-specific and is created by
[`rig proj sync`](#rig-proj-sync).

`--dependencies` only writes `rproj.toml` and never touches `.rvenv`.

`--r-version` sets the R version the project is set up for. It does not have to
be installed, and it does not change what is written: the manifest's R
requirement always comes from the `DESCRIPTION` file, and the
`.rvenvlib/rvenv` shim package works with every R. The default is the
current default R version, or the current R release if there is no default.

rig refuses to overwrite any of the `.rvenv` files above; pass `--force` to
replace them, and it does not lift the refusal to overwrite an existing
`rproj.toml`. The `.gitignore` block is the exception: rig never refuses on
an existing `.gitignore`, it just merges its block into it (or adds one),
leaving the rest of the file alone, and `--force` does not change that.

## The `--dependencies` option

`--dependencies` restores the old, dependency-only behavior: only
`[dependencies]`, `[linking-dependencies]` and the dependency groups are
merged from the `DESCRIPTION` file; `[project]` metadata is left alone.
Unlike the default full import, this does not require `rproj.toml` to be
missing: if it exists, its dependencies are merged into it (importing a
package already listed overwrites its entry with the version requirement
from the DESCRIPTION file); if it does not exist, a minimal manifest is
created first, named after the DESCRIPTION file's `Package:` field. Because
`rproj.toml` is rewritten in full, any comments or custom formatting in an
existing file are not preserved.
