use std::cmp::Ordering;

#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::ffi::OsString;

#[derive(Default, Debug, Clone)]
#[allow(dead_code)]
pub struct Rversion {
    pub version: Option<String>,
    pub url: Option<String>,
    pub arch: Option<String>,
    pub ppm: bool,
    pub ppmurl: Option<String>,
}

#[cfg(target_os = "macos")]
#[derive(Default, Debug, Clone)]
pub struct RversionDir {
    pub version: String,
    pub arch: String,
    pub installdir: String,
}

#[derive(Default, Debug, Clone)]
pub struct InstalledVersion {
    pub name: String,
    pub version: Option<String>,
    pub path: Option<String>,
    pub binary: Option<String>,
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct OKInstalledVersion {
    pub name: String,
    pub version: semver::Version,
    pub path: String,
    pub binary: String,
}

impl Ord for OKInstalledVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        self.version.cmp(&other.version)
    }
}

impl PartialOrd for OKInstalledVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for OKInstalledVersion {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version
    }
}

impl Eq for OKInstalledVersion {}

#[cfg(target_os = "linux")]
#[derive(PartialEq, Clone, Debug)]
pub struct OsVersion {
    pub rig_platform: Option<String>,
    pub arch: String,
    pub vendor: String,
    pub os: String,
    pub distro: String,
    pub version: String,
}

#[derive(PartialEq, Clone, Debug)]
pub struct PkgLibrary {
    pub rversion: String,
    pub name: String,
    pub path: std::path::PathBuf,
    pub default: bool,
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[derive(Default, Debug)]
#[allow(dead_code)]
pub struct User {
    pub user: String,
    pub uid: u32,
    pub gid: u32,
    pub dir: OsString,
    pub sudo: bool,
}

#[derive(Default, Debug)]
pub struct Alias {
    pub alias: String,
    pub version: String,
}

#[derive(Default, Debug)]
pub struct Available {
    pub name: String,
    pub version: String,
    pub date: Option<String>,
    pub url: Option<String>,
    pub rtype: Option<String>,
}

#[cfg(target_os = "linux")]
#[derive(Default, Debug)]
pub struct LinuxTools {
    pub package_name: String,
    pub install: Vec<Vec<String>>,
    pub get_package_name: Vec<String>,
    pub is_installed: Vec<String>,
    pub delete: Vec<String>,
}

#[cfg(target_os = "windows")]
#[derive(Default, Debug)]
pub struct RtoolsVersion {
    pub version: String,
    pub url: String,
    pub first: String,
    pub last: String,
}
