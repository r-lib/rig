Remove all admin-mode R installations and links

## Description

Remove all [admin-mode](../admin-vs-user-mode.qmd) R installations and their quick links.

This is an internal helper used by `rig system user-mode` to clean up a
previous admin-mode setup. It self-escalates to obtain the required
administrator rights, so you normally do not need to run it directly.

Use `--keep-install` to keep the R installations and `--keep-links` to
keep the quick links.
