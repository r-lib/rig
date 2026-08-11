Print the directories rig uses

## Description

Print the directories rig uses: where R and (on Windows) Rtools is
installed, where the quick links are created, where (on Linux) rig keeps
the fontconfig configuration and the fallback fonts of the portable R
builds, and where rig keeps its own configuration, data, cache and log
files.

The directories themselves may not exist.

Use `--json` for machine-readable output.

The output contains the directories in effect. They may be the defaults,
or configured via the rig config file or environment variables.

Some directories may depend on the mode and on the architecture, e.g. on
aarch64 Windows the R installation directory is different for aarch64 and
x86_64 R builds.

Use `--user` and `--admin` to override the mode for a single invocation.

## Printing a single directory

With one of the flags (see below) rig prints that single directory as a
bare path, and nothing else, for use in a shell script:

```
cd "$(rig system dirs --r)"
```

The flags are mutually exclusive, and cannot be combined with `--json`.

`--r` is the directory that *contains* the directories of the individual R
versions; it is not an R installation itself. Similarly `--rtools` is the
directory that contains the Rtools directories. Use `rig list --json` and
`rig rtools list` for the paths of the installed versions.

The Rtools installation root is only reported on Windows.
On non-Windows platforms `--rtools` prints nothing and is hidden.

The fontconfig directory is only reported on Linux, where it holds the
`fonts.conf` and the fallback fonts that rig installs for the portable R
builds. On non-Linux platforms `--fonts` prints nothing and is hidden.
