Import a DESCRIPTION file's dependencies into rproj.toml

## Description

Read a `DESCRIPTION` file and merge its dependencies into `rproj.toml`, rig's
project and package manifest. If `rproj.toml` does not exist yet, it is
created first, named after the DESCRIPTION file's `Package:` field.

`Depends` and `Imports` land in the `[dependencies]` table (`Depends`
packages are marked to attach on load); `LinkingTo` also lands in
`[linking-dependencies]`. `Suggests` is imported into
`[dependency-groups.test]` and `Enhances` into
`[dependency-groups.enhances]`.

By default rig reads `DESCRIPTION` in the current directory; use `--input`
to point to a different file.

Importing a package already listed in `rproj.toml` overwrites its entry
with the version requirement from the DESCRIPTION file. Because
`rproj.toml` is rewritten in full, any comments or custom formatting in an
existing file are not preserved.
