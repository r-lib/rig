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

By default rig reads `DESCRIPTION` in the current directory; use `--input`
to point to a different file.

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
