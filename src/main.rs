use clap::ArgMatches;

mod args;
use args::parse_args;

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

mod common;
mod download;
mod resolve;
mod rversion;
mod utils;

use crate::common::*;

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod escalate;

fn main() {
    let args = parse_args();

    match args.subcommand() {
        Some(("add", sub)) => sc_add(sub),
        Some(("default", sub)) => sc_default(sub),
        Some(("list", _)) => sc_list(),
        Some(("rm", sub)) => sc_rm(sub),
        Some(("system", sub)) => sc_system(sub),
        Some(("resolve", sub)) => sc_resolve(sub),
        Some(("rstudio", sub)) => sc_rstudio(sub),
        _ => {} // unreachable
    }
}

fn sc_system(args: &ArgMatches) {
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
        _ => panic!("Usage: rig system [SUBCOMMAND], see help"),
    }
}

fn sc_resolve(args: &ArgMatches) {
    let version = get_resolve(args);
    let url: String = match version.url {
        Some(s) => s.to_string(),
        None => "NA".to_string(),
    };
    println!("{} {}", version.version.unwrap(), url);
}

fn sc_list() {
    let vers = sc_get_list();
    let def = match sc_get_default() {
        None => "".to_string(),
        Some(v) => v
    };
    for ver in vers {
        if def == ver {
            println!("{} (default)", ver)
        } else {
            println!("{}", ver);
        }
    }
}

fn sc_default(args: &ArgMatches) {
    if args.is_present("version") {
        let ver = args.value_of("version").unwrap().to_string();
        sc_set_default(ver);
    } else {
        sc_show_default();
    }
}

pub fn sc_system_create_lib(args: &ArgMatches) {
    let vers = args.values_of("version");
    if vers.is_none() {
        system_create_lib(None);
        return;
    } else {
        let vers: Vec<String> = vers.unwrap().map(|v| v.to_string()).collect();
        system_create_lib(Some(vers));
    }
}

pub fn sc_system_add_pak(args: &ArgMatches) {
    let devel = args.is_present("devel");
    let all = args.is_present("all");
    let vers = args.values_of("version");
    let mut pakver = args.value_of("pak-version").unwrap();
    let pakverx = args.occurrences_of("pak-version") > 0;

    // --devel is deprecated
    if devel && !pakverx {
        println!("Note: --devel is now deprecated, use --pak-version instead");
        println!("Selecting 'devel' version");
        pakver = "devel";
    }
    if devel && pakverx {
        println!("Note: --devel is ignored in favor of --pak-version");
    }
    if all {
        system_add_pak(Some(sc_get_list()), pakver, true);
    } else if vers.is_none() {
        system_add_pak(None, pakver, true);
        return;
    } else {
        let vers: Vec<String> = vers.unwrap().map(|v| v.to_string()).collect();
        system_add_pak(Some(vers), pakver, true);
    }
}
