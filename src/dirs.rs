// The directories rig uses: `rig system dirs` prints all of them, and
// `rig system r-dir`, `rig system rtools-dir` and `rig system binary-dir`
// print a single one, for use in scripts.
//
// All of these report the *effective* values, i.e. what rig will actually use,
// after applying the mode, the RIG_* environment variables and the config file.
// (`rig config get <key>` in contrast reports only an explicit override, and
// prints nothing when the default applies.) None of them touch the file system
// or need an R installation, so they work on a machine with no R at all, which
// is exactly when the answer is most useful.
//
// This module is cross-platform, with #[cfg(target_os = "windows")] islands for
// the Rtools root and the architecture-dependent Windows R roots, in the style
// of src/platform.rs. It is deliberately not part of src/lib.rs, which is the
// macOS menu bar app's static library and does not include command modules.
//
// Note: the module name is safe as long as rig does not depend on the `dirs`
// crate; the (unrelated) directory lookups in src/cache.rs use `directories`.

use std::error::Error;
use std::path::PathBuf;

use clap::ArgMatches;
use tabular::{row, Table};

use crate::cache::{get_cache_dir, get_data_dir, get_logs_dir};
use crate::utils::{get_binary_dir, get_mode};

#[cfg(target_os = "linux")]
use crate::linux::get_r_root;
#[cfg(target_os = "macos")]
use crate::macos::get_r_root;
#[cfg(target_os = "windows")]
use crate::windows::{get_r_root_arch, get_rtools_root, normalize_arch};
#[cfg(target_os = "windows")]
use crate::windows_arch::get_native_arch;

// TODO: it would be useful to also report where each value comes from
// ("default", "env:RIG_R_INSTALL_DIR", "config:r-install-dir"), but that needs
// the provenance threaded through the getters in src/utils.rs.
#[derive(serde::Serialize)]
pub struct RigDirs {
    pub mode: String,
    pub arch: String,
    pub r_root: String,
    #[cfg(target_os = "windows")]
    pub rtools_root: String,
    pub binary_dir: String,
    pub config_file: String,
    pub data_dir: String,
    pub cache_dir: String,
    pub logs_dir: String,
}

// The native architecture of the machine, spelled the way this platform's
// `rig add --arch` spells it, so it can be fed straight back to rig. (This is
// not always the same as `rig system detect-platform`, which reports the Rust
// `std::env::consts::ARCH` spelling, i.e. `aarch64` on Apple silicon.)
//
// std::env::consts::ARCH is not good enough on its own, because it is the
// architecture rig itself was built for, which is not the machine's when rig
// runs emulated.
pub fn native_arch() -> String {
    #[cfg(target_os = "windows")]
    {
        // WOW64-aware, so an x86_64 rig on an aarch64 machine reports aarch64.
        get_native_arch().to_string()
    }
    #[cfg(target_os = "macos")]
    {
        // Rosetta-aware, for the same reason.
        if crate::macos::is_arm64_machine() {
            "arm64".to_string()
        } else {
            "x86_64".to_string()
        }
    }
    #[cfg(target_os = "linux")]
    {
        std::env::consts::ARCH.to_string()
    }
}

// The architecture to report, from `--arch` if given.
fn arch_arg(args: &ArgMatches) -> String {
    #[cfg(target_os = "windows")]
    {
        match args.get_one::<String>("arch") {
            Some(arch) => normalize_arch(arch),
            None => native_arch(),
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        // `--arch` is hidden and ignored off Windows: it is only Windows where
        // the R root depends on the architecture (in admin mode). Everywhere
        // else both architectures share a root and the arch is encoded in the
        // R version directory name instead.
        let _ = args;
        native_arch()
    }
}

// The effective R installation root, i.e. the directory that holds the
// directories of the individual R versions.
fn r_root(arch: &str) -> Result<String, Box<dyn Error>> {
    #[cfg(target_os = "windows")]
    {
        get_r_root_arch(arch)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = arch;
        get_r_root()
    }
}

fn path_string(path: PathBuf) -> String {
    path.display().to_string()
}

pub fn rig_dirs(arch: &str) -> Result<RigDirs, Box<dyn Error>> {
    Ok(RigDirs {
        mode: get_mode()?.to_string(),
        arch: arch.to_string(),
        r_root: r_root(arch)?,
        #[cfg(target_os = "windows")]
        rtools_root: get_rtools_root()?,
        binary_dir: get_binary_dir()?,
        config_file: path_string(crate::config::config_file_path()?),
        data_dir: path_string(get_data_dir()?),
        cache_dir: path_string(get_cache_dir()?),
        logs_dir: path_string(get_logs_dir()?),
    })
}

fn dirs_rows(dirs: &RigDirs) -> Vec<(&'static str, &str)> {
    let mut rows = vec![
        ("Mode", dirs.mode.as_str()),
        ("Architecture", dirs.arch.as_str()),
        ("R root", dirs.r_root.as_str()),
    ];
    #[cfg(target_os = "windows")]
    rows.push(("Rtools root", dirs.rtools_root.as_str()));
    rows.push(("Binary dir", dirs.binary_dir.as_str()));
    rows.push(("Config file", dirs.config_file.as_str()));
    rows.push(("Data dir", dirs.data_dir.as_str()));
    rows.push(("Cache dir", dirs.cache_dir.as_str()));
    rows.push(("Logs dir", dirs.logs_dir.as_str()));
    rows
}

pub fn sc_system_dirs(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let dirs = rig_dirs(&arch_arg(args))?;

    if args.get_flag("json") || mainargs.get_flag("json") {
        println!("{}", serde_json::to_string_pretty(&dirs)?);
    } else {
        let mut tab = Table::new("{:<}  {:<}");
        for (label, value) in dirs_rows(&dirs) {
            tab.add_row(row!(label, value));
        }
        print!("{}", tab);
    }

    Ok(())
}

// The single-value commands below print one bare path and nothing else, so that
// they can be used as `$(rig system r-dir)`. They have no `--json` flag (as
// `rig config config-file-path` does not), so they must not query one: the
// `json` argument id does not exist for them and clap panics on an unknown id.
// The machine-readable form of all of this is `rig system dirs --json`.

pub fn sc_system_r_dir(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    println!("{}", r_root(&arch_arg(args))?);
    Ok(())
}

pub fn sc_system_rtools_dir() -> Result<(), Box<dyn Error>> {
    // On macOS and Linux this is a hidden no-op, like the other Rtools
    // commands, so that scripts can call it unconditionally.
    #[cfg(target_os = "windows")]
    println!("{}", get_rtools_root()?);
    Ok(())
}

pub fn sc_system_binary_dir() -> Result<(), Box<dyn Error>> {
    println!("{}", get_binary_dir()?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // There are no tests for rig_dirs() or r_root() on purpose: they call
    // get_mode(), which caches the mode in a process-wide OnceLock, and
    // get_global_config_value(), which reads the real config file of whoever
    // runs the tests. `cargo test` is a single multi-threaded process, so a
    // test that sets RIG_MODE would leak into all the other tests. The
    // mode/override matrix is covered by the BATS tests instead, which run a
    // fresh rig process for every case.

    #[test]
    fn native_arch_is_an_accepted_arch_name() {
        // Whatever we report must be something `--arch` accepts back.
        let arch = native_arch();
        assert!(
            ["x86_64", "aarch64", "arm64"].contains(&arch.as_str()),
            "unexpected native arch: {}",
            arch
        );
    }

    #[test]
    fn json_keys_are_stable() {
        let dirs = RigDirs {
            mode: "user".to_string(),
            arch: "x86_64".to_string(),
            r_root: "r-root".to_string(),
            #[cfg(target_os = "windows")]
            rtools_root: "rtools-root".to_string(),
            binary_dir: "binary-dir".to_string(),
            config_file: "config-file".to_string(),
            data_dir: "data-dir".to_string(),
            cache_dir: "cache-dir".to_string(),
            logs_dir: "logs-dir".to_string(),
        };

        let mut expected = vec!["mode", "arch", "r_root"];
        if cfg!(target_os = "windows") {
            expected.push("rtools_root");
        }
        expected.extend([
            "binary_dir",
            "config_file",
            "data_dir",
            "cache_dir",
            "logs_dir",
        ]);

        // serde_json's Map sorts its keys, so compare the key set here and the
        // order of the actual output separately, below.
        let value = serde_json::to_value(&dirs).unwrap();
        let keys: Vec<&str> = value.as_object().unwrap().keys().map(|k| &**k).collect();
        let mut sorted = expected.clone();
        sorted.sort_unstable();
        assert_eq!(keys, sorted);

        let json = serde_json::to_string(&dirs).unwrap();
        let mut at = 0;
        for key in &expected {
            let pos = json
                .find(&format!("\"{}\":", key))
                .unwrap_or_else(|| panic!("no {} in {}", key, json));
            assert!(pos > at, "{} is out of order in {}", key, json);
            at = pos;
        }

        // The labels of the plain output track the same fields.
        let labels: Vec<&str> = dirs_rows(&dirs).iter().map(|(label, _)| *label).collect();
        assert_eq!(labels.len(), expected.len());
        assert_eq!(labels[0], "Mode");
        assert_eq!(labels[2], "R root");
    }
}
