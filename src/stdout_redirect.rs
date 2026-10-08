//! Send stdout to stderr for a while, at the OS level.
//!
//! `rig run` sometimes has to install R or packages before it runs R: for a
//! script with inline metadata, or a project that needs a sync. Whatever the
//! user runs owns stdout, e.g. `rig run script.R > out.csv`, so none of that
//! setup output may go there. Patching every `println!` is not enough:
//! `R CMD INSTALL`, the R installers and other child processes inherit
//! rig's stdout too. So [`StdoutToStderr`] points the process's stdout itself
//! at stderr, until it is dropped.

use std::io::Write;

/// While this is alive, everything written to stdout, by rig or by child
/// processes started meanwhile, goes to stderr instead. Dropping it restores
/// the original stdout. If the redirection fails, it does nothing.
pub struct StdoutToStderr {
    #[cfg(unix)]
    saved: Option<libc::c_int>,
    #[cfg(windows)]
    saved: Option<win::Handle>,
}

#[cfg(unix)]
impl StdoutToStderr {
    pub fn new() -> Self {
        let _ = std::io::stdout().flush();
        // SAFETY: plain file descriptor calls on the standard descriptors.
        let saved = unsafe {
            let saved = libc::dup(libc::STDOUT_FILENO);
            if saved < 0 {
                None
            } else if libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO) < 0 {
                libc::close(saved);
                None
            } else {
                Some(saved)
            }
        };
        StdoutToStderr { saved }
    }
}

#[cfg(unix)]
impl Drop for StdoutToStderr {
    fn drop(&mut self) {
        let _ = std::io::stdout().flush();
        if let Some(saved) = self.saved.take() {
            // SAFETY: `saved` is the descriptor `new` duplicated, and only
            // this guard owns it.
            unsafe {
                libc::dup2(saved, libc::STDOUT_FILENO);
                libc::close(saved);
            }
        }
    }
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    pub type Handle = *mut c_void;
    pub const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    pub const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    pub const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;

    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetStdHandle(std_handle: u32) -> Handle;
        pub fn SetStdHandle(std_handle: u32, handle: Handle) -> i32;
    }
}

// Rust's stdout and `std::process::Command` both look up the standard handle
// with `GetStdHandle` when they use it, so swapping the handle is enough.
#[cfg(windows)]
impl StdoutToStderr {
    pub fn new() -> Self {
        let _ = std::io::stdout().flush();
        // SAFETY: plain calls on the process's standard handles.
        let saved = unsafe {
            let out = win::GetStdHandle(win::STD_OUTPUT_HANDLE);
            let err = win::GetStdHandle(win::STD_ERROR_HANDLE);
            if out == win::INVALID_HANDLE_VALUE
                || err == win::INVALID_HANDLE_VALUE
                || err.is_null()
                || win::SetStdHandle(win::STD_OUTPUT_HANDLE, err) == 0
            {
                None
            } else {
                Some(out)
            }
        };
        StdoutToStderr { saved }
    }
}

#[cfg(windows)]
impl Drop for StdoutToStderr {
    fn drop(&mut self) {
        let _ = std::io::stdout().flush();
        if let Some(saved) = self.saved.take() {
            // SAFETY: restores the handle `new` replaced.
            unsafe {
                win::SetStdHandle(win::STD_OUTPUT_HANDLE, saved);
            }
        }
    }
}
