use std::error::Error;

use clap::ArgMatches;
use simple_error::*;
use simplelog::*;

mod args;
use args::*;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos::*;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
use windows::*;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux::*;

mod library;
use library::*;

mod common;
mod config;
mod download;
mod resolve;
mod rversion;
mod utils;

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

    let mut loglevel = match args.occurrences_of("verbose") {
        0 => LevelFilter::Info,
        1 => LevelFilter::Debug,
        _ => LevelFilter::Trace,
    };

    if args.is_present("quiet") {
        loglevel = LevelFilter::Off;
    }

    let config = ConfigBuilder::new()
        .set_time_level(LevelFilter::Trace)
        .set_location_level(LevelFilter::Debug)
        .set_level_color(Level::Error, Some(Color::Magenta))
        .set_level_color(Level::Warn, Some(Color::Yellow))
        .set_level_color(Level::Info, Some(Color::Blue))
        .set_level_color(Level::Debug, None)
        .set_level_color(Level::Trace, None)
        .build();

    match TermLogger::init(loglevel, config, TerminalMode::Stderr, ColorChoice::Auto) {
        Err(e) => {
            eprintln!("Fatal error, cannot set up logger: {}", e.to_string());
            return 2;
        }
        _ => {}
    };

    // --------------------------------------------------------------------

    match main__(&args) {
        Ok(_) => {
            return 0;
        }
        Err(err) => {
            error!("{}", err.to_string());
            return 1;
        }
    }
}

fn main__(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        Some(("add", sub)) => sc_add(sub),
        Some(("default", sub)) => sc_default(sub, args),
        Some(("list", sub)) => sc_list(sub, args),
        Some(("rm", sub)) => sc_rm(sub),
        Some(("system", sub)) => sc_system(sub),
        Some(("resolve", sub)) => sc_resolve(sub, args),
        Some(("rstudio", sub)) => sc_rstudio(sub),
        Some(("library", sub)) => sc_library(sub, args),
        _ => Ok(()), // unreachable
    }
}

fn sc_system(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        Some(("add-pak", s)) => sc_system_add_pak(s),
        Some(("allow-core-dumps", s)) => sc_system_allow_core_dumps(s),
        Some(("allow-debugger", s)) => sc_system_allow_debugger(s),
        Some(("clean-registry", _)) => sc_clean_registry(),
        Some(("create-lib", s)) => sc_system_create_lib(s),
        Some(("make-links", _)) => sc_system_make_links(),
        Some(("make-orthogonal", s)) => sc_system_make_orthogonal(s),
        Some(("fix-permissions", s)) => sc_system_fix_permissions(s),
        Some(("forget", _)) => sc_system_forget(),
        Some(("no-openmp", s)) => sc_system_no_openmp(s),
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
    let url: String = match version.url {
        Some(s) => s.to_string(),
        None => "NA".to_string(),
    };
    let version: String = match version.version {
        Some(s) => s.to_string(),
        None => "???".to_string(),
    };

    if args.is_present("json") || mainargs.is_present("json") {
        println!("[");
        println!("  {{");
        println!("     \"version\": \"{}\",", version);
        println!("     \"url\": \"{}\"", url);
        println!("  }}");
        println!("]");
    } else {
        println!("{} {}", version, url);
    }

    Ok(())
}

// ------------------------------------------------------------------------

fn sc_list(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let vers = sc_get_list_details()?;
    let def = match sc_get_default()? {
        None => "".to_string(),
        Some(v) => v,
    };

    fn or_null(x: &Option<String>) -> String {
        match x {
            None => "null".to_string(),
            Some(x) => x.to_string()
        }
    }

    if args.is_present("json") || mainargs.is_present("json") {
        println!("[");
        let num = vers.len();
        for (idx, ver) in vers.iter().enumerate() {
            let dflt = if def == ver.name { "true" } else { "false" };
            println!("  {{");
            println!("    \"name\": \"{}\",", ver.name);
            println!("    \"default\": {},", dflt);
            println!("    \"version\": \"{}\",", or_null(&ver.version));
            println!("    \"path\": \"{}\",", or_null(&ver.path));
            println!("    \"binary\": \"{}\"", or_null(&ver.binary));
            println!("  }}{}", if idx == num - 1 { "" } else { "," });
        }
        println!("]");
    } else {
        for ver in vers {
            print!("{}", ver.name);
            match ver.version {
                None => print!(" (broken?)"),
                Some(v) => if v != ver.name { print!(" ({})", v); }
            };
            if def == ver.name {
                print!(" (default)");
            }
            println!("");
        }
    }

    Ok(())
}

// ------------------------------------------------------------------------

fn sc_default(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    if args.is_present("version") {
        let ver = args
            .value_of("version")
            .ok_or(SimpleError::new("Internal argument error"))?
            .to_string();
        sc_set_default(&ver)
    } else {
        let default = sc_get_default_or_fail()?;
        if args.is_present("json") || mainargs.is_present("json") {
            println!("{{");
            println!("  \"name\": \"{}\"", default);
            println!("}}");
        } else {
            println!("{}", default);
        }
        Ok(())
    }
}

// ------------------------------------------------------------------------

pub fn sc_system_create_lib(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let vers = args.values_of("version");
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
    let devel = args.is_present("devel");
    let all = args.is_present("all");
    let vers = args.values_of("version");
    let mut pakver = args
        .value_of("pak-version")
        .ok_or(SimpleError::new("Internal argument error"))?;
    let pakverx = args.occurrences_of("pak-version") > 0;

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
