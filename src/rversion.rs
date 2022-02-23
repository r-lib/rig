#[derive(Default, Debug, Clone)]
pub struct Rversion {
    pub version: Option<String>,
    pub url: Option<String>,
    pub arch: Option<String>,
}

#[derive(PartialEq, Clone)]
pub struct LinuxVersion {
    pub distro: String,
    pub version: String,
    pub url: String
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub struct User {
    pub user: String,
    pub uid: u32,
    pub gid: u32,
}
