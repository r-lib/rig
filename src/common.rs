
#[cfg(target_os = "macos")]
use crate::macos::*;

#[cfg(target_os = "windows")]
use crate::windows::*;

pub fn check_installed(ver: &String) -> bool {
    let inst = sc_get_list();
    assert!(
        inst.contains(&ver),
        "Version {} is not installed, see 'rim list'",
        ver
    );
    true
}
