#![cfg(target_os = "macos")]
#![allow(dead_code)]

use std::alloc::System;
use std::error::Error;
use std::sync::Mutex;

// Otherwise C cannot free() the returned strings

#[global_allocator]
static GLOBAL: System = System;

use lazy_static::lazy_static;

#[macro_use]
mod macros;

mod alias;
mod args;
mod auth;
mod built;
mod cache;
mod common;
mod config;
mod dcf;
mod download;
mod escalate;
mod exclude_newer;
mod hardcoded;
mod install;
mod library;
mod logging;
mod macos;
mod output;
mod pager;
mod pkg;
mod pkgsource;
mod platform;
mod ppm;
mod proj;
mod proj_repos;
mod rds;
mod renv;
mod repos;
mod repositories;
mod resolve;
mod rproj;
mod run;
mod rvenv;
mod rversion;
mod scan_deps;
mod script_meta;
mod solver;
mod stdout_redirect;
mod sysreqs;
mod textfmt;
mod utils;
use common::*;
use library::*;
use macos::*;

// ------------------------------------------------------------------------

lazy_static! {
    static ref LAST_ERROR: Mutex<String> = Mutex::new(String::from(""));
    static ref LAST_ERROR2: Mutex<i32> = Mutex::new(0);
}

static SUCCESS: libc::c_int = 0;
static ERROR_NO_DEFAULT: libc::c_int = -1;
static ERROR_DEFAULT_FAILED: libc::c_int = -2;
static ERROR_BUFFER_SHORT: libc::c_int = -3;
static ERROR_SET_DEFAULT_FAILED: libc::c_int = -4;
static ERROR_INVALID_INPUT: libc::c_int = -5;

// ------------------------------------------------------------------------

// Caller must free this

#[no_mangle]
pub extern "C" fn rig_last_error(ptr: *mut libc::c_char, size: libc::size_t) -> libc::c_int {
    let str: String = match LAST_ERROR.lock() {
        Ok(x) => x.to_owned(),
        Err(poisoned) => poisoned.into_inner().to_owned(),
    };

    let str2 = if size <= str.len() {
        str[..(size - 1)].to_string() + "\0"
    } else {
        str.to_string()
    };
    match set_c_string(&str2, ptr, size) {
        Ok(x) => x,
        Err(_) => ERROR_BUFFER_SHORT,
    }
}

fn set_error(str: &str) {
    let mut x = match LAST_ERROR.lock() {
        Ok(x) => x,
        Err(poisoned) => poisoned.into_inner(),
    };
    x.clear();
    x.push_str(str);
}

/// Set up the log file, once per process. The app has no terminal, so this
/// is the only place where rig's diagnostics end up.
fn init_logging() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let log_file = cache::get_logs_dir().ok().map(|dir| dir.join("rig.log"));
        logging::init(0, log_file.as_deref());
    });
}

/// Run a rig command for the app. The app has no terminal, so messages are
/// collected instead of printed, and if the command fails, its warnings and
/// errors are kept for `rig_last_error()`, together with the error itself.
fn run<T>(f: impl FnOnce() -> Result<T, Box<dyn Error>>) -> Result<T, Box<dyn Error>> {
    init_logging();
    output::OUTPUT.start_capture();
    let result = f();
    if let Err(e) = &result {
        // The same as what `main` shows, with the causes of the error.
        output::OUTPUT.fatal(e.as_ref());
    }
    let lines = output::OUTPUT.take_captured();
    if let Err(e) = &result {
        set_error(&error_with_messages(&e.to_string(), lines));
    }
    result
}

// The warnings and errors that were shown, or just `error` if there were none.
fn error_with_messages(error: &str, lines: Vec<String>) -> String {
    let msgs: Vec<String> = lines
        .into_iter()
        .filter(|l| l.starts_with("Error: ") || l.starts_with("Warning: "))
        .collect();
    if msgs.is_empty() {
        error.to_string()
    } else {
        msgs.join("\n")
    }
}

fn invalid_input() -> libc::c_int {
    set_error("Invalid input, it must be UTF-8 text");
    ERROR_INVALID_INPUT
}

fn set_c_string(
    from: &str,
    ptr: *mut libc::c_char,
    size: libc::size_t,
) -> Result<libc::c_int, Box<dyn Error>> {
    let from = from.to_string() + "\0";
    let bts = from.as_bytes();
    let n = from.len();
    if n <= size {
        let ptr2;
        unsafe {
            ptr2 = std::slice::from_raw_parts_mut(ptr as *mut u8, size);
        }
        ptr2[0..n].clone_from_slice(bts);
        Ok(SUCCESS)
    } else {
        bail!("String buffer too short")
    }
}

fn set_c_strings(
    from: Vec<String>,
    ptr: *mut libc::c_char,
    size: libc::size_t,
) -> Result<libc::c_int, Box<dyn Error>> {
    let mut n = from.len() + 1; // terminating \0 plus ultimate temrinating \0
    for s in &from {
        n += s.len();
    }
    if n <= size {
        let mut idx = 0;
        let ptr2;
        unsafe {
            ptr2 = std::slice::from_raw_parts_mut(ptr as *mut u8, size);
        }
        for s in &from {
            let l = s.len();
            ptr2[idx..(idx + l)].clone_from_slice(s.as_bytes());
            idx += l;
            ptr2[idx] = 0;
            idx += 1;
        }
        ptr2[idx] = 0;
        Ok(SUCCESS)
    } else {
        bail!("String buffer too short")
    }
}

// ------------------------------------------------------------------------

// The installation mode rig is configured for ("user" or "admin"). The menu
// bar app has no shell environment, so it cannot see RIG_MODE, but
// get_mode() also reads the `mode` key of the rig config file, which is
// where `rig system user-mode` records it.

#[no_mangle]
pub extern "C" fn rig_mode(ptr: *mut libc::c_char, size: libc::size_t) -> libc::c_int {
    match run(utils::get_mode) {
        Ok(mode) => match set_c_string(&mode.to_string(), ptr, size) {
            Ok(x) => x,
            Err(_) => {
                set_error("Buffer too short for rig mode");
                ERROR_BUFFER_SHORT
            }
        },
        Err(_) => ERROR_DEFAULT_FAILED,
    }
}

// The directory the R versions are installed into. This is
// /Library/Frameworks/R.framework/Versions in admin mode and
// ~/.local/share/rig/r in user mode. The app watches it for changes.

#[no_mangle]
pub extern "C" fn rig_r_root(ptr: *mut libc::c_char, size: libc::size_t) -> libc::c_int {
    match run(get_r_root) {
        Ok(root) => match set_c_string(&root, ptr, size) {
            Ok(x) => x,
            Err(_) => {
                set_error("Buffer too short for R root directory");
                ERROR_BUFFER_SHORT
            }
        },
        Err(_) => ERROR_DEFAULT_FAILED,
    }
}

#[no_mangle]
pub extern "C" fn rig_get_default(ptr: *mut libc::c_char, size: libc::size_t) -> libc::c_int {
    let def = run(sc_get_default);

    match def {
        Ok(x) => match x {
            Some(xx) => match set_c_string(&xx, ptr, size) {
                Ok(x) => x,
                Err(_) => {
                    set_error("Buffer too short for R version");
                    ERROR_BUFFER_SHORT
                }
            },
            None => {
                set_error("No default R version is set currently");
                ERROR_NO_DEFAULT
            }
        },
        Err(_) => ERROR_DEFAULT_FAILED,
    }
}

#[no_mangle]
pub extern "C" fn rig_list(ptr: *mut libc::c_char, size: libc::size_t) -> libc::c_int {
    let vers = run(sc_get_list);

    match vers {
        Ok(x) => match set_c_strings(x, ptr, size) {
            Ok(x) => x,
            Err(_) => {
                set_error("Buffer too short for R version");
                ERROR_BUFFER_SHORT
            }
        },
        Err(_) => ERROR_DEFAULT_FAILED,
    }
}

#[no_mangle]
pub extern "C" fn rig_list_with_versions(
    ptr: *mut libc::c_char,
    size: libc::size_t,
) -> libc::c_int {
    let vers = run(sc_get_list_details);

    match vers {
        Ok(x) => {
            let mut vers: Vec<String> = vec![];
            for it in x {
                let r = it.name + "|" + &it.version.unwrap_or("".to_string());
                vers.push(r)
            }
            match set_c_strings(vers, ptr, size) {
                Ok(x) => x,
                Err(_) => {
                    set_error("Buffer too short for R version");
                    ERROR_BUFFER_SHORT
                }
            }
        }
        Err(_) => ERROR_DEFAULT_FAILED,
    }
}

#[no_mangle]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "C" fn rig_set_default(ptr: *const libc::c_char) -> libc::c_int {
    let cver;

    unsafe {
        cver = std::ffi::CStr::from_ptr(ptr);
    }

    let ver = match cver.to_str() {
        Ok(x) => x,
        Err(_) => return invalid_input(),
    };

    match run(|| sc_set_default(ver)) {
        Ok(_) => SUCCESS,
        Err(_) => ERROR_SET_DEFAULT_FAILED,
    }
}

#[no_mangle]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "C" fn rig_start_rstudio(
    pversion: *const libc::c_char,
    pproject: *const libc::c_char,
) -> libc::c_int {
    let cver;
    let cprj;

    unsafe {
        cver = std::ffi::CStr::from_ptr(pversion);
        cprj = std::ffi::CStr::from_ptr(pproject);
    }

    let ver = match cver.to_str() {
        Ok(x) => x,
        Err(_) => {
            return invalid_input();
        }
    };

    let prj = match cprj.to_str() {
        Ok(x) => x,
        Err(_) => {
            return invalid_input();
        }
    };

    let ver = if ver.is_empty() {
        None
    } else {
        Some(ver.to_string())
    };
    let prj = if prj.is_empty() {
        None
    } else {
        Some(prj.to_string())
    };

    match run(|| sc_rstudio2(ver.as_ref(), prj.as_ref())) {
        Ok(_) => SUCCESS,
        Err(_) => ERROR_SET_DEFAULT_FAILED,
    }
}

#[no_mangle]
pub extern "C" fn rig_library_list(ptr: *mut libc::c_char, size: libc::size_t) -> libc::c_int {
    let vers = run(|| sc_library_get_list(None, true));

    match vers {
        Ok(x) => {
            let mut vers: Vec<String> = vec![];
            for it in x {
                let nm = it.name + if it.default { "*" } else { "" };
                vers.push(nm)
            }
            match set_c_strings(vers, ptr, size) {
                Ok(x) => x,
                Err(_) => {
                    set_error("Buffer too short for R libraries");
                    ERROR_BUFFER_SHORT
                }
            }
        }
        Err(_) => ERROR_DEFAULT_FAILED,
    }
}

#[no_mangle]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "C" fn rig_lib_set_default(ptr: *const libc::c_char) -> libc::c_int {
    let cver;

    unsafe {
        cver = std::ffi::CStr::from_ptr(ptr);
    }

    let ver = match cver.to_str() {
        Ok(x) => x,
        Err(_) => return invalid_input(),
    };

    match run(|| sc_library_set_default(ver, None)) {
        Ok(_) => SUCCESS,
        Err(_) => ERROR_SET_DEFAULT_FAILED,
    }
}
