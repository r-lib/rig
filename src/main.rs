
use clap::{Arg, ArgMatches, App, AppSettings, SubCommand};

use std::io::ErrorKind;
use std::path::Path;

fn parse_args() -> ArgMatches<'static> {
    App::new("RIM -- The R Installation Manager")
        .version("0.1.0")
        .author("Gábor Csárdi <csardi.gabor@gmail.com>")
        .about("Install and manage R installations. See https://github.com/gaborcsardi/rim")
        .setting(AppSettings::ArgRequiredElseHelp)
        .subcommand(
            SubCommand::with_name("default")
                .about("Prints or sets default R version")
                .arg(
                    Arg::with_name("version")
                        .help("new default R version to set")
                        .required(false)
                )
        )
        .subcommand(
            SubCommand::with_name("list")
                .about("Lists installed R versions")
        )
        .subcommand(
            SubCommand::with_name("add")
                .about("Installs new R version")
                .aliases(&["install"])
                .arg(
                    Arg::with_name("arch")
                        .help("Selects macOS arch: arm64 or x86_64")
                        .short("a")
                        .long("arch")
                        .required(false)
                        .default_value("x86_64")
                )
                .arg(
                    Arg::with_name("version")
                        .help("R versions to install (see 'rim avail')")
                        .default_value("release")
                        .multiple(true)
                )
        )
        .subcommand(
            SubCommand::with_name("rm")
                .about("Removes R versions")
                .aliases(&["del", "remote", "delete"])
                .arg(
                    Arg::with_name("version")
                        .help("versions to remove")
                        .multiple(true)
                        .required(false)
                )
                .arg(
                    Arg::with_name("all")
                        .help("remove all versions")
                        .long("all")
                        .required(false)
                )
        )
        .subcommand(
            SubCommand::with_name("system")
                .about("Manages current installations")
                .subcommand(
                    SubCommand::with_name("orthogonal")
                        .about("Makes installed versions orthogonal (macOS)")
                )
                .subcommand(
                    SubCommand::with_name("make-links")
                        .about("Creates R-* quick links")
                )
                .subcommand(
                    SubCommand::with_name("create-lib")
                        .about("Creates current user's package libraries")
                )
                .subcommand(
                    SubCommand::with_name("add-pak")
                        .about("Installs or updates pak for all R versions")
                )
        )
        .after_help(r#"EXAMPLES:
    # Install R-release and R-devel
    rim add release devel
    # Install specific version
    rim add 4.1.2
    # Install latest version within a minor version
    rim add 4.1
    # List installed versions
    rim list
    # Set default version
    rim default 4.0
"#)
        .get_matches()
}

#[cfg(target_os = "macos")]
const R_ROOT: &str = "/Library/Frameworks/R.framework/Versions";
const R_CUR:  &str = "/Library/Frameworks/R.framework/Versions/Current";

#[allow(unused_variables)]
fn check_installed(ver: &String) -> bool {
    let inst = sc_get_list();
    assert!(
        inst.contains(&ver),
        "Version {} is not installed, see 'rim list'",
        ver);
    true
}

#[allow(unused_variables)]
fn sc_add(args: &ArgMatches) {
    unimplemented!();
}

fn sc_set_default(ver: String) {
    check_installed(&ver);
    let ret = std::fs::remove_file(R_CUR);
    match ret {
        Err(err) => {
            panic!("Could not remove {}: {}", R_CUR, err)
        },
        Ok(()) => { }
    };

    let path = Path::new(R_ROOT).join(ver.as_str());
    let ret = std::os::unix::fs::symlink(&path, R_CUR);
    match ret {
        Err(err) => {
            panic!("Could not create {}: {}", path.to_str().unwrap(), err)
        },
        Ok(()) => { }
    };
}

#[cfg(target_os = "macos")]
fn sc_show_default() {
    let tgt = std::fs::read_link(R_CUR);
    let tgtbuf = match tgt {
        Err(err) => {
            match err.kind() {
                ErrorKind::NotFound => {
                    panic!("File '{}' does not exist", R_CUR)
                },
                ErrorKind::InvalidInput => {
                    panic!("File '{}' is not a symbolic link", R_CUR)
                },
                _ => panic!("Error resolving {}: {}", R_CUR, err),
            }
        },
        Ok(tgt) => tgt
    };

    // file_name() is only None if tgtbuf ends with "..", the we panic...
    let fname = tgtbuf.file_name().unwrap();

    println!("{}", fname.to_str().unwrap());
}

#[cfg(target_os = "macos")]
fn sc_default(args: &ArgMatches) {
    if args.is_present("version") {
        let ver = args.value_of("version").unwrap().to_string();
        sc_set_default(ver);
    } else {
        sc_show_default();
    }
}

#[cfg(target_os = "macos")]
fn sc_get_list() -> Vec<String> {
    let paths = std::fs::read_dir(R_ROOT);
    assert!(paths.is_ok(), "Cannot list directory {}", R_ROOT);
    let paths = paths.unwrap();

    let mut vers = Vec::new();
    for de in paths {
        let path = de.unwrap().path();
        let fname = path.file_name().unwrap();
        if fname != "Current" {
            vers.push(fname.to_str().unwrap().to_string());
        }
    }
    vers.sort();
    vers
}

#[cfg(target_os = "macos")]
fn sc_list() {
    let vers = sc_get_list();
    for ver in vers {
        println!("{}", ver);
    }
}

#[allow(unused_variables)]
fn sc_rm(args: &ArgMatches) {
    unimplemented!();
}

#[allow(unused_variables)]
fn sc_system(args: &ArgMatches) {
    unimplemented!();
}

fn main() {
    let args = parse_args();

    match args.subcommand() {
        ("add",     Some(sub)) => { sc_add(sub)     },
        ("default", Some(sub)) => { sc_default(sub) },
        ("list",    Some(_)  ) => { sc_list()       },
        ("rm",      Some(sub)) => { sc_rm(sub)      },
        ("system",  Some(sub)) => { sc_system(sub)  },
        _                      => { } // unreachable
    }
}
