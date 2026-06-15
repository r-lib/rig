use std::error::Error;
use std::path::Path;

use clap::ArgMatches;
use log::{debug, info, warn};
use regex::Regex;
use tabular::*;
use winreg::enums::*;
use winreg::RegKey;

use crate::common::*;
use crate::escalate::*;
use crate::output::OUTPUT;
use crate::utils::*;
use crate::windows_arch::*;

use super::{arch_of_name, get_links_dir, get_r_root_for, r_dirname, rig_name_for_arch, version_dir_key};

fn clean_registry_r(key: &RegKey) -> Result<(), Box<dyn Error>> {
    for nm in key.enum_keys() {
        let nm = nm?;
        let subkey = key.open_subkey(&nm)?;
        let path: String = subkey.get_value("InstallPath")?;
        let path2 = Path::new(&path);
        if !path2.exists() {
            debug!("Cleaning registry: R {} (not in {})", &nm, path);
            key.delete_subkey_all(nm)?;
        }
    }
    Ok(())
}

fn clean_registry_rtools(key: &RegKey) -> Result<(), Box<dyn Error>> {
    for nm in key.enum_keys() {
        let nm = nm?;
        let subkey = key.open_subkey(&nm)?;
        let path: String = subkey.get_value("InstallPath")?;
        let path2 = Path::new(&path);
        if !path2.exists() {
            debug!("Cleaning registry: Rtools {} (not in {})", &nm, path);
            key.delete_subkey_all(nm)?;
        }
    }
    Ok(())
}

fn clean_registry_uninst(key: &RegKey) -> Result<(), Box<dyn Error>> {
    for nm in key
        .enum_keys()
        .map(|x| x.unwrap())
        .filter(|x| x.starts_with("Rtools") || x.starts_with("R for Windows"))
    {
        let subkey = key.open_subkey(&nm).unwrap();
        let path: String = subkey.get_value("InstallLocation").unwrap();
        let path2 = Path::new(&path);
        if !path2.exists() {
            debug!("Cleaning registry (uninstaller): {}", nm);
            key.delete_subkey_all(nm).unwrap();
        }
    }
    Ok(())
}

fn r_registry_hive() -> Result<RegKey, Box<dyn Error>> {
    if get_mode()? == Mode::User {
        Ok(RegKey::predef(HKEY_CURRENT_USER))
    } else {
        Ok(RegKey::predef(HKEY_LOCAL_MACHINE))
    }
}

pub fn sc_clean_registry() -> Result<(), Box<dyn Error>> {
    escalate("cleaning up the Windows registry")?;

    OUTPUT.status("Cleaning leftover registry entries");
    info!("Cleaning leftover registry entries");

    let hive = r_registry_hive()?;

    let r64r = hive.open_subkey("SOFTWARE\\R-core\\R");
    if let Ok(x) = r64r {
        clean_registry_r(&x)?;
    };
    let r64r64 = hive.open_subkey("SOFTWARE\\R-core\\R64");
    if let Ok(x) = r64r64 {
        clean_registry_r(&x)?;
    };
    let r32r = hive.open_subkey("SOFTWARE\\WOW6432Node\\R-core\\R");
    if let Ok(x) = r32r {
        clean_registry_r(&x)?;
    };
    let r32r32 = hive.open_subkey("SOFTWARE\\WOW6432Node\\R-core\\R32");
    if let Ok(x) = r32r32 {
        clean_registry_r(&x)?;
    };
    let r32r64 = hive.open_subkey("SOFTWARE\\WOW6432Node\\R-core\\R64");
    if let Ok(x) = r32r64 {
        clean_registry_r(&x)?;
    };

    // Rtools entries only exist in HKLM
    if get_mode()? == Mode::Admin {
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        let rtools64 = hklm.open_subkey("SOFTWARE\\R-core\\Rtools");
        if let Ok(x) = rtools64 {
            clean_registry_rtools(&x)?;
            if x.enum_keys().count() == 0 {
                hklm.delete_subkey("SOFTWARE\\R-core\\Rtools")?;
            }
        };
        let rtools32 = hklm.open_subkey("SOFTWARE\\WOW6432Node\\R-core\\Rtools");
        if let Ok(x) = rtools32 {
            clean_registry_rtools(&x)?;
            if x.enum_keys().count() == 0 {
                hklm.delete_subkey("SOFTWARE\\WOW6432Node\\R-core\\Rtools")?;
            }
        };

        let uninst =
            hklm.open_subkey("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall");
        if let Ok(x) = uninst {
            clean_registry_uninst(&x)?;
        };
        let uninst32 = hklm.open_subkey(
            "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
        );
        if let Ok(x) = uninst32 {
            clean_registry_uninst(&x)?;
        };
    } else {
        // User-mode installs put uninstall entries in HKCU
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let uninst =
            hkcu.open_subkey("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall");
        if let Ok(x) = uninst {
            clean_registry_uninst(&x)?;
        };
    }

    Ok(())
}

pub(super) fn maybe_update_registry_default() -> Result<(), Box<dyn Error>> {
    let links_dir = get_links_dir()?;
    let linkdir = Path::new(&links_dir);
    let linkfile = linkdir.join("R.bat");
    if linkfile.exists() {
        update_registry_default()?;
    }
    Ok(())
}

fn update_registry_default1(key: &RegKey, ver: &String) -> Result<(), Box<dyn Error>> {
    let base = version_dir_key(ver);
    let rroot = get_r_root_for(ver)?;
    key.set_value("Current Version", &base)?;
    let inst = rroot + "\\" + &r_dirname(&base)?;
    key.set_value("InstallPath", &inst)?;
    Ok(())
}

fn update_registry_default_to(default: &String) -> Result<(), Box<dyn Error>> {
    let hive = r_registry_hive()?;
    let native = get_native_arch();
    let arch = arch_of_name(default);

    if native == "aarch64" && arch == "x86_64" {
        // x86_64 R on aarch64 host: update the WOW6432Node key
        let key_path = "SOFTWARE\\WOW6432Node\\R-core\\R64";
        let r = hive.create_subkey(key_path);
        if let Ok(x) = r {
            let (key, _) = x;
            update_registry_default1(&key, default)?;
        }
    } else {
        // native arch: update both R and R64 keys
        let r64r = hive.create_subkey("SOFTWARE\\R-core\\R");
        if let Ok(x) = r64r {
            let (key, _) = x;
            update_registry_default1(&key, default)?;
        }
        let r64r64 = hive.create_subkey("SOFTWARE\\R-core\\R64");
        if let Ok(x) = r64r64 {
            let (key, _) = x;
            update_registry_default1(&key, default)?;
        }
    }
    Ok(())
}

pub(super) fn update_registry_default() -> Result<(), Box<dyn Error>> {
    escalate("Update registry default")?;
    let default = sc_get_default_or_fail()?;
    update_registry_default_to(&default)
}

pub(super) fn unset_registry_default() -> Result<(), Box<dyn Error>> {
    let hive = r_registry_hive()?;
    let r64r = hive.create_subkey("SOFTWARE\\R-core\\R");
    if let Ok(x) = r64r {
        let (key, _) = x;
        let _ = key.delete_value("Current Version");
        let _ = key.delete_value("InstallPath");
    }
    let r64r64 = hive.create_subkey("SOFTWARE\\R-core\\R64");
    if let Ok(x) = r64r64 {
        let (key, _) = x;
        let _ = key.delete_value("Current Version");
        let _ = key.delete_value("InstallPath");
    }
    Ok(())
}

pub(super) fn add_user_bin_to_path() -> Result<(), Box<dyn Error>> {
    let bin_dir = get_binary_dir()?;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let env_key = hkcu.open_subkey_with_flags(
        "Environment",
        winreg::enums::KEY_READ | winreg::enums::KEY_WRITE,
    )?;

    // Read current PATH (handles both REG_SZ and REG_EXPAND_SZ).
    let raw = env_key.get_raw_value("Path").unwrap_or(winreg::RegValue {
        bytes: Vec::new(),
        vtype: winreg::enums::REG_EXPAND_SZ,
    });

    // Decode UTF-16 LE registry string (strip null terminator).
    let words: Vec<u16> = raw
        .bytes
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .take_while(|&c| c != 0)
        .collect();
    let current_path = String::from_utf16_lossy(&words);

    // Check if bin_dir is already a segment (case-insensitive on Windows).
    let bin_lower = bin_dir.to_lowercase();
    let already_present = current_path
        .split(';')
        .any(|s| s.trim().to_lowercase() == bin_lower);

    if already_present {
        return Ok(());
    }

    let new_path = if current_path.is_empty() {
        bin_dir.clone()
    } else {
        format!("{};{}", bin_dir, current_path)
    };

    // Encode back to UTF-16 LE with null terminator, preserving original REG type.
    let encoded: Vec<u8> = new_path
        .encode_utf16()
        .chain(std::iter::once(0u16))
        .flat_map(|c| c.to_le_bytes())
        .collect();
    env_key.set_raw_value(
        "Path",
        &winreg::RegValue {
            bytes: encoded,
            vtype: raw.vtype,
        },
    )?;

    OUTPUT.status(&format!("Added {} to user PATH", bin_dir));
    info!("Added {} to user PATH", bin_dir);
    OUTPUT.warn("Restart your terminal (or sign out and back in) for the PATH change to take effect.");
    warn!("Restart your terminal for the PATH change to take effect.");

    Ok(())
}

#[derive(Default, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RtoolsVersion {
    pub name: String,
    pub version: String,
    pub fullversion: String,
    pub path: String,
    pub arch: String,
}

fn get_rtools_versions(rtoolskey: &RegKey) -> Result<Vec<RtoolsVersion>, Box<dyn Error>> {
    let mut versions: Vec<RtoolsVersion> = vec![];
    for nm in rtoolskey.enum_keys() {
        let nm = nm?;
        let subkey = rtoolskey.open_subkey(&nm)?;
        // e.g. 4.3.5948.5818
        let fullversion: String = subkey.get_value("FullVersion")?;
        let path: String = subkey.get_value("InstallPath")?;
        let verparts: Vec<_> = nm.split(".").collect();
        // e.g. 4.3
        let version = verparts[0..2].join(".");
        // e.g. 43
        let name = verparts[0..2].join("");
        // derive arch from install path: -aarch64 in path => aarch64, else x86_64
        let arch = if path.to_lowercase().contains("-aarch64") {
            "aarch64".to_string()
        } else {
            "x86_64".to_string()
        };
        versions.push(RtoolsVersion {
            name,
            version,
            fullversion,
            path,
            arch,
        });
    }
    Ok(versions)
}

pub(super) fn sc_rtools_ls(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    escalate("listing Rtools in the registry")?;
    let mut versions: Vec<RtoolsVersion> = vec![];

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let rtools32 = hklm.open_subkey("SOFTWARE\\WOW6432Node\\R-core\\Rtools");
    if let Ok(key) = rtools32 {
        versions.append(&mut get_rtools_versions(&key)?);
    }
    let rtools64 = hklm.open_subkey("SOFTWARE\\R-core\\Rtools");
    if let Ok(key) = rtools64 {
        versions.append(&mut get_rtools_versions(&key)?);
    }

    let json = args.get_flag("json") || mainargs.get_flag("json");
    if json {
        println!("{}", serde_json::to_string_pretty(&versions)?);
    } else {
        let mut tab = Table::new("{:<}  {:<}  {:<}  {:<}  {:<}");
        tab.add_row(row!["name", "version", "full-version", "arch", "path"]);
        tab.add_heading("------------------------------------------------------");
        for item in versions {
            tab.add_row(row!(item.name, item.version, item.fullversion, item.arch, item.path));
        }
        println!("{}", tab);
    }

    Ok(())
}

pub(super) fn get_latest_install_path(installed_arch: &str) -> Result<Option<String>, Box<dyn Error>> {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let native = get_native_arch();
    // Choose the registry key written by the installer for this arch.
    // x86_64 R on x86_64 host  -> SOFTWARE\R-core\R64
    // aarch64 R on aarch64 host -> SOFTWARE\R-core\R
    // x86_64 R on aarch64 host  -> SOFTWARE\WOW6432Node\R-core\R64
    let key = if native == "aarch64" && installed_arch == "x86_64" {
        "SOFTWARE\\WOW6432Node\\R-core\\R64"
    } else if native == "aarch64" {
        "SOFTWARE\\R-core\\R"
    } else {
        "SOFTWARE\\R-core\\R64"
    };
    let regkey = hklm.open_subkey(key);
    if let Ok(k) = regkey {
        let ip: Result<String, _> = k.get_value("InstallPath");
        if let Ok(fp) = ip {
            let ufp = fp.replace("\\", "/");
            let p = match basename(&ufp) {
                None => return Ok(None),
                Some(p) => p,
            };
            let re = Regex::new("^R-")?;
            let base = re.replace(p, "").to_string();
            let name = rig_name_for_arch(&base, installed_arch);
            return Ok(Some(name));
        }
    }
    Ok(None)
}
