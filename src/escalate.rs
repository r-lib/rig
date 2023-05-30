use std::error::Error;

#[cfg(target_os = "windows")]
use std::path::Path;

#[cfg(any(target_os = "macos", target_os = "linux"))]
use simple_error::bail;

#[cfg(target_os = "windows")]
use simplelog::debug;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use simplelog::info;

#[cfg(any(target_os = "macos", target_os = "linux"))]
use sudo::with_env;

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
        info!(
            "Running `sudo` for {}. This might need your password.",
            task
        );
        with_env(&["RIG_HOME", "RUST_BACKTRACE"])?;
    }

    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn get_home() -> Result<String, Box<dyn Error>> {
    let home = match std::env::var("HOME") {
        Ok(x) => Ok(x),
        Err(_) => {
            bail!("rig needs the HOME env var set");
        }
    };
    home
}

#[cfg(target_os = "windows")]
pub fn escalate(task: &str) -> Result<(), Box<dyn Error>> {
    if is_elevated::is_elevated() {
        return Ok(());
    }
    debug!("Re-running rig as administrator for {}.", task);
    let args: Vec<String> = std::env::args().collect();
    let args: Vec<String> = [
	vec!["-d".to_string()],
	args
    ].concat();
    let exe = std::env::current_exe()?;
    let exedir = Path::new(&exe).parent();
    let instdir = match exedir {
        Some(d) => d,
        None => Path::new("/"),
    };
    let gsudo = instdir.join("gsudo.exe");
    debug!("gsudo: {:?}", gsudo);
    debug!("Arguments: {:?}.", args);
    let code = std::process::Command::new(gsudo).args(args).status()?;
    std::process::exit(code.code().unwrap());
}
