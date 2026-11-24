#[cfg(target_os = "windows")]
#[path = "../shim_format.rs"]
mod shim_format;

// A rig shim is an unmodified copy of this program. It finds its config in
// the `.shim` file next to itself (`R-4.5.2.exe` -> `R-4.5.2.shim`), sets
// the environment variables listed there and runs the target with the
// shim's own arguments. See src/shim_format.rs.
#[cfg(target_os = "windows")]
fn main() {
    use std::env;
    use std::process::{exit, Command};

    let exe = match env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("rig shim: cannot determine own path: {}", e);
            exit(1);
        }
    };

    let config = match shim_format::read_config(&exe) {
        Ok(Some(c)) => c,
        Ok(None) => {
            eprintln!(
                "rig shim: {} does not exist. {} is a copy of the rig shim, \
                 it needs this file to know what to run.",
                shim_format::config_path(&exe).display(),
                exe.display()
            );
            exit(1);
        }
        Err(e) => {
            eprintln!("rig shim: {}", e);
            exit(1);
        }
    };

    let args: Vec<_> = env::args_os().skip(1).collect();
    match Command::new(&config.target)
        .args(&args)
        .envs(config.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .status()
    {
        Ok(status) => exit(status.code().unwrap_or(1)),
        Err(e) => {
            eprintln!("rig shim: failed to run {}: {}", config.target, e);
            exit(1);
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn main() {}
