Create a DESCRIPTION file from rproj.toml

## Description

Read `rproj.toml`, rig's project and package manifest, and write a
`DESCRIPTION` file from it, the reverse of `rig proj import`.

`[project]` maps back to `Package:`, `Version:`, `Title:`, `Description:`,
`License:` and `Type:` (defaulting to `Package` if `[project].type` is
unset; written as-is otherwise, even for `type = "project"`, since a
`DESCRIPTION` is written regardless of whether the manifest describes an
installable package). `[project.authors]` becomes `Authors@R`, one
`person()` call per entry. `[project.urls]`'s `homepage`/`source` keys
become `URL:`, and `bugreports` becomes `BugReports:`.

`[dependencies]` becomes `Depends`/`Imports` (an entry with `attach = true`,
and `R` itself, become `Depends`; the rest become `Imports`);
`[linking-dependencies]` becomes `LinkingTo`; the `test` and `enhances`
dependency groups become `Suggests` and `Enhances`.

DESCRIPTION's dependency syntax only supports a single version comparison
per package (`pkg (>= 1.2.3)`), unlike `rproj.toml`, which can express a
two-sided range (e.g. `^1.2.3` means `>= 1.2.3, < 2.0.0`). When a dependency
has both a lower and an upper bound, only the lower bound is written; rig
prints a warning listing which packages were affected.

By default rig writes `DESCRIPTION` in the current directory; use
`--output` to write to a different file. rig refuses to overwrite an
existing file unless `--force` is given.
