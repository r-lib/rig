#[cfg(any(target_os = "macos", target_os = "linux"))]

use sudo::escalate_if_needed;

pub fn escalate() {
    let need_sudo = match sudo::check() {
        sudo::RunningAs::Root => { false },
        sudo::RunningAs::User => { true },
        sudo::RunningAs::Suid => { true }
    };
    if need_sudo {
        println!("Sorry, rim needs your password for this.");
        escalate_if_needed().unwrap();
    }
}
