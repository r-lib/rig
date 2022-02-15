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

mod common;
mod download;
mod resolve;
mod rversion;
mod utils;

fn main() {
    let args = parse_args();

    match args.subcommand() {
        Some(("add", sub)) => sc_add(sub),
        Some(("default", sub)) => sc_default(sub),
        Some(("list", _)) => sc_list(),
        Some(("rm", sub)) => sc_rm(sub),
        Some(("system", sub)) => sc_system(sub),
        Some(("resolve", sub)) => sc_resolve(sub),
        _ => {} // unreachable
    }
}

fn sc_system(args: &ArgMatches) {
    match args.subcommand() {
        Some(("add-pak", s)) => sc_system_add_pak(s),
        Some(("clean-registry", _)) => sc_clean_registry(),
        Some(("create-lib", s)) => sc_system_create_lib(s),
        Some(("make-links", _)) => sc_system_make_links(),
        Some(("make-orthogonal", s)) => sc_system_make_orthogonal(s),
        Some(("fix-permissions", s)) => sc_system_fix_permissions(s),
        Some(("forget", _)) => sc_system_forget(),
        _ => panic!("Usage: rim system [SUBCOMMAND], see help"),
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
    for ver in vers {
        println!("{}", ver);
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
