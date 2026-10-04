Remove a custom R package repository

## Description

Remove one or more repositories that you added with [`rig repos add`](#rig-repos-add). rig also
removes them from every R version that uses them.

The repositories that are built into rig cannot be removed, use
[`rig repos disable`](#rig-repos-disable) to stop using them.

If some R versions use the repositories, rig updates their files. If you cannot
write them, which is typical in [admin mode](../admin-vs-user-mode.qmd) on Linux and Windows, rig asks for
administrator rights, e.g. your password for `sudo`.

## Examples

```sh
rig repos rm acme
```
