Switch to user mode and clean up admin-mode installations

## Description

Switch rig to [user mode](../articles/admin-vs-user-mode.qmd) and clean up the machine after admin mode.

In user mode rig installs R into your home directory
(`~/.local/share/rig/r` or `%APPDATA%\rig\data\r` on Windows),
and quick links into `~/.local/bin`, so that rig never needs `sudo`.
This command migrates an existing admin-mode setup to user mode:

1. Sets the rig `mode` configuration to `user`.
2. Reinstalls the admin-mode R versions in user mode, and restores
   the previous default version and version aliases. R versions that
   are already installed in user mode are not reinstalled. Use
   `--no-reinstall` to skip this step entirely and only clean up.
3. Removes the system-wide admin-mode R installations.
4. Removes the system-wide `R-*` quick links and the `R`/`Rscript`
   links.

Steps 3 and 4 remove files outside your home directory, so this command
needs an administrator account or `sudo` on Unix, otherwise it will ask
for your password.

Use `--keep-install` to leave the admin-mode R installations in
in place (skipping step 3), and `--keep-links` to leave the system-wide
links in place (skipping step 4). With both, nothing outside your home
directory is touched and no administrator account or `sudo` is needed.
