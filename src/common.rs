
use clap::ArgMatches;

#[cfg(target_os = "macos")]
use crate::macos::*;

#[cfg(target_os = "windows")]
use crate::windows::*;

#[cfg(target_os = "linux")]
use crate::linux::*;

pub fn check_installed(ver: &String) -> bool {
    let inst = sc_get_list();
    assert!(
        inst.contains(&ver),
        "Version {} is not installed, see 'rig list'",
        ver
    );
    true
}

// -- rig default ---------------------------------------------------------

// Good example of how errors should be handled.
// * The implementation (function with `_` suffix), and it forwards the
//   errors upstream. If there is no error, we return an Option<String>,
//   because there might not be a default set.
// * `sc_get_default()` will panic on error.
// * `sc_get_default_or_fail()` will also panic if no default is set.

pub fn sc_show_default() {
    let default = sc_get_default_or_fail();
    println!("{}", default);
}

pub fn sc_get_default_or_fail() -> String {
    match sc_get_default() {
        None => {
            panic!("No default R version is set, call `rig default <version>`");
        },
        Some(x) => x
    }
}

pub fn sc_get_default() -> Option<String> {
    match sc_get_default_() {
        Err(err) => {
            panic!("Cannot query default R version: {}", err.to_string());
        },
        Ok(res) => res
    }
}

pub fn set_default_if_none(ver: String) {
    let cur = sc_get_default();
    if cur.is_none() {
        sc_set_default(ver);
    }
}

pub fn sc_set_default(ver: String) {
    match sc_set_default_(&ver) {
        Err(err) => {
            panic!("Failed to set R version {}: {}", &ver, err.to_string());
        },
        Ok(_) => { }
    };
}

// -- rig list ------------------------------------------------------------

pub fn sc_get_list() -> Vec<String> {
    match sc_get_list_() {
        Err(err) => {
            panic!("Cannot list installed R versions: {}", err.to_string());
        },
        Ok(res) => res
    }
}

// -- rig rstudio ---------------------------------------------------------

pub fn sc_rstudio(args: &ArgMatches) {
    let mut ver = args.value_of("version");
    let mut prj = args.value_of("project-file");

    // If the first argument is an R project file, and the second is not,
    // then we switch the two
    if ver.is_some() && ver.unwrap().ends_with(".Rproj") {
        ver = args.value_of("project-file");
        prj = args.value_of("version");
    }

    match sc_rstudio_(ver, prj) {
        Ok(_) => { },
        Err(err) => {
            panic!("{}", err.to_string());
        }
    };
}

// ------------------------------------------------------------------------
