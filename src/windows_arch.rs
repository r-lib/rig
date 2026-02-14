#![allow(dead_code)]

use std::ffi::{c_void, OsStr};
use std::os::windows::ffi::OsStrExt;
use std::sync::Once;

/// Windows API types
type BOOL = i32;
type HANDLE = *mut c_void;
type USHORT = u16;

const FALSE: BOOL = 0;
const IMAGE_FILE_MACHINE_I386: USHORT = 0x014c;
const IMAGE_FILE_MACHINE_AMD64: USHORT = 0x8664;
const IMAGE_FILE_MACHINE_ARM64: USHORT = 0xAA64;

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> HANDLE;
    fn GetModuleHandleW(lpModuleName: *const u16) -> HANDLE;
    fn GetProcAddress(hModule: HANDLE, lpProcName: *const u8) -> *mut c_void;
    fn IsWow64Process(hProcess: HANDLE, wow64Process: *mut BOOL) -> BOOL;
}

/// Signature of IsWow64Process2
type FnIsWow64Process2 = unsafe extern "system" fn(HANDLE, *mut USHORT, *mut USHORT) -> BOOL;

static INIT: Once = Once::new();
static mut FN_ISWOW64PROCESS2: Option<FnIsWow64Process2> = None;

unsafe fn init_iswow64process2() {
    // Load kernel32.dll
    let wide_name: Vec<u16> = OsStr::new("kernel32.dll")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let module = GetModuleHandleW(wide_name.as_ptr());
    if module.is_null() {
        return;
    }

    // Look up IsWow64Process2
    let proc_name = b"IsWow64Process2\0";
    let ptr_fn = GetProcAddress(module, proc_name.as_ptr());
    if !ptr_fn.is_null() {
        FN_ISWOW64PROCESS2 = Some(std::mem::transmute(ptr_fn));
    }
}

/// Returns a string representing the **native system architecture**, e.g.
/// `"x86_64"`, `"aarch64"`, `"x86"`, or `"unknown"`.
pub fn get_native_arch() -> &'static str {
    unsafe {
        INIT.call_once(|| init_iswow64process2());

        let hproc = GetCurrentProcess();

        // Try IsWow64Process2 if available
        if let Some(f) = FN_ISWOW64PROCESS2 {
            let mut proc_machine: USHORT = 0;
            let mut native_machine: USHORT = 0;
            let ok = f(hproc, &mut proc_machine, &mut native_machine);
            if ok != FALSE {
                return match native_machine {
                    IMAGE_FILE_MACHINE_AMD64 => "x86_64",
                    IMAGE_FILE_MACHINE_ARM64 => "aarch64",
                    IMAGE_FILE_MACHINE_I386 => "x86",
                    _ => "unknown",
                };
            }
        }

        // Fallback: use IsWow64Process
        let mut is_wow64: BOOL = 0;
        let ok = IsWow64Process(hproc, &mut is_wow64);
        if ok != FALSE {
            // We can only distinguish 32-bit vs 64-bit, not ARM64 vs x86_64
            if is_wow64 != 0 {
                return "x86_64"; // assume 64-bit host when under WOW64
            } else {
                #[cfg(target_arch = "x86_64")]
                {
                    return "x86_64";
                }
                #[cfg(target_arch = "aarch64")]
                {
                    return "aarch64";
                }
                #[cfg(target_arch = "x86")]
                {
                    return "x86";
                }
                #[cfg(not(any(
                    target_arch = "x86_64",
                    target_arch = "aarch64",
                    target_arch = "x86"
                )))]
                return "unknown";
            }
        }

        // Final fallback if everything fails
        #[cfg(target_arch = "x86_64")]
        {
            "x86_64"
        }
        #[cfg(target_arch = "aarch64")]
        {
            "aarch64"
        }
        #[cfg(target_arch = "x86")]
        {
            "x86"
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "x86")))]
        {
            "unknown"
        }
    }
}
