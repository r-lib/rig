// ------------------------------------------------------------------------
// Arguemnt parsing
// ------------------------------------------------------------------------

// crates used here need to go in build-dependencies as well !!!

use clap::{Arg, ArgMatches, Command};

#[cfg(target_os = "macos")]
use simplelog::*;

#[cfg(target_os = "windows")]
mod windows_arch;
#[cfg(target_os = "windows")]
use crate::windows_arch::*;

std::include!("help-common.in");

#[cfg(target_os = "macos")]
std::include!("help-macos.in");

#[cfg(target_os = "windows")]
std::include!("help-windows.in");

#[cfg(target_os = "linux")]
std::include!("help-linux.in");

pub fn rig_app() -> Command {
    let _arch_x86_64: &'static str = "x86_64";
    let _arch_arm64: &'static str = "arm64";
    let _arch_aarch64: &'static str = "aarch64";
    let mut _default_arch: &'static str = "";
    let app_types = [
        "api",
        "shiny",
        "quarto-shiny",
        "rmd-shiny",
        "quarto-static",
        "rmd-static",
        "static"
    ];

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
            warn!("Failed to detect arch, default is 'x86_64'.");
            _default_arch = _arch_x86_64;
        };
    }

    #[cfg(target_os = "windows")]
    {
	_default_arch = match get_native_arch() {
	    "aarch64" => _arch_aarch64,
	    _ => _arch_x86_64
	};
    }

    let mut rig = Command::new("RIG -- The R Installation Manager")
        .version(clap::crate_version!())
        .about(HELP_ABOUT_REAL.as_str())
        .arg_required_else_help(true)
        .term_width(80);

    let cmd_default = Command::new("default")
        .about("Print or set default R version [alias: switch]")
        .display_order(0)
        .aliases(&["switch"])
        .long_about(HELP_DEFAULT)
        .after_help(HELP_DEFAULT_EXAMPLES)
        .arg(
            Arg::new("version")
                .help("new default R version to set")
                .required(false),
        )
        .arg(
            Arg::new("json")
                .help("JSON output")
                .long("json")
                .num_args(0)
                .required(false),
        );

    let cmd_list = Command::new("list")
        .aliases(&["ls"])
        .about("List installed R versions [alias: ls]")
        .display_order(0)
        .long_about(HELP_LIST)
        .arg(
            Arg::new("json")
                .help("JSON output")
                .long("json")
                .num_args(0)
                .required(false),
        )
        .arg(
            Arg::new("plain")
                .help("plain output, only the version names, one per line")
                .long("plain")
                .num_args(0)
                .required(false),
        );

    let mut cmd_add = Command::new("add")
        .about("Install a new R version [alias: install]")
        .display_order(0)
        .long_about(HELP_ADD)
        .after_help(HELP_ADD_EXAMPLES)
        .aliases(&["install"]);

    cmd_add = cmd_add
        .arg(
            Arg::new("str")
                .help("R version to install")
                .default_value("release"),
        )
        .arg(
            Arg::new("without-cran-mirror")
                .help("Do not set the cloud CRAN mirror")
                .long("without-cran-mirror")
                .num_args(0)
                .required(false),
        )
        .arg(
            Arg::new("without-pak")
                .help("Do not install pak.")
                .long("without-pak")
                .num_args(0)
                .required(false),
        )
        .arg(
            Arg::new("pak-version")
                .help("pak version to install.")
                .long("pak-version")
                .required(false)
                .value_parser(["stable", "rc", "devel"])
                .default_value("stable"),
        );

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    {
        cmd_add = cmd_add.arg(
            Arg::new("without-p3m")
                .aliases(&["without-rspm"])
                .help("Do not set up P3M. [alias: --without-rspm]")
                .long("without-p3m")
                .num_args(0)
                .required(false),
        );
    }

    #[cfg(target_os = "linux")]
    {
        cmd_add = cmd_add.arg(
            Arg::new("without-sysreqs")
                .help("Do not set up system requirements installation.")
                .long("without-sysreqs")
                .num_args(0)
                .required(false),
        );
    }

    #[cfg(target_os = "windows")]
    {
	cmd_add = cmd_add.arg(
	    Arg::new("without-translations")
		.help("Do not install translations.")
		.long("without-translations")
                .num_args(0)
		.required(false),
	).arg(
	    Arg::new("with-desktop-icon")
		.help("Install a desktop icon.")
		.long("with-desktop-icon")
                .num_args(0)
		.required(false),
	);
    }

    #[cfg(target_os = "macos")]
    {
        cmd_add = cmd_add.arg(
            Arg::new("arch")
                .help(HELP_ARCH)
                .short('a')
                .long("arch")
                .required(false)
                .default_value(&_default_arch)
                .value_parser(["arm64", "x86_64"]),
        );
    }

    #[cfg(target_os = "windows")]
    {
        cmd_add = cmd_add.arg(
            Arg::new("arch")
                .help(HELP_ARCH)
                .short('a')
                .long("arch")
                .required(false)
                .default_value(&_default_arch)
                .value_parser(["x86_64", "aarch64", "arm64"]),
        );
    }

    let cmd_rm = Command::new("rm")
        .about("Remove R versions [aliases: del, remove, delete]")
        .display_order(0)
        .long_about(HELP_RM)
        .aliases(&["del", "remove", "delete"])
        .arg(
            Arg::new("version")
                .help("versions to remove")
                .action(clap::ArgAction::Append)
                .required(false),
        )
        .arg(
            Arg::new("all")
                .help("remove all versions (TODO)")
                .long("all")
                .num_args(0)
                .required(false),
        );

    let mut cmd_available = Command::new("available")
        .about("List R versions available to install.")
        .display_order(0)
        .long_about(HELP_AVAILABLE);

    cmd_available = cmd_available.arg(
        Arg::new("json")
            .help("JSON output")
            .num_args(0)
            .long("json")
            .required(false)
    )
    .arg(
        Arg::new("all")
            .num_args(0)
            .help("List all available versions.")
            .long("all")
            .required(false),
    )
    .arg(
        Arg::new("platform")
            .help("Use this platform, instead of auto-detecting it.")
            .long("platform")
            .required(false)
    )
    .arg(
        Arg::new("list-distros")
            .help("List supported Linux distributions instead of R versions.")
            .long("list-distros")
            .num_args(0)
            .required(false)
            .conflicts_with("list-rtools-versions")
    )
    .arg(
        Arg::new("list-rtools-versions")
            .help("List available Rtools versions instead of R versions.")
            .long("list-rtools-versions")
            .num_args(0)
            .required(false)
            .conflicts_with("list-distros")
        );

    #[cfg(any(target_os = "linux"))]
    {
        cmd_available = cmd_available.arg(
            Arg::new("arch")
                .help("Use this architecture, instead of auto-detecting it.")
                .short('a')
                .long("arch")
                .required(false)
                .value_parser(clap::value_parser!(String))
        );
    }

    #[cfg(target_os = "macos")]
    {
        cmd_available = cmd_available.arg(
            Arg::new("arch")
                .help(HELP_ARCH)
                .short('a')
                .long("arch")
                .required(false)
                .default_value(&_default_arch)
                .value_parser(["arm64", "aarch64", "x86_64"]),
        );
    }

    #[cfg(target_os = "windows")]
    {
        cmd_available = cmd_available.arg(
            Arg::new("arch")
                .help(HELP_ARCH)
                .short('a')
                .long("arch")
                .required(false)
                .default_value(&_default_arch)
                .value_parser(["x86_64", "aarch64", "arm64"]),
        );
    }

    let mut cmd_system = Command::new("system")
        .about("Manage current installations")
        .long_about(HELP_SYSTEM)
        .display_order(0)
        .arg_required_else_help(true);

    let cmd_system_links = Command::new("make-links")
        .about("Create R-* quick links")
        .display_order(0)
        .long_about(HELP_SYSTEM_LINKS);

    let cmd_system_lib = Command::new("setup-user-lib")
        .about("Set up automatic user package libraries [alias: create-lib]")
        .long_about(HELP_SYSTEM_LIB)
        .display_order(0)
        .aliases(&["create-lib"])
        .arg(
            Arg::new("version")
                .help("R versions (default: all)")
                .required(false)
                .action(clap::ArgAction::Append),
        );

    let cmd_system_pak = Command::new("add-pak")
        .about("Install or update pak for an R version")
        .long_about(HELP_SYSTEM_ADDPAK)
        .display_order(0)
        .arg(
            Arg::new("devel")
                .help("Install the development version of pak (deprecated)")
                .long("devel")
                .num_args(0)
                .required(false),
        )
        .arg(
            Arg::new("pak-version")
                .help("pak version to install.")
                .long("pak-version")
                .required(false)
                .value_parser(["stable", "rc", "devel"])
                .default_value("stable"),
        )
        .arg(
            Arg::new("all")
                .help("Install pak for all R versions")
                .long("all")
                .num_args(0)
                .required(false),
        )
        .arg(
            Arg::new("version")
                .help("R versions to install/update pak for")
                .required(false)
                .action(clap::ArgAction::Append),
        );

    #[cfg(target_os = "windows")]
    {
        let cmd_system_cleanreg = Command::new("clean-registry")
            .about("Clean stale R related entries in the registry")
            .display_order(0)
            .long_about(HELP_SYSTEM_CLEANREG);
        cmd_system = cmd_system.subcommand(cmd_system_cleanreg);

	let cmd_system_update_rtools40 = Command::new("update-rtools40")
	    .about("Update Rtools40 MSYS2 packages")
	    .display_order(0)
	    .long_about(HELP_SYSTEM_UPDATE_RTOOLS40);
	cmd_system = cmd_system.subcommand(cmd_system_update_rtools40);

	let cmd_system_rtools_ls = Command::new("list")
	    .about("List installed Rtools vesions [alias: ls]")
	    .long_about(HELP_SYSTEM_RTOOLS_LS)
        .display_order(0)
	    .aliases(&["ls"])
	    .arg(
		Arg::new("json")
		    .help("JSON output")
		    .long("json")
		    .num_args(0)
		    .required(false)
	    );
	let cmd_system_rtools_add = Command::new("add")
	    .about("Install new Rtools version [alias: install]")
	    .long_about(HELP_SYSTEM_RTOOLS_ADD)
        .display_order(0)
	    .aliases(&["install"])
	    .arg(
		Arg::new("version")
		    .help("Rtools version to add, e.g. '43'")
		    .default_value("all")
	    );
	let cmd_system_rtools_rm = Command::new("rm")
	    .about("Remove rtools versions [aliases: del, remove, delete]")
	    .long_about(HELP_SYSTEM_RTOOLS_RM)
        .display_order(0)
        .aliases(&["del", "remove", "delete"])
	    .arg(
		Arg::new("version")
		    .help("versions to remove")
		    .action(clap::ArgAction::Append)
		    .required(false)
	    );

	let cmd_system_rtools = Command::new("rtools")
	    .about("Manage Rtools installations")
        .display_order(0)
        .arg_required_else_help(true)
	    .subcommand(cmd_system_rtools_ls)
	    .subcommand(cmd_system_rtools_add)
	    .subcommand(cmd_system_rtools_rm);
	cmd_system = cmd_system.subcommand(cmd_system_rtools);
    }

    #[cfg(target_os = "macos")]
    {
        let cmd_system_ortho = Command::new("make-orthogonal")
            .about("Make installed versions orthogonal")
            .long_about(HELP_SYSTEM_ORTHO)
            .display_order(0)
            .arg(
                Arg::new("version")
                    .help("R versions to update (default: all)")
                    .required(false)
                    .action(clap::ArgAction::Append),
            );

        let cmd_system_rights = Command::new("fix-permissions")
            .about("Restrict system library permissions to admin")
            .long_about(HELP_SYSTEM_FIXPERMS)
            .display_order(0)
            .arg(
                Arg::new("version")
                    .help("R versions to update (default: all)")
                    .required(false)
                    .action(clap::ArgAction::Append),
            );

        let cmd_system_forget = Command::new("forget")
            .about("Make system forget about R installations")
            .display_order(0)
            .long_about(HELP_SYSTEM_FORGET);

        let cmd_system_noopenmp = Command::new("no-openmp")
            .about("Remove OpenMP (-fopenmp) option for Apple compilers")
            .long_about(HELP_SYSTEM_NO_OPENMP)
            .display_order(0)
            .arg(
                Arg::new("version")
                    .help("R versions to update (default: all)")
                    .required(false)
                    .action(clap::ArgAction::Append),
            );

        let cmd_system_allow_debugger = Command::new("allow-debugger")
            .about("Allow debugging R with lldb and gdb")
            .long_about(HELP_SYSTEM_ALLOW_DEBUGGER)
            .display_order(0)
            .arg(
                Arg::new("all")
                    .help("Update all R versions")
                    .long("all")
                    .num_args(0)
                    .required(false),
            )
            .arg(
                Arg::new("version")
                    .help("R versions to update (default is the default R version)")
                    .required(false)
                    .action(clap::ArgAction::Append),
            );

        let cmd_system_allow_debugger_rstudio = Command::new("allow-debugger-rstudio")
            .about("Allow debugging RStudio with lldb and gdb")
            .display_order(0)
            .long_about(HELP_SYSTEM_ALLOW_DEBUGGER_RSTUDIO);

        let cmd_system_allow_core_dumps = Command::new("allow-core-dumps")
            .about("Allow creating core dumps when R crashes")
            .long_about(HELP_SYSTEM_ALLOW_CORE_DUMPS)
            .display_order(0)
            .arg(
                Arg::new("all")
                    .help("Update all R versions")
                    .long("all")
                    .num_args(0)
                    .required(false),
            )
            .arg(
                Arg::new("version")
                    .help("R versions to update (default is the default R version)")
                    .required(false)
                    .action(clap::ArgAction::Append),
            );

        cmd_system = cmd_system
            .subcommand(cmd_system_ortho)
            .subcommand(cmd_system_rights)
            .subcommand(cmd_system_forget)
            .subcommand(cmd_system_noopenmp)
            .subcommand(cmd_system_allow_debugger)
            .subcommand(cmd_system_allow_debugger_rstudio)
            .subcommand(cmd_system_allow_core_dumps);
    }

    #[cfg(target_os = "linux")]
    {
        let cmd_system_detect_platform = Command::new("detect-platform")
            .about("Detect operating system version and distribution.")
            .display_order(0)
            .arg(
                Arg::new("json")
                    .help("JSON output")
                    .long("json")
                    .num_args(0)
                    .required(false)
            );

        cmd_system = cmd_system.subcommand(cmd_system_detect_platform);
    }

    cmd_system = cmd_system
        .subcommand(cmd_system_links)
        .subcommand(cmd_system_lib)
        .subcommand(cmd_system_pak);

    let mut cmd_resolve = Command::new("resolve")
        .about("Resolve a symbolic R version")
        .display_order(0)
        .long_about(HELP_RESOLVE)
        .after_help(HELP_RESOLVE_EXAMPLES);

    cmd_resolve = cmd_resolve
        .arg(
            Arg::new("str")
                .help("symbolic version string to resolve")
                .required(true),
        )
        .arg(
            Arg::new("json")
                .help("JSON output")
                .long("json")
                .num_args(0)
                .required(false),
        )
        .arg(
            Arg::new("platform")
                .help("Use this platform, instead of auto-detecting it.")
                .long("platform")
                .required(false)
        );

    #[cfg(any(target_os = "linux"))]
    {
        cmd_resolve = cmd_resolve.arg(
            Arg::new("arch")
                .help("Use this architecture, instead of auto-detecting it.")
                .short('a')
                .long("arch")
                .required(false)
                .value_parser(clap::value_parser!(String))
        );
    }

    #[cfg(any(target_os = "macos"))]
    {
        cmd_resolve = cmd_resolve.arg(
            Arg::new("arch")
                .help(HELP_ARCH)
                .short('a')
                .long("arch")
                .required(false)
                .default_value(&_default_arch)
                .value_parser(["arm64", "x86_64"]),
        );
    }

    #[cfg(any(target_os = "windows"))]
    {
        cmd_resolve = cmd_resolve.arg(
            Arg::new("arch")
                .help(HELP_ARCH)
                .short('a')
                .long("arch")
                .required(false)
                .default_value(&_default_arch)
                .value_parser(["x86_64", "aarch64", "arm64"]),
        );
    }

    let mut cmd_rstudio = Command::new("rstudio")
        .about("Start RStudio with specified R version")
        .display_order(0)
        .long_about(HELP_RSTUDIO);

    cmd_rstudio = cmd_rstudio
        .arg(
            Arg::new("version")
                .help("R version to start")
                .required(false),
        )
        .arg(
            Arg::new("project-file")
                .help("RStudio project file (.Rproj) to open")
                .required(false),
        );

    #[cfg(target_os = "windows")]
    {
        cmd_rstudio = cmd_rstudio
	    .arg(
	        Arg::new("config-path")
		    .help("Do not start RStudio, only print the path of the RStudio config directory")
		    .long("config-path")
		    .required(false)
		    .num_args(0)
	    );
    }

    let cmd_library = Command::new("library")
        .about("Manage package libraries [alias: lib] (experimental)")
        .display_order(0)
        .long_about(HELP_LIBRARY)
        .aliases(&["lib"])
        .arg_required_else_help(true)
        .arg(
            Arg::new("json")
                .help("JSON output")
                .long("json")
                .num_args(0)
                .required(false),
        )
        .subcommand(
            Command::new("list")
                .aliases(&["ls"])
                .about("List libraries [alias: ls]")
                .display_order(0)
                .arg(
                    Arg::new("json")
                        .help("JSON output")
                        .long("json")
                        .num_args(0)
                        .required(false),
                ),
        )
        .subcommand(
            Command::new("add")
                .about("Add a new library")
                .display_order(0)
                .arg(
                    Arg::new("lib-name")
                        .help("name of new library")
                        .required(true),
                ),
        )
        .subcommand(
            Command::new("rm")
                .about("Remove a library")
                .display_order(0)
                .arg(
                    Arg::new("lib-name")
                        .help("name of library to remove")
                        .required(true),
                ),
        )
        .subcommand(
            Command::new("default")
                .about("Set the default library")
                .display_order(0)
                .arg(
                    Arg::new("lib-name")
                        .help("library name to set as default")
                        .required(false),
                )
                .arg(
                    Arg::new("json")
                        .help("JSON output")
                        .long("json")
                        .num_args(0)
                        .required(false),
                ),
        );

    #[cfg(target_os = "macos")]
    {
        let cmd_sysreqs = Command::new("sysreqs")
            .about("Manage R-related system libraries and tools (experimental)")
            .display_order(0)
            .long_about(HELP_SYSREQS)
            .arg_required_else_help(true)
            .arg(
                Arg::new("json")
                    .help("JSON output")
                    .long("json")
                    .num_args(0)
                    .required(false),
            )
            .subcommand(
                Command::new("add")
                    .about("Install system library or tool")
                    .display_order(0)
                    .arg(
                        Arg::new("name")
                            .help("system tool to install")
                            .required(true)
                            .action(clap::ArgAction::Append),
                    )
                    .arg(
                        Arg::new("arch")
                            .help("Architecture to install for")
                            .short('a')
                            .long("arch")
                            .required(false)
                            .default_value(&_default_arch)
                            .value_parser(["arm64", "x86_64"]),
                    )
            )
            .subcommand(
                Command::new("list")
                    .about("List available system libraries and tools")
                    .display_order(0)
                    .arg(
                        Arg::new("json")
                            .help("JSON output")
                            .long("json")
                            .num_args(0)
                            .required(false),
                    )
            )
            .subcommand(
                Command::new("info")
                    .about("Information about a system tool")
                    .display_order(0)
                    .arg(
                        Arg::new("name")
                            .help("system tool to show")
                            .required(true),
                    )
                    .arg(
                        Arg::new("json")
                            .help("JSON output")
                            .long("json")
                            .num_args(0)
                            .required(false),
                    )
            );
        rig = rig.subcommand(cmd_sysreqs);
    }

    let cmd_run = Command::new("run")
        .about("Run R, an R script or an R project")
        .display_order(0)
        .long_about(HELP_RUN)
        .arg(
            Arg::new("r-version")
                .help("R version to use")
                .short('r')
                .long("r-version")
                .required(false)
        )
        .arg(
            Arg::new("app-type")
                .help("Explicitly specify app type to run")
                .short('t')
                .long("app-type")
                .required(false)
                .value_parser(app_types)
                .conflicts_with("eval")
                .conflicts_with("script")
        )
        .arg(
            Arg::new("dry-run")
                .help("Show the command, but do not run it")
                .long("dry-run")
                .required(false)
                .action(clap::ArgAction::SetTrue)
        )
        .arg(
            Arg::new("startup")
                .help("Print R startup message")
                .long("startup")
                .action(clap::ArgAction::SetTrue)
                .required(false)
        )
        .arg(
            Arg::new("no-startup")
                .help("Do not print R startup message")
                .long("no-startup")
                .action(clap::ArgAction::SetTrue)
                .required(false)
                .conflicts_with("startup")
        )
        .arg(
            Arg::new("echo")
                .help("Print input to R")
                .long("echo")
                .action(clap::ArgAction::SetTrue)
                .required(false)
        )
        .arg(
            Arg::new("no-echo")
                .help("Do not print input to R")
                .long("no-echo")
                .action(clap::ArgAction::SetTrue)
                .required(false)
                .conflicts_with("echo")
        )
        .arg(
            Arg::new("eval")
                .help("R expression to evaluate")
                .short('e')
                .long("eval")
                .num_args(1)
                .required(false)
        )
        .arg(
            Arg::new("script")
                .help("R script file to run")
                .short('f')
                .long("script")
                .num_args(1)
                .required(false)
                .conflicts_with("eval")
        )
        .arg(
            Arg::new("command")
                .help("R script or project to run, with parameters")
                .required(false)
                .action(clap::ArgAction::Append)
        );

    let cmd_proj = Command::new("proj")
        .about("Manage R projects (experimental)")
        .display_order(0)
        .long_about("TODO")
        .arg_required_else_help(true)
        .arg(
            Arg::new("json")
                .help("JSON output")
                .long("json")
                .num_args(0)
                .required(false),
        )
        .subcommand(
            Command::new("deps")
                .about("Show project dependencies")
                .display_order(0)
                .arg(
                    Arg::new("json")
                        .help("JSON output")
                        .long("json")
                        .num_args(0)
                        .required(false),
                )
        )
        .subcommand(
            Command::new("solve")
                .about("Solve project dependencies")
                .display_order(0)
                .arg(
                    Arg::new("json")
                        .help("JSON output")
                        .long("json")
                        .num_args(0)
                        .required(false),
                )
        );
    rig = rig.subcommand(cmd_proj);

    let cmd_repos = Command::new("repos")
        .about("Manage package repositories")
        .display_order(0)
        .long_about("TODO")
        .arg_required_else_help(true)
        .arg(
            Arg::new("json")
                .help("JSON output")
                .long("json")
                .num_args(0)
                .required(false),
        )
        .subcommand(
            Command::new("list-packages")
                .about("List packages in package repositories")
                .display_order(0)
                .arg(
                    Arg::new("json")
                        .help("JSON output")
                        .long("json")
                        .num_args(0)
                        .required(false),
                )
        )
        .subcommand(
            Command::new("package-info")
                .about("Information about a package in the repositories")
                .display_order(0)
                .arg(
                    Arg::new("package")
                        .help("package name to show")
                        .required(true),
                )
                .arg(
                    Arg::new("json")
                        .help("JSON output")
                        .long("json")
                        .num_args(0)
                        .required(false),
                )
        );
    rig = rig.subcommand(cmd_repos);

    rig = rig.arg(
        Arg::new("quiet")
            .help("Suppress output (overrides `--verbose`)")
            .short('q')
            .num_args(0)
            .long("quiet")
            .required(false),
    )
    .arg(
        Arg::new("verbose")
            .help("Verbose output")
            .short('v')
            .long("verbose")
            .required(false)
            .action(clap::ArgAction::Count),
    )
    .arg(
        Arg::new("json")
            .help("Output JSON")
            .long("json")
            .num_args(0)
            .required(false),
    )
    .subcommand(cmd_default)
    .subcommand(cmd_list)
    .subcommand(cmd_add)
    .subcommand(cmd_rm)
    .subcommand(cmd_system)
    .subcommand(cmd_resolve)
    .subcommand(cmd_rstudio)
    .subcommand(cmd_library)
    .subcommand(cmd_available)
    .subcommand(cmd_run)
    .after_help(HELP_EXAMPLES);

    rig
}

pub fn parse_args() -> ArgMatches {
    rig_app().get_matches()
}
