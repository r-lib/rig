// `rig self uninstall` -- remove the rig installation made by `install.sh` /
// `install.ps1`, reversing exactly what those scripts created.
//
// Like `rig self update`, this only works for script installs (see
// `crate::install_receipt`). It removes the binary, its known sibling
// completion files, the PATH edit the installer made, and the install
// receipt. It deliberately leaves rig's config file, its download/build
// cache, and any installed R versions alone -- `rig self uninstall` owns
// only what the installer created, the same way `rig rm` owns R versions
// and `rig system clean-admin-r` owns admin-mode R cleanup.

use std::error::Error;
use std::path::{Path, PathBuf};

use clap::ArgMatches;
use log::info;
use simple_error::bail;

use crate::install_receipt::{
    gate, read_receipt, receipt_path, refusal_message, GateResult, Receipt,
};
use crate::output::OUTPUT;
use crate::utils::write_atomically;

const PATH_MARKER: &str = "# Added by the rig installer";

struct UninstallPlan {
    files: Vec<PathBuf>,
    dirs_to_prune: Vec<PathBuf>,
    bin_path: PathBuf,
    bindir: String,
    receipt_path: PathBuf,
    data_dir: PathBuf,
}

fn compute_plan(receipt: &Receipt) -> Result<UninstallPlan, Box<dyn Error>> {
    let bin_path = PathBuf::from(&receipt.bin_path);
    let bindir_path = bin_path
        .parent()
        .ok_or("install receipt has no bin directory")?
        .to_path_buf();
    let bindir = bindir_path.to_string_lossy().to_string();
    let prefix = Path::new(&receipt.prefix);
    let share = prefix.join("share");

    let mut files = vec![
        share
            .join("bash-completion")
            .join("completions")
            .join("rig.bash"),
        share.join("elvish").join("lib").join("rig.elv"),
        share
            .join("fish")
            .join("vendor_completions.d")
            .join("rig.fish"),
        share.join("zsh").join("site-functions").join("_rig"),
    ];

    match receipt.platform.as_str() {
        "linux" => files.push(share.join("rig").join("cacert.pem")),
        "windows" => {
            files.push(bindir_path.join("rig-shim.exe"));
            files.push(bindir_path.join("gsudo.exe"));
            files.push(share.join("rig").join("_rig.ps1"));
        }
        _ => {}
    }

    let dirs_to_prune = vec![
        share.join("bash-completion").join("completions"),
        share.join("bash-completion"),
        share.join("elvish").join("lib"),
        share.join("elvish"),
        share.join("fish").join("vendor_completions.d"),
        share.join("fish"),
        share.join("zsh").join("site-functions"),
        share.join("zsh"),
        share.join("rig"),
        share,
        bindir_path.clone(),
    ];

    Ok(UninstallPlan {
        files,
        dirs_to_prune,
        bin_path,
        bindir,
        receipt_path: receipt_path()?,
        data_dir: crate::cache::get_data_dir()?,
    })
}

// Removes the exact two-line block (plus a preceding blank line, if present)
// that `install.sh` appends to a shell rc file. Returns `None` if the block
// isn't present verbatim -- a hand-edited rc file is left untouched rather
// than guessed at.
fn remove_installer_path_block(contents: &str, bindir: &str) -> Option<String> {
    let export_line = format!("export PATH=\"{}:$PATH\"", bindir);
    let lines: Vec<&str> = contents.lines().collect();

    let marker_idx = lines
        .iter()
        .position(|&l| l == PATH_MARKER)
        .filter(|&i| lines.get(i + 1) == Some(&export_line.as_str()))?;

    let mut start = marker_idx;
    if start > 0 && lines[start - 1].is_empty() {
        start -= 1;
    }
    let end = marker_idx + 2;

    let mut new_lines: Vec<&str> = Vec::with_capacity(lines.len());
    new_lines.extend_from_slice(&lines[..start]);
    new_lines.extend_from_slice(&lines[end..]);

    let mut result = new_lines.join("\n");
    if contents.ends_with('\n') {
        result.push('\n');
    }
    Some(result)
}

#[cfg(unix)]
fn rc_file_for_shell(shell_env: Option<&str>, home: &Path) -> PathBuf {
    let shell_name = shell_env.and_then(|s| s.rsplit('/').next()).unwrap_or("sh");
    match shell_name {
        "zsh" => home.join(".zshrc"),
        "bash" => home.join(".bashrc"),
        _ => home.join(".profile"),
    }
}

#[cfg(unix)]
#[allow(deprecated)]
fn remove_path_from_rc_file(bindir: &str) -> Result<Option<PathBuf>, Box<dyn Error>> {
    let home = std::env::home_dir().ok_or("cannot determine home directory")?;
    let rc = rc_file_for_shell(std::env::var("SHELL").ok().as_deref(), &home);
    if !rc.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(&rc)?;
    match remove_installer_path_block(&contents, bindir) {
        Some(new_contents) => {
            write_atomically(&rc, new_contents.as_bytes())?;
            Ok(Some(rc))
        }
        None => Ok(None),
    }
}

// Removes the exact `;`-separated segment matching `segment` (case
// insensitive, matching Windows PATH semantics) from `current`. Returns
// `None` if the segment isn't present.
#[cfg(target_os = "windows")]
fn remove_path_segment(current: &str, segment: &str) -> Option<String> {
    let segment_lower = segment.to_lowercase();
    if !current
        .split(';')
        .any(|s| s.trim().to_lowercase() == segment_lower)
    {
        return None;
    }
    let kept: Vec<&str> = current
        .split(';')
        .filter(|s| s.trim().to_lowercase() != segment_lower)
        .collect();
    Some(kept.join(";"))
}

#[cfg(target_os = "windows")]
fn remove_bindir_from_user_path(bindir: &str) -> Result<bool, Box<dyn Error>> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let env_key = hkcu.open_subkey_with_flags(
        "Environment",
        winreg::enums::KEY_READ | winreg::enums::KEY_WRITE,
    )?;

    let raw = match env_key.get_raw_value("Path") {
        Ok(v) => v,
        Err(_) => return Ok(false),
    };

    let words: Vec<u16> = raw
        .bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes(*b))
        .take_while(|&c| c != 0)
        .collect();
    let current_path = String::from_utf16_lossy(&words);

    let new_path = match remove_path_segment(&current_path, bindir) {
        Some(p) => p,
        None => return Ok(false),
    };

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
    Ok(true)
}

fn print_plan(plan: &UninstallPlan) {
    OUTPUT.status("rig self uninstall would remove:");
    OUTPUT.status(&format!("  {}", plan.bin_path.display()));
    for f in &plan.files {
        OUTPUT.status(&format!("  {}", f.display()));
    }
    OUTPUT.status(&format!("  {}", plan.receipt_path.display()));
    #[cfg(unix)]
    OUTPUT.status(&format!(
        "  the PATH entry for {} added to your shell rc file, if present",
        plan.bindir
    ));
    #[cfg(target_os = "windows")]
    OUTPUT.status(&format!(
        "  the PATH entry for {} in your user environment, if present",
        plan.bindir
    ));
}

fn prune_empty_dirs(dirs: &[PathBuf]) {
    for dir in dirs {
        let _ = std::fs::remove_dir(dir);
    }
}

fn execute_plan(plan: &UninstallPlan) -> Result<(), Box<dyn Error>> {
    for f in &plan.files {
        if f.exists() {
            std::fs::remove_file(f)?;
        }
    }
    prune_empty_dirs(&plan.dirs_to_prune);

    #[cfg(unix)]
    if let Some(rc) = remove_path_from_rc_file(&plan.bindir)? {
        OUTPUT.status(&format!("Removed the PATH entry from {}", rc.display()));
    }
    #[cfg(target_os = "windows")]
    if remove_bindir_from_user_path(&plan.bindir)? {
        OUTPUT.status("Removed the PATH entry from your user environment");
    }

    if plan.receipt_path.exists() {
        std::fs::remove_file(&plan.receipt_path)?;
    }
    let _ = std::fs::remove_dir(&plan.data_dir);

    self_replace::self_delete_at(&plan.bin_path)?;

    Ok(())
}

pub fn sc_self_uninstall(args: &ArgMatches, _mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let receipt = read_receipt()?;
    let receipt = match gate(receipt) {
        GateResult::Ok(r) => r,
        other => bail!("{}", refusal_message("rig self uninstall", &other)),
    };

    let plan = compute_plan(&receipt)?;

    if args.get_flag("dry-run") || !args.get_flag("force") {
        print_plan(&plan);
        if !args.get_flag("dry-run") {
            OUTPUT.status("Run 'rig self uninstall --force' to actually remove rig.");
        }
        return Ok(());
    }

    execute_plan(&plan)?;

    OUTPUT.success("rig has been uninstalled");
    OUTPUT.status(
        "Your rig config, cache, and any R versions installed with rig were left in place.",
    );
    OUTPUT.status(
        "Run 'rig rm all' first next time if you also want to remove installed R versions.",
    );
    info!("rig self uninstall: removed {}", receipt.bin_path);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(platform: &str) -> Receipt {
        Receipt {
            receipt_version: 1,
            install_method: "script".to_string(),
            rig_version: "0.10.0".to_string(),
            platform: platform.to_string(),
            arch: "arm64".to_string(),
            bin_path: if platform == "windows" {
                "C:\\Users\\me\\.local\\bin\\rig.exe".to_string()
            } else {
                "/home/me/.local/bin/rig".to_string()
            },
            prefix: if platform == "windows" {
                "C:\\Users\\me\\.local".to_string()
            } else {
                "/home/me/.local".to_string()
            },
            installed_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn compute_plan_linux_includes_cacert() {
        let plan = compute_plan(&receipt("linux")).unwrap();
        assert!(plan
            .files
            .iter()
            .any(|f| f.ends_with("share/rig/cacert.pem")));
        assert!(!plan.files.iter().any(|f| f.ends_with("rig-shim.exe")));
    }

    #[test]
    fn compute_plan_macos_has_no_platform_extras() {
        let plan = compute_plan(&receipt("macos")).unwrap();
        assert!(!plan
            .files
            .iter()
            .any(|f| f.ends_with("share/rig/cacert.pem")));
        assert!(!plan.files.iter().any(|f| f.ends_with("rig-shim.exe")));
    }

    #[test]
    fn compute_plan_windows_includes_shim_and_gsudo() {
        let plan = compute_plan(&receipt("windows")).unwrap();
        assert!(plan.files.iter().any(|f| f.ends_with("rig-shim.exe")));
        assert!(plan.files.iter().any(|f| f.ends_with("gsudo.exe")));
        assert!(plan.files.iter().any(|f| f.ends_with("_rig.ps1")));
    }

    #[test]
    fn remove_installer_path_block_found_with_blank_line() {
        let contents = "export FOO=bar\n\n# Added by the rig installer\nexport PATH=\"/home/me/.local/bin:$PATH\"\n";
        let result = remove_installer_path_block(contents, "/home/me/.local/bin").unwrap();
        assert_eq!(result, "export FOO=bar\n");
    }

    #[test]
    fn remove_installer_path_block_found_without_blank_line() {
        let contents = "# Added by the rig installer\nexport PATH=\"/home/me/.local/bin:$PATH\"\n";
        let result = remove_installer_path_block(contents, "/home/me/.local/bin").unwrap();
        assert_eq!(result, "\n");
    }

    #[test]
    fn remove_installer_path_block_absent() {
        let contents = "export FOO=bar\n";
        assert!(remove_installer_path_block(contents, "/home/me/.local/bin").is_none());
    }

    #[test]
    fn remove_installer_path_block_similar_line_untouched() {
        let contents = "# Added by the rig installer\nexport PATH=\"/other/bin:$PATH\"\n";
        assert!(remove_installer_path_block(contents, "/home/me/.local/bin").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn rc_file_for_shell_picks_zsh_bash_or_profile() {
        let home = Path::new("/home/me");
        assert_eq!(
            rc_file_for_shell(Some("/bin/zsh"), home),
            home.join(".zshrc")
        );
        assert_eq!(
            rc_file_for_shell(Some("/bin/bash"), home),
            home.join(".bashrc")
        );
        assert_eq!(
            rc_file_for_shell(Some("/bin/fish"), home),
            home.join(".profile")
        );
        assert_eq!(rc_file_for_shell(None, home), home.join(".profile"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn remove_path_segment_present_among_others() {
        let current = r"C:\a;C:\Users\me\.local\bin;C:\b";
        let result = remove_path_segment(current, r"C:\Users\me\.local\bin").unwrap();
        assert_eq!(result, r"C:\a;C:\b");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn remove_path_segment_absent() {
        let current = r"C:\a;C:\b";
        assert!(remove_path_segment(current, r"C:\Users\me\.local\bin").is_none());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn remove_path_segment_case_insensitive() {
        let current = r"C:\A;C:\USERS\me\.local\BIN";
        let result = remove_path_segment(current, r"c:\users\me\.local\bin").unwrap();
        assert_eq!(result, r"C:\A");
    }
}
