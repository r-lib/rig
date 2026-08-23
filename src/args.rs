// ------------------------------------------------------------------------
// Arguemnt parsing
// ------------------------------------------------------------------------

// crates used here need to go in build-dependencies as well !!!

use clap::{Arg, ArgGroup, ArgMatches, Command};

#[cfg(target_os = "macos")]
use log::warn;

#[cfg(target_os = "windows")]
mod windows_arch;
#[cfg(target_os = "windows")]
use crate::windows_arch::*;

// The command help text is written in Markdown in `src/help/*.md`: the lead
// paragraph is the short summary (`about`, `ABOUT_*`) and the rest is the long
// `--help` text (`long_about`, `HELP_*`). The `xtask` crate
// (`cargo xtask gen-help`) renders both to colored ANSI into the generated,
// committed `src/help-generated.in` below. Edit the Markdown, then regenerate
// with `make help`. Do not edit `src/help-generated.in` by hand.
std::include!("help-generated.in");

// Accepted values of `--pak-version`. "rc" is deprecated: it is the same as
// "stable" now, so we still accept it, but do not advertise it in the help.
fn pak_version_values() -> clap::builder::PossibleValuesParser {
    clap::builder::PossibleValuesParser::new([
        clap::builder::PossibleValue::new("stable"),
        clap::builder::PossibleValue::new("rc").hide(true),
        clap::builder::PossibleValue::new("devel"),
    ])
}

fn reference_mode() -> bool {
    std::env::var_os("RIG_GEN_REFERENCE").is_some()
}

trait PlatformItem {
    fn platform(self, os: &str) -> Self;
}

impl PlatformItem for Command {
    fn platform(self, os: &str) -> Self {
        if reference_mode() {
            let about = self.get_about().map(|s| s.to_string());
            let cmd = self.hide(false);
            match about {
                Some(about) => cmd.about(format!("{} {{platform:{}}}", about, os)),
                None => cmd,
            }
        } else {
            self.hide(std::env::consts::OS != os)
        }
    }
}

impl PlatformItem for Arg {
    fn platform(self, os: &str) -> Self {
        if reference_mode() {
            let help = self.get_help().map(|s| s.to_string());
            let arg = self.hide(false);
            match help {
                Some(help) => arg.help(format!("{} {{platform:{}}}", help, os)),
                None => arg,
            }
        } else {
            self.hide(std::env::consts::OS != os)
        }
    }
}

fn add_name_headers(cmd: &mut Command, path: &str) {
    // clap's default layout, minus `{about}` (moved to `before_long_help`).
    const BODY: &str = "{before-help}{usage-heading} {usage}\n\n{all-args}{after-help}";

    for sub in cmd.get_subcommands_mut() {
        let full = format!("{} {}", path, sub.get_name());
        if let Some(about) = sub.get_about().map(|s| s.ansi().to_string()) {
            let template = format!(
                "\u{1b}[1m\u{1b}[34mName:\u{1b}[39m\u{1b}[22m\n  {} - {}\n\n{}",
                full, about, BODY
            );
            let long = sub.get_long_about().map(|s| s.ansi().to_string());
            let mut updated = std::mem::take(sub).help_template(template);
            if let Some(long) = long {
                updated = updated.before_long_help(long);
            }
            *sub = updated;
        }
        add_name_headers(sub, &full);
    }
}

// Tag every aliased subcommand's `about` with an `{aliases:<a>,<b>}` sentinel,
// so that `website/gen-cli-reference.sh` can pick the aliases up from the
// `--help` output and document them. Only used in reference mode.
//
// Must run *after* `add_name_headers`, which bakes the `about` text into each
// subcommand's `Name:` help template; otherwise the sentinel would show up
// there as well.
fn add_alias_markers(cmd: &mut Command) {
    for sub in cmd.get_subcommands_mut() {
        let aliases: Vec<String> = sub.get_all_aliases().map(|a| a.to_string()).collect();
        if !aliases.is_empty() {
            if let Some(about) = sub.get_about().map(|s| s.to_string()) {
                let about = format!("{} {{aliases:{}}}", about, aliases.join(","));
                *sub = std::mem::take(sub).about(about);
            }
        }
        add_alias_markers(sub);
    }
}

fn cmd_rtools() -> Command {
    let cmd_rtools_ls = Command::new("list")
        .about(ABOUT_RTOOLS_LIST)
        .long_about(HELP_RTOOLS_LIST)
        .display_order(0)
        .aliases(["ls"])
        .arg(
            Arg::new("json")
                .help("JSON output")
                .long("json")
                .num_args(0)
                .required(false),
        );
    let cmd_rtools_add = Command::new("add")
        .about(ABOUT_RTOOLS_ADD)
        .long_about(HELP_RTOOLS_ADD)
        .display_order(0)
        .aliases(["install"])
        .arg(
            Arg::new("version")
                .help("Rtools version to add, e.g. '43'")
                .default_value("all"),
        )
        .arg(
            Arg::new("arch")
                .help("Architecture to install Rtools for (default: native arch).")
                .short('a')
                .long("arch")
                .required(false)
                .value_parser(["x86_64", "aarch64", "arm64"]),
        );
    let cmd_rtools_rm = Command::new("rm")
        .about(ABOUT_RTOOLS_RM)
        .long_about(HELP_RTOOLS_RM)
        .display_order(0)
        .aliases(["del", "remove", "delete"])
        .arg(
            Arg::new("version")
                .help("versions to remove")
                .action(clap::ArgAction::Append)
                .required(false),
        )
        .arg(
            Arg::new("arch")
                .help("Architecture of Rtools to remove (default: native arch).")
                .short('a')
                .long("arch")
                .required(false)
                .value_parser(["x86_64", "aarch64", "arm64"]),
        );

    Command::new("rtools")
        .about(ABOUT_RTOOLS)
        .long_about(HELP_RTOOLS)
        .display_order(0)
        .platform("windows")
        .arg_required_else_help(true)
        .subcommand(cmd_rtools_ls)
        .subcommand(cmd_rtools_add)
        .subcommand(cmd_rtools_rm)
}

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
        "static",
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

        if _default_arch.is_empty() {
            warn!("Failed to detect arch, default is 'x86_64'.");
            _default_arch = _arch_x86_64;
        };
    }

    #[cfg(target_os = "windows")]
    {
        _default_arch = match get_native_arch() {
            "aarch64" => _arch_aarch64,
            _ => _arch_x86_64,
        };
    }

    let styles = clap::builder::Styles::styled()
        .header(clap::builder::styling::AnsiColor::Blue.on_default().bold())
        .usage(clap::builder::styling::AnsiColor::Blue.on_default().bold())
        .literal(clap::builder::styling::AnsiColor::Green.on_default())
        .placeholder(clap::builder::styling::AnsiColor::Cyan.on_default());

    // The `{version}` placeholder in the `Name:` line of `about.md` is filled
    // in here, so the top-level help shows the current rig version.
    let about = HELP_ABOUT.replace("{version}", clap::crate_version!());

    let mut rig = Command::new("RIG -- The R Installation Manager")
        .version(clap::crate_version!())
        .about(about)
        .styles(styles)
        .arg_required_else_help(true)
        .term_width(80);

    let cmd_default = Command::new("default")
        .about(ABOUT_DEFAULT)
        .display_order(0)
        .aliases(["switch"])
        .long_about(HELP_DEFAULT)
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
        .aliases(["ls"])
        .about(ABOUT_LIST)
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
        .about(ABOUT_ADD)
        .display_order(0)
        .long_about(HELP_ADD)
        .aliases(["install"]);

    cmd_add = cmd_add
        .arg(
            Arg::new("str")
                .help("R version to install")
                .default_value("release"),
        )
        .arg(
            Arg::new("with-repos")
                .help(
                    "Repositories to enable, in addition to the ones enabled by default.\n\
                    If --without-repos is also specified (without a value), then only these\n\
                    repositories will be enabled.",
                )
                .long("with-repos")
                .num_args(1)
                .require_equals(true)
                .required(false)
                .conflicts_with_all(["without-cran-mirror", "without-p3m"]),
        )
        .arg(
            Arg::new("without-repos")
                .help(
                    "Do not set up package repositories.\n\
                    Alternatively, specify which ones to skip, a comma-separated list.\n\
                    If --with-repos is also specified, then only the repositories in that\n\
                    argument will be enabled.",
                )
                .long("without-repos")
                .num_args(0..=1)
                .require_equals(true)
                .default_missing_value("ALL REPOSITORIES")
                .required(false)
                .conflicts_with_all(["without-cran-mirror", "without-p3m"]),
        )
        .arg(
            Arg::new("without-cran-mirror")
                .help(
                    "Do not set the cloud CRAN mirror.\n\
                    Deprecated in favor of --without-repos=cran.",
                )
                .long("without-cran-mirror")
                .num_args(0)
                .required(false)
                .conflicts_with_all(["with-repos", "without-repos"]),
        )
        .arg(
            Arg::new("without-p3m")
                .aliases(["without-rspm"])
                .help(
                    "Do not set up P3M. This is the default on macOS.\n\
                    Deprecated in favor of --without-repos=p3m. \n\
                    [alias: --without-rspm]",
                )
                .long("without-p3m")
                .num_args(0)
                .required(false)
                .conflicts_with_all(["with-repos", "without-repos"]),
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
                .value_parser(pak_version_values())
                .default_value("stable"),
        );

    {
        cmd_add = cmd_add.arg(
            Arg::new("without-sysreqs")
                .help("Do not set up system requirements installation.")
                .long("without-sysreqs")
                .num_args(0)
                .required(false)
                .platform("linux"),
        );
    }

    {
        cmd_add = cmd_add.arg(
            Arg::new("without-fonts")
                .help(
                    "Do not download fallback fonts. rig still writes the\n\
                    fontconfig configuration, but R will only see the fonts\n\
                    that are already installed on the system.",
                )
                .long("without-fonts")
                .num_args(0)
                .required(false)
                .platform("linux"),
        );
    }

    {
        cmd_add = cmd_add
            .arg(
                Arg::new("without-translations")
                    .help("Do not install translations.")
                    .long("without-translations")
                    .num_args(0)
                    .required(false)
                    .platform("windows"),
            )
            .arg(
                Arg::new("with-desktop-icon")
                    .help("Install a desktop icon.")
                    .long("with-desktop-icon")
                    .num_args(0)
                    .required(false)
                    .platform("windows"),
            )
            .arg(
                Arg::new("platform")
                    .help(
                        "Install the build for this platform, instead of \
                         auto-detecting it.\nUse `linux-portable` to install \
                         a portable (glibc/musl) build.",
                    )
                    .long("platform")
                    .required(false)
                    .platform("linux"),
            );
    }

    #[cfg(target_os = "macos")]
    {
        cmd_add = cmd_add.arg(
            Arg::new("arch")
                .help("Select architecture: arm64 or x86_64")
                .short('a')
                .long("arch")
                .required(false)
                .default_value(_default_arch)
                .value_parser(["arm64", "x86_64"]),
        );
    }

    #[cfg(target_os = "windows")]
    {
        cmd_add = cmd_add.arg(
            Arg::new("arch")
                .help("Select architecture: arm64 or x86_64")
                .short('a')
                .long("arch")
                .required(false)
                .default_value(_default_arch)
                .value_parser(["x86_64", "aarch64", "arm64"]),
        );
    }

    // On Linux `--arch` is accepted (so it is defined on all platforms) but
    // using it is an error, see sc_add() in src/linux.rs.
    #[cfg(target_os = "linux")]
    {
        cmd_add = cmd_add.arg(
            Arg::new("arch")
                .help("Select architecture: arm64 or x86_64")
                .short('a')
                .long("arch")
                .required(false),
        );
    }

    let cmd_rm = Command::new("rm")
        .about(ABOUT_RM)
        .display_order(0)
        .long_about(HELP_RM)
        .aliases(["del", "remove", "delete"])
        .arg(
            Arg::new("version")
                .help("versions to remove")
                .action(clap::ArgAction::Append)
                .required(false),
        );

    let mut cmd_available = Command::new("available")
        .about(ABOUT_AVAILABLE)
        .display_order(0)
        .long_about(HELP_AVAILABLE);

    cmd_available = cmd_available
        .arg(
            Arg::new("json")
                .help("JSON output")
                .num_args(0)
                .long("json")
                .required(false),
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
                .required(false),
        )
        .arg(
            Arg::new("list-distros")
                .help("List supported Linux distributions instead of R versions.")
                .long("list-distros")
                .num_args(0)
                .required(false)
                .conflicts_with("list-rtools-versions"),
        )
        .arg(
            Arg::new("list-rtools-versions")
                .help("List available Rtools versions instead of R versions.")
                .long("list-rtools-versions")
                .num_args(0)
                .required(false)
                .conflicts_with("list-distros"),
        );

    #[cfg(target_os = "linux")]
    {
        cmd_available = cmd_available.arg(
            Arg::new("arch")
                .help("Use this architecture, instead of auto-detecting it.")
                .short('a')
                .long("arch")
                .required(false)
                .value_parser(clap::value_parser!(String)),
        );
    }

    #[cfg(target_os = "macos")]
    {
        cmd_available = cmd_available.arg(
            Arg::new("arch")
                .help("Select architecture: arm64 or x86_64")
                .short('a')
                .long("arch")
                .required(false)
                .default_value(_default_arch)
                .value_parser(["arm64", "aarch64", "x86_64"]),
        );
    }

    #[cfg(target_os = "windows")]
    {
        cmd_available = cmd_available.arg(
            Arg::new("arch")
                .help("Select architecture: arm64 or x86_64")
                .short('a')
                .long("arch")
                .required(false)
                .default_value(_default_arch)
                .value_parser(["x86_64", "aarch64", "arm64"]),
        );
    }

    let mut cmd_system = Command::new("system")
        .about(ABOUT_SYSTEM)
        .long_about(HELP_SYSTEM)
        .display_order(0)
        .arg_required_else_help(true);

    let cmd_system_links = Command::new("make-links")
        .about(ABOUT_SYSTEM_MAKE_LINKS)
        .display_order(0)
        .long_about(HELP_SYSTEM_MAKE_LINKS);

    let cmd_system_lib = Command::new("setup-user-lib")
        .about(ABOUT_SYSTEM_SETUP_USER_LIB)
        .long_about(HELP_SYSTEM_SETUP_USER_LIB)
        .display_order(0)
        .aliases(["create-lib"])
        .arg(
            Arg::new("version")
                .help("R versions (default: all)")
                .required(false)
                .action(clap::ArgAction::Append),
        );

    let cmd_system_pak = Command::new("add-pak")
        .about(ABOUT_SYSTEM_ADD_PAK)
        .long_about(HELP_SYSTEM_ADD_PAK)
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
                .value_parser(pak_version_values())
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

    {
        // `clean-registry`, `update-rtools40` and `rtools` are real commands on
        // Windows, but hidden no-ops on macOS and Linux, so that they are
        // always available (e.g. in scripts).
        let cmd_system_cleanreg = Command::new("clean-registry")
            .about(ABOUT_SYSTEM_CLEAN_REGISTRY)
            .display_order(0)
            .platform("windows")
            .long_about(HELP_SYSTEM_CLEAN_REGISTRY);
        cmd_system = cmd_system.subcommand(cmd_system_cleanreg);

        let cmd_system_update_rtools40 = Command::new("update-rtools40")
            .about(ABOUT_SYSTEM_UPDATE_RTOOLS40)
            .display_order(0)
            .platform("windows")
            .long_about(HELP_SYSTEM_UPDATE_RTOOLS40);
        cmd_system = cmd_system.subcommand(cmd_system_update_rtools40);

        // `rtools` is also available as a top-level command (`rig rtools`);
        // the `rig system rtools` form is kept for backwards compatibility,
        // but always hidden.
        let cmd_system_rtools = cmd_rtools().hide(true);
        cmd_system = cmd_system.subcommand(cmd_system_rtools);

        let cmd_system_fix_r_alias = Command::new("fix-r-alias")
            .about(ABOUT_SYSTEM_FIX_R_ALIAS)
            .display_order(0)
            .platform("windows")
            .long_about(HELP_SYSTEM_FIX_R_ALIAS)
            .arg(
                Arg::new("undo")
                    .help("Remove the block rig added to the PowerShell profile(s).")
                    .long("undo")
                    .num_args(0)
                    .required(false),
            );
        cmd_system = cmd_system.subcommand(cmd_system_fix_r_alias);
    }

    {
        // `make-orthogonal` is a real command on macOS, but a hidden no-op on
        // Windows and Linux, so that it is always available (e.g. in scripts).
        let cmd_system_ortho = Command::new("make-orthogonal")
            .about(ABOUT_SYSTEM_MAKE_ORTHOGONAL)
            .long_about(HELP_SYSTEM_MAKE_ORTHOGONAL)
            .display_order(0)
            .platform("macos")
            .arg(
                Arg::new("version")
                    .help("R versions to update (default: all)")
                    .required(false)
                    .action(clap::ArgAction::Append),
            );
        cmd_system = cmd_system.subcommand(cmd_system_ortho);

        // `fix-permissions` is a real command on macOS, but a hidden no-op on
        // Windows and Linux, so that it is always available (e.g. in scripts).
        let cmd_system_rights = Command::new("fix-permissions")
            .about(ABOUT_SYSTEM_FIX_PERMISSIONS)
            .long_about(HELP_SYSTEM_FIX_PERMISSIONS)
            .display_order(0)
            .platform("macos")
            .arg(
                Arg::new("version")
                    .help("R versions to update (default: all)")
                    .required(false)
                    .action(clap::ArgAction::Append),
            );
        cmd_system = cmd_system.subcommand(cmd_system_rights);

        // The following are real commands on macOS, but hidden no-ops on
        // Windows and Linux, so that they are always available (e.g. in
        // scripts).
        let cmd_system_noopenmp = Command::new("no-openmp")
            .about(ABOUT_SYSTEM_NO_OPENMP)
            .long_about(HELP_SYSTEM_NO_OPENMP)
            .display_order(0)
            .platform("macos")
            .arg(
                Arg::new("version")
                    .help("R versions to update (default: all)")
                    .required(false)
                    .action(clap::ArgAction::Append),
            );

        let cmd_system_allow_debugger = Command::new("allow-debugger")
            .about(ABOUT_SYSTEM_ALLOW_DEBUGGER)
            .long_about(HELP_SYSTEM_ALLOW_DEBUGGER)
            .display_order(0)
            .platform("macos")
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
            .about(ABOUT_SYSTEM_ALLOW_DEBUGGER_RSTUDIO)
            .display_order(0)
            .platform("macos")
            .long_about(HELP_SYSTEM_ALLOW_DEBUGGER_RSTUDIO);

        let cmd_system_allow_core_dumps = Command::new("allow-core-dumps")
            .about(ABOUT_SYSTEM_ALLOW_CORE_DUMPS)
            .long_about(HELP_SYSTEM_ALLOW_CORE_DUMPS)
            .display_order(0)
            .platform("macos")
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
            .subcommand(cmd_system_noopenmp)
            .subcommand(cmd_system_allow_debugger)
            .subcommand(cmd_system_allow_debugger_rstudio)
            .subcommand(cmd_system_allow_core_dumps);
    }

    {
        let cmd_system_forget = Command::new("forget")
            .about(ABOUT_SYSTEM_FORGET)
            .display_order(0)
            .platform("macos")
            .long_about(HELP_SYSTEM_FORGET);

        cmd_system = cmd_system.subcommand(cmd_system_forget);
    }

    {
        let cmd_system_update_certs = Command::new("update-certs")
            .about(ABOUT_SYSTEM_UPDATE_CERTS)
            .long_about(HELP_SYSTEM_UPDATE_CERTS)
            .display_order(0)
            .platform("linux");
        cmd_system = cmd_system.subcommand(cmd_system_update_certs);
    }

    {
        let cmd_system_user_mode = Command::new("user-mode")
            .about(ABOUT_SYSTEM_USER_MODE)
            .long_about(HELP_SYSTEM_USER_MODE)
            .display_order(0)
            .arg(
                Arg::new("no-reinstall")
                    .help("Do not reinstall admin-mode R versions in user mode.")
                    .long("no-reinstall")
                    .num_args(0)
                    .required(false),
            )
            .arg(
                Arg::new("keep-install")
                    .help("Keep the admin-mode R installations, do not remove them.")
                    .long("keep-install")
                    .num_args(0)
                    .required(false),
            )
            .arg(
                Arg::new("keep-links")
                    .help("Keep the admin-mode quick links, do not remove them.")
                    .long("keep-links")
                    .num_args(0)
                    .required(false),
            )
            // The global --user/--admin flags are irrelevant and confusing here,
            // so override them with hidden versions to keep them out of the help.
            .arg(
                Arg::new("user")
                    .long("user")
                    .global(false)
                    .action(clap::ArgAction::SetTrue)
                    .hide(true),
            )
            .arg(
                Arg::new("admin")
                    .long("admin")
                    .global(false)
                    .action(clap::ArgAction::SetTrue)
                    .hide(true),
            );

        let cmd_system_clean_admin_r = Command::new("clean-admin-r")
            .about(ABOUT_SYSTEM_CLEAN_ADMIN_R)
            .long_about(HELP_SYSTEM_CLEAN_ADMIN_R)
            .display_order(0)
            .hide(true)
            .arg(
                Arg::new("keep-install")
                    .long("keep-install")
                    .num_args(0)
                    .required(false),
            )
            .arg(
                Arg::new("keep-links")
                    .long("keep-links")
                    .num_args(0)
                    .required(false),
            );

        cmd_system = cmd_system
            .subcommand(cmd_system_user_mode)
            .subcommand(cmd_system_clean_admin_r);
    }

    let cmd_system_detect_platform = Command::new("detect-platform")
        .about(ABOUT_SYSTEM_DETECT_PLATFORM)
        .long_about(HELP_SYSTEM_DETECT_PLATFORM)
        .display_order(0)
        .arg(
            Arg::new("json")
                .help("JSON output")
                .long("json")
                .num_args(0)
                .required(false),
        );

    cmd_system = cmd_system.subcommand(cmd_system_detect_platform);

    // Each of these prints a single directory, as a bare path, for use in
    // scripts. They are mutually exclusive (see the `dir` group below), and
    // `--rtools` is hidden off Windows, where there is no Rtools.
    fn dir_arg(name: &'static str, help: &'static str) -> Arg {
        Arg::new(name)
            .help(help)
            .long(name)
            .num_args(0)
            .required(false)
    }

    let cmd_system_dirs = Command::new("dirs")
        .about(ABOUT_SYSTEM_DIRS)
        .long_about(HELP_SYSTEM_DIRS)
        .display_order(0)
        .arg(
            Arg::new("json")
                .help("JSON output")
                .long("json")
                .num_args(0)
                .required(false),
        )
        .arg(dir_arg("r", "Print the R installation root only."))
        .arg(dir_arg("rtools", "Print the Rtools installation root only.").platform("windows"))
        .arg(dir_arg("binary", "Print the quick link directory only."))
        .arg(dir_arg("data", "Print rig's data directory only."))
        .arg(dir_arg("fonts", "Print the fontconfig directory only.").platform("linux"))
        .arg(dir_arg("cache", "Print rig's cache directory only."))
        .arg(dir_arg(
            "download",
            "Print rig's installer download directory only.",
        ))
        .arg(dir_arg("log", "Print rig's log directory only.").alias("logs"))
        .group(ArgGroup::new("dir").args([
            "r", "rtools", "binary", "data", "fonts", "cache", "download", "log",
        ]))
        // `--arch` only changes the answer on Windows, where the admin mode R
        // installation root is architecture dependent, so it is hidden
        // elsewhere. It is still defined on every platform, so that scripts do
        // not break, and it must not have a default value: `_default_arch` is
        // empty on Linux and would fail the value parser. (This is why
        // `rig add --arch` and `rig available --arch` need a `#[cfg]` block and
        // this one does not.)
        .arg(
            Arg::new("arch")
                .help("Architecture of the R installation root (default: native arch).")
                .short('a')
                .long("arch")
                .required(false)
                .value_parser(["x86_64", "aarch64", "arm64"])
                .platform("windows"),
        );

    cmd_system = cmd_system.subcommand(cmd_system_dirs);

    cmd_system = cmd_system
        .subcommand(cmd_system_links)
        .subcommand(cmd_system_lib)
        .subcommand(cmd_system_pak);

    let mut cmd_resolve = Command::new("resolve")
        .about(ABOUT_RESOLVE)
        .display_order(0)
        .long_about(HELP_RESOLVE);

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
                .required(false),
        );

    #[cfg(target_os = "linux")]
    {
        cmd_resolve = cmd_resolve.arg(
            Arg::new("arch")
                .help("Use this architecture, instead of auto-detecting it.")
                .short('a')
                .long("arch")
                .required(false)
                .value_parser(clap::value_parser!(String)),
        );
    }

    #[cfg(target_os = "macos")]
    {
        cmd_resolve = cmd_resolve.arg(
            Arg::new("arch")
                .help("Select architecture: arm64 or x86_64")
                .short('a')
                .long("arch")
                .required(false)
                .default_value(_default_arch)
                .value_parser(["arm64", "x86_64"]),
        );
    }

    #[cfg(target_os = "windows")]
    {
        cmd_resolve = cmd_resolve.arg(
            Arg::new("arch")
                .help("Select architecture: arm64 or x86_64")
                .short('a')
                .long("arch")
                .required(false)
                .default_value(_default_arch)
                .value_parser(["x86_64", "aarch64", "arm64"]),
        );
    }

    let mut cmd_rstudio = Command::new("rstudio")
        .about(ABOUT_RSTUDIO)
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

    cmd_rstudio = cmd_rstudio.arg(
        Arg::new("config-path")
            .help("Do not start RStudio, only print the path of the RStudio config directory")
            .long("config-path")
            .required(false)
            .num_args(0),
    );

    let cmd_library = Command::new("library")
        .about(ABOUT_LIBRARY)
        .display_order(0)
        .long_about(HELP_LIBRARY)
        .aliases(["lib"])
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
                .aliases(["ls"])
                .about(ABOUT_LIBRARY_LIST)
                .long_about(HELP_LIBRARY_LIST)
                .display_order(0)
                .arg(
                    Arg::new("json")
                        .help("JSON output")
                        .long("json")
                        .num_args(0)
                        .required(false),
                )
                .arg(
                    Arg::new("r-version")
                        .help("R version to operate on, instead of the default")
                        .long("r-version")
                        .short('r')
                        .num_args(1)
                        .required(false),
                ),
        )
        .subcommand(
            Command::new("add")
                .about(ABOUT_LIBRARY_ADD)
                .long_about(HELP_LIBRARY_ADD)
                .display_order(0)
                .arg(
                    Arg::new("lib-name")
                        .help("name of new library")
                        .required(true),
                )
                .arg(
                    Arg::new("r-version")
                        .help("R version to operate on, instead of the default")
                        .long("r-version")
                        .short('r')
                        .num_args(1)
                        .required(false),
                ),
        )
        .subcommand(
            Command::new("rm")
                .about(ABOUT_LIBRARY_RM)
                .long_about(HELP_LIBRARY_RM)
                .display_order(0)
                .arg(
                    Arg::new("lib-name")
                        .help("name of library to remove")
                        .required(true),
                )
                .arg(
                    Arg::new("r-version")
                        .help("R version to operate on, instead of the default")
                        .long("r-version")
                        .short('r')
                        .num_args(1)
                        .required(false),
                ),
        )
        .subcommand(
            Command::new("default")
                .about(ABOUT_LIBRARY_DEFAULT)
                .long_about(HELP_LIBRARY_DEFAULT)
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
                )
                .arg(
                    Arg::new("r-version")
                        .help("R version to operate on, instead of the default")
                        .long("r-version")
                        .short('r')
                        .num_args(1)
                        .required(false),
                ),
        );

    {
        let cmd_config = Command::new("config")
            .about(ABOUT_CONFIG)
            .long_about(HELP_CONFIG)
            .display_order(0)
            .arg_required_else_help(true)
            .subcommand(
                Command::new("config-file-path")
                    .about(ABOUT_CONFIG_CONFIG_FILE_PATH)
                    .long_about(HELP_CONFIG_CONFIG_FILE_PATH)
                    .display_order(0),
            )
            .subcommand(
                Command::new("list")
                    .about(ABOUT_CONFIG_LIST)
                    .long_about(HELP_CONFIG_LIST)
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
                Command::new("get")
                    .about(ABOUT_CONFIG_GET)
                    .long_about(HELP_CONFIG_GET)
                    .display_order(0)
                    .arg(Arg::new("key").help("config key to get").required(true))
                    .arg(
                        Arg::new("json")
                            .help("JSON output")
                            .long("json")
                            .num_args(0)
                            .required(false),
                    ),
            )
            .subcommand(
                Command::new("set")
                    .about(ABOUT_CONFIG_SET)
                    .long_about(HELP_CONFIG_SET)
                    .display_order(0)
                    .arg(
                        Arg::new("keyvalue")
                            .help("key=value pair to set")
                            .required(true),
                    ),
            );
        rig = rig.subcommand(cmd_config);
    }

    {
        let cmd_sysreqs = Command::new("sysreqs")
            .about(ABOUT_SYSREQS)
            .display_order(0)
            .platform("macos")
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
                    .about(ABOUT_SYSREQS_ADD)
                    .long_about(HELP_SYSREQS_ADD)
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
                            .default_value(_default_arch)
                            .value_parser(["arm64", "x86_64"]),
                    ),
            )
            .subcommand(
                Command::new("list")
                    .about(ABOUT_SYSREQS_LIST)
                    .long_about(HELP_SYSREQS_LIST)
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
                Command::new("info")
                    .about(ABOUT_SYSREQS_INFO)
                    .long_about(HELP_SYSREQS_INFO)
                    .display_order(0)
                    .arg(Arg::new("name").help("system tool to show").required(true))
                    .arg(
                        Arg::new("json")
                            .help("JSON output")
                            .long("json")
                            .num_args(0)
                            .required(false),
                    ),
            );
        rig = rig.subcommand(cmd_sysreqs);
    }

    let cmd_run = Command::new("run")
        .about(ABOUT_RUN)
        .display_order(0)
        .long_about(HELP_RUN)
        .arg(
            Arg::new("r-version")
                .help("R version to use")
                .short('r')
                .long("r-version")
                .required(false),
        )
        .arg(
            Arg::new("app-type")
                .help("Explicitly specify app type to run")
                .short('t')
                .long("app-type")
                .required(false)
                .value_parser(app_types)
                .conflicts_with("eval")
                .conflicts_with("script"),
        )
        .arg(
            Arg::new("dry-run")
                .help("Show the command, but do not run it")
                .long("dry-run")
                .required(false)
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("startup")
                .help("Print R startup message")
                .long("startup")
                .action(clap::ArgAction::SetTrue)
                .required(false),
        )
        .arg(
            Arg::new("no-startup")
                .help("Do not print R startup message")
                .long("no-startup")
                .action(clap::ArgAction::SetTrue)
                .required(false)
                .conflicts_with("startup"),
        )
        .arg(
            Arg::new("echo")
                .help("Print input to R")
                .long("echo")
                .action(clap::ArgAction::SetTrue)
                .required(false),
        )
        .arg(
            Arg::new("no-echo")
                .help("Do not print input to R")
                .long("no-echo")
                .action(clap::ArgAction::SetTrue)
                .required(false)
                .conflicts_with("echo"),
        )
        .arg(
            Arg::new("eval")
                .help("R expression to evaluate")
                .short('e')
                .long("eval")
                .num_args(1)
                .required(false),
        )
        .arg(
            Arg::new("script")
                .help("R script file to run")
                .short('f')
                .long("script")
                .num_args(1)
                .required(false)
                .conflicts_with("eval"),
        )
        .arg(
            Arg::new("cmd")
                .help("Run `R CMD <command>` with the trailing arguments")
                .long("cmd")
                .action(clap::ArgAction::SetTrue)
                .required(false)
                .conflicts_with_all([
                    "eval",
                    "script",
                    "app-type",
                    "startup",
                    "no-startup",
                    "echo",
                    "no-echo",
                ]),
        )
        .arg(
            Arg::new("command")
                .help("R script, project or R CMD command to run, with parameters")
                .required(false)
                .action(clap::ArgAction::Append),
        );

    let cmd_proj = Command::new("proj")
        .about(ABOUT_PROJ)
        .display_order(0)
        .long_about(HELP_PROJ)
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
                .about(ABOUT_PROJ_DEPS)
                .long_about(HELP_PROJ_DEPS)
                .display_order(0)
                .arg(
                    Arg::new("input")
                        .help("Project file to solve (e.g. DESCRIPTION)")
                        .long("input")
                        .short('i')
                        .num_args(1)
                        .required(false),
                )
                .arg(
                    Arg::new("recursive")
                        .help("Show recursive (transitive) dependencies")
                        .long("recursive")
                        .short('r')
                        .num_args(0)
                        .required(false),
                )
                .arg(
                    Arg::new("json")
                        .help("JSON output")
                        .long("json")
                        .num_args(0)
                        .required(false),
                )
                .arg(
                    Arg::new("dev")
                        .help("Include dev (development) dependencies")
                        .long("dev")
                        .num_args(0)
                        .required(false),
                ),
        )
        .subcommand(
            Command::new("tree")
                .about(ABOUT_PROJ_TREE)
                .long_about(HELP_PROJ_TREE)
                .display_order(0)
                .arg(
                    Arg::new("input")
                        .help("Project file to solve (e.g. DESCRIPTION)")
                        .long("input")
                        .short('i')
                        .num_args(1)
                        .required(false),
                )
                .arg(
                    Arg::new("dev")
                        .help("Include dev (development) dependencies")
                        .long("dev")
                        .num_args(0)
                        .required(false),
                )
                .arg(
                    Arg::new("no-base")
                        .help("Leave out R and the base packages")
                        .long("no-base")
                        .num_args(0)
                        .required(false),
                )
                .arg(
                    Arg::new("why")
                        .help("Invert the tree: show what pulls this package in")
                        .long("why")
                        .visible_alias("explain")
                        .value_name("PACKAGE")
                        .num_args(1)
                        .required(false),
                )
                .arg(
                    Arg::new("json")
                        .help("JSON output")
                        .long("json")
                        .num_args(0)
                        .required(false),
                ),
        )
        .subcommand(
            Command::new("solve")
                .about(ABOUT_PROJ_SOLVE)
                .long_about(HELP_PROJ_SOLVE)
                .display_order(0)
                .arg(
                    Arg::new("input")
                        .help("Project file to solve (e.g. DESCRIPTION)")
                        .long("input")
                        .short('i')
                        .num_args(1)
                        .required(false),
                )
                .arg(
                    Arg::new("renv")
                        .help("Output and renv.lock file")
                        .long("renv")
                        .num_args(0)
                        .required(false),
                )
                .arg(
                    Arg::new("json")
                        .help("JSON output")
                        .long("json")
                        .num_args(0)
                        .required(false),
                )
                .arg(
                    Arg::new("r-version")
                        .help("R version to solve dependencies for")
                        .long("r-version")
                        .short('r')
                        .num_args(1)
                        .required(false),
                )
                .arg(
                    Arg::new("platform")
                        .help(
                            "Platform to solve binary packages for, e.g. macos, windows,\n\
                            ubuntu-24.04, or a full platform string like\n\
                            aarch64-unknown-linux-gnu-ubuntu-24.04 (default: this machine).\n\
                            Use --platform source to solve for source packages only.",
                        )
                        .long("platform")
                        .num_args(1)
                        .required(false),
                )
                .arg(
                    Arg::new("prefer-binary")
                        .help(
                            "Prefer an older version that has a binary package over a\n\
                            newer one that does not. Optionally give how many of the\n\
                            newest versions to consider, e.g. --prefer-binary=5\n\
                            (default: 3).",
                        )
                        .long("prefer-binary")
                        .num_args(0..=1)
                        .require_equals(true)
                        .default_missing_value("3")
                        .value_parser(clap::value_parser!(usize))
                        .required(false),
                )
                .arg(
                    Arg::new("dev")
                        .help("Include dev (development) dependencies")
                        .long("dev")
                        .num_args(0)
                        .required(false),
                ),
        )
        .subcommand(
            Command::new("deploy")
                .about(ABOUT_PROJ_DEPLOY)
                .long_about(HELP_PROJ_DEPLOY)
                .display_order(0)
                .arg(
                    Arg::new("library")
                        .help("Library path where packages should be installed")
                        .long("library")
                        .short('l')
                        .num_args(1)
                        .required(true),
                )
                .arg(
                    Arg::new("r-binary")
                        .help("Path to R binary (default: R)")
                        .long("r-binary")
                        .num_args(1)
                        .required(false),
                )
                .arg(
                    Arg::new("max-concurrent")
                        .help("Maximum number of concurrent installations (default: 4)")
                        .long("max-concurrent")
                        .num_args(1)
                        .value_parser(clap::value_parser!(usize))
                        .required(false),
                ),
        );
    rig = rig.subcommand(cmd_proj);

    let cmd_pkg = Command::new("pkg")
        .about(ABOUT_PKG)
        .long_about(HELP_PKG)
        .display_order(0)
        .arg_required_else_help(true)
        .arg(
            Arg::new("json")
                .help("JSON output")
                .long("json")
                .num_args(0)
                .required(false),
        )
        .subcommand(
            Command::new("available")
                .about(ABOUT_PKG_AVAILABLE)
                .long_about(HELP_PKG_AVAILABLE)
                .display_order(0)
                .arg(
                    Arg::new("json")
                        .help("JSON output")
                        .long("json")
                        .num_args(0)
                        .required(false),
                )
                .arg(
                    Arg::new("platform")
                        .help("Platform to use, instead of the current")
                        .long("platform")
                        .num_args(1)
                        .required(false),
                )
                .arg(
                    Arg::new("r-version")
                        .help("R version to use, instead of the default")
                        .long("r-version")
                        .num_args(1)
                        .required(false),
                )
                .arg(
                    Arg::new("pkg-type")
                        .help("Type of packages to list (e.g. source, binary)")
                        .long("pkg-type")
                        .num_args(1)
                        .required(false),
                ),
        )
        .subcommand(
            Command::new("deps")
                .about(ABOUT_PKG_DEPS)
                .long_about(HELP_PKG_DEPS)
                .display_order(0)
                .arg(
                    Arg::new("package")
                        .help("package to show the dependencies of")
                        .required(true),
                )
                .arg(
                    Arg::new("version")
                        .long("version")
                        .short('v')
                        .help("package version to use (default: latest)")
                        .required(false),
                )
                .arg(
                    Arg::new("recursive")
                        .help("Show recursive (transitive) dependencies")
                        .long("recursive")
                        .short('r')
                        .num_args(0)
                        .required(false),
                )
                .arg(
                    Arg::new("dev")
                        .help("Include dev (development) dependencies")
                        .long("dev")
                        .num_args(0)
                        .required(false),
                )
                .arg(
                    Arg::new("json")
                        .help("JSON output")
                        .long("json")
                        .num_args(0)
                        .required(false),
                ),
        )
        .subcommand(
            Command::new("info")
                .about(ABOUT_PKG_INFO)
                .long_about(HELP_PKG_INFO)
                .display_order(0)
                .arg(Arg::new("package").help("package to show").required(true))
                .arg(
                    Arg::new("version")
                        .long("version")
                        .short('v')
                        .help("package version to show (default: latest)")
                        .required(false),
                )
                .arg(
                    Arg::new("versions")
                        .help("List all versions of the package, instead of the details of one")
                        .long("versions")
                        .num_args(0)
                        .required(false)
                        .conflicts_with("version"),
                )
                .arg(
                    Arg::new("readme")
                        .help("Show the README of the package, instead of its metadata")
                        .long("readme")
                        .num_args(0)
                        .required(false)
                        .conflicts_with("versions"),
                )
                .arg(
                    Arg::new("json")
                        .help("JSON output")
                        .long("json")
                        .num_args(0)
                        .required(false),
                ),
        )
        .subcommand(
            Command::new("install")
                .about(ABOUT_PKG_INSTALL)
                .long_about(HELP_PKG_INSTALL)
                .display_order(0)
                .arg(
                    Arg::new("package")
                        .help("packages to install")
                        .required(true)
                        .num_args(1..),
                )
                .arg(
                    Arg::new("library")
                        .help("Library name or path, instead of the default library")
                        .long("library")
                        .short('l')
                        .num_args(1)
                        .required(false),
                )
                .arg(
                    Arg::new("r-version")
                        .help("R version to operate on, instead of the default")
                        .long("r-version")
                        .short('r')
                        .num_args(1)
                        .required(false),
                )
                .arg(
                    Arg::new("reinstall")
                        .help("Install the packages even if they are already up to date")
                        .long("reinstall")
                        .num_args(0)
                        .required(false),
                )
                .arg(
                    Arg::new("dry-run")
                        .help("Show what would be installed, install nothing")
                        .long("dry-run")
                        .num_args(0)
                        .required(false),
                )
                .arg(
                    Arg::new("dev")
                        .help("Include dev (development) dependencies")
                        .long("dev")
                        .num_args(0)
                        .required(false),
                )
                .arg(
                    Arg::new("ignore-unavailable")
                        .help(
                            "Skip dev dependencies that are not available in the\n\
                            repositories, instead of failing",
                        )
                        .long("ignore-unavailable")
                        .num_args(0)
                        .requires("dev")
                        .required(false),
                )
                .arg(
                    Arg::new("platform")
                        .help(
                            "Platform to install binary packages for, e.g. macos, windows,\n\
                            ubuntu-24.04, or a full platform string like\n\
                            aarch64-unknown-linux-gnu-ubuntu-24.04 (default: this machine).\n\
                            Use --platform source to install source packages only.",
                        )
                        .long("platform")
                        .num_args(1)
                        .required(false),
                )
                .arg(
                    Arg::new("prefer-binary")
                        .help(
                            "Prefer an older version that has a binary package over a\n\
                            newer one that does not. Optionally give how many of the\n\
                            newest versions to consider, e.g. --prefer-binary=5\n\
                            (default: 3).",
                        )
                        .long("prefer-binary")
                        .num_args(0..=1)
                        .require_equals(true)
                        .default_missing_value("3")
                        .value_parser(clap::value_parser!(usize))
                        .required(false),
                )
                .arg(
                    Arg::new("json")
                        .help("JSON output")
                        .long("json")
                        .num_args(0)
                        .required(false),
                ),
        )
        .subcommand(
            Command::new("list")
                .aliases(["ls"])
                .about(ABOUT_PKG_LIST)
                .long_about(HELP_PKG_LIST)
                .display_order(0)
                .arg(
                    Arg::new("library")
                        .help("Library name or path, instead of the default library")
                        .long("library")
                        .short('l')
                        .num_args(1)
                        .required(false),
                )
                .arg(
                    Arg::new("r-version")
                        .help("R version to operate on, instead of the default")
                        .long("r-version")
                        .short('r')
                        .num_args(1)
                        .required(false),
                )
                .arg(
                    Arg::new("json")
                        .help("JSON output")
                        .long("json")
                        .num_args(0)
                        .required(false),
                ),
        )
        .subcommand(
            Command::new("remove")
                .aliases(["rm"])
                .about(ABOUT_PKG_REMOVE)
                .long_about(HELP_PKG_REMOVE)
                .display_order(0)
                .arg(
                    Arg::new("package")
                        .help("packages to remove")
                        .required(true)
                        .num_args(1..),
                )
                .arg(
                    Arg::new("library")
                        .help("Library name or path, instead of the default library")
                        .long("library")
                        .short('l')
                        .num_args(1)
                        .required(false),
                )
                .arg(
                    Arg::new("r-version")
                        .help("R version to operate on, instead of the default")
                        .long("r-version")
                        .short('r')
                        .num_args(1)
                        .required(false),
                )
                .arg(
                    Arg::new("force")
                        .help("Remove base packages as well, which R itself needs")
                        .long("force")
                        .num_args(0)
                        .required(false),
                )
                .arg(
                    Arg::new("json")
                        .help("JSON output")
                        .long("json")
                        .num_args(0)
                        .required(false),
                ),
        )
        .subcommand(
            Command::new("tree")
                .about(ABOUT_PKG_TREE)
                .long_about(HELP_PKG_TREE)
                .display_order(0)
                .arg(
                    Arg::new("package")
                        .help("package to show the dependency tree of")
                        .required(true),
                )
                .arg(
                    Arg::new("version")
                        .long("version")
                        .short('v')
                        .help("package version to use (default: latest)")
                        .required(false),
                )
                .arg(
                    Arg::new("dev")
                        .help("Include dev (development) dependencies")
                        .long("dev")
                        .num_args(0)
                        .required(false),
                )
                .arg(
                    Arg::new("no-base")
                        .help("Leave out R and the base packages")
                        .long("no-base")
                        .num_args(0)
                        .required(false),
                )
                .arg(
                    Arg::new("why")
                        .help("Invert the tree: show what pulls this package in")
                        .long("why")
                        .visible_alias("explain")
                        .value_name("PACKAGE")
                        .num_args(1)
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

    rig = rig.subcommand(cmd_pkg);

    let cmd_ppm = Command::new("ppm")
        .about(ABOUT_PPM)
        .long_about(HELP_PPM)
        .display_order(0)
        .arg_required_else_help(true)
        .arg(
            Arg::new("json")
                .help("JSON output")
                .long("json")
                .num_args(0)
                .required(false),
        )
        .subcommand(
            Command::new("builds")
                .about(ABOUT_PPM_BUILDS)
                .long_about(HELP_PPM_BUILDS)
                .display_order(0)
                .arg(
                    Arg::new("package")
                        .help("package to list the builds of")
                        .required(true),
                )
                .arg(
                    Arg::new("version")
                        .help("package version to list (default: all)")
                        .long("version")
                        .short('v')
                        .num_args(1)
                        .required(false),
                )
                .arg(
                    Arg::new("json")
                        .help("JSON output")
                        .long("json")
                        .num_args(0)
                        .required(false),
                ),
        )
        .subcommand(
            Command::new("platforms")
                .about(ABOUT_PPM_PLATFORMS)
                .long_about(HELP_PPM_PLATFORMS)
                .display_order(0)
                .arg(
                    Arg::new("all")
                        .help("Also list the platforms P3M has retired")
                        .long("all")
                        .num_args(0)
                        .required(false),
                )
                .arg(
                    Arg::new("json")
                        .help("JSON output")
                        .long("json")
                        .num_args(0)
                        .required(false),
                ),
        )
        .subcommand(
            Command::new("r-versions")
                .about(ABOUT_PPM_R_VERSIONS)
                .long_about(HELP_PPM_R_VERSIONS)
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
            Command::new("status")
                .about(ABOUT_PPM_STATUS)
                .long_about(HELP_PPM_STATUS)
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
            Command::new("url")
                .about(ABOUT_PPM_URL)
                .long_about(HELP_PPM_URL)
                .display_order(0)
                .arg(
                    Arg::new("json")
                        .help("JSON output")
                        .long("json")
                        .num_args(0)
                        .required(false),
                ),
        );

    rig = rig.subcommand(cmd_ppm);

    let cmd_repos_setup = Command::new("setup")
        .about(ABOUT_REPOS_SETUP)
        .long_about(HELP_REPOS_SETUP)
        .display_order(0)
        .arg(
            Arg::new("r-version")
                .help("R version to set up repositories for (default: all)")
                .long("r-version")
                .short('r')
                .num_args(1)
                .required(false),
        )
        .arg(
            Arg::new("with-repos")
                .help(
                    "Repositories to enable, in addition to the ones enabled by default.\n\
                    If --without-repos is also specified (without a value), then only these\n\
                    repositories will be enabled.",
                )
                .long("with-repos")
                .num_args(1)
                .require_equals(true)
                .default_missing_value("DEFAULT REPOSITORIES")
                .required(false),
        )
        .arg(
            Arg::new("without-repos")
                .help(
                    "Do not set up package repositories.\n\
                    Alternatively, specify which ones to skip, a comma-separated list.\n\
                    If --with-repos is also specified, then only the repositories in that\n\
                    argument will be enabled.",
                )
                .long("without-repos")
                .num_args(0..=1)
                .require_equals(true)
                .default_missing_value("ALL REPOSITORIES")
                .required(false),
        );

    let cmd_repos = Command::new("repos")
        .about(ABOUT_REPOS)
        .display_order(0)
        .long_about(HELP_REPOS)
        .arg_required_else_help(true)
        .arg(
            Arg::new("json")
                .help("JSON output")
                .long("json")
                .num_args(0)
                .required(false),
        )
        // .subcommand(
        //     Command::new("add")
        //         .about("Add an R package repository")
        //         .display_order(0)
        //         .arg(
        //             Arg::new("enable")
        //                 .help("Enable the repository after adding it")
        //                 .long("enable")
        //                 .num_args(0)
        //                 .required(false),
        //         )
        //         .arg(
        //             Arg::new("name")
        //                 .help("name of the repository, e.g. 'CRAN'")
        //                 .required(true),
        //         )
        //         .arg(Arg::new("url").help("URL of the repository").required(true)),
        // )
        // .subcommand(
        //     Command::new("disable")
        //         .about("Disable an R package repository")
        //         .display_order(0)
        //         .arg(
        //             Arg::new("name")
        //                 .help("name of the repository, e.g. 'CRAN'")
        //                 .required(true),
        //         ),
        // )
        // .subcommand(
        //     Command::new("enable")
        //         .about("Enable an R package repository")
        //         .display_order(0)
        //         .arg(
        //             Arg::new("name")
        //                 .help("name of the repository, e.g. 'CRAN'")
        //                 .required(true),
        //         ),
        // )
        .subcommand(
            Command::new("list")
                .about(ABOUT_REPOS_LIST)
                .long_about(HELP_REPOS_LIST)
                .display_order(0)
                .arg(
                    Arg::new("json")
                        .help("JSON output")
                        .long("json")
                        .num_args(0)
                        .required(false),
                )
                .arg(
                    Arg::new("all")
                        .help("Show all repositories, not just the default ones")
                        .long("all")
                        .num_args(0)
                        .required(false),
                )
                .arg(
                    Arg::new("raw")
                        .help("Do not resolve `%` variables in repository URLs.")
                        .long("raw")
                        .num_args(0)
                        .required(false),
                )
                .arg(
                    Arg::new("r-version")
                        .help("R version to list repositories for, instead of the default")
                        .long("r-version")
                        .short('r')
                        .num_args(1)
                        .required(false),
                ),
        )
        .subcommand(
            Command::new("available")
                .about(ABOUT_REPOS_AVAILABLE)
                .long_about(HELP_REPOS_AVAILABLE)
                .display_order(0)
                .arg(
                    Arg::new("json")
                        .help("JSON output")
                        .long("json")
                        .num_args(0)
                        .required(false),
                )
                .arg(
                    Arg::new("name")
                        .help("Repository to show in detail, e.g. 'P3M' (case insensitive)")
                        .required(false),
                ),
        )
        .subcommand(
            Command::new("status")
                .about(ABOUT_REPOS_STATUS)
                .long_about(HELP_REPOS_STATUS)
                .display_order(0)
                .arg(
                    Arg::new("json")
                        .help("JSON output")
                        .long("json")
                        .num_args(0)
                        .required(false),
                )
                .arg(
                    Arg::new("all")
                        .help("Check all repositories, not just the default ones")
                        .long("all")
                        .num_args(0)
                        .required(false),
                )
                .arg(
                    Arg::new("raw")
                        .help("Show repository URLs without resolving `%` variables.")
                        .long("raw")
                        .num_args(0)
                        .required(false),
                )
                .arg(
                    Arg::new("r-version")
                        .help("R version to check repositories for, instead of the default")
                        .long("r-version")
                        .short('r')
                        .num_args(1)
                        .required(false),
                ),
        )
        // .subcommand(
        //     Command::new("reset")
        //         .about("Reset R package repositories to rig or R default")
        //         .display_order(0),
        // )
        // .subcommand(
        //     Command::new("rm")
        //         .about("Remove an R package repository")
        //         .display_order(0),
        // )
        .subcommand(cmd_repos_setup);

    rig = rig.subcommand(cmd_repos);

    #[cfg(debug_assertions)]
    {
        let cmd_test = Command::new("test")
            .about("Run tests (for rig developers)")
            .display_order(0)
            // Developer-only command; it exists in debug builds only, but the
            // website's CLI reference is generated from a debug build on CI, so
            // keep it out of the reference explicitly.
            .hide(reference_mode())
            .arg_required_else_help(true)
            .subcommand(
                Command::new("download-lockfile")
                    .about("Download packages in the pkg.lock file")
                    .display_order(0),
            )
            .subcommand(
                Command::new("read-rds")
                    .about("Test reading RDS files")
                    .display_order(0)
                    .arg(Arg::new("path").required(true)),
            )
            .subcommand(
                Command::new("read-packages-rds")
                    .about("Test reading packages from RDS files")
                    .display_order(0)
                    .arg(Arg::new("path").required(true)),
            )
            .subcommand(
                Command::new("parse-platform-string")
                    .about("Test parsing platform strings")
                    .display_order(0)
                    .arg(Arg::new("platform").required(true)),
            )
            .subcommand(
                Command::new("platform-to-pkg-type")
                    .about("Test repo directory for platform string")
                    .display_order(0)
                    .arg(Arg::new("platform").required(true))
                    .arg(Arg::new("r-version").required(true).long("r-version")),
            )
            .subcommand(
                Command::new("binary-index")
                    .about("Show a package's binary index from P3M")
                    .display_order(0)
                    .arg(Arg::new("package").required(true))
                    .arg(
                        Arg::new("version")
                            .help("package version to show, defaults to the newest")
                            .long("version"),
                    )
                    .arg(
                        Arg::new("platform")
                            .help("target platform, defaults to the current one")
                            .long("platform"),
                    )
                    .arg(
                        Arg::new("r-version")
                            .help("target R version, needed to look up binaries")
                            .long("r-version"),
                    ),
            );
        rig = rig.subcommand(cmd_test);
    }

    rig = rig
        .arg(
            Arg::new("quiet")
                .help("Suppress output (overrides `--verbose`)")
                .short('q')
                .long("quiet")
                .required(false)
                .action(clap::ArgAction::Count),
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
        );

    rig = rig
        .arg(
            Arg::new("user")
                .help("Run in user mode (overrides RIG_MODE and config)")
                .long("user")
                .global(true)
                .action(clap::ArgAction::SetTrue)
                .conflicts_with("admin"),
        )
        .arg(
            Arg::new("admin")
                .help("Run in admin mode (overrides RIG_MODE and config)")
                .long("admin")
                .global(true)
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("no-cache")
                .help("Do not read or write rig's cache (overrides RIG_NO_CACHE and config)")
                .long("no-cache")
                .global(true)
                .action(clap::ArgAction::SetTrue),
        );

    rig = rig
        .subcommand(cmd_default)
        .subcommand(cmd_list)
        .subcommand(cmd_add)
        .subcommand(cmd_rm)
        .subcommand(cmd_system)
        .subcommand(cmd_rtools())
        .subcommand(cmd_resolve)
        .subcommand(cmd_rstudio)
        .subcommand(cmd_library)
        .subcommand(cmd_available)
        .subcommand(cmd_run)
        .after_help(HELP_EXAMPLES);

    add_name_headers(&mut rig, "rig");

    if reference_mode() {
        add_alias_markers(&mut rig);
    }

    rig
}

// With `rig run --cmd <command> [args...]` everything after `--cmd` is passed
// on to `R CMD <command>`, including arguments that happen to be rig's own
// flags, e.g. `rig run --cmd check --help`. Insert the `--` separator that
// tells clap to stop parsing arguments, so users don't need to type it.
// Nothing after the first `--cmd` token is modified, and `--cmd` is not an
// argument of any other rig command.
fn rcmd_argv<I>(argv: I) -> Vec<std::ffi::OsString>
where
    I: IntoIterator<Item = std::ffi::OsString>,
{
    use std::ffi::{OsStr, OsString};
    let mut out: Vec<OsString> = Vec::new();
    let mut argv = argv.into_iter();
    while let Some(arg) = argv.next() {
        let is_cmd = arg.as_os_str() == OsStr::new("--cmd");
        out.push(arg);
        if !is_cmd {
            continue;
        }
        let rest: Vec<OsString> = argv.collect();
        if let Some(first) = rest.first() {
            if first.as_os_str() != OsStr::new("--") {
                out.push(OsString::from("--"));
            }
        }
        out.extend(rest);
        break;
    }
    out
}

pub fn parse_args() -> ArgMatches {
    match rig_app().try_get_matches_from(rcmd_argv(std::env::args_os())) {
        Ok(matches) => matches,
        Err(e) => {
            use clap::error::ErrorKind::*;
            match e.kind() {
                // Help output: page it when interactive, then exit cleanly.
                DisplayHelp | DisplayHelpOnMissingArgumentOrSubcommand => {
                    show_help(&e);
                    std::process::exit(0);
                }
                // Version and real parse errors keep clap's default behavior
                // (version -> stdout / exit 0, errors -> stderr / exit 2).
                _ => e.exit(),
            }
        }
    }
}

// Display clap help output, paging it through `$PAGER` (defaulting to `less`)
// when stdout is a terminal, mirroring how git pages its help. When stdout is
// not a terminal, or the pager is disabled/unavailable, the help is printed
// directly (clap strips ANSI colors automatically for non-terminals).
fn show_help(err: &clap::Error) {
    use std::io::IsTerminal;

    // Non-interactive: let clap print (it auto-strips color) and return.
    if !std::io::stdout().is_terminal() {
        let _ = err.print();
        return;
    }

    // Colored help text. `.ansi()` forces clap's own styling on even though we
    // capture to a String; the ABOUT_*/HELP_* constants already carry escapes.
    crate::pager::page_text(&err.render().ansi().to_string());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn rcmd(argv: &[&str]) -> Vec<String> {
        rcmd_argv(argv.iter().map(OsString::from))
            .iter()
            .map(|x| x.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn test_rcmd_argv_no_cmd() {
        assert_eq!(
            rcmd(&["rig", "run", "-e", "1+1"]),
            ["rig", "run", "-e", "1+1"]
        );
        assert_eq!(rcmd(&[]), Vec::<String>::new());
    }

    #[test]
    fn test_rcmd_argv_inserts_separator() {
        assert_eq!(
            rcmd(&["rig", "run", "--cmd", "check", "pkg.tar.gz"]),
            ["rig", "run", "--cmd", "--", "check", "pkg.tar.gz"]
        );
    }

    #[test]
    fn test_rcmd_argv_keeps_rig_flags_before_cmd() {
        assert_eq!(
            rcmd(&[
                "rig",
                "run",
                "-r",
                "4.5.1",
                "--dry-run",
                "--cmd",
                "check",
                "--help"
            ]),
            [
                "rig",
                "run",
                "-r",
                "4.5.1",
                "--dry-run",
                "--cmd",
                "--",
                "check",
                "--help"
            ]
        );
    }

    #[test]
    fn test_rcmd_argv_explicit_separator() {
        assert_eq!(
            rcmd(&["rig", "run", "--cmd", "--", "check", "."]),
            ["rig", "run", "--cmd", "--", "check", "."]
        );
    }

    #[test]
    fn test_rcmd_argv_no_arguments_after_cmd() {
        assert_eq!(rcmd(&["rig", "run", "--cmd"]), ["rig", "run", "--cmd"]);
    }

    #[test]
    fn test_rcmd_argv_only_first_cmd() {
        assert_eq!(
            rcmd(&["rig", "run", "--cmd", "check", "--cmd", "x"]),
            ["rig", "run", "--cmd", "--", "check", "--cmd", "x"]
        );
    }

    #[test]
    fn test_run_cmd_args() {
        let m = rig_app()
            .try_get_matches_from(rcmd_argv(
                [
                    "rig",
                    "run",
                    "--dry-run",
                    "--cmd",
                    "check",
                    "--no-manual",
                    ".",
                ]
                .iter()
                .map(OsString::from),
            ))
            .unwrap();
        let run = m.subcommand_matches("run").unwrap();
        assert!(run.get_flag("cmd"));
        assert!(run.get_flag("dry-run"));
        let cmdargs: Vec<&String> = run.get_many::<String>("command").unwrap().collect();
        assert_eq!(cmdargs, ["check", "--no-manual", "."]);
    }

    // The `rig system dirs` family. These tests run on every platform, so they
    // are also the guard that the items that are merely *hidden* off Windows
    // stay defined everywhere.

    fn sysargs(argv: &[&str]) -> ArgMatches {
        let m = rig_app().try_get_matches_from(argv).unwrap();
        m.subcommand_matches("system").unwrap().to_owned()
    }

    #[test]
    fn test_system_dirs_args() {
        let m = sysargs(&["rig", "system", "dirs"]);
        let dirs = m.subcommand_matches("dirs").unwrap();
        assert!(!dirs.get_flag("json"));

        let m = sysargs(&["rig", "system", "dirs", "--json"]);
        assert!(m.subcommand_matches("dirs").unwrap().get_flag("json"));

        // The global --json also works, and must precede the subcommand.
        let m = rig_app()
            .try_get_matches_from(["rig", "--json", "system", "dirs"])
            .unwrap();
        assert!(m.get_flag("json"));
    }

    #[test]
    fn test_system_dirs_selectors() {
        for sel in ["r", "rtools", "binary", "data", "fonts", "cache", "log"] {
            // `--rtools` is hidden off Windows and `--fonts` off Linux, but
            // they must still parse, so that scripts can call them
            // unconditionally.
            let m = sysargs(&["rig", "system", "dirs", &format!("--{}", sel)]);
            let dirs = m.subcommand_matches("dirs").unwrap();
            assert!(dirs.get_flag(sel), "--{} is not set", sel);
            // Setting one must not set any of the others.
            for other in ["r", "rtools", "binary", "data", "fonts", "cache", "log"] {
                assert_eq!(dirs.get_flag(other), other == sel);
            }
        }

        // --logs is an alias of --log.
        let m = sysargs(&["rig", "system", "dirs", "--logs"]);
        assert!(m.subcommand_matches("dirs").unwrap().get_flag("log"));

        // The selectors are mutually exclusive.
        assert!(rig_app()
            .try_get_matches_from(["rig", "system", "dirs", "--r", "--binary"])
            .is_err());
    }

    #[test]
    fn test_system_dirs_arch() {
        // --arch is hidden off Windows, but always defined.
        let m = sysargs(&["rig", "system", "dirs", "--arch", "aarch64"]);
        let dirs = m.subcommand_matches("dirs").unwrap();
        assert_eq!(dirs.get_one::<String>("arch"), Some(&"aarch64".to_string()));

        let m = sysargs(&["rig", "system", "dirs", "-a", "arm64"]);
        let dirs = m.subcommand_matches("dirs").unwrap();
        assert_eq!(dirs.get_one::<String>("arch"), Some(&"arm64".to_string()));

        // No default value: an empty one would fail the value parser on Linux.
        let m = sysargs(&["rig", "system", "dirs"]);
        let dirs = m.subcommand_matches("dirs").unwrap();
        assert_eq!(dirs.get_one::<String>("arch"), None);

        assert!(rig_app()
            .try_get_matches_from(["rig", "system", "dirs", "--arch", "amd64"])
            .is_err());
    }

    #[test]
    fn test_repos_status_args() {
        let m = rig_app()
            .try_get_matches_from(["rig", "repos", "status"])
            .unwrap();
        let repos = m.subcommand_matches("repos").unwrap();
        let status = repos.subcommand_matches("status").unwrap();
        assert!(!status.get_flag("json"));
        assert!(!status.get_flag("all"));
        assert!(!status.get_flag("raw"));
        assert_eq!(status.get_one::<String>("r-version"), None);

        let m = rig_app()
            .try_get_matches_from(["rig", "repos", "status", "--all", "--raw", "-r", "4.4.1"])
            .unwrap();
        let status = m
            .subcommand_matches("repos")
            .unwrap()
            .subcommand_matches("status")
            .unwrap();
        assert!(status.get_flag("all"));
        assert!(status.get_flag("raw"));
        assert_eq!(
            status.get_one::<String>("r-version"),
            Some(&"4.4.1".to_string())
        );

        // --json works at all three levels.
        let m = rig_app()
            .try_get_matches_from(["rig", "repos", "status", "--json"])
            .unwrap();
        assert!(m
            .subcommand_matches("repos")
            .unwrap()
            .subcommand_matches("status")
            .unwrap()
            .get_flag("json"));

        let m = rig_app()
            .try_get_matches_from(["rig", "repos", "--json", "status"])
            .unwrap();
        assert!(m.subcommand_matches("repos").unwrap().get_flag("json"));

        let m = rig_app()
            .try_get_matches_from(["rig", "--json", "repos", "status"])
            .unwrap();
        assert!(m.get_flag("json"));
    }

    #[test]
    fn test_ppm_args() {
        // Every subcommand `sc_ppm` dispatches on has to exist here, or the
        // dispatch bails out.
        for name in ["builds", "platforms", "r-versions", "status", "url"] {
            let argv: Vec<&str> = if name == "builds" {
                vec!["rig", "ppm", name, "dplyr"]
            } else {
                vec!["rig", "ppm", name]
            };
            let m = rig_app().try_get_matches_from(argv).unwrap();
            assert!(
                m.subcommand_matches("ppm")
                    .unwrap()
                    .subcommand_matches(name)
                    .is_some(),
                "rig ppm {} did not parse",
                name
            );
        }

        let m = rig_app()
            .try_get_matches_from(["rig", "ppm", "builds", "dplyr"])
            .unwrap();
        let builds = m
            .subcommand_matches("ppm")
            .unwrap()
            .subcommand_matches("builds")
            .unwrap();
        assert_eq!(
            builds.get_one::<String>("package"),
            Some(&"dplyr".to_string())
        );
        assert_eq!(builds.get_one::<String>("version"), None);
        assert!(!builds.get_flag("json"));

        // --version and its short form land in the same place.
        for argv in [
            ["rig", "ppm", "builds", "dplyr", "--version", "1.1.4"],
            ["rig", "ppm", "builds", "dplyr", "-v", "1.1.4"],
        ] {
            let m = rig_app().try_get_matches_from(argv).unwrap();
            let builds = m
                .subcommand_matches("ppm")
                .unwrap()
                .subcommand_matches("builds")
                .unwrap();
            assert_eq!(
                builds.get_one::<String>("version"),
                Some(&"1.1.4".to_string())
            );
        }

        // The package is required, and `ppm` alone is not a command.
        assert!(rig_app()
            .try_get_matches_from(["rig", "ppm", "builds"])
            .is_err());
        assert!(rig_app().try_get_matches_from(["rig", "ppm"]).is_err());

        // `platforms` hides the retired targets unless asked.
        let platforms = |argv: &[&str]| {
            rig_app()
                .try_get_matches_from(argv.to_vec())
                .unwrap()
                .subcommand_matches("ppm")
                .unwrap()
                .subcommand_matches("platforms")
                .unwrap()
                .get_flag("all")
        };
        assert!(!platforms(&["rig", "ppm", "platforms"]));
        assert!(platforms(&["rig", "ppm", "platforms", "--all"]));

        // --json works at all three levels.
        let m = rig_app()
            .try_get_matches_from(["rig", "ppm", "status", "--json"])
            .unwrap();
        assert!(m
            .subcommand_matches("ppm")
            .unwrap()
            .subcommand_matches("status")
            .unwrap()
            .get_flag("json"));

        let m = rig_app()
            .try_get_matches_from(["rig", "ppm", "--json", "builds", "dplyr"])
            .unwrap();
        assert!(m.subcommand_matches("ppm").unwrap().get_flag("json"));

        let m = rig_app()
            .try_get_matches_from(["rig", "--json", "ppm", "url"])
            .unwrap();
        assert!(m.get_flag("json"));
    }
}
