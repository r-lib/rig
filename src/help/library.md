## Description

Manage package libraries [alias: lib] (experimental)

rig supports multiple user package libraries. The usual user library is
called "main".

`rig library default` shows or sets the default library for the
current R version.
`rig library list` lists all libraries for the current R version.
`rig library add` adds a new library for the current R version.
`rig library rm` deletes a library, including all packages in it.
It is not possible to delete the current default library, and it is not
possible to delete the main library.

User libraries are implemented at the user level, no administrator or
root password is needed to add, set or delete them. If you delete an
R installation, the user package libraries and their configurations are
kept for all users on the system.

`rig library` is currently experimental, and might change in future
versions. Feedback is appreciated.
