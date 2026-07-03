Install a new R version [alias: install]

## Description

Download and install an R version, from the official sources. It keeps
the already installed R versions, except on macOS in admin mode, where
patch versions of the same minor overwrite each other.

On macOS and Windows rig uses the R builds at https://cran.r-project.org.
On Linux rig uses the Posit R builds from
https://github.com/rstudio/r-builds.

The desired R version can be specified in various ways:

- `rig add devel` adds the latest available development version,
- `rig add next` is the next version (patched, alpha, beta, rc, etc.),
- `rig add release` adds the latest release.
- `rig add x.y.z` adds a specific version.
- `rig add x.y` adds the latest release within the `x.y` minor branch.
- `rig add oldrel/n` adds the latest release within the `n`th previous
  minor branch (`oldrel` is the same as `oldrel/1`).
- `rig add <url>` uses a build from `<url>`.

In user mode rig installs R into your home directory and never needs
`sudo`. In admin mode you usually need to run this command with `sudo`:
`sudo rig add ...`, otherwise rig will need to ask for your password.

In admin mode on macOS rig cannot add multiple R versions from the same
minor branch. E.g. it is not possible to have R 4.6.0 and R 4.6.1
installed at the same time. Adding one of them will automatically remove
the other. In user mode there is no such restriction.

You can use `rig add` to install Rtools:

```sh
rig add rtools
```

will install all Rtools versions that are needed for the currently
installed R versions. You can also request a specific Rtools version,
e.g. `rig add rtools45`.

In user mode rig installs R and Rtools into your user profile, without
administrator rights. In admin mode you need an administrator account to
run this command.

## Examples

```sh
# Add the latest development snapshot
rig add devel

# Add the latest release
rig add release

# Install specific version
rig add 4.6.1

# Install latest version within a minor branch
rig add 4.6

# Install arm64 build of R (default on arm64 machines)
rig add -a arm64 release

# Install x86_64 build of R (default on x86_64 machines)
rig add -a x86_64 release

# Install all needed Rtools versions (Windows only)
rig add rtools
```
