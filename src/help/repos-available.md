List available R package repositories

## Description

List the package repositories that rig knows about and can set up.

These are the repositories you can enable with `--with-repos` when running
`rig add` or `rig repos setup`.

Without arguments rig prints one row per repository: its name, whether it is
part of the default repository set, and its title.

Pass a repository name to see its description and its URLs, together with the
platforms, architectures and R versions each URL applies to. Repository names
are matched case insensitively.

## Examples

```sh
# List all repositories rig knows about
rig repos available

# Show the URLs of one repository
rig repos available P3M
```
