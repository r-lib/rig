use clap::ArgMatches;

mod args;
use args::parse_args;

mod macos;
use macos::*;

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
        Some(("available", _)) => sc_available(),
        _ => {} // unreachable
    }
}

fn sc_system(args: &ArgMatches) {
    match args.subcommand() {
        Some(("add-pak", s)) => sc_system_add_pak(s),
        Some(("create-lib", s)) => sc_system_create_lib(s),
        Some(("make-links", _)) => sc_system_make_links(),
        Some(("make-orthogonal", s)) => sc_system_make_orthogonal(s),
        Some(("fix-permissions", s)) => sc_system_fix_permissions(s),
        Some(("clean-system-lib", _)) => sc_system_clean_system_lib(),
        Some(("forget", _)) => sc_system_forget(),
        _ => panic!("Usage: rim system [SUBCOMMAND], see help"),
    }
}

fn sc_available() {
    unimplemented!();
}
