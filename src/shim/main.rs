#[cfg(target_os = "windows")]
#[path = "../shim_format.rs"]
mod shim_format;

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

    let footer = match shim_format::read_shim_footer(&exe) {
        Ok(Some(f)) => f,
        Ok(None) => {
            eprintln!(
                "rig shim: {} has no shim footer; it is a bare copy of the shim \
                 template, not a quick link created by rig.",
                exe.display()
            );
            exit(1);
        }
        Err(e) => {
            eprintln!(
                "rig shim: cannot read shim footer of {}: {}",
                exe.display(),
                e
            );
            exit(1);
        }
    };

    let args: Vec<_> = env::args_os().skip(1).collect();
    match Command::new(&footer.target).args(&args).status() {
        Ok(status) => exit(status.code().unwrap_or(1)),
        Err(e) => {
            eprintln!("rig shim: failed to run {}: {}", footer.target, e);
            exit(1);
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn main() {}
