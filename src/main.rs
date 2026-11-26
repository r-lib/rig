use std::error::Error;

use clap::ArgMatches;
use log::debug;
use simple_error::SimpleError;
use tabular::*;

#[macro_use]
mod macros;

mod args;
use args::*;

mod script_meta;
mod scrun;
mod stdout_redirect;
use scrun::*;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos::*;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
mod windows_arch;
#[cfg(target_os = "windows")]
use windows::*;
#[cfg(target_os = "windows")]
mod shim_format;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux::*;

use resolve::*;

mod alias;
mod audit;
mod auth;
mod built;
mod cache;
mod cache_cmd;
mod common;
mod config;
mod dcf;
mod dirs;
mod download;
mod exclude_newer;
mod hardcoded;
mod install;
mod install_receipt;
mod library;
mod logging;
mod output;
mod pager;
mod pkg;
mod pkgsource;
mod platform;
mod ppm;
mod proj;
mod proj_repos;
mod rds;
mod renv;
mod repos;
mod repositories;
mod resolve;
mod rproj;
mod run;
mod rvenv;
mod rversion;
mod self_uninstall;
mod self_update;
mod solver;
mod sysreqs;
mod test;
mod textfmt;
mod utils;

use cache::get_logs_dir;
use library::*;
use pkg::*;
use platform::*;
use ppm::*;
use proj::*;
use repos::*;
use utils::unset_r_envvars;

use crate::common::*;
use crate::output::OUTPUT;
use crate::test::*;

mod escalate;

// ------------------------------------------------------------------------

// `rig_app()` builds the whole clap command tree in a single function, so in an
// unoptimized build its stack frame is larger than the 1 MB main thread stack we
// get on Windows, and `rig` overflows the stack before it parses its arguments.
// Do the work on a thread with a bigger stack. (build.rs needs the same
// workaround for the completion generator.)
const STACK_SIZE: usize = 32 * 1024 * 1024;

fn main() {
    let exit_code = std::thread::Builder::new()
        .name("main".to_string())
        .stack_size(STACK_SIZE)
        .spawn(main_)
        .expect("failed to spawn the main thread")
        .join()
        // The panic itself was already reported by the default panic hook, so
        // just use the exit code the runtime would have used for it.
        .unwrap_or(101);
    std::process::exit(exit_code);
}

fn main_() -> i32 {
    let args = parse_args();

    // The logs live in the cache directory, so this must come before the
    // logger is set up.
    if let Some(dir) = args.get_one::<String>("cache-dir") {
        if let Err(e) = cache::set_cache_dir(dir) {
            OUTPUT.error(&format!("{}", e));
            return 1;
        }
    }

    // -- set up logger output --------------------------------------------

    // `--quiet` overrides `--verbose`.
    let quiet = args.get_count("quiet") >= 1;
    let verbose_count = if quiet { 0 } else { args.get_count("verbose") };
    OUTPUT.set_verbose(verbose_count >= 1);
    OUTPUT.set_cli_quiet(quiet);
    let log_file = get_logs_dir().ok().map(|dir| dir.join("rig.log"));
    logging::init(verbose_count, log_file.as_deref());
    set_panic_hook();

    unset_r_envvars();

    if args.get_flag("user") {
        if let Err(e) = utils::set_mode(utils::Mode::User) {
            OUTPUT.error(&format!("{}", e));
            return 1;
        }
    } else if args.get_flag("admin") {
        if let Err(e) = utils::set_mode(utils::Mode::Admin) {
            OUTPUT.error(&format!("{}", e));
            return 1;
        }
    }

    if args.get_flag("no-cache") {
        if let Err(e) = cache::set_no_cache(true) {
            OUTPUT.error(&format!("{}", e));
            return 1;
        }
    }

    #[cfg(target_os = "linux")]
    set_cert_envvar();

    // --------------------------------------------------------------------

    let cmdline = std::env::args().collect::<Vec<_>>().join(" ");
    let pid = std::process::id();
    debug!("RIG START [pid:{}] {}", pid, cmdline);

    let result = match main__(&args) {
        Ok(exitcode) => {
            debug!("RIG END [pid:{}] {}", pid, cmdline);
            exitcode
        }
        Err(err) => {
            let status = output::find_error::<output::ExitStatusError>(err.as_ref())
                .map(|e| e.status)
                .unwrap_or(1);
            OUTPUT.fatal(err.as_ref());
            if json_requested(&args) {
                print_json_error(err.as_ref());
            }
            debug!("RIG END [pid:{}] {}: {}", pid, cmdline, err);
            status
        }
    };

    cache::cleanup_ephemeral_cache_dir();

    result
}

/// Whether `--json` was given, to the command or to rig itself.
fn json_requested(args: &ArgMatches) -> bool {
    let mut next = Some(args);
    while let Some(m) = next {
        if m.try_get_one::<bool>("json").ok().flatten() == Some(&true) {
            return true;
        }
        next = m.subcommand().map(|(_, sub)| sub);
    }
    false
}

/// With `--json`, a failed command writes `{"error": ..., "causes": [...]}`
/// to stdout, so a program that reads the output does not have to parse
/// stderr. The error is shown on stderr as well.
fn print_json_error(err: &(dyn Error + 'static)) {
    let mut msgs = output::error_messages(err);
    if msgs.is_empty() {
        // Already shown to the user, e.g. a failed package install.
        msgs.push(err.to_string());
    }
    let json = serde_json::json!({
        "error": msgs[0],
        "causes": &msgs[1..],
    });
    if let Ok(text) = serde_json::to_string_pretty(&json) {
        println!("{}", text);
    }
}

/// Add a line to the default panic message, so a crash is reported, and
/// write it to the log file as well.
fn set_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_hook(info);
        debug!("rig crashed: {}", info);
        let log = get_logs_dir()
            .map(|dir| format!(" The log file is at {}.", dir.join("rig.log").display()))
            .unwrap_or_default();
        OUTPUT.emit(&format!(
            "rig crashed, this is a bug in rig. Please report it at \
             https://github.com/r-lib/rig/issues.{}",
            log
        ));
    }));
}

fn main__(args: &ArgMatches) -> Result<i32, Box<dyn Error>> {
    let mut retval: i32 = 0;
    match args.subcommand() {
        Some(("add", sub)) => sc_add_cmd(sub, args)?,
        Some(("audit", sub)) => retval = audit::sc_audit(sub, args)?,
        Some(("default", sub)) => sc_default(sub, args)?,
        Some(("list", sub)) => sc_list(sub, args)?,
        Some(("proj", sub)) => sc_proj(sub, args)?,
        Some(("rm", sub)) => sc_rm(sub)?,
        Some(("self", sub)) => sc_self(sub, args)?,
        Some(("system", sub)) => sc_system(sub, args)?,
        Some(("rtools", sub)) => sc_system_rtools(sub, args)?,
        Some(("pkg", sub)) => match sub.subcommand() {
            // The only `rig pkg` command with an exit status.
            Some(("audit", s)) => retval = audit::sc_pkg_audit(s, args)?,
            _ => sc_pkg(sub, args)?,
        },
        Some(("ppm", sub)) => sc_ppm(sub, args)?,
        Some(("auth", sub)) => auth::cmd::sc_auth(sub, args)?,
        Some(("repos", sub)) => sc_repos(sub, args)?,
        Some(("resolve", sub)) => sc_resolve(sub, args)?,
        Some(("rstudio", sub)) => sc_rstudio(sub)?,
        Some(("library", sub)) => sc_library(sub, args)?,
        Some(("cache", sub)) => sc_cache(sub)?,
        Some(("config", sub)) => crate::config::sc_config(sub, args)?,
        Some(("available", sub)) => sc_available(sub, args)?,
        Some(("run", sub)) => retval = sc_run(sub, args)?,
        Some(("test", sub)) => sc_test(sub, args)?,
        _ => (), // unreachable
    }
    Ok(retval)
}

fn sc_self(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        Some(("update", s)) => self_update::sc_self_update(s, mainargs),
        Some(("uninstall", s)) => self_uninstall::sc_self_uninstall(s, mainargs),
        // Every subcommand defined in `args.rs` has an arm above, so this is
        // only reached if one is renamed there without updating this list.
        _ => Ok(()),
    }
}

fn sc_system(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        Some(("add-pak", s)) => sc_system_add_pak(s),
        Some(("allow-core-dumps", s)) => sc_system_allow_core_dumps(s),
        Some(("allow-debugger", s)) => sc_system_allow_debugger(s),
        Some(("allow-debugger-rstudio", s)) => sc_system_allow_debugger_rstudio(s),
        Some(("clean-registry", _)) => sc_clean_registry(),
        Some(("setup-user-lib", s)) => sc_system_setup_user_lib(s),
        Some(("dirs", s)) => crate::dirs::sc_system_dirs(s, mainargs),
        Some(("make-links", _)) => sc_system_make_links(),
        Some(("fix-aliases", s)) => sc_system_fix_aliases(s),
        Some(("make-orthogonal", s)) => sc_system_make_orthogonal(s),
        Some(("fix-permissions", s)) => sc_system_fix_permissions(s),
        Some(("forget", _)) => sc_system_forget(),
        Some(("no-openmp", s)) => sc_system_no_openmp(s),
        Some(("blas", s)) => match s.subcommand() {
            Some(("status", s2)) => sc_system_blas_status(s2),
            Some(("set", s2)) => sc_system_blas_set(s2),
            Some((name, _)) => bail!(
                "Internal error: unknown `rig system blas` subcommand: {}",
                name
            ),
            None => Ok(()),
        },
        Some(("user-mode", s)) => sc_system_user_mode(s),
        Some(("clean-admin-r", s)) => sc_system_clean_admin_r(s),
        Some(("fix-r-alias", s)) => sc_system_fix_r_alias(s),
        Some(("script-assoc", s)) => sc_system_script_assoc(s),
        Some(("update-certs", _)) => sc_system_update_certs(),
        Some(("update-rtools40", _)) => sc_system_update_rtools40(),
        Some(("detect-platform", s)) => sc_system_detect_platform(s, mainargs),
        Some(("rtools", s)) => sc_system_rtools(s, mainargs),
        // Every subcommand defined in `args.rs` has an arm above, so this is
        // only reached if one is renamed there without updating this list.
        // Bail instead of silently doing nothing, so that such a mismatch
        // fails loudly rather than becoming a no-op that exits with 0.
        Some((name, _)) => bail!("Internal error: unknown `rig system` subcommand: {}", name),
        None => Ok(()),
    }
}

fn sc_library(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        Some(("list", s)) => sc_library_ls(s, args, mainargs),
        Some(("add", s)) => sc_library_add(s),
        Some(("rm", s)) => sc_library_rm(s),
        Some(("default", s)) => sc_library_default(s, args, mainargs),
        Some((name, _)) => bail!("Internal error: unknown `rig library` subcommand: {}", name),
        None => Ok(()),
    }
}

fn sc_cache(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        Some(("info", s)) => cache_cmd::sc_cache_info(s),
        Some(("clean", s)) => cache_cmd::sc_cache_clean(s),
        Some((name, _)) => bail!("Internal error: unknown `rig cache` subcommand: {}", name),
        None => Ok(()),
    }
}

// ------------------------------------------------------------------------

fn sc_resolve(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let version = get_resolve(args)?;

    if args.get_flag("json") || mainargs.get_flag("json") {
        println!("{}", serde_json::to_string_pretty(&vec![&version])?);
    } else {
        let url: String = match &version.url {
            Some(s) => s.to_string(),
            None => "NA".to_string(),
        };
        let ver: String = match &version.version {
            Some(s) => s.to_string(),
            None => "???".to_string(),
        };
        println!("{} {}", ver, url);
    }

    Ok(())
}

// ------------------------------------------------------------------------

fn sc_add_cmd(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    #[derive(serde::Serialize)]
    struct AddedVersion<'a> {
        name: &'a str,
        default: bool,
        version: &'a Option<String>,
        aliases: &'a Vec<String>,
        path: &'a Option<String>,
        binary: &'a Option<String>,
        #[serde(rename = "new-install")]
        new_install: bool,
    }

    let json = args.get_flag("json") || mainargs.get_flag("json");

    #[cfg(target_os = "windows")]
    {
        let str: &String = args.get_one("str").unwrap();
        if json && str.starts_with("rtools") {
            let msg = "--json is not supported for `rig add rtools`, use `rig rtools add --json`";
            bail!("{}", msg);
        }
    }

    let res = sc_add(args)?;
    if !json {
        return Ok(());
    }
    let res = match res {
        Some(r) => r,
        None => bail!("Cannot determine the installed R version"),
    };

    let vers = sc_get_list_details()?;
    let ver = match vers.iter().find(|v| v.name == res.name) {
        Some(v) => v,
        None => bail!("Cannot find the installed R version '{}'", res.name),
    };
    let def = sc_get_default()?.unwrap_or_default();

    let out = AddedVersion {
        name: &ver.name,
        default: def == ver.name,
        version: &ver.version,
        aliases: &ver.aliases,
        path: &ver.path,
        binary: &ver.binary,
        new_install: res.new_install,
    };
    println!("{}", serde_json::to_string_pretty(&out)?);

    Ok(())
}

fn sc_list(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    #[derive(serde::Serialize)]
    struct InstalledVersionWithDefault<'a> {
        name: &'a str,
        default: bool,
        version: &'a Option<String>,
        aliases: &'a Vec<String>,
        path: &'a Option<String>,
        binary: &'a Option<String>,
    }

    let vers = sc_get_list_details()?;
    let def = match sc_get_default()? {
        None => "".to_string(),
        Some(v) => v,
    };

    if args.get_flag("plain") {
        if args.get_flag("json") || mainargs.get_flag("json") {
            bail!("the argument '--plain' cannot be used with '--json'");
        }
        for ver in vers.iter() {
            println!("{}", ver.name);
        }
    } else if args.get_flag("json") || mainargs.get_flag("json") {
        let json_vers: Vec<InstalledVersionWithDefault> = vers
            .iter()
            .map(|ver| InstalledVersionWithDefault {
                name: &ver.name,
                default: def == ver.name,
                version: &ver.version,
                aliases: &ver.aliases,
                path: &ver.path,
                binary: &ver.binary,
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&json_vers)?);
    } else {
        let mut tab = Table::new("{:<} {:<}  {:<}  {:<}");
        tab.add_row(row!["*", "name", "version", "aliases"]);
        tab.add_heading("------------------------------------------");
        for ver in vers {
            let dflt = if def == ver.name { "*" } else { " " };
            let note = match ver.version {
                None => "(broken?)".to_string(),
                Some(v) => {
                    if v != ver.name {
                        format!("(R {})", v)
                    } else {
                        "".to_string()
                    }
                }
            };
            let als = ver.aliases.join(", ");
            tab.add_row(row!(dflt, ver.name, note, als));
        }

        print!("{}", tab);
    }

    Ok(())
}

// ------------------------------------------------------------------------

fn sc_default(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    #[derive(serde::Serialize)]
    struct DefaultVersion {
        name: String,
    }

    if args.contains_id("version") {
        let ver = args.get_one::<String>("version").unwrap().to_string();
        sc_set_default(&ver)
    } else {
        let default = sc_get_default_or_fail()?;
        if args.get_flag("json") || mainargs.get_flag("json") {
            println!(
                "{}",
                serde_json::to_string_pretty(&DefaultVersion {
                    name: default.clone()
                })?
            );
        } else {
            println!("{}", default);
        }
        Ok(())
    }
}

// ------------------------------------------------------------------------

pub fn sc_system_setup_user_lib(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let vers = args.get_many::<String>("version");
    let vers: Vec<String> = match vers {
        None => sc_get_list()?,
        Some(vers) => vers.map(|v| v.to_string()).collect(),
    };

    for ver in vers {
        library_update_rprofile(&ver)?;
    }
    Ok(())
}

// ------------------------------------------------------------------------

pub fn sc_system_add_pak(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let devel = args.get_flag("devel");
    let all = args.get_flag("all");
    let vers = args.get_many::<String>("version");
    let pakver = args.get_one::<String>("pak-version").unwrap();
    let mut pakver = &pakver[..];
    let pakverx = args.value_source("pak-version") == Some(clap::parser::ValueSource::CommandLine);

    // --devel is deprecated
    if devel && !pakverx {
        OUTPUT.warn(
            "--devel is now deprecated, use --pak-version instead. Selecting 'devel' version",
        );
        pakver = "devel";
    }
    if devel && pakverx {
        OUTPUT.warn("--devel is ignored in favor of --pak-version");
    }
    if all {
        system_add_pak(Some(sc_get_list()?), pakver, true)?;
    } else if vers.is_none() {
        system_add_pak(None, pakver, true)?;
    } else {
        let vers: Vec<String> = vers
            .ok_or(SimpleError::new("Internal argument error"))?
            .map(|v| v.to_string())
            .collect();
        system_add_pak(Some(vers), pakver, true)?;
    }

    Ok(())
}
