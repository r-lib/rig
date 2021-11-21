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
        ("add", Some(sub)) => sc_add(sub),
        ("default", Some(sub)) => sc_default(sub),
        ("list", Some(_)) => sc_list(),
        ("rm", Some(sub)) => sc_rm(sub),
        ("system", Some(sub)) => sc_system(sub),
        ("resolve", Some(sub)) => sc_resolve(sub),
        ("available", Some(_)) => sc_available(),
        _ => {} // unreachable
    }
}

fn sc_system(args: &ArgMatches) {
    match args.subcommand() {
        ("add-pak", Some(s)) => sc_system_add_pak(s),
        ("create-lib", Some(s)) => sc_system_create_lib(s),
        ("make-links", Some(_)) => sc_system_make_links(),
        ("make-orthogonal", Some(s)) => sc_system_make_orthogonal(s),
        ("fix-permissions", Some(s)) => sc_system_fix_permissions(s),
        ("clean-system-lib", Some(_)) => sc_system_clean_system_lib(),
        ("forget", Some(_)) => sc_system_forget(),
        _ => panic!("Usage: rim system [SUBCOMMAND], see help"),
    }
}

fn sc_available() {
    unimplemented!();
}

fn sc_resolve(args: &ArgMatches) {
    let version = get_resolve(args);
    let url: String = match version.url {
        Some(s) => s.to_string(),
        None => "NA".to_string(),
    };
    println!("{} {}", version.version, url);
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
