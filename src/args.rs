// ------------------------------------------------------------------------
// Arguemnt parsing
// ------------------------------------------------------------------------

use clap::{App, AppSettings, Arg, ArgMatches};

#[cfg(target_os = "macos")]
std::include!("help-macos.in");

#[cfg(target_os = "windows")]
std::include!("help-windows.in");

#[cfg(target_os = "linux")]
std::include!("help-linux.in");

pub fn rim_app() -> App<'static> {

    let rim = App::new("RIM -- The R Installation Manager")
        .version("0.1.6")
        .about(HELP_ABOUT)
        .setting(AppSettings::ArgRequiredElseHelp)
        .term_width(80);

    let cmd_default = App::new("default")
        .about("Print or set default R version")
        .long_about(HELP_DEFAULT)
        .after_help(HELP_DEFAULT_EXAMPLES)
        .arg(
            Arg::new("version")
                .help("new default R version to set")
                .required(false),
        );

    let cmd_list = App::new("list")
        .aliases(&["ls"])
        .about("List installed R versions")
        .long_about(HELP_LIST);

    let mut cmd_add = App::new("add")
        .about("Install a new R version")
        .long_about(HELP_ADD)
        .after_help(HELP_ADD_EXAMPLES)
        .aliases(&["install"]);

    cmd_add = cmd_add.arg(
        Arg::new("str")
            .help("R version to install")
            .default_value("release")
            .multiple_occurrences(false)
    );

#[cfg(target_os = "macos")]
{
    cmd_add = cmd_add
        .arg(
            Arg::new("arch")
                .help(HELP_ARCH)
                .short('a')
                .long("arch")
                .required(false)
                .default_value(DEFAULT_ARCH)
        );
}

    let cmd_rm = App::new("rm")
        .about("Remove R versions")
        .long_about(HELP_RM)
        .aliases(&["del", "remove", "delete"])
        .arg(
            Arg::new("version")
                .help("versions to remove")
                .multiple_occurrences(true)
                .required(false),
        )
        .arg(
            Arg::new("all")
                .help("remove all versions (TODO)")
                .long("all")
                .required(false),
        );

    let mut cmd_system = App::new("system")
        .about("Manage current installations")
        .long_about(HELP_SYSTEM);

    let cmd_system_links = App::new("make-links")
        .about("Create R-* quick links")
        .long_about(HELP_SYSTEM_LINKS);

    let cmd_system_lib = App::new("create-lib")
        .about("Create current user's package libraries")
        .long_about(HELP_SYSTEM_LIB)
        .arg(
            Arg::new("version")
                .help("R versions to create the library for (default: all)")
                .required(false)
                .multiple_occurrences(true),
        );

    let cmd_system_pak = App::new("add-pak")
        .about("Install or update pak for an R version")
        .long_about(HELP_SYSTEM_ADDPAK)
        .arg(
            Arg::new("devel")
                .help("Install the development version of pak")
                .long("devel")
                .required(false),
        )
        .arg(
            Arg::new("all")
                .help("Install pak for all R versions")
                .long("all")
                .required(false),
        )
        .arg(
            Arg::new("version")
                .help("R versions to install/update pak for")
                .required(false)
                .multiple_occurrences(true),
        );

#[cfg(target_os = "windows")]
{
    let cmd_system_cleanreg = App::new("clean-registry")
        .about("Clean the R related entries in the registry")
        .long_about(HELP_SYSTEM_CLEANREG);

    cmd_system = cmd_system
        .subcommand(cmd_system_cleanreg)
}

#[cfg(target_os = "macos")]
{
    let cmd_system_ortho = App::new("make-orthogonal")
        .about("Make installed versions orthogonal (macOS)")
        .long_about(HELP_SYSTEM_ORTHO)
        .arg(
            Arg::new("version")
                .help("R versions to update (default: all)")
                .required(false)
                .multiple_occurrences(true),
        );

    let cmd_system_rights = App::new("fix-permissions")
        .about("Restrict permissions to admin")
        .long_about(HELP_SYSTEM_FIXPERMS)
        .arg(
            Arg::new("version")
                .help("R versions to update (default: all)")
                .required(false)
                .multiple_occurrences(true),
        );

    let cmd_system_forget = App::new("forget")
        .about("Make system forget about R installations (macOS)")
        .long_about(HELP_SYSTEM_FORGET);


    let cmd_system_noopenmp = App::new("no-openmp")
        .about("Remove OpemMP (-fopenmp) option for Apple compilers")
        .long_about(HELP_SYSTEM_NO_OPENMP)
        .arg(
            Arg::new("version")
                .help("R versions to update (default: all)")
                .required(false)
                .multiple_occurrences(true)
        );

    cmd_system = cmd_system
        .subcommand(cmd_system_ortho)
        .subcommand(cmd_system_rights)
        .subcommand(cmd_system_forget)
        .subcommand(cmd_system_noopenmp);
}

    cmd_system = cmd_system
        .subcommand(cmd_system_links)
        .subcommand(cmd_system_lib)
        .subcommand(cmd_system_pak);

    let mut cmd_resolve = App::new("resolve")
        .about("Resolve a symbolic R version")
        .long_about(HELP_RESOLVE)
        .after_help(HELP_RESOLVE_EXAMPLES);

    cmd_resolve = cmd_resolve.arg(
        Arg::new("str")
            .help("symbolic version string to resolve")
            .required(true)
    );

#[cfg(target_os = "macos")]
{
    cmd_resolve = cmd_resolve
        .arg(
            Arg::new("arch")
                .help(HELP_ARCH)
                .short('a')
                .long("arch")
                .required(false)
                .default_value(DEFAULT_ARCH)
        );
}

    rim
        .subcommand(cmd_default)
        .subcommand(cmd_list)
        .subcommand(cmd_add)
        .subcommand(cmd_rm)
        .subcommand(cmd_system)
        .subcommand(cmd_resolve)
        .after_help(HELP_EXAMPLES)
}

pub fn parse_args() -> ArgMatches {
    rim_app().get_matches()
}
