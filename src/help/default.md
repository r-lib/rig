## Description

Print or set the default R version. The default R version is the one that
is started with the `R` command, via the `R` quick link in the rig binary
directory (`~/.local/bin` in user mode).

Call without any arguments to see the current default. Call with the
version number/name to set the default. Before setting a default, you
can call `rig list` to see the installed R versions.

The default R version is set by updating the `current` symbolic link in
the R installation directory and pointing it to the specified R version.

In user mode rig works entirely within your home directory, so no `sudo`
is needed. In admin mode this command can change the default version
without `sudo` as long as the user is in the `admin` group; otherwise you
need to run it as `sudo rig default ...`.

You don't need to update the default R version to just run a non-default R
version. You can use the `R-<ver>` links, see `rig system make-links`.

`rig switch` is an alias of `rig default`.

## Examples

```sh
# Query default R version
rig default

# Set the default version
rig default 4.1.2
```
