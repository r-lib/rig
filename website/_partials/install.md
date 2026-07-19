## Installing rig on macOS

You can use one of our installers or our Homebrew tap.

### macOS (installer)

Download the latest release from <https://github.com/r-lib/rig/releases>
and install it the usual way.

### macOS (Homebrew)

If you use Homebrew (Intel or Arm version), you can install rig from our
tap:

```sh
brew tap r-lib/rig
brew trust r-lib/rig
brew install --cask rig
```

You can use x86_64 rig on Arm macs, and it will be able to install Arm
builds of R. But you cannot use Arm rig on Intel macs. If you use both brew
versions, only install rig with one of them.

To update rig you can run

```sh
brew upgrade --cask rig
```

### macOS (user install, no admin)

If you don't have administrator rights, you can install rig entirely inside
your home directory. The easiest way is the install script, which downloads
the right build for your Mac, unpacks it into `~/.local`, and adds
`~/.local/bin` to your `PATH`:

```sh
curl -LsSf https://r-lib.github.io/rig/install.sh | sh
```

Then switch rig to user mode and install R:

```sh
rig system user-mode
rig add release
```

Alternatively, download the `rig-macos-<arch>-<version>.tar.gz` archive for
your architecture (`arm64` or `x86_64`) from
<https://github.com/r-lib/rig/releases> and unpack it into `~/.local`
yourself:

```sh
mkdir -p ~/.local
curl -Ls https://github.com/r-lib/rig/releases/download/latest/rig-macos-arm64-latest.tar.gz |
  tar xz -C ~/.local
```

Make sure `~/.local/bin` is on your `PATH`. The binary in these archives is
signed and notarized, so it runs without Gatekeeper warnings.

## Installing rig on Windows

There are several possible ways to install rig on Windows: with our
installer, `scoop`, `choco` or `winget`.

### Windows (installer)

Download the latest release from <https://github.com/r-lib/rig/releases>
and install it the usual way.

`rig` adds itself to the user's path, but you might need to restart your
terminal after the installation on Windows.

### Windows (user install, no admin)

If you don't have administrator rights, you can install rig into your user
profile with the install script. It downloads the right build, unpacks it into
`%USERPROFILE%\.local`, and adds `%USERPROFILE%\.local\bin` to your user
`PATH`:

``` powershell
irm https://r-lib.github.io/rig/install.ps1 | iex
```

Open a new terminal, then switch rig to user mode and install R:

``` powershell
rig system user-mode
rig add release
```

Alternatively, download the `rig-windows-<arch>-<version>.zip` archive
(`x86_64` or `arm64`) from <https://github.com/r-lib/rig/releases> and extract
it into `%USERPROFILE%\.local`, then add `%USERPROFILE%\.local\bin` to your
`PATH`.

### Windows (Scoop)

If you use [Scoop](https://scoop.sh/), you can install rig from the scoop
bucket at
[`cderv/r-bucket`](https://github.com/cderv/r-bucket#r-installation-manager-rig):

``` powershell
scoop bucket add r-bucket https://github.com/cderv/r-bucket.git
scoop install rig
```

To update run

``` powershell
scoop update rig
```

### Windows (Chocolatey)

If you use [Chocolatey](https://chocolatey.org/) (e.g. on GitHub
Actions) you can install `rig` with

``` powershell
choco install rig
```

and upgrade to the latest version with

``` powershell
choco upgrade rig
```

### Windows (WinGet)

An easy way to install rig on Windows 10 and above is to use the
built-in WinGet package manager. The name of the package is `posit.rig`.

```
winget install posit.rig
```

Note that updating a WinGet package typically takes some time, so
WinGet might not have the latest version of rig.

## Installing rig on Linux

On Linux you can install rig from a DEB or RPM package, or from a tarball.

### Supported Linux distributions <a id="id-supported-linux-distributions"></a>

- Debian 12, 13
- Ubuntu 20.04, 22.04, 24.04, 26.04
- Fedora Linux 42, 43
- OpenSUSE 15.6, 16.0
- SUSE Linux Enterprise 15 SP6
- Red Hat Enterprise Linux 7, 8, 9, 10
- AlmaLinux 8, 9, 10
- Rocky Linux 8, 9 10

We use the R builds from the Posit
[R-builds project](https://github.com/rstudio/r-builds#r-builds).

<details><summary>Retired Linux distributions</summary>
These are not updated any more, no new R builds are added for them,
but existing R builds still work.

- CentOS 6 (only x86_64, last R version: 4.0.4),
- CentOS 7 (last R version: 4.4.3),
- CentOS 8 (last R version: 4.4.3),
- Debian 9 (last R version: 4.2.1),
- Debian 10 (last R version: 4.4.3),
- Fedora 37 (last R version: 4.3.2),
- Fedora 38 (last R version: 4.4.2),
- Fedora 39 (last R version: 4.4.3),
- Fedora 40 (last R version: 4.5.1),
- Fedora 41 (last R version: 4.5.3),
- OpenSUSE 42 (only x86_64, last R version: 4.2.1),
- OpenSUSE 15.1 (only x86_64, last R version: 4.1.2),
- OpenSUSE 15.2 (only x86_64, last R version: 4.1.3),
- OpenSUSE 15.3 (last R version: x86_64: 4.4.3, aarch64: 4.3.1),
- OpenSUSE 15.4 (last R version: 4.4.0),
- OpenSUSE 15.5 (last R version: 4.4.3),
- SUSE Linux Enterprise 15 (only x86_64, last R version: 4.1.2),
- SUSE Linux Enterprise 15.1 (only x86_64, last R version: 4.1.2),
- SUSE Linux Enterprise 15.2 (only x86_64, last R version: 4.1.3),
- SUSE Linux Enterprise 15.3 (last R version: x86_64: 4.4.3, aarch64: 4.3.1),
- SUSE Linux Enterprise 15.4 (last R version: 4.4.0),
- SUSE Linux Enterprise 15.5 (last R version: 4.4.3),
- Ubuntu 16.04 (only x86_64, last R version: 4.1.2),
- Ubuntu 18.04 (last R version: 4.3.1).
</details>

### Ubuntu and Debian (DEB package)

On any Ubuntu or Debian distro, you can use our package repository to
install rig. First you add our key to your config:
```
`which sudo` curl -L https://rig.r-pkg.org/deb/rig.gpg -o /etc/apt/trusted.gpg.d/rig.gpg
```

Then add the rig repository:
```
`which sudo` sh -c 'echo "deb http://rig.r-pkg.org/deb rig main" > /etc/apt/sources.list.d/rig.list'
```

If you already added both the key and the repository, then install the `r-rig`
package (`rig` is a different package in Debian and Ubuntu):
```
`which sudo` apt update
`which sudo` apt install r-rig
```

### RHEL, Fedora, Rocky Linux, Almalinux, etc. (RPM package)

On most RPM based distros (except for OpenSUSE and SLES) you can install
our RPM package directly:

```
`which sudo` yum install -y https://github.com/r-lib/rig/releases/download/latest/r-rig-latest-1.$(arch).rpm
```

### OpenSUSE and SLES (RPM package)

On OpenSUSE and SLES use `zypper` instead of `yum`:

```
`which sudo` zypper install -y --allow-unsigned-rpm https://github.com/r-lib/rig/releases/download/latest/r-rig-latest-1.$(arch).rpm
```

### Any Linux distribution (tarball)

Download the latest release from <https://github.com/r-lib/rig/releases>
and uncompress it to `/usr/local`

```
curl -Ls https://github.com/r-lib/rig/releases/download/latest/rig-linux-$(arch)-latest.tar.gz |
  `which sudo` tar xz -C /usr/local
```

## Installing auto-complete

All rig installers and archives ship shell completion files.

### macOS and Linux

The macOS and Linux installers install completion files for `zsh` and `bash`
into system locations.

`zsh` completions work out of the box.

For `bash` completions install the `bash-completion` package from Homebrew
or your Linux distribution and make sure it is loaded from your `.bashrc`.
(You don't need to install `bash` from Homebrew, but you can if you like.)

For **user-mode** installs the completion files are placed under the install
prefix instead (e.g. `~/.local/share`):

- `zsh`: add `~/.local/share/zsh/site-functions` to your `fpath` (before
  `compinit`).
- `bash`: `bash-completion` picks up `~/.local/share/bash-completion/completions`
  automatically once it is loaded from your `.bashrc`.

### Windows

The Windows installer and archive ship a PowerShell completion script. To
enable tab-completion, dot-source it from your PowerShell profile (`$PROFILE`).
For a user-mode archive install that is:

``` powershell
. "$env:USERPROFILE\.local\share\rig\_rig.ps1"
```
