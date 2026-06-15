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

#[derive(PartialEq, Clone, Debug, Serialize, Deserialize)]
pub struct OsVersion {
    pub rig_platform: Option<String>,
    pub arch: String,
    pub vendor: String,
    pub os: String,
    #[serde(rename = "distribution")]
    pub distro: Option<String>,
    pub version: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ok_ver(version: &str) -> OKInstalledVersion {
        OKInstalledVersion {
            name: version.to_string(),
            version: semver::Version::parse(version).unwrap(),
            path: "/usr/local".to_string(),
            binary: "/usr/bin/R".to_string(),
        }
    }

    #[test]
    fn ok_installed_version_ordering() {
        assert!(ok_ver("4.0.0") < ok_ver("4.1.0"));
        assert!(ok_ver("4.1.0") < ok_ver("4.2.0"));
        assert!(ok_ver("4.2.0") > ok_ver("4.0.0"));
        assert!(ok_ver("4.1.0") == ok_ver("4.1.0"));
    }

    #[test]
    fn ok_installed_version_sort() {
        let mut versions = vec![ok_ver("4.2.0"), ok_ver("4.0.0"), ok_ver("4.1.0")];
        versions.sort();
        assert_eq!(
            versions[0].version,
            semver::Version::parse("4.0.0").unwrap()
        );
        assert_eq!(
            versions[1].version,
            semver::Version::parse("4.1.0").unwrap()
        );
        assert_eq!(
            versions[2].version,
            semver::Version::parse("4.2.0").unwrap()
        );
    }

    #[test]
    fn pkglibrary_posix_path_unchanged() {
        let lib = PkgLibrary {
            rversion: "4.4".to_string(),
            name: "default".to_string(),
            path: PathBuf::from("/usr/local/lib/R"),
            default: true,
        };
        let json = serde_json::to_string(&lib).unwrap();
        assert!(json.contains("/usr/local/lib/R"));
    }

    #[test]
    fn pkglibrary_backslashes_converted_to_forward_slashes() {
        // On Unix, PathBuf treats backslashes as regular characters; the
        // serializer must still convert them to forward slashes.
        let lib = PkgLibrary {
            rversion: "4.4".to_string(),
            name: "default".to_string(),
            path: PathBuf::from("C:\\Program Files\\R"),
            default: true,
        };
        let json = serde_json::to_string(&lib).unwrap();
        assert!(json.contains("C:/Program Files/R"));
        assert!(!json.contains('\\'));
    }

    #[test]
    fn installed_version_path_backslashes_converted() {
        let iv = InstalledVersion {
            name: "4.4".to_string(),
            version: None,
            path: Some("C:\\Program Files\\R\\R-4.4".to_string()),
            binary: None,
            aliases: vec![],
        };
        let json = serde_json::to_string(&iv).unwrap();
        assert!(json.contains("C:/Program Files/R/R-4.4"));
        assert!(!json.contains('\\'));
    }

    #[test]
    fn installed_version_path_none_serializes_as_null() {
        let iv = InstalledVersion {
            name: "4.4".to_string(),
            version: None,
            path: None,
            binary: None,
            aliases: vec![],
        };
        let json = serde_json::to_string(&iv).unwrap();
        assert!(json.contains("\"path\":null"));
    }
}
