use std::error::Error;
use std::ffi::CStr;
use std::sync::OnceLock;

use clap::ArgMatches;
use simple_error::bail;

use crate::rversion::*;

pub fn parse_platform_string(platform: &str) -> Result<OsVersion, Box<dyn Error>> {
    // macos -> {aarch64,x86_64}-apple-darwin<x> depending on R version
    // linux-ubuntu-22.04
    // ubuntu-22.04
    // aarch64-unknown-linux-gnu-ubuntu-22.04
    // aarch64-unknown-linux-musl-alpine-3.22
    // windows -> {aarch64,x86_64}-w64-mingw32 (should we still care about 32-bit windows?)
    let native_arch = std::env::consts::ARCH;
    let platform = if platform == "macos" {
        #[cfg(target_os = "macos")]
        {
            let darwin_version = get_darwin_version()?;
            format!("{}-apple-darwin{}", native_arch, darwin_version)
        }
        #[cfg(not(target_os = "macos"))]
        {
            bail!("Cannot detect macOS platform on non-macOS system");
        }
    } else if platform == "windows" {
        format!("{}-w64-mingw32", native_arch)
    } else if platform.starts_with("linux-") {
        let platform = platform.strip_prefix("linux-").unwrap();
        format!("{}-unknown-linux-{}", native_arch, platform)
    } else if platform.matches('-').count() == 1 {
        format!("{}-unknown-linux-{}", native_arch, platform)
    } else {
        platform.to_string()
    };

    let pieces = platform.split('-').collect::<Vec<_>>();
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
        rig_platform: Some(platform.to_string()),
        arch: arch.to_string(),
        vendor: vendor.to_string(),
        distro,
        os,
        version,
    })
}

#[cfg(target_os = "linux")]
pub fn detect_platform_impl() -> Result<OsVersion, Box<dyn Error>> {
    let release_file = Path::new("/etc/os-release");
    let lines = read_lines(release_file)?;

    let mut id;
    let mut ver;

    let re_id = Regex::new("^ID=")?;
    let wid_line = grep_lines(&re_id, &lines);
    id = if wid_line.len() == 0 {
        "".to_string()
    } else {
        let id_line = &lines[wid_line[0]];
        let id = re_id.replace(&id_line, "").to_string();
        unquote(&id)
    };

    let re_ver = Regex::new("^VERSION_ID=")?;
    let wver_line = grep_lines(&re_ver, &lines);
    ver = if wver_line.len() == 0 {
        "".to_string()
    } else {
        let ver_line = &lines[wver_line[0]];
        let ver = re_ver.replace(&ver_line, "").to_string();
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
    // If RIG_PLATFORM is not set, use cache
    match PLATFORM_DETECTION_CACHE.get() {
        Some(cached) => Ok(cached.clone()),
        None => {
            let result = detect_platform_impl()?;
            // Try to cache it (this might fail if another thread
            // cached it first, which is fine)
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

    if args.get_flag("json") || mainargs.get_flag("json") {
        println!("{}", serde_json::to_string_pretty(&platform)?);
    } else {
        println!("Detected platform:");
        println!(
            "Rig platform: {}",
            platform.rig_platform.as_deref().unwrap_or("unset")
        );
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
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(result.arch, std::env::consts::ARCH);
        assert_eq!(result.vendor, "unknown");
        assert_eq!(result.os, "linux");
        assert_eq!(result.distro, Some("ubuntu".to_string()));
        assert_eq!(result.version, Some("22.04".to_string()));
    }

    #[test]
    fn test_parse_platform_string_short_linux() {
        // "ubuntu-22.04" (one dash) should expand to current arch
        let result = parse_platform_string("ubuntu-22.04").unwrap();
        assert_eq!(result.arch, std::env::consts::ARCH);
        assert_eq!(result.vendor, "unknown");
        assert_eq!(result.os, "linux");
        assert_eq!(result.distro, Some("ubuntu".to_string()));
        assert_eq!(result.version, Some("22.04".to_string()));
    }

    #[test]
    fn test_parse_platform_string_windows() {
        let result = parse_platform_string("windows").unwrap();
        assert_eq!(result.arch, std::env::consts::ARCH);
        assert_eq!(result.vendor, "w64");
        assert_eq!(result.os, "mingw32");
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn test_parse_platform_string_macos() {
        let result = parse_platform_string("macos").unwrap();
        assert_eq!(result.arch, std::env::consts::ARCH);
        assert_eq!(result.vendor, "apple");
        // OS includes the darwin version, e.g. "darwin25.3.0"
        assert!(result.os.starts_with("darwin"));
        assert_eq!(result.distro, None);
        assert_eq!(result.version, None);
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
}
