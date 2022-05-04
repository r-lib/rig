
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
        "Version {} is not installed, see 'rim list'",
        ver
    );
    true
}

// -- rim default ---------------------------------------------------------

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
            panic!("No default R version is set, call `rim default <version>`");
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

// ------------------------------------------------------------------------
