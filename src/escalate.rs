#[cfg(any(target_os = "macos", target_os = "linux"))]

use std::error::Error;

use simple_error::bail;
use simplelog::info;
use sudo::with_env;

pub fn escalate(task: &str) -> Result<(), Box<dyn Error>> {
    let need_sudo = match sudo::check() {
        sudo::RunningAs::Root => { false },
        sudo::RunningAs::User => { true },
        sudo::RunningAs::Suid => { true }
    };

    match std::env::var("RIG_HOME") {
	Ok(_) => { },
	Err(_) => {
	    let home = get_home()?;
	    std::env::set_var("RIG_HOME", home);
	}
    };

    if need_sudo {
        info!("<cyan>[INFO]</> Running `sudo` for {}. This might need your password.", task);
        with_env(&["RIG_HOME", "RUST_BACKTRACE"])?;
    }

    Ok(())
}

pub fn get_home() -> Result<String, Box<dyn Error>> {
    let home = match std::env::var("HOME") {
	Ok(x) => { Ok(x) },
	Err(_) => { bail!("rig needs the HOME env var set"); }
    };
    home
}
