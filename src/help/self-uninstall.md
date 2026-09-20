Remove the rig installation

## Description

Removes the `rig` binary, its shell completions, and the PATH entry the
install script added, then deletes itself.

This only works if rig was installed with the install script (`install.sh`
on macOS/Linux, `install.ps1` on Windows).

`rig self uninstall` only removes rig itself. It does not remove rig's
configuration, its download/build cache, or any R version it installed.
Use `rig rm` to remove R version and `rig cache clean` to clean the cache.

Without `--force`, it only prints what it would remove. Use `--dry-run` for
the same preview, or `--force` to actually remove rig.
