#![cfg(target_os = "macos")]
#![allow(dead_code)]

use std::alloc::System;
use std::error::Error;
use std::sync::Mutex;

// Otherwise C cannot free() the returned strings

#[global_allocator]
static GLOBAL: System = System;

use lazy_static::lazy_static;
use libc;
use simple_error::bail;

mod common;
mod config;
mod download;
mod escalate;
mod library;
mod macos;
mod resolve;
mod rversion;
mod utils;
use macos::*;

// ------------------------------------------------------------------------

lazy_static! {
    static ref LAST_ERROR: Mutex<String> = Mutex::new(String::from(""));
    static ref LAST_ERROR2: Mutex<i32> = Mutex::new(0);
}

static SUCCESS:                  libc::c_int =  0;
static ERROR_NO_DEFAULT:         libc::c_int = -1;
static ERROR_DEFAULT_FAILED:     libc::c_int = -2;
static ERROR_BUFFER_SHORT:       libc::c_int = -3;
static ERROR_SET_DEFAULT_FAILED: libc::c_int = -4;
static ERROR_INVALID_INPUT:      libc::c_int = -5;

// ------------------------------------------------------------------------

// Caller must free this

#[no_mangle]
pub extern "C" fn rig_last_error(
    ptr: *mut libc::c_char,
    size: libc::size_t
) -> libc::c_int {
    let str: String = match LAST_ERROR.try_lock() {
        Ok(x) => x.to_owned(),
        Err(_) => "Unknown error".to_string()
    };

    let str2;
    if size <= str.len() {
        str2 = str[..(size-1)].to_string() + "\0";
    } else {
        str2 = str.to_string()
    }
    match set_c_string(&str2, ptr, size) {
        Ok(x) => x,
        Err(_) => ERROR_BUFFER_SHORT
    }
}

fn set_error(str: &str) {
    match LAST_ERROR.try_lock() {
        Ok(mut x) => {
            x.clear();
            x.insert_str(0, str);
        },
        Err(_) => {
            // cannot save error message, not much we can do
        }
    };
}

fn set_c_string(from: &str, ptr: *mut libc::c_char, size: libc::size_t)
                -> Result<libc::c_int, Box<dyn Error>> {
    let from = from.to_string() + "\0";
    let bts = from.as_bytes();
    let n = from.bytes().count();
    if n <= size {
        let ptr2;
        unsafe {
            ptr2 = std::slice::from_raw_parts_mut(ptr as *mut u8, size as usize);
        }
        ptr2[0..n].clone_from_slice(bts);
        Ok(SUCCESS)
    } else {
        bail!("String buffer too short")
    }
}

fn set_c_strings(from: Vec<String>, ptr: *mut libc::c_char, size: libc::size_t)
                 -> Result<libc::c_int, Box<dyn Error>> {
    let mut n = from.len() + 1; // terminating \0 plus ultimate temrinating \0
    for s in &from {
        n += s.len();
    }
    if n <= size {
        let mut idx = 0;
        let ptr2;
        unsafe {
            ptr2 = std::slice::from_raw_parts_mut(ptr as *mut u8, size as usize);
        }
        for s in &from {
            let l = s.len();
            ptr2[idx..(idx+l)].clone_from_slice(s.as_bytes());
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

#[no_mangle]
pub extern "C" fn rig_get_default(
    ptr: *mut libc::c_char,
    size: libc::size_t
) -> libc::c_int {

    let def = sc_get_default();

    match def {
        Ok(x) => {
            match x {
                Some(xx) => {
                    match set_c_string(&xx, ptr, size) {
                        Ok(x) => x,
                        Err(_) => {
                            set_error("Buffer too short for R version");
                            ERROR_BUFFER_SHORT
                        }
                    }
                },
                None => {
                    set_error("No default R version is set currently");
                    ERROR_NO_DEFAULT
                }
            }
        },
        Err(e) => {
            let msg = e.to_string();
            set_error(&msg);
            ERROR_DEFAULT_FAILED
        }
    }
}

#[no_mangle]
pub extern "C" fn rig_list(
    ptr: *mut libc::c_char,
    size: libc::size_t
) -> libc::c_int {

    let vers = sc_get_list();

    match vers {
        Ok(x) => {
            match set_c_strings(x, ptr, size) {
                Ok(x) => x,
                Err(_) => {
                    set_error("Buffer too short for R version");
                    ERROR_BUFFER_SHORT
                }
            }
        },
        Err(e) => {
            let msg = e.to_string();
            set_error(&msg);
            ERROR_DEFAULT_FAILED
        }
    }
}

#[no_mangle]
pub extern "C" fn rig_list_with_versions(
    ptr: *mut libc::c_char,
    size: libc::size_t
) -> libc::c_int {

    let vers = sc_get_list_with_versions();

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
                    return ERROR_BUFFER_SHORT
                }
            }
        },
        Err(e) => {
            let msg = e.to_string();
            set_error(&msg);
            ERROR_DEFAULT_FAILED
        }
    }
}

#[no_mangle]
pub extern "C" fn rig_set_default(
    ptr: *const libc::c_char) -> libc::c_int {

    let cver;

    unsafe {
        cver = std::ffi::CStr::from_ptr(ptr);
    }

    let ver = match cver.to_str() {
        Ok(x) => x,
        Err(_) => {
            return ERROR_INVALID_INPUT
        }
    };

    match sc_set_default(ver) {
        Ok(_) => {
            SUCCESS
        },
        Err(e) => {
            let msg = e.to_string();
            set_error(&msg);
            ERROR_SET_DEFAULT_FAILED
        }
    }
}

#[no_mangle]
pub extern "C" fn rig_start_rstudio(
    pversion: *const libc::c_char,
    pproject: *const libc::c_char) -> libc::c_int {

    let cver;
    let cprj;

    unsafe {
        cver = std::ffi::CStr::from_ptr(pversion);
        cprj = std::ffi::CStr::from_ptr(pproject);
    }

    let ver = match cver.to_str() {
        Ok(x) => x,
        Err(_) => {
            return ERROR_INVALID_INPUT;
        }
    };

    let prj = match cprj.to_str() {
        Ok(x) => x,
        Err(_) => {
            return ERROR_INVALID_INPUT;
        }
    };

    let ver = if ver == "" { None } else { Some(ver) };
    let prj = if prj == "" { None } else { Some(prj) };

    match sc_rstudio_(ver, prj) {
        Ok(_) => {
            SUCCESS
        },
        Err(e) => {
            let msg = e.to_string();
            set_error(&msg);
            ERROR_SET_DEFAULT_FAILED
        }
    }
}
