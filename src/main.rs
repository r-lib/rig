use std::error::Error;
use std::io::Write;

use clap::ArgMatches;
use env_logger;
use log::{error, info, Level, LevelFilter};
use owo_colors::OwoColorize;
use simple_error::*;
use tabular::*;

mod args;
use args::*;

mod scrun;
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

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux::*;

use resolve::*;

mod alias;
mod common;
mod config;
mod dcf;
mod download;
mod hardcoded;
mod library;
mod proj;
mod renv;
mod repos;
mod repositories;
mod resolve;
mod run;
mod rversion;
mod solver;
mod sysreqs;
mod utils;

use library::*;
use proj::*;
use repos::*;
use sysreqs::*;
use utils::unset_r_envvars;

use crate::common::*;

mod escalate;

// ------------------------------------------------------------------------

fn main() {
    let exit_code = main_();
    std::process::exit(exit_code);
}

fn main_() -> i32 {
    let args = parse_args();

    // -- set up logger output --------------------------------------------

    let verbose_count = args.get_count("verbose");
    let mut loglevel = match verbose_count {
        0 => LevelFilter::Info,
        1 => LevelFilter::Debug,
        _ => LevelFilter::Trace,
    };

    match args.get_count("quiet") {
        0 => {}
        1 => loglevel = LevelFilter::Warn,
        2 => loglevel = LevelFilter::Error,
        _ => loglevel = LevelFilter::Off,
    };

    // Build filter string: set default level for rig, lower level for noisy crates
    let filter_str = match loglevel {
        LevelFilter::Off => "off".to_string(),
        LevelFilter::Trace => "rig=trace,reqwest=trace,hyper=trace,pubgrub=trace".to_string(),
        LevelFilter::Debug => "rig=debug,reqwest=info,hyper=info,pubgrub=info".to_string(),
        LevelFilter::Info => "rig=info,reqwest=warn,hyper=warn,pubgrub=warn".to_string(),
        LevelFilter::Warn => "rig=warn,reqwest=warn,hyper=warn,pubgrub=warn".to_string(),
        LevelFilter::Error => "rig=error,reqwest=error,hyper=error,pubgrub=error".to_string(),
    };

    match env_logger::Builder::from_env(env_logger::Env::default())
        .filter(None, loglevel)
        .parse_filters(&filter_str)
        .target(env_logger::Target::Stderr)
        .format(move |buf, record| {
            let level_string = match record.level() {
                Level::Error => record.level().to_string().red().bold().to_string(),
                Level::Warn => record.level().to_string().yellow().bold().to_string(),
                Level::Info => record.level().to_string().blue().to_string(),
                Level::Debug => record.level().to_string().cyan().to_string(),
                Level::Trace => record.level().to_string().white().to_string(),
            };

            // Show timestamps and location info only if -v was passed
            if verbose_count >= 1 {
                writeln!(
                    buf,
                    "[{} {} {}:{}] {}",
                    buf.timestamp(),
                    level_string,
                    record.file().unwrap_or("unknown"),
                    record.line().unwrap_or(0),
                    record.args()
                )
            } else {
                // No verbose flag: just show the message
                writeln!(buf, "[{}] {}", level_string, record.args())
            }
        })
        .try_init()
    {
        Err(e) => {
            eprintln!("Fatal error, cannot set up logger: {}", e.to_string());
            return 2;
        }
        _ => {}
    };

    unset_r_envvars();

    #[cfg(target_os = "linux")]
    set_cert_envvar();

    // --------------------------------------------------------------------

    match main__(&args) {
        Ok(exitcode) => {
            return exitcode;
        }
        Err(err) => {
            error!("{}", err.to_string());
            return 1;
        }
    }
}

fn main__(args: &ArgMatches) -> Result<i32, Box<dyn Error>> {
    let mut retval: i32 = 0;
    match args.subcommand() {
        Some(("add", sub)) => sc_add(sub)?,
        Some(("default", sub)) => sc_default(sub, args)?,
        Some(("list", sub)) => sc_list(sub, args)?,
        Some(("proj", sub)) => sc_proj(sub, args)?,
        Some(("rm", sub)) => sc_rm(sub)?,
        Some(("system", sub)) => sc_system(sub, args)?,
        Some(("repos", sub)) => sc_repos(sub, args)?,
        Some(("resolve", sub)) => sc_resolve(sub, args)?,
        Some(("rstudio", sub)) => sc_rstudio(sub)?,
        Some(("library", sub)) => sc_library(sub, args)?,
        Some(("sysreqs", sub)) => sc_sysreqs(sub, args)?,
        Some(("available", sub)) => sc_available(sub, args)?,
        Some(("run", sub)) => retval = sc_run(sub, args)?,
        _ => (), // unreachable
    }
    Ok(retval)
}

fn sc_system(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        Some(("add-pak", s)) => sc_system_add_pak(s),
        Some(("allow-core-dumps", s)) => sc_system_allow_core_dumps(s),
        Some(("allow-debugger", s)) => sc_system_allow_debugger(s),
        Some(("allow-debugger-rstudio", s)) => sc_system_allow_debugger_rstudio(s),
        Some(("clean-registry", _)) => sc_clean_registry(),
        Some(("create-lib", s)) => sc_system_create_lib(s),
        Some(("make-links", _)) => sc_system_make_links(),
        Some(("make-orthogonal", s)) => sc_system_make_orthogonal(s),
        Some(("fix-permissions", s)) => sc_system_fix_permissions(s),
        Some(("forget", _)) => sc_system_forget(),
        Some(("no-openmp", s)) => sc_system_no_openmp(s),
        Some(("update-rtools40", _)) => sc_system_update_rtools40(),
        Some(("detect-platform", s)) => sc_system_detect_platform(s, mainargs),
        Some(("rtools", s)) => sc_system_rtools(s, mainargs),
        _ => Ok(()), // unreachable
    }
}

fn sc_library(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        Some(("list", s)) => sc_library_ls(s, args, mainargs),
        Some(("add", s)) => sc_library_add(s),
        Some(("rm", s)) => sc_library_rm(s),
        Some(("default", s)) => sc_library_default(s, args, mainargs),
        _ => Ok(()), // unreachable
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

pub fn sc_system_create_lib(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
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
        info!("Note: --devel is now deprecated, use --pak-version instead");
        info!("Selecting 'devel' version");
        pakver = "devel";
    }
    if devel && pakverx {
        info!("Note: --devel is ignored in favor of --pak-version");
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
