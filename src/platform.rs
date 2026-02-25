use regex::Regex;
use std::error::Error;
use std::path::Path;
use std::sync::OnceLock;

use clap::ArgMatches;

use crate::rversion::*;
use crate::utils::*;

pub fn detect_linux_impl(rig_platform: Option<String>) -> Result<OsVersion, Box<dyn Error>> {
    let release_file = Path::new("/etc/os-release");
    let lines = read_lines(release_file)?;

    let mut id;
    let mut ver;

    if rig_platform.is_some() {
        let mut rig_platform2 = rig_platform.clone().unwrap();
        if rig_platform2.starts_with("linux-") {
            rig_platform2 = rig_platform2.strip_prefix("linux-").unwrap().to_string();
        }

        (id, ver) = match rig_platform2.split_once("-") {
            Some((x, y)) => (x.to_string(), y.to_string()),
            None => (rig_platform2, "".to_string()),
        };
    } else {
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
    }

    let arch = std::env::consts::ARCH.to_string();
    let vendor = "unknown".to_string();
    let os = "linux".to_string();
    let distro = id.to_owned();
    let version = ver.to_owned();

    Ok(OsVersion {
        rig_platform,
        arch,
        vendor,
        os,
        distro,
        version,
    })
}

// Cache for detect_linux() when RIG_PLATFORM is not set
static LINUX_DETECTION_CACHE: OnceLock<OsVersion> = OnceLock::new();

pub fn detect_linux() -> Result<OsVersion, Box<dyn Error>> {
    // Check if RIG_PLATFORM is set
    let rig_platform = std::env::var("RIG_PLATFORM").ok();

    // If RIG_PLATFORM is set, always compute fresh (don't use cache)
    if rig_platform.is_some() {
        return detect_linux_impl(rig_platform);
    }

    // If RIG_PLATFORM is not set, use cache
    match LINUX_DETECTION_CACHE.get() {
        Some(cached) => Ok(cached.clone()),
        None => {
            let result = detect_linux_impl(None)?;
            // Try to cache it (this might fail if another thread cached it first, which is fine)
            let _ = LINUX_DETECTION_CACHE.set(result.clone());
            Ok(result)
        }
    }
}

pub fn sc_system_detect_platform(
    args: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let linux = detect_linux()?;

    if args.get_flag("json") || mainargs.get_flag("json") {
        println!("{}", serde_json::to_string_pretty(&linux)?);
    } else {
        println!("Detected platform:");
        println!("Architecture: {}", linux.arch);
        println!("OS:           {}", linux.os);
        println!("Distribution: {}", linux.distro);
        println!("Version:      {}", linux.version);
    }
    Ok(())
}
