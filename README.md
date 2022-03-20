
# The R Installation Manager

Install, remove, configure R versions.

## 🚀  Features

-   Works on macOS, Windows and Linux (Ubuntu and Debian).
-   Install multiple R versions, select the default one.
-   Select R version to install using symbolic names: `devel`,
    `release`, `oldrel`, etc.
-   Run multiple versions *at the same* time using quick links. E.g.
    `R-4.1` or `R-4.1.2` starts R 4.1.x. Quick links are automatically
    added to the user’s path.
-   On M1 macs select between x86_64 and arm64 versions or R, or install
    both.
-   Creates and configures user level package libraries.
-   Restricts permissions to the system library. (On macOS, not needed
    on Windows and Linux).
-   Includes auto-complete for `zsh` and `bash`, on macOS and Linux.
-   Updates R installations to allow debugging with `lldb`, and to allow
    core dumps, on macOS.
-   Installs the appropriate Rtools versions on Windows and sets them
    up.
-   Cleans up stale R-related entries from the Windows registry.
-   Switches to root/administrator user as needed.

## ⬇️  Installation

### macOS (installer)

Download the latest release from
<https://github.com/gaborcsardi/rim/releases> and install it the usual
way.

### macOS (Homebrew)

If you use Homebrew (Intel or Arm version), you can install rim from our
tap:

``` sh
brew tap gaborcsardi/rim
brew install --cask rim
```

You can use x86_64 rim on Arm macs, and it will be able to install Arm
builds of R. But you cannot use Arm rim on Intel macs. If you use both
brew versions, only install rim with one of them.

### Windows

Download the latest release from
<https://github.com/gaborcsardi/rim/releases> and install it the usual
way.

`rim` adds itself to the user’s path, but you might need to restart your
terminal after the installation on Windows.

### Linux

Download the latest releast from
<https://github.com/gaborcsardi/rim/releases> and uncompress it to
`/usr/local`

    curl -OL https://github.com/gaborcsardi/rim/releases/download/v0.2.0/rim-linux-0.2.0.tar.gz
    sudo tar xzf rim-linux-0.2.0.tar.gz -C /usr/local

Supported Linux distributions:

-   Ubuntu from
    [r-builds](https://github.com/rstudio/r-builds#r-builds), currently
    18.04, 20.04, 22.04.
-   Debian from
    [r-builds](https://github.com/rstudio/r-builds#r-builds), currently
    9 and 10.

Other Linux distributions are coming soon.

### Auto-complete

The macOS and Linux installers also install completion files for `zsh`
and `bash`.

`zsh` completions work out of the box.

For `bash` completions install the `bash-completion` package from
Homebrew or your Linux distribution and make sure it is loaded from your
`.bashrc`. (You don’t need to install `bash` from Homebrew, but you can
if you like.)

## ⚙️  Usage

Use `rim add` to add a new R installation:

    rim add release

Use `rim list` to list the currently installed R versions, and
`rim default` to set the default one.

Run `rim` to see all commands and examples.

### Command list:

    rim add        -- install a new R version
    rim default    -- print or set default R version
    rim list       -- list installed R versions
    rim resolve    -- resolve a symbolic R version
    rim rm         -- remove R versions
    rim system     -- manage current installations

Run `rim <subcommand> --help` for information about a subcommand.

### macOS `rim system` subcommands

    rim system add-pak           -- install or update pak for an R version
    rim system create-lib        -- create current user's package libraries
    rim system fix-permissions   -- restrict system library permissions to admin
    rim system forget            -- make system forget about R installations
    rim system make-links        -- create R-* quick links
    rim system make-orthogonal   -- make installed versions orthogonal
    rim system no-openmp         -- remove OpemMP (-fopenmp) option for Apple compilers

### Windows `rim system` subcommands

    rim system add-pak           -- install or update pak for an R version
    rim system clean-registry    -- clean stale R related entries in the registry
    rim system create-lib        -- create current user's package libraries
    rim system make-links        -- create R-* quick links

### Linux `rim system` subcommands

    rim system add-pak           -- install or update pak for an R version
    rim system create-lib        -- create current user's package libraries
    rim system make-links        -- create R-* quick links

## 🤝  Feedback

Please open an issue in our issue tracker at
<https://github.com/gaborcsardi/rim/issues>

## 📘  License

MIT 2021-2022 © RStudio Pbc.
