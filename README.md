
# The R Installation Manager

Install, remove, configure R versions.

## 🚀&nbsp; Features

* Works on macOS, Windows version is coming soon!
* Install multiple R vresions, select the default one, run multiple
  versions at the same time using quick links. E.g. `R-4.1` starts
  R 4.1.x.
* Select R version to install using symbolic names: `devel`, `release`,
  `oldrel` and more.
* On M1 macs select between x86_64 and arm64 versions or R, or install both.
* Makes sure that installed packages are kept separete from the R
  installation.

## ⬇️&nbsp; Installation

Download the latest release from https://github.com/gaborcsardi/rim/releases.
Unpack archive and move the `rim` file to `/usr/local/bin` or some other
directory in your PATH.

## ⚙️&nbsp; Usage

Use `rim add` to add a new R installation:

```
rim add release
```

Use `rim list` to list the currently installed R versions, and `rim default`
to set the default one.

Run `rim` to see all commands and examples:

```
rim
```

```
RIM -- The R Installation Manager 0.1.0

DESCRIPTION
    rim manages your R installations, on macOS and Windows. It can install
    and set up multiple versions R, and it makes sure that they work
    together.

    On macOS R versions installed by rim do not interfere. You can run multiple
    versions at the same time. rim also makes sure that packages installed by
    the user go into a user package library, so reinstalling R will not wipe
    out your installed packages.

    rim is currently work in progress. Feedback is appreciated.
    See https://github.com/gaborcsardi/rim for bug reports and more.

USAGE:
    rim [SUBCOMMAND]

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

SUBCOMMANDS:
    add          Install a new R version
    available    List R versions available to install (TODO)
    default      Print or set default R version
    help         Prints this message or the help of the given subcommand(s)
    list         List installed R versions
    resolve      Resolve a symbolic R version
    rm           Remove R versions
    system       Manage current installations

EXAMPLES:
    # Add the latest development snapshot
    rim add devel

    # Add the latest release
    rim add release

    # Install specific version
    rim add 4.1.2

    # Install latest version within a minor branch
    rim add 4.1

    # List installed versions
    rim list

    # Set default version
    rim default 4.0
```

Run `rim <subcommand> --help` for information about a subcommand:

```
rim default --help
```

```
rim-default

DESCRIPTION
    Print or set the default R version. The default R version is the one that
    is started with the `R` command, usually via the `/usr/local/bin/R`
     symbolic link.

    Call without any arguments to see the current default. Call with the
    version number/name to set the default. Before setting a default, you
    can call `rim list` to see the installed R versions.

    The default R version is set by updating the symbolic link at
    `/Library/Frameworks/R.framework/Versions/Current` and pointing it to the
    specified R version.

    Potentially you need to run this command with `sudo` to change the
    default version: `sudo rim default ...`.

    You don't need to update the default R version to just run a non-default R
    version. You can use the `R-<ver>` links, see `rim system make-links`.

USAGE:
    rim default [version]

FLAGS:
    -h, --help
            Prints help information

    -V, --version
            Prints version information


ARGS:
    <version>
            new default R version to set


EXAMPLES:
    # Query default R version
    rim default

    # Set the default version
    rim default 4.1
```

## 🤝&nbsp; Feedback

Please open an issue in our issue tracker at
https://github.com/gaborcsardi/rim/issues

## 📘&nbsp; License

MIT 2021 © RStudio Pbc.
