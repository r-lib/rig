#![cfg(target_os = "windows")]

use clap::ArgMatches;

use crate::resolve::resolve_versions;

use crate::rversion::Rversion;

#[warn(unused_variables)]
pub fn sc_add(args: &ArgMatches) {
    unimplemented!();
}

pub fn sc_default(args: &ArgMatches) {
    unimplemented!();
}

pub fn sc_list() {
    unimplemented!();
}

pub fn sc_rm(args: &ArgMatches) {
    unimplemented!();
}

pub fn sc_system_add_pak(args: &ArgMatches) {
    unimplemented!();
}

pub fn sc_system_create_lib(args: &ArgMatches) {
    unimplemented!();
}

pub fn sc_system_make_links() {
    unimplemented!();
}

pub fn sc_system_make_orthogonal(_args: &ArgMatches) {
    // Nothing to do on Windows
}

pub fn sc_system_fix_permissions(args: &ArgMatches) {
    unimplemented!();
}

pub fn sc_system_clean_system_lib() {
    unimplemented!();
}

pub fn sc_system_forget() {
    // Nothing to do on Windows
}

pub fn get_resolve(args: &ArgMatches) -> Rversion {
    let str = args.value_of("str").unwrap().to_string();
    let arch = match args.value_of("arch") {
        Some(a) => a.to_string(),
        None => "default".to_string(),
    };

    if !valid_windows_archs().contains(&arch) {
	panic!("Unknown Windows arch: {}", arch);
    }
    let arch = match arch.as_str() {
	"default" => "msvcrt",
	other => other,
    }.to_string();

    let eps = vec![str];
    let version = resolve_versions(eps, "win".to_string(), arch);
    version[0].to_owned()
}

// ------------------------------------------------------------------------

fn valid_windows_archs() -> Vec<String> {
    vec![
	"msvcrt".to_string(),
	"ucrt".to_string(),
	"default".to_string()
    ]
}
