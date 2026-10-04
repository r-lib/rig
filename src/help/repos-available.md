List available R package repositories

## Description

List the package repositories that rig knows about and can set up.

These are the repositories built into rig, plus the ones you added with
[`rig repos add`](#rig-repos-add). You can enable them with `--with-repos` when running `rig add` or
`rig repos setup`, or with [`rig repos enable`](#rig-repos-enable).

Without arguments rig prints one row per repository: its name, whether it is
part of the default repository set, whether it is built in or custom, and its
title. A custom repository is part of the default set, i.e. new R versions get
it, if you added it with `rig repos add --enable --all-versions`.

Pass a repository name to see its description and its URLs, together with
the platforms, architectures and R versions each URL applies to. Repository
names are matched case insensitively.

## Examples

```sh
# List all repositories rig knows about
rig repos available

# Show the URLs of one repository
rig repos available P3M
```
