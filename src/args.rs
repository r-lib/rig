// ------------------------------------------------------------------------
// Arguemnt parsing
// ------------------------------------------------------------------------

use clap::{Command, Arg, ArgMatches};

#[cfg(target_os = "macos")]
use simplelog::*;

#[cfg(target_os = "macos")]
std::include!("help-macos.in");

#[cfg(target_os = "windows")]
std::include!("help-windows.in");

#[cfg(target_os = "linux")]
std::include!("help-linux.in");

pub fn rig_app() -> Command<'static> {

    let _arch_x86_64: &'static str = "x86_64";
    let _arch_arm64: &'static str = "arm64";
    let mut _default_arch: &'static str = "";

#[cfg(target_os = "macos")]
{
    let proc = std::process::Command::new("arch")
        .args(["-arm64", "true"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    if let Ok(mut proc) = proc {
        let out = proc.wait();
        if let Ok(out) = out {
            if out.success() {
                _default_arch = _arch_arm64;
            } else {
                _default_arch = _arch_x86_64;
            }
        }
    } else {
        _default_arch = _arch_x86_64;
    }

    if _default_arch == "" {
        warn!("<magenta>[WARN]</> Failed to detect arch, default is 'x86_64'.");
        _default_arch = _arch_x86_64;
    };
}

    let rig = Command::new("RIG -- The R Installation Manager")
        .version("0.3.1")
        .about(HELP_ABOUT)
        .arg_required_else_help(true)
        .term_width(80);

    let cmd_default = Command::new("default")
        .about("Print or set default R version")
        .aliases(&["switch"])
        .long_about(HELP_DEFAULT)
        .after_help(HELP_DEFAULT_EXAMPLES)
        .arg(
            Arg::new("version")
                .help("new default R version to set")
                .required(false),
        );

    let cmd_list = Command::new("list")
        .aliases(&["ls"])
        .about("List installed R versions")
        .long_about(HELP_LIST);

    let mut cmd_add = Command::new("add")
        .about("Install a new R version")
        .long_about(HELP_ADD)
        .after_help(HELP_ADD_EXAMPLES)
        .aliases(&["install"]);

    cmd_add = cmd_add
        .arg(
            Arg::new("str")
                .help("R version to install")
                .default_value("release")
                .multiple_occurrences(false)
        )
        .arg(
            Arg::new("without-cran-mirror")
                .help("Do not set the cloud CRAN mirror")
                .long("without-cran-mirror")
                .required(false)
        )
        .arg(
            Arg::new("without-pak")
                .help("Do not install pak.")
                .long("without-pak")
                .required(false)
        )
        .arg(
            Arg::new("pak-version")
                .help("pak version to install.")
                .long("pak-version")
                .required(false)
                .possible_values(["stable", "rc", "devel"])
                .default_value("stable")
        );

#[cfg(any(target_os = "windows", target_os = "linux"))]
{
        cmd_add = cmd_add
        .arg(
            Arg::new("without-rspm")
                .help("Do not set up RSPM.")
                .long("without-rspm")
                .required(false)
        );
}

#[cfg(target_os = "macos")]
{
    cmd_add = cmd_add
        .arg(
            Arg::new("arch")
                .help(HELP_ARCH)
                .short('a')
                .long("arch")
                .required(false)
                .default_value(&_default_arch)
                .possible_values(["arm64", "x86_64"])
        );
}

    let cmd_rm = Command::new("rm")
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

    let mut cmd_system = Command::new("system")
        .about("Manage current installations")
        .long_about(HELP_SYSTEM)
        .arg_required_else_help(true);

    let cmd_system_links = Command::new("make-links")
        .about("Create R-* quick links")
        .long_about(HELP_SYSTEM_LINKS);

    let cmd_system_lib = Command::new("create-lib")
        .about("Create current user's package libraries")
        .long_about(HELP_SYSTEM_LIB)
        .arg(
            Arg::new("version")
                .help("R versions to create the library for (default: all)")
                .required(false)
                .multiple_occurrences(true),
        );

    let cmd_system_pak = Command::new("add-pak")
        .about("Install or update pak for an R version")
        .long_about(HELP_SYSTEM_ADDPAK)
        .arg(
            Arg::new("devel")
                .help("Install the development version of pak (deprecated)")
                .long("devel")
                .required(false),
        )
        .arg(
            Arg::new("pak-version")
                .help("pak version to install.")
                .long("pak-version")
                .required(false)
                .possible_values(["stable", "rc", "devel"])
                .default_value("stable")
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
    let cmd_system_cleanreg = Command::new("clean-registry")
        .about("clean stale R related entries in the registry")
        .long_about(HELP_SYSTEM_CLEANREG);

    cmd_system = cmd_system
        .subcommand(cmd_system_cleanreg)
}

#[cfg(target_os = "macos")]
{
    let cmd_system_ortho = Command::new("make-orthogonal")
        .about("Make installed versions orthogonal")
        .long_about(HELP_SYSTEM_ORTHO)
        .arg(
            Arg::new("version")
                .help("R versions to update (default: all)")
                .required(false)
                .multiple_occurrences(true),
        );

    let cmd_system_rights = Command::new("fix-permissions")
        .about("Restrict system library permissions to admin")
        .long_about(HELP_SYSTEM_FIXPERMS)
        .arg(
            Arg::new("version")
                .help("R versions to update (default: all)")
                .required(false)
                .multiple_occurrences(true),
        );

    let cmd_system_forget = Command::new("forget")
        .about("Make system forget about R installations")
        .long_about(HELP_SYSTEM_FORGET);


    let cmd_system_noopenmp = Command::new("no-openmp")
        .about("Remove OpemMP (-fopenmp) option for Apple compilers")
        .long_about(HELP_SYSTEM_NO_OPENMP)
        .arg(
            Arg::new("version")
                .help("R versions to update (default: all)")
                .required(false)
                .multiple_occurrences(true)
        );

    let cmd_system_allow_debugger = Command::new("allow-debugger")
        .about("Allow debugging R with lldb and gdb")
        .long_about(HELP_SYSTEM_ALLOW_DEBUGGER)
        .arg(
            Arg::new("all")
                .help("Update all R versions")
                .long("all")
                .required(false),
        )
        .arg(
            Arg::new("version")
                .help("R versions to update (default is the default R version)")
                .required(false)
                .multiple_occurrences(true)
        );

    let cmd_system_allow_core_dumps = Command::new("allow-core-dumps")
        .about("Allow creating core dumps when R crashes")
        .long_about(HELP_SYSTEM_ALLOW_CORE_DUMPS)
        .arg(
            Arg::new("all")
                .help("Update all R versions")
                .long("all")
                .required(false),
        )
        .arg(
            Arg::new("version")
                .help("R versions to update (default is the default R version)")
                .required(false)
                .multiple_occurrences(true)
        );

    cmd_system = cmd_system
        .subcommand(cmd_system_ortho)
        .subcommand(cmd_system_rights)
        .subcommand(cmd_system_forget)
        .subcommand(cmd_system_noopenmp)
        .subcommand(cmd_system_allow_debugger)
        .subcommand(cmd_system_allow_core_dumps);
}

    cmd_system = cmd_system
        .subcommand(cmd_system_links)
        .subcommand(cmd_system_lib)
        .subcommand(cmd_system_pak);

    let mut cmd_resolve = Command::new("resolve")
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
                .default_value(&_default_arch)
                .possible_values(["arm64", "x86_64"])
        );
}

    let cmd_rstudio = Command::new("rstudio")
        .about("Start RStudio with specified R version")
        .long_about(HELP_RSTUDIO)
        .arg(
            Arg::new("version")
                .help("R version to start")
                .multiple_occurrences(false)
                .required(false)
        )
        .arg(
            Arg::new("project-file")
                .help("RStudio project file (.Rproj) to open")
                .multiple_occurrences(false)
                .required(false)
        );

    rig
        .arg(
            Arg::new("quiet")
                .help("Suppress output (overrides `--verbose`)")
                .short('q')
                .long("quiet")
                .required(false))
        .arg(
            Arg::new("verbose")
                .help("Verbose output")
                .short('v')
                .long("verbose")
                .required(false)
                .multiple_occurrences(true))
        .subcommand(cmd_default)
        .subcommand(cmd_list)
        .subcommand(cmd_add)
        .subcommand(cmd_rm)
        .subcommand(cmd_system)
        .subcommand(cmd_resolve)
        .subcommand(cmd_rstudio)
        .after_help(HELP_EXAMPLES)
}

pub fn parse_args() -> ArgMatches {
    rig_app().get_matches()
}
