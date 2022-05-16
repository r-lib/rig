#[cfg(any(target_os = "macos", target_os = "linux"))]

use sudo::with_env;

pub fn escalate(task: &str) {
    let need_sudo = match sudo::check() {
        sudo::RunningAs::Root => { false },
        sudo::RunningAs::User => { true },
        sudo::RunningAs::Suid => { true }
    };

    match std::env::var("RIG_HOME") {
	Ok(_) => { },
	Err(_) => {
	    let home = get_home();
	    std::env::set_var("RIG_HOME", home);
	}
    };

    if need_sudo {
        println!("Running `sudo` for {}. This might need your password.", task);
        with_env(&["RIG_HOME", "RUST_BACKTRACE"]).unwrap();
    }
}

pub fn get_home() -> String {
    let home = match std::env::var("HOME") {
	Ok(x) => { x },
	Err(_) => { panic!("rig needs the HOME env var set"); }
    };
    home
}
