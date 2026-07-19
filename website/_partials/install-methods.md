::: {.panel-tabset}

## macOS

### User install (command line only)

Use our install script, which downloads the right build for your Mac,
unpacks it into `~/.local`, and adds `~/.local/bin` to your `PATH`:

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

### System install (with menu bar app)

Download the latest release from <https://github.com/r-lib/rig/releases>
and install it the usual way.

### Homebrew

#### Menu bar app + command line app:

```sh
brew install r-rig-app
```

This is a homebrew cask and needs your password to install.

#### Command line app:

```sh
brew install r-rig
```

You can use x86_64 rig on Arm macs, and it will be able to install Arm
builds of R. But you cannot use Arm rig on Intel macs. If you use both brew
versions, only install rig with one of them.

To update rig you can run

```sh
brew upgrade r-rig-app
```

## Windows

### User install

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

### System install

Download the latest release from <https://github.com/r-lib/rig/releases>
and install it the usual way.

`rig` adds itself to the user's path, but you might need to restart your
terminal after the installation on Windows.

### Scoop

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

### Chocolatey

If you use [Chocolatey](https://chocolatey.org/) (e.g. on GitHub
Actions) you can install `rig` with

``` powershell
choco install rig
```

and upgrade to the latest version with

``` powershell
choco upgrade rig
```

### WinGet

An easy way to install rig on Windows 10 and above is to use the
built-in WinGet package manager. The name of the package is `posit.rig`.

``` powershell
winget install posit.rig
```

Note that updating a WinGet package typically takes some time, so
WinGet might not have the latest version of rig.

## Linux

On Linux you can install rig from a DEB or RPM package, or from a tarball.
See the [supported distributions](install.qmd#id-supported-linux-distributions)
below.

### Ubuntu and Debian (DEB package)

On any Ubuntu or Debian distro, you can use our package repository to
install rig. First you add our key to your config:

```sh
`which sudo` curl -L https://rig.r-pkg.org/deb/rig.gpg -o /etc/apt/trusted.gpg.d/rig.gpg
```

Then add the rig repository:

```sh
`which sudo` sh -c 'echo "deb http://rig.r-pkg.org/deb rig main" > /etc/apt/sources.list.d/rig.list'
```

If you already added both the key and the repository, then install the `r-rig`
package (`rig` is a different package in Debian and Ubuntu):

```sh
`which sudo` apt update
`which sudo` apt install r-rig
```

### RHEL, Fedora, Rocky Linux, Almalinux, etc. (RPM package)

On most RPM based distros (except for OpenSUSE and SLES) you can install
our RPM package directly:

```sh
`which sudo` yum install -y https://github.com/r-lib/rig/releases/download/latest/r-rig-latest-1.$(arch).rpm
```

### OpenSUSE and SLES (RPM package)

On OpenSUSE and SLES use `zypper` instead of `yum`:

```sh
`which sudo` zypper install -y --allow-unsigned-rpm https://github.com/r-lib/rig/releases/download/latest/r-rig-latest-1.$(arch).rpm
```

### Any Linux distribution (tarball)

Download the latest release from <https://github.com/r-lib/rig/releases>
and uncompress it to `/usr/local`

```sh
curl -Ls https://github.com/r-lib/rig/releases/download/latest/rig-linux-$(arch)-latest.tar.gz |
  `which sudo` tar xz -C /usr/local
```

:::
