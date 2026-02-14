use std::cmp::Ordering;

use serde_derive::Deserialize;
use serde_derive::Serialize;

#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::ffi::OsString;

#[allow(dead_code)]
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct Rversion {
    pub version: Option<String>,
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    #[serde(skip)]
    pub ppm: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ppmurl: Option<String>,
}

#[cfg(target_os = "macos")]
#[derive(Default, Debug, Clone)]
pub struct RversionDir {
    pub version: String,
    pub arch: String,
    pub installdir: String,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct InstalledVersion {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(
        default,
        serialize_with = "serialize_option_string_with_forward_slashes"
    )]
    pub path: Option<String>,
    #[serde(
        default,
        serialize_with = "serialize_option_string_with_forward_slashes"
    )]
    pub binary: Option<String>,
    #[serde(default)]
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
#[derive(PartialEq, Clone, Debug, Serialize, Deserialize)]
pub struct OsVersion {
    pub rig_platform: Option<String>,
    pub arch: String,
    pub vendor: String,
    pub os: String,
    #[serde(rename = "distribution")]
    pub distro: String,
    pub version: String,
}

fn serialize_path_with_forward_slashes<S>(
    path: &std::path::PathBuf,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let path_str = path.display().to_string().replace("\\", "/");
    serializer.serialize_str(&path_str)
}

fn serialize_option_string_with_forward_slashes<S>(
    path: &Option<String>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match path {
        None => serializer.serialize_none(),
        Some(p) => {
            let path_str = p.replace("\\", "/");
            serializer.serialize_some(&path_str)
        }
    }
}

#[derive(Serialize, Deserialize, PartialEq, Clone, Debug)]
pub struct PkgLibrary {
    pub rversion: String,
    pub name: String,
    #[serde(serialize_with = "serialize_path_with_forward_slashes")]
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
    pub url: String,
}
