
// ------------------------------------------------------------------------
// Arguemnt parsing
// ------------------------------------------------------------------------

use clap::{Arg, ArgMatches, App, AppSettings, SubCommand};

#[cfg(target_os = "macos")]
const HELP_ABOUT: &str = r#"
rim manages your R installations, on macOS and Windows. It can install and set
up multiple versions R, and it makes sure that they work together well.

On macOS R versions installed by rim do not interfere. You can run multiple
versions at the same time. rim also makes sure that packages installed by
the user go into a user package library, so reinstalling R will not wipe out
your installed packages.

rim is currently work in progress. Feedback is appreciated.
See https://github.com/gaborcsardi/rim for bug reports and more.
"#;

const HELP_EXAMPLES: &str = r#"EXAMPLES:
    # Install R-release and R-devel
    rim add release devel

    # Install specific version
    rim add 4.1.2

    # Install latest version within a minor version
    rim add 4.1

    # List installed versions
    rim list

    # Set default version
    rim default 4.0
"#;

#[cfg(target_os = "macos")]
const HELP_DEFAULT: &str = r#"
Print or set the default R version. The default R version is the one that is
started with the `R` command, usually via the `/usr/local/bin/R` symbolic link.

Call without any arguments to see the current default. Call with the version
number/name to set the default. Before setting a default, you can call
`rim list` to see the installed R versions.

The default R version is set by updating the symbolic link at
`/Library/Frameworks/R.framework/Versions/Current` and pointing it to the
specified R version.

You don't need to update the default R version to just run a non-default R
version. You can use the `R-<ver>` quick links, see `rim system make-links`.
"#;

#[cfg(target_os = "macos")]
const HELP_DEFAULT_EXAMPLES: &str = r#"EXAMPLES:
    # Query default R version
    rim default

    # Set the default version
    rim default 4.1
"#;

#[cfg(target_os = "win")]
const HELP_DEFAULT: &str = r#"
Print or set the default
"#;

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
        )
        .subcommand(
            SubCommand::with_name("add")
                .about("Install new R version")
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
                .aliases(&["del", "remote", "delete"])
                .arg(
                    Arg::with_name("version")
                        .help("versions to remove")
                        .multiple(true)
                        .required(false)
                )
                .arg(
                    Arg::with_name("all")
                        .help("remove all versions")
                        .long("all")
                        .required(false)
                )
        )
        .subcommand(
            SubCommand::with_name("system")
                .about("Manage current installations")
                .subcommand(
                    SubCommand::with_name("make-orthogonal")
                        .about("Make installed versions orthogonal (macOS)")
                )
                .subcommand(
                    SubCommand::with_name("make-links")
                        .about("Create R-* quick links")
                )
                .subcommand(
                    SubCommand::with_name("create-lib")
                        .about("Create current user's package libraries")
                )
                .subcommand(
                    SubCommand::with_name("add-pak")
                        .about("Install or update pak for all R versions")
                )
                .subcommand(
                    SubCommand::with_name("fix-permissions")
                        .about("Restrict permissions to admin")
                )
                .subcommand(
                    SubCommand::with_name("clean-sytem-lib")
                        .about("Clean system library from non-core packages")
                )
                .subcommand(
                    SubCommand::with_name("forget")
                        .about("Make system forget about R installations (macOS)")
                )
        )
        .subcommand(
            SubCommand::with_name("available")
                .about("List R versions available to install")
        )
        .subcommand(
            SubCommand::with_name("resolve")
                .about("Resolve a symbolic R version")
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
