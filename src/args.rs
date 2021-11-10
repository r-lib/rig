
// ------------------------------------------------------------------------
// Arguemnt parsing
// ------------------------------------------------------------------------

use clap::{Arg, ArgMatches, App, AppSettings, SubCommand};

// ------------------------------------------------------------------------
// macOS help
// ------------------------------------------------------------------------

#[cfg(target_os = "macos")]
const HELP_ABOUT: &str = r#"
DESCRIPTION
    rim manages your R installations, on macOS and Windows. It can install
    and set up multiple versions R, and it makes sure that they work together.

    On macOS R versions installed by rim do not interfere. You can run multiple
    versions at the same time. rim also makes sure that packages installed by
    the user go into a user package library, so reinstalling R will not wipe
    out your installed packages.

    rim is currently work in progress. Feedback is appreciated.
    See https://github.com/gaborcsardi/rim for bug reports and more.
"#;

const HELP_EXAMPLES: &str = r#"EXAMPLES:
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
"#;

#[cfg(target_os = "macos")]
const HELP_DEFAULT: &str = r#"
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
"#;

#[cfg(target_os = "macos")]
const HELP_DEFAULT_EXAMPLES: &str = r#"EXAMPLES:
    # Query default R version
    rim default

    # Set the default version
    rim default 4.1
"#;

#[cfg(target_os = "macos")]
const HELP_LIST: &str = r#"
DESCRIPTION
    List installed R versions from `/Library/Framework/R.framework/versions`.
    It does _not_ check if they are working properly.
"#;

#[cfg(target_os = "macos")]
const HELP_ADD: &str = r#"
DESCRIPTION
    Download and install an R version, from the official sources.
    It keeps the already installed R versions, except versions within the
    same minor branch, see below.

    NOTE: it is best to quit from all currently running R sessions before
    adding new R versions. THe newly added R version will be the default
    after the installation, if you don't want that, call `rim default`.

    The newly added version can be specified in various ways:
    - `rim add devel` adds the latest available development version,
    - `rim add release` adds the latest release.
    - `rim add x.y.z` adds a specific version.
    - `rim add x.y` adds the latest release within the `x.y` minor branch.
    - `rim add oldrel/n` adds the latest release within the `n`th previous
      minor branch (`oldrel` is the same as `oldrel/1`).

    Usually you need to run this command with `sudo`: `sudo rim add ...`.

    On macOS rim cannot add multiple R versions from the same minor branch.
    E.g. it is not possible to have R 4.1.1 and R 4.1.2 installed at the
    same time. Adding one of them will automatically remove the other.

    `rim add` will automatically call `rim system forget` before the
    installation, to make sure that already installed R versions are kept.
    It will also call the following rim command after the installation:
    - `rim system forget`
    - `rim system fix-permissions`
    - `rim system make-orthogonal`
    - `rim system create-lib`
    - `rim system make-links`
    See their help pages for details.
"#;

#[cfg(target_os = "macos")]
const HELP_ADD_EXAMPLES: &str = r#"EXAMPLES
    # Add the latest development snapshot
    rim add devel

    # Add the latest release
    rim add release

    # Install specific version
    rim add 4.1.2

    # Install latest version within a minor branch
    rim add 4.1
"#;

#[cfg(target_os = "macos")]
const HELP_RM: &str = r#"
DESCRIPTION
    Remove an R installation. It keeps the users' package libraries.
    It automatically calls `rm system forget` before removing the files.

    Usually you need to run this command with `sudo`: `sudo rim rm ...`.
"#;

#[cfg(target_os = "macos")]
const HELP_SYSTEM: &str = r#"
DESCRIPTION
    Various commands to modify and configure the installed R versions.
    See their help pages for details. E.g. run `rim system make-links --help`.
"#;

#[cfg(target_os = "macos")]
const HELP_SYSTEM_ORTHO: &str = r#"
DESCRIPTION
    Make the current R installations orthogonal. This allows running multiple
    R versions at the same time, as long as they are started with their
    quick links (see `rim system make-links --help`). For example you
    can run a script using R 4.1.x in one terminal:

    R-4.1 -q -f script1.R

    while running another script using R 4.0.x in another terminal:

    R-4.0 -q -f script2.R

    `rim add` runs `rim system make-orthogonal`, so if you only use rim to
    install R, then you do not need to run it manually.

    This command probably needs `sudo`: `sudo rim system make-orthogonal`.
"#;

#[cfg(target_os = "macos")]
const HELP_SYSTEM_LINKS: &str = r#"
DESCRIPTION
    Create quick links in `/usr/local/bin` for the current R installations.
    This lets you directly run a specific R version. E.g. `R-4.1` will start
    R 4.1.x.

    `rim add` runs `rim system make-links`, so if you only use rim to
    install R, then you do not need to run it manually.

    This command probably needs `sudo`: `sudo rim system make-links`.
"#;

#[cfg(target_os = "macos")]
const HELP_SYSTEM_LIB: &str = r#"
DESCRIPTION
    Create directories for the current user's package libraries, for all
    current R versions.

    `rim add` runs `rim system create-lib`, so if you only use rim to
    install R, then you do not need to run it manually.
"#;

#[cfg(target_os = "macos")]
const HELP_SYSTEM_ADDPAK: &str = r#"
DESCRIPTION
    Install/update pak for all current R versions. (TODO)
"#;

#[cfg(target_os = "macos")]
const HELP_SYSTEM_FIXPERMS: &str = r#"
DESCRIPTION
    Update the permissions of the current R versions, so only the
    administrator can install R packages into the system library.

    `rim add` runs `rim system fix-permissions`, so if you only use rim to
    install R, then you do not need to run it manually.

    This command probably needs `sudo`: `sudo rim system fix-permissions`.
"#;

#[cfg(target_os = "macos")]
const HELP_SYSTEM_CLEANSYSLIB: &str = r#"
DESCRIPTION
    Remove non-core packages from the system libraries of the current R
    versions. (TODO)
"#;

#[cfg(target_os = "macos")]
const HELP_SYSTEM_FORGET: &str = r#"
DESCRIPTION
    Tell macOS to forget about the currently installed R versions.
    This is needed to have multiple R installations at the same time.

    `rim add` runs `rim system forget` before and after the installation,
    so if you only use rim to install R, then you don't need to run this
    command manually.

    This command probably needs `sudo`: `sudo rim system forget`.
"#;

#[cfg(target_os = "macos")]
const HELP_RESOLVE: &str = r#"
DESCRIPTION
    Resolve R versions. Check the version number of an R version (e.g.
    release, devel, etc.), and looks up the URL of the installer for it,
    if an installer is available.

    It prints the R version number, and after a space the URL of the
    installer. If no installer is available for this R version and the
    current platform, the URL is `NA`.

    An R version can be specified in various ways:
    - `rim resolve devel` is the latest available development version,
    - `rim resolve release` is the latest release.
    - `rim resolve x.y.z` is a specific version.
    - `rim resolve x.y` is the latest release within the `x.y` minor branch.
    - `rim resolve oldrel/n` is the latest release within the `n`th previous
      minor branch (`oldrel` is the same as `oldrel/1`).
"#;

#[cfg(target_os = "macos")]
const HELP_RESOLVE_EXAMPLES: &str = r#"EXAMPLES
    # Latest development snapshot
    rim resolve devel

    # Latest release (that has an installer available)
    rim resolve release

    # URL for a specific version
    rim resolve 4.1.2

    # Latest version within a minor branch
    rim resolve 4.1
"#;

// ------------------------------------------------------------------------
// Windows help (TODO)
// ------------------------------------------------------------------------

pub fn parse_args() -> ArgMatches<'static> {
    App::new("RIM -- The R Installation Manager")
        .version("0.1.0")
        .about("Install and manage R installations. See https://github.com/gaborcsardi/rim")
        .long_about(HELP_ABOUT)
        .setting(AppSettings::ArgRequiredElseHelp)
        .set_term_width(80)
        .subcommand(
            SubCommand::with_name("default")
                .about("Print or set default R version")
                .long_about(HELP_DEFAULT)
                .after_help(HELP_DEFAULT_EXAMPLES)
                .arg(
                    Arg::with_name("version")
                        .help("new default R version to set")
                        .required(false)
                )
        )
        .subcommand(
            SubCommand::with_name("list")
                .about("List installed R versions")
                .long_about(HELP_LIST)
        )
        .subcommand(
            SubCommand::with_name("add")
                .about("Install a new R version")
                .long_about(HELP_ADD)
                .after_help(HELP_ADD_EXAMPLES)
                .aliases(&["install"])
                .arg(
                    Arg::with_name("arch")
                        .help("Select macOS arch: arm64 or x86_64")
                        .short("a")
                        .long("arch")
                        .required(false)
                        .default_value("x86_64")
                )
                .arg(
                    Arg::with_name("str")
                        .help("R versions to install (see 'rim available')")
                        .default_value("release")
                        .multiple(false) // TODO: install multiple versions
                )
        )
        .subcommand(
            SubCommand::with_name("rm")
                .about("Remove R versions")
                .long_about(HELP_RM)
                .aliases(&["del", "remote", "delete"])
                .arg(
                    Arg::with_name("version")
                        .help("versions to remove")
                        .multiple(true)
                        .required(false)
                )
                .arg(
                    Arg::with_name("all")
                        .help("remove all versions (TODO)")
                        .long("all")
                        .required(false)
                )
        )
        .subcommand(
            SubCommand::with_name("system")
                .about("Manage current installations")
                .long_about(HELP_SYSTEM)
                .subcommand(
                    SubCommand::with_name("make-orthogonal")
                        .about("Make installed versions orthogonal (macOS)")
                        .long_about(HELP_SYSTEM_ORTHO)
                )
                .subcommand(
                    SubCommand::with_name("make-links")
                        .about("Create R-* quick links")
                        .long_about(HELP_SYSTEM_LINKS)
                )
                .subcommand(
                    SubCommand::with_name("create-lib")
                        .about("Create current user's package libraries")
                        .long_about(HELP_SYSTEM_LIB)
                )
                .subcommand(
                    SubCommand::with_name("add-pak")
                        .about("Install or update pak for all R versions (TODO)")
                        .long_about(HELP_SYSTEM_ADDPAK)
                )
                .subcommand(
                    SubCommand::with_name("fix-permissions")
                        .about("Restrict permissions to admin")
                        .long_about(HELP_SYSTEM_FIXPERMS)
                )
                .subcommand(
                    SubCommand::with_name("clean-sytem-lib")
                        .about("Clean system library from non-core packages (TODO)")
                        .long_about(HELP_SYSTEM_CLEANSYSLIB)
                )
                .subcommand(
                    SubCommand::with_name("forget")
                        .about("Make system forget about R installations (macOS)")
                        .long_about(HELP_SYSTEM_FORGET)
                )
        )
        .subcommand(
            SubCommand::with_name("available")
                .about("List R versions available to install (TODO)")
        )
        .subcommand(
            SubCommand::with_name("resolve")
                .about("Resolve a symbolic R version")
                .long_about(HELP_RESOLVE)
                .after_help(HELP_RESOLVE_EXAMPLES)
                .arg(
                    Arg::with_name("str")
                        .help("symbolic version string to resolve")
                        .required(true)
                )
                .arg(
                    Arg::with_name("arch")
                        .help("Select macOS arch: arm64 or x86_64")
                        .short("a")
                        .long("arch")
                        .required(false)
                        .default_value("x86_64")
                )
        )
        .after_help(HELP_EXAMPLES)
        .get_matches()
}
