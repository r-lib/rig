
// ------------------------------------------------------------------------
// Arguemnt parsing
// ------------------------------------------------------------------------

use clap::{Arg, ArgMatches, App, AppSettings, SubCommand};

pub fn parse_args() -> ArgMatches<'static> {
    App::new("RIM -- The R Installation Manager")
        .version("0.1.0")
        .author("Gábor Csárdi <csardi.gabor@gmail.com>")
        .about("Install and manage R installations. See https://github.com/gaborcsardi/rim")
        .setting(AppSettings::ArgRequiredElseHelp)
        .set_term_width(80)
        .subcommand(
            SubCommand::with_name("default")
                .about("Print or set default R version")
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
                    Arg::with_name("version")
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
        )
        .after_help(r#"EXAMPLES:
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
"#)
        .get_matches()
}
