// The install receipt (`install-receipt.json`, in the app data directory,
// see `get_data_dir()`) that `install.sh` / `install.ps1` write after a
// successful install. Its presence, with `install_method == "script"`,
// proves rig fully owns its own binary location, which is what
// `rig self update` and `rig self uninstall` both need before they can
// safely touch the binary. Every other distribution channel (`.pkg`,
// `.deb`/`.rpm`, Chocolatey, WinGet, Homebrew, or a manually extracted
// tarball) leaves no such receipt, so both commands refuse and point the
// user at the right tool instead.

use std::error::Error;
use std::path::{Path, PathBuf};

use log::debug;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Receipt {
    pub receipt_version: u32,
    pub install_method: String,
    pub rig_version: String,
    pub platform: String,
    pub arch: String,
    pub bin_path: String,
    pub prefix: String,
    pub installed_at: String,
}

pub enum GateResult {
    Ok(Receipt),
    NoReceipt,
    WrongMethod(String),
}

pub fn gate(receipt: Option<Receipt>) -> GateResult {
    match receipt {
        None => GateResult::NoReceipt,
        Some(r) if r.install_method == "script" => GateResult::Ok(r),
        Some(r) => GateResult::WrongMethod(r.install_method),
    }
}

pub fn receipt_path() -> Result<PathBuf, Box<dyn Error>> {
    Ok(crate::cache::get_data_dir()?.join("install-receipt.json"))
}

// Missing or malformed receipts both come back as `Ok(None)`: a corrupt
// receipt should make these commands decline safely, not crash.
pub fn read_receipt_at(path: &Path) -> Result<Option<Receipt>, Box<dyn Error>> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(path)?;
    match serde_json::from_slice::<Receipt>(&bytes) {
        Ok(r) => Ok(Some(r)),
        Err(err) => {
            debug!(
                "Cannot parse install receipt at {}: {}",
                path.display(),
                err
            );
            Ok(None)
        }
    }
}

pub fn read_receipt() -> Result<Option<Receipt>, Box<dyn Error>> {
    read_receipt_at(&receipt_path()?)
}

pub fn refusal_message(command: &str, reason: &GateResult) -> String {
    let how = if cfg!(target_os = "macos") {
        "If you installed the .pkg, download a newer one from \
         https://github.com/r-lib/rig/releases and run it again. If you \
         installed with Homebrew, use 'brew upgrade' / 'brew uninstall' with \
         'r-rig' / 'r-rig-app' instead."
    } else if cfg!(target_os = "windows") {
        "If you installed the .exe installer, Chocolatey, WinGet, or Scoop, \
         use that tool instead ('choco', 'winget', or 'scoop')."
    } else {
        "If you installed the .deb/.rpm package, use your package manager \
         (apt/dnf/zypper) instead."
    };

    let cause = match reason {
        GateResult::NoReceipt => "rig was not installed with the install script".to_string(),
        GateResult::WrongMethod(method) => {
            format!("rig was installed via '{}', not the install script", method)
        }
        GateResult::Ok(_) => unreachable!(),
    };

    format!("{}, so '{}' cannot do it. {}", cause, command, how)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn receipt(method: &str) -> Receipt {
        Receipt {
            receipt_version: 1,
            install_method: method.to_string(),
            rig_version: "0.10.0".to_string(),
            platform: "macos".to_string(),
            arch: "arm64".to_string(),
            bin_path: "/home/me/.local/bin/rig".to_string(),
            prefix: "/home/me/.local".to_string(),
            installed_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn gate_no_receipt() {
        assert!(matches!(gate(None), GateResult::NoReceipt));
    }

    #[test]
    fn gate_wrong_method() {
        match gate(Some(receipt("pkg"))) {
            GateResult::WrongMethod(m) => assert_eq!(m, "pkg"),
            _ => panic!("expected WrongMethod"),
        }
    }

    #[test]
    fn gate_script_ok() {
        assert!(matches!(gate(Some(receipt("script"))), GateResult::Ok(_)));
    }

    #[test]
    fn read_receipt_at_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("install-receipt.json");
        assert!(read_receipt_at(&path).unwrap().is_none());
    }

    #[test]
    fn read_receipt_at_malformed_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("install-receipt.json");
        std::fs::write(&path, b"not json").unwrap();
        assert!(read_receipt_at(&path).unwrap().is_none());
    }

    #[test]
    fn read_receipt_at_valid_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("install-receipt.json");
        std::fs::write(&path, serde_json::to_vec(&receipt("script")).unwrap()).unwrap();
        let r = read_receipt_at(&path).unwrap().unwrap();
        assert_eq!(r.install_method, "script");
    }
}
