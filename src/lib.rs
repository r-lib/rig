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
mod download;
mod escalate;
mod macos;
mod resolve;
mod rversion;
mod utils;
use macos::*;

// ------------------------------------------------------------------------

lazy_static! {
    static ref LAST_ERROR: Mutex<String> = Mutex::new(String::from(""));
}

static SUCCESS:              libc::c_int =  0;
static ERROR_NO_DEFAULT:     libc::c_int = -1;
static ERROR_DEFAULT_FAILED: libc::c_int = -2;
static ERROR_BUFFER_SHORT:   libc::c_int = -3;

// ------------------------------------------------------------------------

// Caller must free this

#[no_mangle]
pub extern "C" fn rim_last_error() -> *const libc::c_char {
    let str = LAST_ERROR.lock().unwrap();
    let bytes = Box::new(str.as_bytes());
    let ptr = bytes.as_ptr();
    std::mem::forget(bytes);
    ptr as *const libc::c_char
}

fn set_error(str: &str) {
    let mut err = LAST_ERROR.lock().unwrap();
    err.clear();
    err.insert_str(0, str);
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
pub extern "C" fn rim_get_default(
    ptr: *mut libc::c_char,
    size: libc::size_t    
) -> libc::c_int {

    let def = sc_get_default_();
    
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
pub extern "C" fn rim_list(
    ptr: *mut libc::c_char,
    size: libc::size_t    
) -> libc::c_int {

    let vers = sc_get_list_();

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
