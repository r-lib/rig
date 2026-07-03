Manage rig configuration

## Description

Manage the rig configuration file.

rig reads a number of settings from a configuration file, e.g. the
installation `mode` (user or admin), the quick-link directory
(`binary-dir`) and the R installation root (`r-install-dir`). Most
settings can also be overridden with environment variables.

`rig config config-file-path` prints the path to the config file.
`rig config list` lists the names of all configuration entries.
`rig config get` prints the value of a configuration entry.
`rig config set` sets a configuration entry.
