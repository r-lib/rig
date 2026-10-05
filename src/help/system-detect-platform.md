Detect operating system version and distribution.

## Description

Detect and print the operating system version and distribution.

rig uses this information to choose the right R builds and package
repositories. Use `--json` for machine-readable output.

The first line is the platform string of this machine. rig writes platform
strings as target triples, with `aarch64` or `x86_64` as the arch:

- `aarch64-apple-darwin`, `x86_64-apple-darwin` -- macOS.
- `x86_64-w64-mingw32`, `aarch64-w64-mingw32` -- Windows.
- `x86_64-unknown-linux-gnu` -- any Linux with glibc.
- `x86_64-unknown-linux-gnu-ubuntu-24.04` -- a specific Linux distribution
  and version.

This is the form `rproj.lock` uses, and that rig prints in messages.
Commands that take a platform, e.g. `rig proj lock --platform` or the
`RIG_PLATFORM` environment variable, also take shorter forms: `macos`,
`windows`, `linux`, `macos-arm64`, `ubuntu-24.04`, `linux-ubuntu-24.04`,
and P3M platform names like `jammy-x86_64`. `arm64` and `amd64` work as
arch names, too.

Set `RIG_PLATFORM` to make rig act as if it ran on another platform, e.g.:

```sh
RIG_PLATFORM=ubuntu-22.04 rig system detect-platform
```
