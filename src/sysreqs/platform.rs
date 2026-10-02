//! Which system of the r-system-requirements rules a Linux platform is, and
//! how to query and install its OS packages.

use crate::repos::binaries::suse_version_with_dot;
use crate::rversion::OsVersion;

/// The package manager of a system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Apt,
    Dnf,
    Yum,
    Zypper,
    Apk,
}

/// A Linux system, as the rules name it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SysreqsSystem {
    /// e.g. `ubuntu`, `redhat`, `opensuse`.
    pub distribution: String,
    /// e.g. `22.04`, `9`, `15.6`.
    pub version: String,
    pub family: Family,
}

/// The major version, `9` for `9.4`.
fn major(version: &str) -> &str {
    version.split('.').next().unwrap_or(version)
}

/// The first two components, `3.20` for `3.20.3`.
fn major_minor(version: &str) -> String {
    version.split('.').take(2).collect::<Vec<_>>().join(".")
}

impl SysreqsSystem {
    /// The rules' system for an `/etc/os-release` `ID` and `VERSION_ID`, the
    /// way `detect_platform()` reports them, or `None` if the rules do not
    /// know the distribution.
    pub fn new(id: &str, version: &str) -> Option<SysreqsSystem> {
        let (distribution, version) = match id {
            "ubuntu" | "pop" => ("ubuntu", version.to_string()),
            "debian" if version.is_empty() => ("debian", "unstable".to_string()),
            "debian" => ("debian", major(version).to_string()),
            "centos" => ("centos", major(version).to_string()),
            "rhel" | "redhat" => ("redhat", major(version).to_string()),
            // The `redhat` entries use `subscription-manager`, which only RHEL
            // has. AlmaLinux is a rebuild of RHEL, like Rocky Linux.
            "rocky" | "rockylinux" | "almalinux" | "alma" => {
                ("rockylinux", major(version).to_string())
            }
            "fedora" => ("fedora", version.to_string()),
            "opensuse" | "opensuse-leap" => ("opensuse", suse_version_with_dot(version)),
            "sles" | "sle" => ("sle", suse_version_with_dot(version)),
            "alpine" => ("alpine", major_minor(version)),
            _ => return None,
        };
        let family = match distribution {
            "ubuntu" | "debian" => Family::Apt,
            "fedora" => Family::Dnf,
            "centos" | "redhat" | "rockylinux" => match major(&version).parse::<u32>() {
                Ok(v) if v < 8 => Family::Yum,
                _ => Family::Dnf,
            },
            "opensuse" | "sle" => Family::Zypper,
            _ => Family::Apk,
        };
        Some(SysreqsSystem {
            distribution: distribution.to_string(),
            version,
            family,
        })
    }

    /// The system of a platform, from `detect_platform()` or
    /// `parse_platform_string()`. `None` for macOS, Windows, a platform
    /// without a distribution version, e.g. a P3M codename like `jammy`, and
    /// a distribution the rules do not know.
    pub fn from_os_version(os: &OsVersion) -> Option<SysreqsSystem> {
        if !os.os.starts_with("linux") {
            return None;
        }
        let distro = os.distro.as_deref()?;
        let version = os.version.as_deref()?;
        SysreqsSystem::new(distro, version)
    }

    /// e.g. `Ubuntu 22.04`.
    pub fn display(&self) -> String {
        format!("{} {}", self.distribution, self.version)
    }

    /// The command that installs `packages`.
    pub fn install_command(&self, packages: &[String]) -> Vec<String> {
        let mut cmd: Vec<String> = match self.family {
            Family::Apt => vec!["apt-get", "install", "-y"],
            Family::Dnf => vec!["dnf", "install", "-y"],
            Family::Yum => vec!["yum", "install", "-y"],
            Family::Zypper => vec!["zypper", "--non-interactive", "install"],
            Family::Apk => vec!["apk", "add", "--no-cache"],
        }
        .into_iter()
        .map(|s| s.to_string())
        .collect();
        cmd.extend(packages.iter().cloned());
        cmd
    }

    /// The command that refreshes the package index before an install, if
    /// the package manager needs one. dnf, yum and zypper refresh their
    /// metadata on their own.
    pub fn update_command(&self) -> Option<Vec<String>> {
        match self.family {
            Family::Apt => Some(vec!["apt-get".to_string(), "update".to_string()]),
            Family::Apk => Some(vec!["apk".to_string(), "update".to_string()]),
            Family::Dnf | Family::Yum | Family::Zypper => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::parse_platform_string;

    fn sys(id: &str, version: &str) -> Option<(String, String, Family)> {
        SysreqsSystem::new(id, version).map(|s| (s.distribution, s.version, s.family))
    }

    #[test]
    fn os_release_ids_map_to_rules_systems() {
        let s = |d: &str, v: &str, f| Some((d.to_string(), v.to_string(), f));
        assert_eq!(sys("ubuntu", "24.04"), s("ubuntu", "24.04", Family::Apt));
        assert_eq!(sys("debian", "12"), s("debian", "12", Family::Apt));
        assert_eq!(sys("debian", ""), s("debian", "unstable", Family::Apt));
        assert_eq!(sys("rhel", "9.4"), s("redhat", "9", Family::Dnf));
        assert_eq!(sys("almalinux", "8.10"), s("rockylinux", "8", Family::Dnf));
        assert_eq!(sys("centos", "7"), s("centos", "7", Family::Yum));
        assert_eq!(sys("rocky", "10.0"), s("rockylinux", "10", Family::Dnf));
        assert_eq!(sys("fedora", "42"), s("fedora", "42", Family::Dnf));
        // `detect_platform()` reports openSUSE 15.6 as `156`.
        assert_eq!(
            sys("opensuse", "156"),
            s("opensuse", "15.6", Family::Zypper)
        );
        assert_eq!(sys("sles", "15.6"), s("sle", "15.6", Family::Zypper));
        assert_eq!(sys("alpine", "3.22.1"), s("alpine", "3.22", Family::Apk));
        assert_eq!(sys("amzn", "2023"), None);
    }

    #[test]
    fn platform_strings_map_to_rules_systems() {
        let os = parse_platform_string("ubuntu-22.04").unwrap();
        let s = SysreqsSystem::from_os_version(&os).unwrap();
        assert_eq!(
            (s.distribution.as_str(), s.version.as_str()),
            ("ubuntu", "22.04")
        );

        let os = parse_platform_string("linux-rhel-9").unwrap();
        let s = SysreqsSystem::from_os_version(&os).unwrap();
        assert_eq!(
            (s.distribution.as_str(), s.version.as_str()),
            ("redhat", "9")
        );

        // A P3M codename has no version to match the rules with.
        let os = parse_platform_string("jammy-x86_64").unwrap();
        assert_eq!(SysreqsSystem::from_os_version(&os), None);

        let os = parse_platform_string("macos-arm64").unwrap();
        assert_eq!(SysreqsSystem::from_os_version(&os), None);
    }

    #[test]
    fn commands() {
        let s = SysreqsSystem::new("ubuntu", "22.04").unwrap();
        assert_eq!(
            s.install_command(&["libcurl4-openssl-dev".to_string()]),
            vec!["apt-get", "install", "-y", "libcurl4-openssl-dev"]
        );
        assert_eq!(
            s.update_command(),
            Some(vec!["apt-get".into(), "update".into()])
        );
        let s = SysreqsSystem::new("fedora", "42").unwrap();
        assert_eq!(s.update_command(), None);
    }
}
