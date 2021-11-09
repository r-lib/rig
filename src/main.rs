
use clap::ArgMatches;

mod args;
use args::parse_args;

mod macos;
use macos::*;

mod resolve;
mod rversion;
mod download;

fn main() {
    let args = parse_args();

    match args.subcommand() {
        ("add",       Some(sub)) => { sc_add(sub)     },
        ("default",   Some(sub)) => { sc_default(sub) },
        ("list",      Some(_)  ) => { sc_list()       },
        ("rm",        Some(sub)) => { sc_rm(sub)      },
        ("system",    Some(sub)) => { sc_system(sub)  },
        ("resolve",   Some(sub)) => { sc_resolve(sub) },
        ("available", Some(_)  ) => { sc_available()  },
        _                        => { } // unreachable
    }
}

fn sc_system(args: &ArgMatches) {
    match args.subcommand() {
        ("add-pak",          Some(_)) => { sc_system_add_pak()          },
        ("create-lib",       Some(_)) => { sc_system_create_lib()       },
        ("make-links",       Some(_)) => { sc_system_make_links()       },
        ("make-orthogonal",  Some(_)) => { sc_system_make_orthogonal()  },
        ("fix-permissions",  Some(_)) => { sc_system_fix_permissions()  },
        ("clean-system-lib", Some(_)) => { sc_system_clean_system_lib() },
        ("forget",           Some(_)) => { sc_system_forget()           },
        _ => panic!("Usage: rim system [SUBCOMMAND], see help"),
    }
}

fn sc_available() {
    unimplemented!();
}
