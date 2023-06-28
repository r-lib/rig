

# The R Installation Manager

Install, remove, configure R versions.


- 🚀  <a href="#id-features">Features</a>
- 🐞  <a href="#id-known-issues">Known Issues</a>
- ⬇️  <a href="#id-installation">Installation</a>
- ⚙️  <a href="#id-usage">Usage</a>
- ⛵  <a href="#id-macos-menu-bar-app">macOS menu bar app</a>
- 📦  <a href="#id-container">Docker container with rig</a>
- 🤝  <a href="#id-feedback">Feedback</a>
- ❓  <a href="#id-faq">FAQ</a>
- 📘  <a href="#id-license">License</a>

## 🚀  Features <a id="id-features">

- Works on macOS, Windows and Linux (Ubuntu LTS and Debian, x86_64 and
  aarch64).
- Easy installation and update, no system requirements on any platform.
- Install multiple R versions.
- Select the default R version, for the terminal and RStudio.
- Select R version to install using symbolic names: `devel`, `next`,
  `release`, `oldrel`, etc.
- Run multiple versions *at the same* time using quick links. E.g.
  `R-4.1` or `R-4.1.2` starts R 4.1.x. Quick links are automatically
  added to the user’s path.
- On M1 macs select between x86_64 and arm64 versions or R, or install
  both.
- Creates and configures user level package libraries.
- Restricts permissions to the system library. (On macOS, not needed on
  Windows and Linux).
- Includes auto-complete for `zsh` and `bash`, on macOS and Linux.
- Updates R installations to allow debugging with `lldb`, and to allow
  core dumps, on macOS.
- Installs the appropriate Rtools versions on Windows and sets them up.
- Cleans up stale R-related entries from the Windows registry.
- Switches to root/administrator user as needed.

## 🐞  Known Issues <a id="id-known-issues">

- On macOS, R.app often does not work if you install multiple R
  versions.
- On Windows you need to restart your shell or terminal after installing
  Rtools, for the changes to take effect.
- On Windows, `rig rstudio` changes the R version in the registry
  temporarily before starting RStudio and then changes it back after a
  short wait. If RStudio starts up very slowly, then the wait might be
  too short, and it might start up with the wrong R version.
- On Windows Rtools installation will fail if the same version of Rtools
  is already installed.

Found another issue? Please report it in our [issue
tracker](https://github.com/r-lib/rig/issues).

## ⬇️  Installation <a id="id-installation">

### macOS (installer)

Download the latest release from <https://github.com/r-lib/rig/releases>
and install it the usual way.

### macOS (Homebrew)

If you use Homebrew (Intel or Arm version), you can install rig from our
tap:

``` sh
brew tap r-lib/rig
brew install --cask rig
```

You can use x86_64 rig on Arm macs, and it will be able to install Arm
builds of R. But you cannot use Arm rig on Intel macs. If you use both
brew versions, only install rig with one of them.

To update rig you can run

``` sh
brew upgrade --cask rig
```

### Windows (WinGet)

The simplest way to install rig on Windows 10 and above is to use the
built-in WinGet package manager. The name of the package is `posit.rig`.

    winget install posit.rig

### Windows (installer)

Download the latest release from <https://github.com/r-lib/rig/releases>
and install it the usual way.

`rig` adds itself to the user’s path, but you might need to restart your
terminal after the installation on Windows.

### Windows (Scoop)

If you use [Scoop](https://scoop.sh/), you can install rig from the
scoop bucket at
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

If you use [Chocolatey](https://chocolatey.org/) (e.g. on GitHub
Actions) you can install `rig` with

``` powershell
choco install rig
```

and upgrade to the latest version with

``` powershell
choco upgrade rig
```

Note that a new rig version might take a week or two to publish on
Chocolatey, so the installer and the version in Scoop might be newer.

### Linux

Download the latest releast from <https://github.com/r-lib/rig/releases>
and uncompress it to `/usr/local`

    curl -Ls https://github.com/r-lib/rig/releases/download/latest/rig-linux-latest.tar.gz |
      sudo tar xz -C /usr/local

If you are running Linux on arm64, download the arm64 build:

    curl -Ls https://github.com/r-lib/rig/releases/download/latest/rig-linux-arm64-latest.tar.gz |
      sudo tar xz -C /usr/local

Supported Linux distributions:

- Ubuntu LTS from
  [r-builds](https://github.com/rstudio/r-builds#r-builds), currently
  18.04, 20.04, 22.04.
- Debian from [r-builds](https://github.com/rstudio/r-builds#r-builds),
  currently 9, 10 and 11.

Other Linux distributions are coming soon.

### Auto-complete

The macOS and Linux installers also install completion files for `zsh`
and `bash`.

`zsh` completions work out of the box.

For `bash` completions install the `bash-completion` package from
Homebrew or your Linux distribution and make sure it is loaded from your
`.bashrc`. (You don’t need to install `bash` from Homebrew, but you can
if you like.)

## ⚙️  Usage <a id="id-usage">

Use `rig add` to add a new R installation:

    rig add release

Use `rig list` to list the currently installed R versions, and
`rig default` to set the default one.

Run `rig` to see all commands and examples.

### Command list:

    rig add        -- install a new R version [alias: install]
    rig default    -- print or set default R version [alias: switch]
    rig library    -- manage package libraries [alias: lib] (experimental)
    rig list       -- list installed R versions [alias: ls]
    rig resolve    -- resolve a symbolic R version
    rig rm         -- remove R versions [aliases: del, delete, remove]
    rig rstudio    -- start RStudio with the specified R version
    rig sysreqs    -- manage R-related system libraries and tools (experimental) (macOS)
    rig system     -- manage current installations

Run `rig <subcommand> --help` for information about a subcommand.

### macOS `rig system` subcommands

    rig system add-pak                 -- install or update pak for an R version
    rig system allow-debugger          -- allow debugging R with lldb and gdb
    rig system allow-debugger-rstudio  -- allow debugging RStudio with lldb and gdb
    rig system allow-core-dumps        -- allow creating core dumps when R crashes
    rig system fix-permissions         -- restrict system library permissions to admin
    rig system forget                  -- make system forget about R installations
    rig system make-links              -- create R-* quick links
    rig system make-orthogonal         -- make installed versions orthogonal
    rig system no-openmp               -- remove OpenMP (-fopenmp) option for Apple compilers
    rig system setup-user-lib          -- set up automatic user package libraries [alias: create-lib]

### Windows `rig system` subcommands

    rig system add-pak                 -- install or update pak for an R version
    rig system clean-registry          -- clean stale R related entries in the registry
    rig system make-links              -- create R-* quick links
    rig system setup-user-lib          -- set up automatic user package libraries [alias: create-lib]
    rig system update-rtools40         -- update Rtools40 MSYS2 packages

### Linux `rig system` subcommands

    rig system add-pak                 -- install or update pak for an R version
    rig system make-links              -- create R-* quick links
    rig system setup-user-lib          -- set up automatic user package libraries [alias: create-lib]

## ⛵  macOS menu bar app <a id="id-macos-menu-bar-app">

View and select the default R version in the macOS menu bar. Start
RStudio or a recent RStudio project with the selected R version. Select
between your package libraries.

<img src="rig-app.png">

## 📦  Docker container with rig (and multiple R versions) <a id="id-container">

Use the `rhub/rig` (also at ghcr.io/r-lib/rig/r) Docker container to
easily run multiple R versions. It is currently based on Ubuntu 22.04
and contains rig and the six latest R versions, including R-next and
R-devel. It is available for x86_64 and arm64 systems:

    > docker run ghcr.io/r-lib/rig/r rig ls
    * name   version    aliases
    ------------------------------------------
      3.5.3
      3.6.3
      4.0.5
      4.1.3             oldrel
    * 4.2.2             release
      devel  (R 4.3.0)
      next   (R 4.2.2)

### Docker container features:

- <https://github.com/r-lib/pak> is installed for all R versions.
- Automatic system dependency installation via pak.
- Linux binary packages are automatically installed from the [Posit
  Public Package Manager](https://packagemanager.posit.co/client/#/) in
  x86_64 containers.

See this image on [Docker Hub](https://hub.docker.com/r/rhub/rig) or
[GitHub](https://github.com/r-lib/rig/pkgs/container/rig%2Fr).

## 🤝  Feedback <a id="id-feedback">

Please open an issue in our issue tracker at
<https://github.com/r-lib/rig/issues>

## ❓  FAQ <a id="id-faq">

<details>
<summary>
Why does rig create a user package library?
</summary>

> Installing non-base packages into a user package library has several
> benefits:
>
> - The system library is not writeable for regular users on some
>   systems (Windows and Linux, typically), so we might as well create a
>   properly versioned user library at the default place.
> - Some tools need a clean R environment, with base packages only, and
>   do not work well if user packages are installed into the system
>   library. E.g. `R CMD check` is such a tool, and
>   <https://github.com/r-lib/revdepcheck> is another.
> - You can delete an R installation (e.g. with `rig rm`) and then and
>   then install it again, without losing your R packages.

</details>
<details>
<summary>
Why does rig install pak?
</summary>

> To be able to install R packages efficiently, from CRAN, Bioconductor
> or GitHub, right from the start. pak also supports installing system
> libraries automatically on some Linux systems.
>
> If you don’t want `rig add` to install pak, use the `--without-pak`
> option.

</details>
<details>
<summary>
Why does rig change the permissions of the system library (on macOS)?
</summary>

> To make sure that you don’t install packages accidentally into the
> system library. See “Why does rig create a user package library?”
> above.

</details>
<details>
<summary>
Why does rig set the default CRAN mirror?
</summary>

> To avoid the extra work the users need to spend on this.
>
> The <https://cloud.r-project.org> mirror is usually better than the
> other, in that it is a CDN that is close to most users, and that it is
> updated more often.
>
> If you want to use a different mirror, you can set the `repos` option
> in your `.Rprofile`, so the rig repo settings will be ignored.
>
> You can also use the `--without-cran-mirror` option of `rig add`.

</details>
<details>
<summary>
Why does rig set up P3M?
</summary>

> P3M ([Posit Public Package
> Manager](https://packagemanager.posit.co/client/#/)) is generally
> superior to a regular CRAN mirror on Windows and many Linux systems.
>
> On Linux it includes binary packages for many popular distributions.
>
> On Windows, it includes up to date binary packages for older R
> versions as well.
>
> To avoid P3M use the `--without-p3m` option (or the legacy
> `--without-rspm`) option of `rig add`.

</details>
<details>
<summary>
Can rig install R without admin permissions
</summary>

> No, currently it cannot.

</details>
<details>
<summary>
How is rig different from RSwitch?
</summary>

> While there is a small overlap in functionality, rig and
> [RSwitch](https://rud.is/rswitch/) are very different. I suggest you
> look over the features of both to decide which one suits your needs
> better.
>
> If you like rig and also like the extra features of RSwitch, then you
> can use them together just fine: changing the default R version in
> RSwitch also changes it in rig and vice versa. You can use the rig cli
> and the RSwitch app together, or you can also use both menu bar apps
> at the same time.

</details>

## 📘   License <a id="id-license">

MIT 2021-2023 © Posit Software, PBC.
