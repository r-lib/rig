#![cfg(target_os = "linux")]

use regex::Regex;
use std::path::Path;

use clap::ArgMatches;

use crate::resolve::resolve_versions;
use crate::rversion::Rversion;

use crate::utils::*;

pub struct LinuxVersion {
    pub distro: String,
    pub version: String,
}

pub fn sc_add(args: &ArgMatches) {
    let linux = detect_linux();
    unimplemented!();
}

pub fn sc_rm(args: &ArgMatches) {
    unimplemented!();
}

pub fn sc_system_add_pak(args: &ArgMatches) {
    unimplemented!();
}

pub fn system_create_lib(vers: Option<Vec<String>>) {
    unimplemented!();
}

pub fn sc_system_make_links() {
    unimplemented!();
}

pub fn get_resolve(args: &ArgMatches) -> Rversion {
    let str = args.value_of("str").unwrap().to_string();

    let eps = vec![str];
    let version = resolve_versions(eps, "linux".to_string(), "default".to_string());
    version[0].to_owned()
}

pub fn sc_get_list() -> Vec<String> {
    unimplemented!();
}

pub fn sc_set_default(ver: String) {
    unimplemented!();
}

pub fn sc_show_default() {
    unimplemented!();
}

pub fn sc_system_make_orthogonal(_args: &ArgMatches) {
    // Nothing to do on Windows
}

pub fn sc_system_fix_permissions(args: &ArgMatches) {
    // Nothing to do on Windows
}

pub fn sc_system_forget() {
    // Nothing to do on Windows
}

fn detect_linux() -> LinuxVersion {
    let release_file = Path::new("/etc/os-release");
    let lines = match read_lines(release_file) {
        Ok(x) => { x },
        Err(err) => { panic!("Unknown Linux, no /etc/os-release"); }
    };

    let re_id = Regex::new("^ID=").unwrap();
    let wid_line = grep_lines(&re_id, &lines);
    if wid_line.len() == 0 {
        panic!("Unknown Linux distribution");
    }
    let id_line = &lines[wid_line[0]];
    let id = re_id.replace(&id_line, "").to_string();

    let re_ver = Regex::new("^VERSION_ID=").unwrap();
    let wver_line = grep_lines(&re_ver, &lines);
    if wver_line.len() == 0 {
        panic!("Unknown {} Linux version", id);
    }
    let ver_line = &lines[wver_line[0]];
    let ver = re_ver.replace(&ver_line, "").to_string();

    println!("id: {}, ver: {}", id, ver);

    LinuxVersion {
        distro: "ubuntu".to_string(),
        version: "18.04".to_string()
    }
}

pub fn sc_clean_registry() {
    // Nothing to do on Linux
}
