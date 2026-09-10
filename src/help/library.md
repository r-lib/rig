Manage package libraries [alias: lib]

## Description

Manage package libraries [alias: lib]

rig supports multiple user package libraries. The usual user library is
called "main".

Each subcommand operates on the default R version, unless you select
another installed R version with `--r-version`.

User libraries are implemented at the user level, no administrator or root
password is needed to add, set or delete them. If you delete an R
installation, the user package libraries and their configurations are kept
for all users on the system.

