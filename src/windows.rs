#![cfg(target_os = "windows")]

use clap::ArgMatches;

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

pub fn sc_resolve(args: &ArgMatches) {
    unimplemented!();
}
