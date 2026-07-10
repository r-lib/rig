use std::error::Error;
use std::path::Path;

use clap::ArgMatches;
use log::{debug, info, warn};
use regex::Regex;
use tabular::*;
use winreg::enums::*;
use winreg::{RegKey, RegValue};

use crate::common::*;
use crate::escalate::*;
use crate::output::OUTPUT;
use crate::utils::*;
use crate::windows_arch::*;

use super::{
    arch_of_name, get_links_dir, get_r_root_for, r_dirname, rig_name_for_arch, version_dir_key,
};

// The registry locations the Rtools installers record version info under: the
// native (64-bit) view and the WOW6432Node view. The 32-bit legacy (2.x/3.x)
// installers have their HKLM writes redirected to WOW6432Node, so both views
// must be checked when reading, cleaning or relocating Rtools keys.
const RTOOLS_REG_PATHS: [&str; 2] = [
    "SOFTWARE\\R-core\\Rtools",
    "SOFTWARE\\WOW6432Node\\R-core\\Rtools",
];

// The "Add/Remove Programs" uninstall registry locations: the native (64-bit)
// view and the WOW6432Node view. Inno Setup (used by the Rtools installers)
// records one `<AppId>_is1` subkey per install here, and treats a pre-existing
// entry as "already installed", so both views must be handled when relocating
// the legacy Rtools keys.
const UNINSTALL_REG_PATHS: [&str; 2] = [
    "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
    "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
];

// Prefix of the Rtools uninstaller subkey names (e.g. "Rtools_is1").
const RTOOLS_UNINST_PREFIX: &str = "Rtools";

fn clean_registry_r(key: &RegKey) -> Result<(), Box<dyn Error>> {
    for nm in key.enum_keys() {
        let nm = nm?;
        let subkey = key.open_subkey(&nm)?;
        let path: String = subkey.get_value("InstallPath")?;
        let path2 = Path::new(&path);
        if !path2.exists() {
            debug!("Cleaning registry: R {} (not in {})", nm, path);
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
            debug!("Cleaning registry: Rtools {} (not in {})", nm, path);
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

// Rtools registers under HKLM for system (admin) installs and under HKCU for per-user
// (/CURRENTUSER) installs, so the hive to read/clean depends on the active mode.
fn rtools_registry_hive() -> Result<RegKey, Box<dyn Error>> {
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

    // Rtools registers in HKLM for admin installs and HKCU for per-user installs.
    let rtools_hive = rtools_registry_hive()?;
    for path in RTOOLS_REG_PATHS {
        if let Ok(x) = rtools_hive.open_subkey(path) {
            clean_registry_rtools(&x)?;
            if x.enum_keys().count() == 0 {
                rtools_hive.delete_subkey(path)?;
            }
        }
    }

    if get_mode()? == Mode::Admin {
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        let uninst = hklm.open_subkey("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall");
        if let Ok(x) = uninst {
            clean_registry_uninst(&x)?;
        };
        let uninst32 = hklm
            .open_subkey("SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall");
        if let Ok(x) = uninst32 {
            clean_registry_uninst(&x)?;
        };
    } else {
        // User-mode installs put uninstall entries in HKCU
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let uninst = hkcu.open_subkey("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall");
        if let Ok(x) = uninst {
            clean_registry_uninst(&x)?;
        };
    }

    Ok(())
}

// Clean up the admin-mode (HKLM) R registry entries, used by
// `rig system clean-admin-r`. Unlike sc_clean_registry(), which targets the
// hive matching the configured mode, this always operates on HKEY_LOCAL_MACHINE
// so it works after the mode has already been switched to `user`. It removes
// the per-version subkeys whose install directory is gone, clears the recorded
// default (the top-level `Current Version`/`InstallPath` values), and prunes the
// stale uninstaller entries.
pub(super) fn clean_admin_registry() -> Result<(), Box<dyn Error>> {
    OUTPUT.status("Cleaning leftover registry entries");
    info!("Cleaning leftover admin-mode registry entries");

    let hive = RegKey::predef(HKEY_LOCAL_MACHINE);
    let r_keys = [
        "SOFTWARE\\R-core\\R",
        "SOFTWARE\\R-core\\R64",
        "SOFTWARE\\WOW6432Node\\R-core\\R",
        "SOFTWARE\\WOW6432Node\\R-core\\R32",
        "SOFTWARE\\WOW6432Node\\R-core\\R64",
    ];
    for path in r_keys {
        if let Ok(key) = hive.open_subkey(path) {
            clean_registry_r(&key)?;
        }
        // Clear the recorded default version if it points nowhere anymore.
        if let Ok(key) = hive.open_subkey_with_flags(path, KEY_READ | KEY_WRITE) {
            let stale = match key.get_value::<String, _>("InstallPath") {
                Ok(p) => !Path::new(&p).exists(),
                Err(_) => false,
            };
            if stale {
                let _ = key.delete_value("Current Version");
                let _ = key.delete_value("InstallPath");
            }
        }
    }

    if let Ok(key) = hive.open_subkey("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall") {
        clean_registry_uninst(&key)?;
    }
    if let Ok(key) =
        hive.open_subkey("SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall")
    {
        clean_registry_uninst(&key)?;
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

fn update_registry_default1(key: &RegKey, ver: &str) -> Result<(), Box<dyn Error>> {
    let base = version_dir_key(ver);
    let rroot = get_r_root_for(ver)?;
    key.set_value("Current Version", &base)?;
    let inst = rroot + "\\" + &r_dirname(&base)?;
    key.set_value("InstallPath", &inst)?;
    Ok(())
}

fn update_registry_default_to(default: &str) -> Result<(), Box<dyn Error>> {
    let hive = r_registry_hive()?;
    let native = get_native_arch();
    let arch = arch_of_name(default);

    if native == "aarch64" && arch == "x86_64" {
        // x86_64 R on aarch64 host: update both WOW6432Node R and R64 keys
        let r32r = hive.create_subkey("SOFTWARE\\WOW6432Node\\R-core\\R");
        if let Ok(x) = r32r {
            let (key, _) = x;
            update_registry_default1(&key, default)?;
        }
        let r32r64 = hive.create_subkey("SOFTWARE\\WOW6432Node\\R-core\\R64");
        if let Ok(x) = r32r64 {
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
    let mut key_paths = vec!["SOFTWARE\\R-core\\R", "SOFTWARE\\R-core\\R64"];
    // On aarch64 hosts x86_64 R also records its default in the WOW6432Node
    // keys (see update_registry_default_to), so clear those as well.
    if get_native_arch() == "aarch64" {
        key_paths.push("SOFTWARE\\WOW6432Node\\R-core\\R");
        key_paths.push("SOFTWARE\\WOW6432Node\\R-core\\R64");
    }
    for key_path in key_paths {
        if let Ok((key, _)) = hive.create_subkey(key_path) {
            let _ = key.delete_value("Current Version");
            let _ = key.delete_value("InstallPath");
        }
    }
    Ok(())
}

fn path_contains_dir(current_path: &str, dir: &str) -> bool {
    let dir_lower = dir.to_lowercase();
    current_path
        .split(';')
        .any(|s| s.trim().to_lowercase() == dir_lower)
}

fn prepend_dir_to_path(dir: &str, current_path: &str) -> String {
    if current_path.is_empty() {
        dir.to_string()
    } else {
        format!("{};{}", dir, current_path)
    }
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

    if path_contains_dir(&current_path, &bin_dir) {
        return Ok(());
    }

    let new_path = prepend_dir_to_path(&bin_dir, &current_path);

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
    OUTPUT.warn(
        "Restart your terminal (or sign out and back in) for the PATH change to take effect.",
    );
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

    // Admin installs register under HKLM, per-user installs under HKCU.
    let hive = rtools_registry_hive()?;
    for path in RTOOLS_REG_PATHS {
        if let Ok(key) = hive.open_subkey(path) {
            versions.append(&mut get_rtools_versions(&key)?);
        }
    }

    let json = args.get_flag("json") || mainargs.get_flag("json");
    if json {
        println!("{}", serde_json::to_string_pretty(&versions)?);
    } else {
        let mut tab = Table::new("{:<}  {:<}  {:<}  {:<}  {:<}");
        tab.add_row(row!["name", "version", "full-version", "arch", "path"]);
        tab.add_heading("------------------------------------------------------");
        for item in versions {
            tab.add_row(row!(
                item.name,
                item.version,
                item.fullversion,
                item.arch,
                item.path
            ));
        }
        println!("{}", tab);
    }

    Ok(())
}

// List the admin-mode (HKLM) installed Rtools as (version-name, arch) pairs,
// e.g. ("44", "x86_64"). Used by `rig system user-mode` to reinstall them in
// user mode. Reads HKLM directly regardless of the configured mode, and skips
// entries whose install directory no longer exists.
pub(super) fn list_admin_rtools() -> Result<Vec<(String, String)>, Box<dyn Error>> {
    let mut out: Vec<(String, String)> = vec![];
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    for path in RTOOLS_REG_PATHS {
        if let Ok(key) = hklm.open_subkey(path) {
            for v in get_rtools_versions(&key)? {
                if Path::new(&v.path).exists() {
                    out.push((v.name, v.arch));
                }
            }
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

// Install paths of the admin-mode (HKLM) Rtools that still exist on disk, used
// by `rig system clean-admin-r` to remove the admin-mode Rtools directories.
// Reads HKLM directly regardless of the configured mode.
pub(super) fn admin_rtools_paths() -> Result<Vec<String>, Box<dyn Error>> {
    let mut out: Vec<String> = vec![];
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    for path in RTOOLS_REG_PATHS {
        if let Ok(key) = hklm.open_subkey(path) {
            for v in get_rtools_versions(&key)? {
                if Path::new(&v.path).exists() && !out.contains(&v.path) {
                    out.push(v.path);
                }
            }
        }
    }
    Ok(out)
}

// Prune the admin-mode (HKLM) Rtools registry entries whose install directory
// no longer exists, after the admin-mode Rtools directories have been removed.
// Deletes the now-empty `R-core\Rtools` keys. The matching uninstaller entries
// are pruned by clean_admin_registry().
pub(super) fn clean_admin_rtools_registry() -> Result<(), Box<dyn Error>> {
    let hive = RegKey::predef(HKEY_LOCAL_MACHINE);
    for path in RTOOLS_REG_PATHS {
        if let Ok(key) = hive.open_subkey(path) {
            clean_registry_rtools(&key)?;
            if key.enum_keys().count() == 0 {
                let _ = hive.delete_subkey(path);
            }
        }
    }
    Ok(())
}

pub(super) fn get_latest_install_path(
    installed_arch: &str,
) -> Result<Option<String>, Box<dyn Error>> {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let native = get_native_arch();
    // Choose the registry key written by the installer for this arch.
    // x86_64 R on x86_64 host  -> SOFTWARE\R-core\R64
    // aarch64 R on aarch64 host -> SOFTWARE\R-core\R
    // x86_64 R on aarch64 host  -> SOFTWARE\WOW6432Node\R-core\R
    let key = if native == "aarch64" && installed_arch == "x86_64" {
        "SOFTWARE\\WOW6432Node\\R-core\\R"
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

// An in-memory snapshot of a registry subtree (values + child subkeys),
// used to back up and recreate Rtools keys when relocating them between hives.
#[derive(Default)]
struct RegNode {
    values: Vec<(String, RegValue)>,
    subkeys: Vec<(String, RegNode)>,
}

// Read a registry subtree (raw values, to preserve their types, plus all
// descendant subkeys) into a RegNode.
fn read_reg_tree(key: &RegKey) -> Result<RegNode, Box<dyn Error>> {
    let mut node = RegNode::default();
    for v in key.enum_values() {
        let (name, value) = v?;
        node.values.push((name, value));
    }
    for k in key.enum_keys() {
        let name = k?;
        let sub = key.open_subkey(&name)?;
        node.subkeys.push((name, read_reg_tree(&sub)?));
    }
    Ok(node)
}

// Write a RegNode's values and subkeys into an already-opened key.
fn write_reg_tree(key: &RegKey, node: &RegNode) -> Result<(), Box<dyn Error>> {
    for (name, value) in &node.values {
        key.set_raw_value(name, value)?;
    }
    for (name, sub) in &node.subkeys {
        let (subkey, _) = key.create_subkey(name)?;
        write_reg_tree(&subkey, sub)?;
    }
    Ok(())
}

// Full paths (relative to the hive) of the child subkeys under any of
// `parent_paths` whose name starts with `prefix`. Used to discover the Rtools
// `<AppId>_is1` uninstaller entries, whose exact names are not known in advance,
// so they can be relocated with the same subtree machinery as the fixed
// R-core\Rtools keys.
fn matching_subkey_paths(hive: &RegKey, parent_paths: &[&str], prefix: &str) -> Vec<String> {
    let mut out = vec![];
    for parent_path in parent_paths {
        if let Ok(parent) = hive.open_subkey(parent_path) {
            for name in parent.enum_keys().filter_map(|n| n.ok()) {
                if name.starts_with(prefix) {
                    out.push(format!("{}\\{}", parent_path, name));
                }
            }
        }
    }
    out
}

// The HKLM registry subtrees that an Rtools install touches and that this
// relocation backs up/moves: the fixed R-core\Rtools keys plus the Rtools
// `<AppId>_is1` uninstaller entries currently present (discovered by name).
fn rtools_reloc_paths(hklm: &RegKey) -> Vec<String> {
    let mut paths: Vec<String> = RTOOLS_REG_PATHS.iter().map(|p| p.to_string()).collect();
    paths.extend(matching_subkey_paths(
        hklm,
        &UNINSTALL_REG_PATHS,
        RTOOLS_UNINST_PREFIX,
    ));
    paths
}

// Back up and restore HKLM registry because rtools 3.x and older overwrites it
// even in user mode.
pub(super) struct LegacyRtoolsRegRelocation {
    backup: Vec<(String, RegNode)>,
    committed: bool,
}

impl LegacyRtoolsRegRelocation {
    // Step 2: back up and clear any pre-existing HKLM Rtools keys, so that after
    // the install they hold only what this installer wrote. Besides the
    // R-core\Rtools keys this also covers the `<AppId>_is1` uninstaller entry
    // under HKLM\...\Uninstall: rtools 3.x treats a pre-existing entry as
    // "already installed" and refuses to run.
    pub(super) fn begin() -> Result<Self, Box<dyn Error>> {
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        let mut backup = vec![];
        for path in rtools_reloc_paths(&hklm) {
            if let Ok(key) = hklm.open_subkey(&path) {
                let tree = read_reg_tree(&key)?;
                drop(key);
                hklm.delete_subkey_all(&path)?;
                debug!("Backed up and cleared HKLM\\{} before Rtools install", path);
                backup.push((path, tree));
            }
        }
        Ok(LegacyRtoolsRegRelocation {
            backup,
            committed: false,
        })
    }

    // Steps 4 + 5: move the keys the installer wrote under HKLM into HKCU (where
    // a per-user /CURRENTUSER install records them), then restore the original
    // HKLM state from the backup taken in begin().
    pub(super) fn commit(mut self) -> Result<(), Box<dyn Error>> {
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        for path in rtools_reloc_paths(&hklm) {
            if let Ok(src) = hklm.open_subkey(&path) {
                let tree = read_reg_tree(&src)?;
                drop(src);
                let (dst, _) = hkcu.create_subkey(&path)?;
                write_reg_tree(&dst, &tree)?;
                hklm.delete_subkey_all(&path)?;
                debug!("Moved HKLM\\{0} -> HKCU\\{0} after Rtools install", path);
            }
        }
        restore_reg_backup(&hklm, &self.backup);
        self.committed = true;
        Ok(())
    }
}

impl Drop for LegacyRtoolsRegRelocation {
    fn drop(&mut self) {
        if !self.committed {
            // The install failed after begin() cleared HKLM: best-effort restore
            // so we never leave the system with the user's original Rtools keys
            // missing.
            let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
            restore_reg_backup(&hklm, &self.backup);
        }
    }
}

// Recreate the backed-up HKLM Rtools keys. Best-effort: failures are logged but
// not propagated, since this runs both on the success and the cleanup paths.
fn restore_reg_backup(hklm: &RegKey, backup: &[(String, RegNode)]) {
    for (path, tree) in backup {
        match hklm.create_subkey(path) {
            Ok((key, _)) => {
                if let Err(e) = write_reg_tree(&key, tree) {
                    warn!("Failed to restore HKLM\\{}: {}", path, e);
                }
            }
            Err(e) => warn!("Failed to recreate HKLM\\{} for restore: {}", path, e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    // Drop guard: deletes the registry subtree when the test ends (pass or fail).
    struct TempKey(String);
    impl Drop for TempKey {
        fn drop(&mut self) {
            let hkcu = RegKey::predef(HKEY_CURRENT_USER);
            let _ = hkcu.delete_subkey_all(&self.0);
        }
    }

    fn make_test_key(name: &str) -> (RegKey, TempKey) {
        let path = format!("SOFTWARE\\rig-test\\{}", name);
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (key, _) = hkcu.create_subkey(&path).expect("create test key");
        (key, TempKey(path))
    }

    // ── path_contains_dir ────────────────────────────────────────────────────

    #[test]
    fn path_contains_dir_finds_exact_match() {
        assert!(path_contains_dir("C:\\foo;C:\\bar", "C:\\foo"));
        assert!(path_contains_dir("C:\\bar", "C:\\bar"));
    }

    #[test]
    fn path_contains_dir_is_case_insensitive() {
        assert!(path_contains_dir("C:\\FOO;C:\\bar", "C:\\foo"));
        assert!(path_contains_dir("C:\\foo", "C:\\FOO"));
    }

    #[test]
    fn path_contains_dir_ignores_whitespace_around_segments() {
        assert!(path_contains_dir(" C:\\foo ;C:\\bar", "C:\\foo"));
    }

    #[test]
    fn path_contains_dir_returns_false_when_absent() {
        assert!(!path_contains_dir("C:\\foo;C:\\bar", "C:\\baz"));
        assert!(!path_contains_dir("", "C:\\foo"));
    }

    // ── prepend_dir_to_path ──────────────────────────────────────────────────

    #[test]
    fn prepend_dir_to_path_empty_base() {
        assert_eq!(prepend_dir_to_path("C:\\new", ""), "C:\\new");
    }

    #[test]
    fn prepend_dir_to_path_nonempty_base() {
        assert_eq!(
            prepend_dir_to_path("C:\\new", "C:\\existing"),
            "C:\\new;C:\\existing"
        );
        assert_eq!(
            prepend_dir_to_path("C:\\new", "C:\\a;C:\\b"),
            "C:\\new;C:\\a;C:\\b"
        );
    }

    // ── clean_registry_r ────────────────────────────────────────────────────

    #[test]
    fn clean_registry_r_removes_missing_keeps_present() {
        let (root, _guard) = make_test_key("clean_r");

        // Subkey whose InstallPath does not exist — should be removed.
        let (gone, _) = root.create_subkey("4.3.0").unwrap();
        gone.set_value("InstallPath", &"C:\\nonexistent\\R-4.3.0")
            .unwrap();
        drop(gone);

        // Subkey whose InstallPath exists — should survive.
        let tmp = std::env::temp_dir().to_string_lossy().to_string();
        let (keep, _) = root.create_subkey("4.4.0").unwrap();
        keep.set_value("InstallPath", &tmp).unwrap();
        drop(keep);

        clean_registry_r(&root).unwrap();

        assert!(
            root.open_subkey("4.3.0").is_err(),
            "stale entry should be removed"
        );
        assert!(
            root.open_subkey("4.4.0").is_ok(),
            "live entry should remain"
        );
    }

    // ── clean_registry_uninst ────────────────────────────────────────────────

    #[test]
    fn clean_registry_uninst_filters_by_prefix_and_path() {
        let (root, _guard) = make_test_key("uninst");
        let tmp = std::env::temp_dir().to_string_lossy().to_string();

        // "R for Windows" with missing path — removed.
        let (k, _) = root.create_subkey("R for Windows 4.3.0").unwrap();
        k.set_value("InstallLocation", &"C:\\nonexistent\\R-4.3.0")
            .unwrap();
        drop(k);

        // "Rtools" with missing path — removed.
        let (k, _) = root.create_subkey("Rtools43").unwrap();
        k.set_value("InstallLocation", &"C:\\nonexistent\\Rtools43")
            .unwrap();
        drop(k);

        // Unrelated prefix with missing path — NOT touched by the function.
        let (k, _) = root.create_subkey("Python 3.12.0").unwrap();
        k.set_value("InstallLocation", &"C:\\nonexistent\\Python")
            .unwrap();
        drop(k);

        // "R for Windows" with existing path — kept.
        let (k, _) = root.create_subkey("R for Windows 4.4.0").unwrap();
        k.set_value("InstallLocation", &tmp).unwrap();
        drop(k);

        clean_registry_uninst(&root).unwrap();

        assert!(root.open_subkey("R for Windows 4.3.0").is_err());
        assert!(root.open_subkey("Rtools43").is_err());
        assert!(
            root.open_subkey("Python 3.12.0").is_ok(),
            "unrelated key must not be deleted"
        );
        assert!(root.open_subkey("R for Windows 4.4.0").is_ok());
    }

    // ── get_rtools_versions ──────────────────────────────────────────────────

    #[test]
    fn get_rtools_versions_parses_name_version_and_arch() {
        let (root, _guard) = make_test_key("rtools");

        // x86_64: no "-aarch64" in path.
        let (k, _) = root.create_subkey("4.3.5948.5818").unwrap();
        k.set_value("FullVersion", &"4.3.5948.5818").unwrap();
        k.set_value("InstallPath", &"C:\\rtools43").unwrap();
        drop(k);

        // aarch64: "-aarch64" appears in path.
        let (k, _) = root.create_subkey("4.4.6459.5818").unwrap();
        k.set_value("FullVersion", &"4.4.6459.5818").unwrap();
        k.set_value("InstallPath", &"C:\\rtools44-aarch64").unwrap();
        drop(k);

        let mut versions = get_rtools_versions(&root).unwrap();
        versions.sort_by(|a, b| a.version.cmp(&b.version));

        assert_eq!(versions.len(), 2);

        assert_eq!(versions[0].name, "43");
        assert_eq!(versions[0].version, "4.3");
        assert_eq!(versions[0].fullversion, "4.3.5948.5818");
        assert_eq!(versions[0].arch, "x86_64");

        assert_eq!(versions[1].name, "44");
        assert_eq!(versions[1].version, "4.4");
        assert_eq!(versions[1].fullversion, "4.4.6459.5818");
        assert_eq!(versions[1].arch, "aarch64");
    }

    // ── matching_subkey_paths ────────────────────────────────────────────────

    #[test]
    fn matching_subkey_paths_returns_only_prefixed_children() {
        let (root, _guard) = make_test_key("match_subkeys");
        let parent_path = "SOFTWARE\\rig-test\\match_subkeys";

        root.create_subkey("Rtools_is1").unwrap();
        root.create_subkey("Rtools35").unwrap();
        root.create_subkey("Python 3.12.0").unwrap();

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let mut got = matching_subkey_paths(&hkcu, &[parent_path], "Rtools");
        got.sort();

        assert_eq!(
            got,
            vec![
                format!("{}\\Rtools35", parent_path),
                format!("{}\\Rtools_is1", parent_path),
            ]
        );
    }

    #[test]
    fn matching_subkey_paths_skips_missing_parent() {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let got = matching_subkey_paths(&hkcu, &["SOFTWARE\\rig-test\\does-not-exist"], "Rtools");
        assert!(got.is_empty());
    }

    // ── read_reg_tree / write_reg_tree / restore_reg_backup round-trip ───────

    #[test]
    fn reg_tree_round_trips_values_and_subkeys() {
        let (root, _guard) = make_test_key("reg_tree");
        let path = "SOFTWARE\\rig-test\\reg_tree\\Rtools_is1";
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);

        // Populate a subtree with a value and a nested subkey.
        let (k, _) = root.create_subkey("Rtools_is1").unwrap();
        k.set_value("InstallLocation", &"C:\\rtools35").unwrap();
        let (child, _) = k.create_subkey("inner").unwrap();
        child.set_value("flag", &"1").unwrap();
        drop(child);

        // Snapshot, then delete it.
        let tree = read_reg_tree(&k).unwrap();
        drop(k);
        hkcu.delete_subkey_all(path).unwrap();
        assert!(root.open_subkey("Rtools_is1").is_err());

        // Restoring recreates the values and the nested subkey.
        restore_reg_backup(&hkcu, &[(path.to_string(), tree)]);
        let restored = root.open_subkey("Rtools_is1").unwrap();
        let loc: String = restored.get_value("InstallLocation").unwrap();
        assert_eq!(loc, "C:\\rtools35");
        let inner = restored.open_subkey("inner").unwrap();
        let flag: String = inner.get_value("flag").unwrap();
        assert_eq!(flag, "1");
    }
}
