Delete a config entry

## Description

Delete an entry from the rig configuration file.

E.g. `rig config delete mode` removes the `mode` entry, so rig uses its
built-in default, unless the `RIG_MODE` environment variable is set, which
takes precedence over the configuration file anyway. Deleting an entry that
is not in the configuration file does nothing. See `rig config --help` for
the known keys, and `rig config list` for the entries that are in the
configuration file.
