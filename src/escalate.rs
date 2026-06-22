use std::error::Error;

use log::*;

#[cfg(target_os = "windows")]
use std::path::{Path, PathBuf};

#[cfg(target_os = "windows")]
use simple_error::bail;

#[cfg(any(target_os = "macos", target_os = "linux"))]
use simple_error::bail;

#[cfg(any(target_os = "macos", target_os = "linux"))]
use sudo::with_env;

#[cfg(any(target_os = "macos", target_os = "linux"))]
use crate::output::OUTPUT;

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn escalate(task: &str) -> Result<(), Box<dyn Error>> {
    let need_sudo = match sudo::check() {
        sudo::RunningAs::Root => false,
        sudo::RunningAs::User => true,
        sudo::RunningAs::Suid => true,
    };

    match std::env::var("RIG_HOME") {
        Ok(_) => {}
        Err(_) => {
            let home = get_home()?;
            std::env::set_var("RIG_HOME", home);
        }
    };

    if need_sudo {
        eprintln!(
            "Running `sudo` for {}. This might need your password.",
            task
        );
        with_env(&[
            "RIG_HOME",
            "RIG_BINARY_DIR",
            "RIG_MODE",
            "RIG_R_INSTALL_DIR",
            "RUST_BACKTRACE",
            "http_proxy",
            "https_proxy",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "LANG",
            "LC_ALL",
            "LC_COLLATE",
            "LC_CTYPE",
            "LC_MESSAGES",
            "LC_MONETARY",
            "LC_NUMERIC",
            "LC_TIME",
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
            "RIG_PLATFORM",
        ])?;
    }

    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn get_home() -> Result<String, Box<dyn Error>> {
    let home = match std::env::var("HOME") {
        Ok(x) => Ok(x),
        Err(_) => {
            OUTPUT.error(
                "The HOME environment variable is not set. Please set it to your home directory.",
            );
            error!("The HOME environment variable is not set.");
            bail!("The HOME environment variables is not set. rig needs the HOME env var set");
        }
    };
    home
}

// Locate gsudo.exe on the PATH, so admin mode still works when rig itself was
// installed without a bundled gsudo somehow.
#[cfg(target_os = "windows")]
fn gsudo_on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join("gsudo.exe");
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

#[cfg(target_os = "windows")]
pub fn escalate(task: &str) -> Result<(), Box<dyn Error>> {
    if crate::utils::get_mode()? == crate::utils::Mode::User {
        return Ok(());
    }
    if is_elevated::is_elevated() {
        return Ok(());
    }
    debug!("Re-running rig as administrator for {}.", task);
    let args: Vec<String> = std::env::args().collect();
    let args: Vec<String> = [vec!["-d".to_string()], args].concat();
    let exe = std::env::current_exe()?;
    let exedir = Path::new(&exe).parent();
    let instdir = match exedir {
        Some(d) => d,
        None => Path::new("/"),
    };

    // Prefer the gsudo.exe shipped next to rig.exe; otherwise fall back to one on the
    // PATH. If neither exists we cannot elevate, so report it instead of failing silently
    // (in interactive mode a bare `error!` only reaches the log file, not the terminal).
    let bundled = instdir.join("gsudo.exe");
    let gsudo = if bundled.is_file() {
        bundled
    } else if let Some(found) = gsudo_on_path() {
        found
    } else {
        let msg = format!(
            "Cannot run admin task ({}): elevation helper gsudo.exe was not found next to \
             rig ({}) or on the PATH. Re-run from an elevated (Administrator) terminal, \
             or install gsudo and ensure it is on the PATH.",
            task,
            instdir.display()
        );
        crate::output::OUTPUT.error(&msg);
        error!("{}", msg);
        bail!("{}", msg);
    };

    debug!("gsudo: {:?}", gsudo);
    debug!("Arguments: {:?}.", args);
    let code = match std::process::Command::new(&gsudo).args(&args).status() {
        Ok(code) => code,
        Err(err) => {
            let msg = format!(
                "Cannot run admin task ({}): failed to start elevation helper {}: {}",
                task,
                gsudo.display(),
                err
            );
            crate::output::OUTPUT.error(&msg);
            error!("{}", msg);
            bail!("{}", msg);
        }
    };
    std::process::exit(code.code().unwrap_or(1));
}
