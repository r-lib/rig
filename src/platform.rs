use std::error::Error;
#[cfg(target_os = "macos")]
use std::ffi::CStr;
use std::sync::OnceLock;

#[cfg(target_os = "linux")]
use std::path::Path;

#[cfg(target_os = "linux")]
use regex::Regex;

use clap::ArgMatches;

#[cfg(target_os = "linux")]
use crate::utils::{grep_lines, read_lines, unquote};

use crate::rversion::*;

/// P3M/rig's arch spelling, normalized to the `std::env::consts::ARCH` one
/// used elsewhere in the codebase, or `None` if `arch` isn't a recognized
/// arch token at all (e.g. it's actually a distro version like `22.04`).
pub(crate) fn normalize_arch(arch: &str) -> Option<&'static str> {
    match arch {
        "x86_64" | "amd64" => Some("x86_64"),
        "aarch64" | "arm64" => Some("aarch64"),
        _ => None,
    }
}

/// The OS family of the machine rig runs on: `macos`, `windows` or `linux`.
fn host_os_family() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    }
}

/// The arch a platform string without one means: this machine's, if it runs
/// the same OS, otherwise the usual one for that OS, `aarch64` for macOS and
/// `x86_64` for Windows and Linux. So `macos` means the same thing on every
/// machine that is not a Mac.
fn default_arch(family: &str) -> &'static str {
    if family == host_os_family() {
        normalize_arch(std::env::consts::ARCH).unwrap_or(std::env::consts::ARCH)
    } else if family == "macos" {
        "aarch64"
    } else {
        "x86_64"
    }
}

fn macos_platform(arch: &str) -> OsVersion {
    OsVersion {
        rig_platform: None,
        arch: arch.to_string(),
        vendor: "apple".to_string(),
        os: "darwin".to_string(),
        distro: None,
        version: None,
    }
}

fn windows_platform(arch: &str) -> OsVersion {
    OsVersion {
        rig_platform: None,
        arch: arch.to_string(),
        vendor: "w64".to_string(),
        os: "mingw32".to_string(),
        distro: None,
        version: None,
    }
}

fn linux_platform(arch: &str, distro: Option<&str>, version: Option<&str>) -> OsVersion {
    OsVersion {
        rig_platform: None,
        arch: arch.to_string(),
        vendor: "unknown".to_string(),
        os: "linux-gnu".to_string(),
        distro: distro.map(|d| d.to_string()),
        version: version.map(|v| v.to_string()),
    }
}

/// Parse a platform string into its parts.
///
/// The canonical spelling is a target triple, with a distro and its version
/// for a specific Linux distribution, and `aarch64` / `x86_64` as the arch:
///
/// - `aarch64-apple-darwin`, `x86_64-apple-darwin`
/// - `x86_64-w64-mingw32`
/// - `x86_64-unknown-linux-gnu`: any glibc Linux, P3M's generic build
/// - `x86_64-unknown-linux-gnu-ubuntu-24.04`
///
/// [`platform_string`] writes this spelling. These are accepted too:
///
/// - `macos`, `windows`, `linux`: the arch is this machine's, if it runs that
///   OS, otherwise `aarch64` for macOS and `x86_64` for the others.
/// - `<os>-<arch>`, e.g. `macos-arm64`, `windows-x86_64`, `linux-aarch64`.
/// - `ubuntu-24.04`, `linux-ubuntu-24.04`, `ubuntu-24.04-arm64`.
/// - P3M target names, e.g. `jammy-x86_64`, `manylinux_2_28-arm64`. These
///   have a distro (the P3M name) but no version; see [`normalize_platform`].
/// - Triples with a darwin version (`aarch64-apple-darwin23`), without the
///   libc (`aarch64-unknown-linux-ubuntu-22.04`), with `arm64` or `amd64`.
///
/// `source` is not a platform, callers check for it first.
pub fn parse_platform_string(platform: &str) -> Result<OsVersion, Box<dyn Error>> {
    let mut parsed = parse_platform_parts(platform)?;
    if let Some(arch) = normalize_arch(&parsed.arch) {
        parsed.arch = arch.to_string();
    }
    parsed.rig_platform = Some(platform_string(&parsed));
    Ok(parsed)
}

fn parse_platform_parts(platform: &str) -> Result<OsVersion, Box<dyn Error>> {
    match platform {
        "macos" => return Ok(macos_platform(default_arch("macos"))),
        "windows" => return Ok(windows_platform(default_arch("windows"))),
        "linux" => return Ok(linux_platform(default_arch("linux"), None, None)),
        _ => {}
    }

    let pieces = platform.split('-').collect::<Vec<_>>();
    if pieces.iter().any(|p| p.is_empty()) {
        bail!("Invalid platform string: '{}'", platform);
    }

    // A target triple starts with the arch.
    if pieces.len() >= 3 && normalize_arch(pieces[0]).is_some() {
        return parse_triple(platform, &pieces);
    }

    // `<os>-<arch>`, `<distro>-<version>-<arch>`, `<p3m-name>-<arch>`
    let (rest, arch) = match pieces.split_last() {
        Some((last, rest)) if !rest.is_empty() && normalize_arch(last).is_some() => {
            (rest, Some(normalize_arch(last).unwrap()))
        }
        _ => (&pieces[..], None),
    };
    let rest = match rest {
        ["linux", rest @ ..] if !rest.is_empty() => rest,
        _ => rest,
    };
    let linux_arch = || arch.unwrap_or_else(|| default_arch("linux"));
    match rest {
        ["macos"] => Ok(macos_platform(
            arch.unwrap_or_else(|| default_arch("macos")),
        )),
        ["windows"] => Ok(windows_platform(
            arch.unwrap_or_else(|| default_arch("windows")),
        )),
        ["linux"] => Ok(linux_platform(linux_arch(), None, None)),
        // The generic build, under its P3M name.
        [name] if name.starts_with("manylinux") && arch.is_some() => {
            Ok(linux_platform(linux_arch(), None, None))
        }
        // A P3M target name, e.g. `jammy`.
        [name] if arch.is_some() => Ok(linux_platform(linux_arch(), Some(name), None)),
        [distro, version] => Ok(linux_platform(linux_arch(), Some(distro), Some(version))),
        _ => bail!("Invalid platform string: '{}'", platform),
    }
}

fn parse_triple(platform: &str, pieces: &[&str]) -> Result<OsVersion, Box<dyn Error>> {
    let mut distro = None;
    let mut version = None;
    let (arch, vendor, os);
    match pieces.len() {
        3 => {
            arch = pieces[0];
            vendor = pieces[1];
            os = pieces[2].to_string();
        }
        4 => {
            arch = pieces[0];
            vendor = pieces[1];
            os = pieces[2].to_string() + "-" + pieces[3];
        }
        5 => {
            arch = pieces[0];
            vendor = pieces[1];
            os = pieces[2].to_string();
            distro = Some(pieces[3].to_string());
            version = Some(pieces[4].to_string());
        }
        6 => {
            arch = pieces[0];
            vendor = pieces[1];
            os = pieces[2].to_string() + "-" + pieces[3];
            distro = Some(pieces[4].to_string());
            version = Some(pieces[5].to_string());
        }
        _ => {
            bail!("Invalid platform string format: '{}'", platform);
        }
    }

    Ok(OsVersion {
        rig_platform: None,
        arch: arch.to_string(),
        vendor: vendor.to_string(),
        distro,
        os,
        version,
    })
}

/// The canonical spelling of a platform, see [`parse_platform_string`].
///
/// A Linux platform with a P3M target name and no distro version, e.g. from
/// `jammy-x86_64`, has no canonical spelling without P3M's status document.
/// It is written in that same `<name>-<arch>` form, which parses back to the
/// same platform. [`normalize_platform`] resolves these.
pub fn platform_string(platform: &OsVersion) -> String {
    let arch = normalize_arch(&platform.arch).unwrap_or(&platform.arch);
    if platform.os.starts_with("darwin") {
        format!("{}-apple-darwin", arch)
    } else if platform.os == "mingw32" || platform.os.ends_with("-mingw32") {
        format!("{}-w64-mingw32", arch)
    } else if platform.os.starts_with("linux") {
        let musl = platform.os == "linux-musl" || platform.distro.as_deref() == Some("alpine");
        let libc = if musl { "musl" } else { "gnu" };
        match (&platform.distro, &platform.version) {
            (Some(distro), Some(version)) => {
                format!("{}-unknown-linux-{}-{}-{}", arch, libc, distro, version)
            }
            (Some(name), None) => format!("{}-{}", name, arch),
            _ => format!("{}-unknown-linux-{}", arch, libc),
        }
    } else {
        format!("{}-{}-{}", arch, platform.vendor, platform.os)
    }
}

/// Bring a platform string into its canonical spelling, see
/// [`parse_platform_string`]. `source` stays `source`.
///
/// P3M target names (`jammy-x86_64`) are looked up in P3M's status document,
/// and fail if it cannot be loaded.
pub fn normalize_platform(platform: &str) -> Result<String, Box<dyn Error>> {
    if platform == "source" {
        return Ok(platform.to_string());
    }
    let parsed = parse_platform_string(platform)?;
    if parsed.os.starts_with("linux") && parsed.distro.is_some() && parsed.version.is_none() {
        let status = crate::repos::binaries::PpmStatus::load(None)?;
        match status.ppm_target(&parsed) {
            Some(target) => return Ok(target.triple),
            None => bail!("Unknown P3M platform: '{}'", platform),
        }
    }
    Ok(platform_string(&parsed))
}

#[cfg(target_os = "linux")]
pub fn detect_platform_impl() -> Result<OsVersion, Box<dyn Error>> {
    let release_file = Path::new("/etc/os-release");
    let lines = read_lines(release_file)?;

    let mut id;
    let mut ver;

    let re_id = Regex::new("^ID=")?;
    let wid_line = grep_lines(&re_id, &lines);
    id = if wid_line.is_empty() {
        "".to_string()
    } else {
        let id_line = &lines[wid_line[0]];
        let id = re_id.replace(id_line, "").to_string();
        unquote(&id)
    };

    let re_ver = Regex::new("^VERSION_ID=")?;
    let wver_line = grep_lines(&re_ver, &lines);
    ver = if wver_line.is_empty() {
        "".to_string()
    } else {
        let ver_line = &lines[wver_line[0]];
        let ver = re_ver.replace(ver_line, "").to_string();
        unquote(&ver)
    };

    // workaround for a node-rversions bug
    if id == "opensuse-leap" {
        id = "opensuse".to_string()
    }
    if id == "opensuse" {
        ver = ver.replace(".", "");
    }

    let arch = std::env::consts::ARCH.to_string();
    let vendor = "unknown".to_string();
    let os = "linux".to_string();
    let distro = Some(id.to_owned());
    let version = Some(ver.to_owned());

    Ok(OsVersion {
        rig_platform: None,
        arch,
        vendor,
        os,
        distro,
        version,
    })
}

#[cfg(target_os = "macos")]
pub fn detect_platform_impl() -> Result<OsVersion, Box<dyn Error>> {
    Ok(OsVersion {
        rig_platform: None,
        arch: std::env::consts::ARCH.to_string(),
        vendor: "apple".to_string(),
        os: "darwin".to_string() + &get_darwin_version()?,
        distro: None,
        version: None,
    })
}

#[cfg(target_os = "windows")]
pub fn detect_platform_impl() -> Result<OsVersion, Box<dyn Error>> {
    Ok(OsVersion {
        rig_platform: None,
        arch: std::env::consts::ARCH.to_string(),
        vendor: "w64".to_string(),
        os: "mingw32".to_string(),
        distro: None,
        version: None,
    })
}

// Cache for detect_platform() when RIG_PLATFORM is not set
static PLATFORM_DETECTION_CACHE: OnceLock<OsVersion> = OnceLock::new();

pub fn detect_platform() -> Result<OsVersion, Box<dyn Error>> {
    if let Ok(rp) = std::env::var("RIG_PLATFORM") {
        return parse_platform_string(&rp);
    }
    match PLATFORM_DETECTION_CACHE.get() {
        Some(cached) => Ok(cached.clone()),
        None => {
            let result = detect_platform_impl()?;
            let _ = PLATFORM_DETECTION_CACHE.set(result.clone());
            Ok(result)
        }
    }
}

#[cfg(target_os = "macos")]
pub fn get_darwin_version() -> Result<String, Box<dyn Error>> {
    unsafe {
        let mut utsname: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut utsname) == 0 {
            let version = CStr::from_ptr(utsname.release.as_ptr())
                .to_str()
                .map_err(|e| format!("Failed to parse uname release: {}", e))?
                .to_string();
            Ok(version)
        } else {
            Err("Failed to get Darwin version via uname".into())
        }
    }
}

pub fn sc_system_detect_platform(
    args: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    // Check if RIG_PLATFORM is set, if not detect current platform
    let platform = match std::env::var("RIG_PLATFORM") {
        Ok(rp) => parse_platform_string(&rp)?,
        Err(_) => detect_platform()?,
    };

    // On Linux we also report the C library (glibc or musl) and its version.
    // This is a property of the running system, so it is only meaningful when
    // detecting the current platform, not when parsing a RIG_PLATFORM string.
    #[cfg(target_os = "linux")]
    let libc = crate::linux::detect_libc().ok();

    if args.get_flag("json") || mainargs.get_flag("json") {
        #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
        let mut value = serde_json::to_value(&platform)?;
        if let Some(obj) = value.as_object_mut() {
            obj.insert(
                "platform".to_string(),
                serde_json::json!(platform_string(&platform)),
            );
        }
        #[cfg(target_os = "linux")]
        if let (Some(libc), Some(obj)) = (&libc, value.as_object_mut()) {
            obj.insert("libc".to_string(), serde_json::json!(libc.kind.to_string()));
            obj.insert("libc_version".to_string(), serde_json::json!(libc.version));
        }
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("Detected platform:");
        println!("Platform:     {}", platform_string(&platform));
        println!("Vendor:       {}", platform.vendor);
        println!("Architecture: {}", platform.arch);
        println!("OS:           {}", platform.os);
        println!(
            "Distribution: {}",
            platform.distro.as_deref().unwrap_or("N/A")
        );
        println!(
            "Version:      {}",
            platform.version.as_deref().unwrap_or("N/A")
        );
        #[cfg(target_os = "linux")]
        {
            let (libc_type, libc_version) = match &libc {
                Some(libc) => (libc.kind.to_string(), libc.version.clone()),
                None => ("N/A".to_string(), "N/A".to_string()),
            };
            println!("Libc:         {}", libc_type);
            println!("Libc version: {}", libc_version);
        }
    }
    Ok(())
}

pub fn platform_to_pkg_type(platform: &OsVersion, r_version: &str) -> Option<String> {
    let r_version = semver::Version::parse(r_version).ok()?;
    if platform.os == "mingw32" && platform.arch == "x86_64" {
        Some("win.binary".to_string())
    } else if platform.os.starts_with("darwin") {
        if r_version < semver::Version::parse("3.1.0").unwrap() {
            None
        } else if r_version < semver::Version::parse("3.4.0").unwrap() {
            Some("mac.binary.mavericks".to_string())
        } else if r_version < semver::Version::parse("4.0.0").unwrap() {
            Some("mac.binary.el-capitan".to_string())
        } else if r_version < semver::Version::parse("4.1.0").unwrap() {
            Some("mac.binary".to_string())
        } else if r_version < semver::Version::parse("4.3.0").unwrap() {
            if platform.arch == "aarch64" {
                Some("mac.binary.big-sur-arm64".to_string())
            } else {
                Some("mac.binary".to_string())
            }
        } else if r_version < semver::Version::parse("4.6.0").unwrap() {
            if platform.arch == "aarch64" {
                Some("mac.binary.big-sur-arm64".to_string())
            } else {
                Some("mac.binary.big-sur-x86_64".to_string())
            }
        } else if r_version < semver::Version::parse("4.7.0").unwrap() {
            if platform.arch == "aarch64" {
                Some("mac.binary.sonoma-arm64".to_string())
            } else {
                Some("mac.binary.big-sur-x86_64".to_string())
            }
        } else if platform.arch == "aarch64" {
            Some("macos.binary.arm64".to_string())
        } else {
            Some("mac.binary.big-sur-x86_64".to_string())
        }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static RIG_PLATFORM_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_detect_platform_rig_platform_short() {
        let _guard = RIG_PLATFORM_LOCK.lock().unwrap();
        // "ubuntu-24.04" should be treated as a linux platform shorthand
        std::env::set_var("RIG_PLATFORM", "ubuntu-24.04");
        let result = detect_platform().unwrap();
        std::env::remove_var("RIG_PLATFORM");
        assert_eq!(result.distro, Some("ubuntu".to_string()));
        assert_eq!(result.version, Some("24.04".to_string()));
        assert_eq!(result.os, "linux-gnu");
        assert_eq!(result.vendor, "unknown");
    }

    #[test]
    fn test_detect_platform_rig_platform_prefixed() {
        let _guard = RIG_PLATFORM_LOCK.lock().unwrap();
        // "linux-ubuntu-22.04" explicit form
        std::env::set_var("RIG_PLATFORM", "linux-ubuntu-22.04");
        let result = detect_platform().unwrap();
        std::env::remove_var("RIG_PLATFORM");
        assert_eq!(result.distro, Some("ubuntu".to_string()));
        assert_eq!(result.version, Some("22.04".to_string()));
        assert_eq!(result.os, "linux-gnu");
    }

    #[test]
    fn test_parse_platform_string_three_parts() {
        let result = parse_platform_string("aarch64-apple-darwin").unwrap();
        assert_eq!(result.arch, "aarch64");
        assert_eq!(result.vendor, "apple");
        assert_eq!(result.os, "darwin");
        assert_eq!(result.distro, None);
        assert_eq!(result.version, None);
    }

    #[test]
    fn test_parse_platform_string_four_parts() {
        let result = parse_platform_string("x86_64-w64-mingw32").unwrap();
        assert_eq!(result.arch, "x86_64");
        assert_eq!(result.vendor, "w64");
        assert_eq!(result.os, "mingw32");
        assert_eq!(result.distro, None);
        assert_eq!(result.version, None);
    }

    #[test]
    fn test_parse_platform_string_five_parts() {
        let result = parse_platform_string("aarch64-unknown-linux-ubuntu-22.04").unwrap();
        assert_eq!(result.arch, "aarch64");
        assert_eq!(result.vendor, "unknown");
        assert_eq!(result.os, "linux");
        assert_eq!(result.distro, Some("ubuntu".to_string()));
        assert_eq!(result.version, Some("22.04".to_string()));
    }

    #[test]
    fn test_parse_platform_string_six_parts() {
        let result = parse_platform_string("aarch64-unknown-linux-gnu-ubuntu-22.04").unwrap();
        assert_eq!(result.arch, "aarch64");
        assert_eq!(result.vendor, "unknown");
        assert_eq!(result.os, "linux-gnu");
        assert_eq!(result.distro, Some("ubuntu".to_string()));
        assert_eq!(result.version, Some("22.04".to_string()));
    }

    #[test]
    fn test_parse_platform_string_linux_prefix() {
        // "linux-ubuntu-22.04" should expand to current arch
        let result = parse_platform_string("linux-ubuntu-22.04").unwrap();
        assert_eq!(result.arch, default_arch("linux"));
        assert_eq!(result.vendor, "unknown");
        assert_eq!(result.os, "linux-gnu");
        assert_eq!(result.distro, Some("ubuntu".to_string()));
        assert_eq!(result.version, Some("22.04".to_string()));
    }

    #[test]
    fn test_parse_platform_string_short_linux() {
        // "ubuntu-22.04" (one dash) should expand to current arch
        let result = parse_platform_string("ubuntu-22.04").unwrap();
        assert_eq!(result.arch, default_arch("linux"));
        assert_eq!(result.vendor, "unknown");
        assert_eq!(result.os, "linux-gnu");
        assert_eq!(result.distro, Some("ubuntu".to_string()));
        assert_eq!(result.version, Some("22.04".to_string()));
    }

    #[test]
    fn test_parse_platform_string_macos_arm64() {
        // Lockfile spelling `BinaryTarget::name()` writes, e.g. "macos-arm64".
        let result = parse_platform_string("macos-arm64").unwrap();
        assert_eq!(result.arch, "aarch64");
        assert_eq!(result.vendor, "apple");
        assert!(result.os.starts_with("darwin"));
    }

    #[test]
    fn test_parse_platform_string_windows_x86_64() {
        let result = parse_platform_string("windows-x86_64").unwrap();
        assert_eq!(result.arch, "x86_64");
        assert_eq!(result.vendor, "w64");
        assert_eq!(result.os, "mingw32");
    }

    #[test]
    fn test_parse_platform_string_manylinux() {
        // P3M's generic build is the generic glibc Linux platform.
        let result = parse_platform_string("manylinux_2_28-arm64").unwrap();
        assert_eq!(result.arch, "aarch64");
        assert_eq!(result.vendor, "unknown");
        assert_eq!(result.os, "linux-gnu");
        assert_eq!(result.distro, None);
        assert_eq!(result.version, None);
    }

    #[test]
    fn test_parse_platform_string_windows() {
        let result = parse_platform_string("windows").unwrap();
        let arch = if cfg!(target_os = "windows") {
            std::env::consts::ARCH
        } else {
            "x86_64"
        };
        assert_eq!(result.arch, arch);
        assert_eq!(result.vendor, "w64");
        assert_eq!(result.os, "mingw32");
    }

    #[test]
    fn test_parse_platform_string_macos() {
        // Works on any machine, with this machine's arch on macOS.
        let result = parse_platform_string("macos").unwrap();
        let arch = if cfg!(target_os = "macos") {
            std::env::consts::ARCH
        } else {
            "aarch64"
        };
        assert_eq!(result.arch, arch);
        assert_eq!(result.vendor, "apple");
        assert_eq!(result.os, "darwin");
        assert_eq!(result.distro, None);
        assert_eq!(result.version, None);
    }

    /// Every spelling with an arch normalizes to its canonical string, which
    /// is the same on every machine.
    #[test]
    fn test_platform_string_aliases() {
        let cases = [
            ("aarch64-apple-darwin", "aarch64-apple-darwin"),
            ("aarch64-apple-darwin23", "aarch64-apple-darwin"),
            ("arm64-apple-darwin", "aarch64-apple-darwin"),
            ("macos-arm64", "aarch64-apple-darwin"),
            ("macos-x86_64", "x86_64-apple-darwin"),
            ("x86_64-w64-mingw32", "x86_64-w64-mingw32"),
            ("windows-x86_64", "x86_64-w64-mingw32"),
            ("windows-amd64", "x86_64-w64-mingw32"),
            ("aarch64-w64-mingw32", "aarch64-w64-mingw32"),
            ("x86_64-unknown-linux-gnu", "x86_64-unknown-linux-gnu"),
            ("linux-x86_64", "x86_64-unknown-linux-gnu"),
            ("linux-arm64", "aarch64-unknown-linux-gnu"),
            ("manylinux_2_28-x86_64", "x86_64-unknown-linux-gnu"),
            (
                "x86_64-unknown-linux-gnu-ubuntu-24.04",
                "x86_64-unknown-linux-gnu-ubuntu-24.04",
            ),
            (
                "aarch64-unknown-linux-ubuntu-22.04",
                "aarch64-unknown-linux-gnu-ubuntu-22.04",
            ),
            (
                "ubuntu-24.04-arm64",
                "aarch64-unknown-linux-gnu-ubuntu-24.04",
            ),
            (
                "linux-ubuntu-24.04-x86_64",
                "x86_64-unknown-linux-gnu-ubuntu-24.04",
            ),
            ("x86_64-unknown-linux-musl", "x86_64-unknown-linux-musl"),
            (
                "x86_64-unknown-linux-musl-alpine-3.22",
                "x86_64-unknown-linux-musl-alpine-3.22",
            ),
        ];
        for (input, want) in cases {
            let parsed = parse_platform_string(input).unwrap();
            assert_eq!(platform_string(&parsed), want, "{}", input);
            assert_eq!(parsed.rig_platform.as_deref(), Some(want), "{}", input);
            assert_eq!(normalize_platform(input).unwrap(), want, "{}", input);
        }
        assert_eq!(normalize_platform("source").unwrap(), "source");
        // A P3M name needs P3M's status document to normalize, see
        // `PpmStatus::ppm_target`, so `platform_string` keeps it.
        let jammy = parse_platform_string("jammy-x86_64").unwrap();
        assert_eq!(platform_string(&jammy), "jammy-x86_64");
    }

    /// Without an arch, the arch is this machine's, if it runs that OS.
    #[test]
    fn test_platform_string_default_arch() {
        let linux = if cfg!(target_os = "linux") {
            std::env::consts::ARCH
        } else {
            "x86_64"
        };
        let canonical = |p: &str| platform_string(&parse_platform_string(p).unwrap());
        assert_eq!(
            canonical("ubuntu-24.04"),
            format!("{}-unknown-linux-gnu-ubuntu-24.04", linux)
        );
        assert_eq!(
            canonical("linux-ubuntu-24.04"),
            format!("{}-unknown-linux-gnu-ubuntu-24.04", linux)
        );
        assert_eq!(canonical("linux"), format!("{}-unknown-linux-gnu", linux));
    }

    #[test]
    fn test_parse_platform_string_invalid_too_few_parts() {
        // Note: "x86_64-apple" has 1 dash, so it gets treated as short Linux format
        // and expanded to "{arch}-unknown-linux-x86_64-apple" which is valid.
        // Test with something that truly has too few parts after expansion
        let result = parse_platform_string("x86_64");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_platform_string_invalid_too_many_parts() {
        let result = parse_platform_string("a-b-c-d-e-f-g");
        assert!(result.is_err());
    }

    // Tests for platform_to_pkg_type

    #[test]
    fn test_platform_to_pkg_type_windows_x86_64() {
        let platform = OsVersion {
            rig_platform: None,
            arch: "x86_64".to_string(),
            vendor: "w64".to_string(),
            os: "mingw32".to_string(),
            distro: None,
            version: None,
        };
        let result = platform_to_pkg_type(&platform, "4.3.0");
        assert_eq!(result, Some("win.binary".to_string()));
    }

    #[test]
    fn test_platform_to_pkg_type_windows_aarch64() {
        let platform = OsVersion {
            rig_platform: None,
            arch: "aarch64".to_string(),
            vendor: "w64".to_string(),
            os: "mingw32".to_string(),
            distro: None,
            version: None,
        };
        let result = platform_to_pkg_type(&platform, "4.3.0");
        assert_eq!(result, None);
    }

    #[test]
    fn test_platform_to_pkg_type_macos_old() {
        let platform = OsVersion {
            rig_platform: None,
            arch: "x86_64".to_string(),
            vendor: "apple".to_string(),
            os: "darwin".to_string(),
            distro: None,
            version: None,
        };
        // R < 3.1.0 should return None
        let result = platform_to_pkg_type(&platform, "3.0.3");
        assert_eq!(result, None);
    }

    #[test]
    fn test_platform_to_pkg_type_macos_mavericks() {
        let platform = OsVersion {
            rig_platform: None,
            arch: "x86_64".to_string(),
            vendor: "apple".to_string(),
            os: "darwin".to_string(),
            distro: None,
            version: None,
        };
        // R 3.1.0 - 3.3.x should use mavericks
        let result = platform_to_pkg_type(&platform, "3.2.5");
        assert_eq!(result, Some("mac.binary.mavericks".to_string()));
    }

    #[test]
    fn test_platform_to_pkg_type_macos_el_capitan() {
        let platform = OsVersion {
            rig_platform: None,
            arch: "x86_64".to_string(),
            vendor: "apple".to_string(),
            os: "darwin".to_string(),
            distro: None,
            version: None,
        };
        // R 3.4.0 - 3.x.x should use el-capitan
        let result = platform_to_pkg_type(&platform, "3.6.3");
        assert_eq!(result, Some("mac.binary.el-capitan".to_string()));
    }

    #[test]
    fn test_platform_to_pkg_type_macos_4_0() {
        let platform = OsVersion {
            rig_platform: None,
            arch: "x86_64".to_string(),
            vendor: "apple".to_string(),
            os: "darwin".to_string(),
            distro: None,
            version: None,
        };
        // R 4.0.0 - 4.0.x
        let result = platform_to_pkg_type(&platform, "4.0.5");
        assert_eq!(result, Some("mac.binary".to_string()));
    }

    #[test]
    fn test_platform_to_pkg_type_macos_4_1_x86_64() {
        let platform = OsVersion {
            rig_platform: None,
            arch: "x86_64".to_string(),
            vendor: "apple".to_string(),
            os: "darwin".to_string(),
            distro: None,
            version: None,
        };
        // R 4.1.0 - 4.2.x on x86_64
        let result = platform_to_pkg_type(&platform, "4.2.3");
        assert_eq!(result, Some("mac.binary".to_string()));
    }

    #[test]
    fn test_platform_to_pkg_type_macos_4_1_aarch64() {
        let platform = OsVersion {
            rig_platform: None,
            arch: "aarch64".to_string(),
            vendor: "apple".to_string(),
            os: "darwin".to_string(),
            distro: None,
            version: None,
        };
        // R 4.1.0 - 4.2.x on aarch64
        let result = platform_to_pkg_type(&platform, "4.2.3");
        assert_eq!(result, Some("mac.binary.big-sur-arm64".to_string()));
    }

    #[test]
    fn test_platform_to_pkg_type_macos_4_3_x86_64() {
        let platform = OsVersion {
            rig_platform: None,
            arch: "x86_64".to_string(),
            vendor: "apple".to_string(),
            os: "darwin".to_string(),
            distro: None,
            version: None,
        };
        // R 4.3.0 - 4.5.x on x86_64
        let result = platform_to_pkg_type(&platform, "4.4.1");
        assert_eq!(result, Some("mac.binary.big-sur-x86_64".to_string()));
    }

    #[test]
    fn test_platform_to_pkg_type_macos_4_3_aarch64() {
        let platform = OsVersion {
            rig_platform: None,
            arch: "aarch64".to_string(),
            vendor: "apple".to_string(),
            os: "darwin".to_string(),
            distro: None,
            version: None,
        };
        // R 4.3.0 - 4.5.x on aarch64
        let result = platform_to_pkg_type(&platform, "4.4.1");
        assert_eq!(result, Some("mac.binary.big-sur-arm64".to_string()));
    }

    #[test]
    fn test_platform_to_pkg_type_macos_4_6_x86_64() {
        let platform = OsVersion {
            rig_platform: None,
            arch: "x86_64".to_string(),
            vendor: "apple".to_string(),
            os: "darwin".to_string(),
            distro: None,
            version: None,
        };
        // R 4.6.0 - 4.6.x on x86_64
        let result = platform_to_pkg_type(&platform, "4.6.0");
        assert_eq!(result, Some("mac.binary.big-sur-x86_64".to_string()));
    }

    #[test]
    fn test_platform_to_pkg_type_macos_4_6_aarch64() {
        let platform = OsVersion {
            rig_platform: None,
            arch: "aarch64".to_string(),
            vendor: "apple".to_string(),
            os: "darwin".to_string(),
            distro: None,
            version: None,
        };
        // R 4.6.0 - 4.6.x on aarch64
        let result = platform_to_pkg_type(&platform, "4.6.0");
        assert_eq!(result, Some("mac.binary.sonoma-arm64".to_string()));
    }

    #[test]
    fn test_platform_to_pkg_type_macos_4_7_x86_64() {
        let platform = OsVersion {
            rig_platform: None,
            arch: "x86_64".to_string(),
            vendor: "apple".to_string(),
            os: "darwin".to_string(),
            distro: None,
            version: None,
        };
        // R 4.7.0+ on x86_64 stays on the old path
        let result = platform_to_pkg_type(&platform, "4.7.0");
        assert_eq!(result, Some("mac.binary.big-sur-x86_64".to_string()));
    }

    #[test]
    fn test_platform_to_pkg_type_macos_4_7_aarch64() {
        let platform = OsVersion {
            rig_platform: None,
            arch: "aarch64".to_string(),
            vendor: "apple".to_string(),
            os: "darwin".to_string(),
            distro: None,
            version: None,
        };
        // R 4.7.0+ on aarch64 uses the new "macos" (no codename) layout
        let result = platform_to_pkg_type(&platform, "4.7.0");
        assert_eq!(result, Some("macos.binary.arm64".to_string()));
    }

    #[test]
    fn test_platform_to_pkg_type_linux() {
        let platform = OsVersion {
            rig_platform: None,
            arch: "x86_64".to_string(),
            vendor: "unknown".to_string(),
            os: "linux".to_string(),
            distro: Some("ubuntu".to_string()),
            version: Some("22.04".to_string()),
        };
        // Linux should return None
        let result = platform_to_pkg_type(&platform, "4.3.0");
        assert_eq!(result, None);
    }

    #[test]
    fn test_platform_to_pkg_type_invalid_version() {
        let platform = OsVersion {
            rig_platform: None,
            arch: "x86_64".to_string(),
            vendor: "w64".to_string(),
            os: "mingw32".to_string(),
            distro: None,
            version: None,
        };
        // Invalid version string should return None
        let result = platform_to_pkg_type(&platform, "invalid");
        assert_eq!(result, None);
    }
}
