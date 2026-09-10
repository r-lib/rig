Create a DESCRIPTION file from rproj.toml

## Description

Read `rproj.toml`, rig's project and package manifest, and write a
`DESCRIPTION` file from it, the reverse of `rig proj import`.

| `rproj.toml` field             | `DESCRIPTION` field   |
|------------------------------|---------------------|
| `[project].name`               | `Package:`            |
| `[project].version`            | `Version:`            |
| `[project].title`              | `Title:`              |
| `[project].description`        | `Description:`        |
| `[project].license`            | `License:`            |
| `[project].type`               | `Type:`               |
| `[project].authors`            | `Authors@R`           |
| `[project.urls].homepage`      | `URL:`                |
| `[project.urls].source`        | `URL:`                |
| `[project.urls].bugreports`    | `BugReports:`         |
| `[dependencies]`               | `Depends` / `Imports`   |
| `[linking-dependencies]`       | `LinkingTo`           |
| `[dependency-groups.test]`     | `Suggests`            |
| `[dependency-groups.enhances]` | `Enhances`            |
| `[dependency-groups.<name>]`   | `Config/Needs/<name>` |

DESCRIPTION's dependency syntax only supports a single version comparison
per package (`pkg (>= 1.2.3)`), unlike `rproj.toml`, which can express a
two-sided range (e.g. `^1.2.3` means `>= 1.2.3, < 2.0.0`). When a dependency
has both a lower and an upper bound, only the lower bound is written; rig
prints a warning listing which packages were affected.

By default rig writes `DESCRIPTION` in the current directory; use `--output` to
write to a different file. rig refuses to overwrite an existing file unless
`--force` is given.
