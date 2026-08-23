use std::error::Error;
use std::path::PathBuf;
use std::sync::OnceLock;

use directories::ProjectDirs;
use log::*;
use simple_error::bail;

use crate::output::OUTPUT;

static NO_CACHE: OnceLock<bool> = OnceLock::new();

/// Record whether this run may use rig's cache, from the `--no-cache` flag.
pub fn set_no_cache(value: bool) -> Result<(), Box<dyn Error>> {
    match NO_CACHE.set(value) {
        Ok(()) => Ok(()),
        Err(existing) if existing == value => Ok(()),
        Err(existing) => bail!(
            "Cannot set no-cache to {}, already set to {}",
            value,
            existing
        ),
    }
}

/// Whether this run must neither read nor write rig's cache.
pub fn no_cache() -> bool {
    if let Some(cached) = NO_CACHE.get() {
        return *cached;
    }

    let value = if let Ok(val) = std::env::var("RIG_NO_CACHE") {
        match parse_bool(&val) {
            Some(val) => val,
            None => {
                warn!(
                    "Invalid RIG_NO_CACHE value: '{}', expected 'true' or 'false', ignoring it",
                    val
                );
                false
            }
        }
    } else {
        match crate::config::get_global_config_bool("no-cache") {
            Ok(val) => val.unwrap_or(false),
            Err(err) => {
                warn!("{}, ignoring it", err);
                false
            }
        }
    };

    let _ = NO_CACHE.set(value);
    value
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" | "" => Some(false),
        _ => None,
    }
}

/// Get the project cache directory
///
/// Returns the cache directory for the rig application.
/// This is used for storing temporary data like downloaded packages.
pub fn get_cache_dir() -> Result<PathBuf, Box<dyn Error>> {
    if no_cache() {
        return ephemeral_cache_dir();
    }
    real_cache_dir()
}

/// The persistent cache directory, whatever `--no-cache` says.
pub fn real_cache_dir() -> Result<PathBuf, Box<dyn Error>> {
    let cache_dir = ProjectDirs::from("com", "gaborcsardi", "rig")
        .ok_or("Cannot determine cache directory")?
        .cache_dir()
        .to_path_buf();
    Ok(cache_dir)
}

static EPHEMERAL_CACHE_DIR: OnceLock<PathBuf> = OnceLock::new();

fn ephemeral_cache_dir() -> Result<PathBuf, Box<dyn Error>> {
    if let Some(dir) = EPHEMERAL_CACHE_DIR.get() {
        return Ok(dir.clone());
    }

    let dir = std::env::temp_dir().join(ephemeral_cache_dir_name());
    create_download_dir_checked(&dir)?;
    debug!("Using the throwaway cache directory {}", dir.display());
    let _ = EPHEMERAL_CACHE_DIR.set(dir.clone());
    Ok(dir)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn ephemeral_cache_dir_name() -> String {
    format!(
        "rig-nocache-{}-{}",
        nix::unistd::geteuid().as_raw(),
        std::process::id()
    )
}

#[cfg(target_os = "windows")]
fn ephemeral_cache_dir_name() -> String {
    // `%TEMP%` is already per user on Windows, see `default_download_dir()`.
    format!("rig-nocache-{}", std::process::id())
}

pub fn cleanup_ephemeral_cache_dir() {
    let dir = match EPHEMERAL_CACHE_DIR.get() {
        Some(dir) => dir,
        None => return,
    };
    match std::fs::remove_dir_all(dir) {
        Ok(()) => debug!("Removed the throwaway cache directory {}", dir.display()),
        Err(err) => debug!(
            "Cannot remove the throwaway cache directory {}: {}",
            dir.display(),
            err
        ),
    }
}

/// Get the project data directory
///
/// Returns the data directory for the rig application.
/// This is used for storing persistent application data like configuration files.
pub fn get_data_dir() -> Result<PathBuf, Box<dyn Error>> {
    let data_dir = ProjectDirs::from("com", "gaborcsardi", "rig")
        .ok_or("Cannot determine data directory")?
        .data_dir()
        .to_path_buf();
    Ok(data_dir)
}

/// Get the project logs directory
///
/// Returns the appropriate logs directory for each platform:
/// - macOS: ~/Library/Logs/com.gaborcsardi.rig/
/// - Linux: ~/.cache/rig/logs/
/// - Windows: %LOCALAPPDATA%\gaborcsardi\rig\cache\logs\
pub fn get_logs_dir() -> Result<PathBuf, Box<dyn Error>> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").map_err(|_| "Cannot determine home directory")?;
        Ok(PathBuf::from(home).join("Library/Logs/com.gaborcsardi.rig"))
    }

    #[cfg(not(target_os = "macos"))]
    {
        // Use cache_dir for Linux/Windows
        let logs_dir = ProjectDirs::from("com", "gaborcsardi", "rig")
            .ok_or("Cannot determine logs directory")?
            .cache_dir()
            .join("logs");
        Ok(logs_dir.to_path_buf())
    }
}

/// Get the directory rig downloads installers and other temporary files into
///
/// It is not mode-aware on purpose, in admin mode we are UID 0 anyway.
///
/// It is also not under `get_cache_dir()`: that follows `HOME`, and the sudo
/// configurations that preserve `HOME` would then have root write into the
/// user's own cache directory.
///
/// This function does not touch the file system, see `ensure_download_dir()`
/// for that.
pub fn get_download_dir() -> Result<PathBuf, Box<dyn Error>> {
    if let Ok(val) = std::env::var("RIG_DOWNLOAD_DIR") {
        return Ok(PathBuf::from(val));
    }

    if let Some(val) = crate::config::get_global_config_value("download-dir")? {
        return Ok(PathBuf::from(val));
    }

    Ok(default_download_dir())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn default_download_dir() -> PathBuf {
    let euid = nix::unistd::geteuid().as_raw();
    std::env::temp_dir().join(format!("rig-{}", euid))
}

#[cfg(target_os = "windows")]
fn default_download_dir() -> PathBuf {
    // `%TEMP%` is already per user on Windows, and elevating with gsudo keeps
    // the same user profile, so there is nothing to disambiguate here.
    std::env::temp_dir().join("rig")
}

/// Whether `get_download_dir()` was overridden by the user
fn download_dir_is_overridden() -> Result<bool, Box<dyn Error>> {
    if std::env::var("RIG_DOWNLOAD_DIR").is_ok() {
        return Ok(true);
    }
    Ok(crate::config::get_global_config_value("download-dir")?.is_some())
}

/// Get the download directory, creating it if needed
pub fn ensure_download_dir() -> Result<PathBuf, Box<dyn Error>> {
    let dir = get_download_dir()?;

    if download_dir_is_overridden()? {
        create_dir(&dir)?;
        return Ok(dir);
    }

    create_download_dir_checked(&dir)?;
    Ok(dir)
}

fn create_dir(dir: &PathBuf) -> Result<(), Box<dyn Error>> {
    if let Err(err) = std::fs::create_dir_all(dir) {
        OUTPUT.error(&format!(
            "Cannot create download directory {}: {}",
            dir.display(),
            err
        ));
        error!(
            "Cannot create download directory {}: {}",
            dir.display(),
            err
        );
        bail!(
            "Cannot create download directory {}: {}",
            dir.display(),
            err.to_string()
        );
    }
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn create_download_dir_checked(dir: &PathBuf) -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::DirBuilderExt;
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::PermissionsExt;

    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    if let Err(err) = builder.create(dir) {
        OUTPUT.error(&format!(
            "Cannot create download directory {}: {}",
            dir.display(),
            err
        ));
        error!(
            "Cannot create download directory {}: {}",
            dir.display(),
            err
        );
        bail!(
            "Cannot create download directory {}: {}",
            dir.display(),
            err.to_string()
        );
    }

    // symlink_metadata(), not metadata(), so that a symlink pointing at a
    // directory we do own is still rejected.
    let meta = std::fs::symlink_metadata(dir)?;
    let mode = meta.permissions().mode() & 0o777;
    let bad = if !meta.is_dir() {
        Some("it is not a directory".to_string())
    } else if meta.uid() != nix::unistd::geteuid().as_raw() {
        Some(format!("it is owned by uid {}, not by us", meta.uid()))
    } else if mode & 0o022 != 0 {
        Some(format!(
            "its permissions ({:o}) let other users write into it",
            mode
        ))
    } else {
        None
    };

    if let Some(why) = bad {
        let msg = format!(
            "Refusing to use the download directory {}: {}. \
             rig installs the files it downloads there, so this is not safe. \
             Remove it, or set the RIG_DOWNLOAD_DIR environment variable (or \
             the `download-dir` config entry) to a directory you trust.",
            dir.display(),
            why
        );
        OUTPUT.error(&msg);
        error!("{}", msg);
        bail!("{}", msg);
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn create_download_dir_checked(dir: &PathBuf) -> Result<(), Box<dyn Error>> {
    // `%TEMP%` is inside the user's profile and is not shared with other
    // users, so there is nothing extra to check here.
    create_dir(dir)
}

// There are no tests for get_download_dir() here: it reads RIG_DOWNLOAD_DIR
// and the real config file of whoever runs the tests, and `cargo test` is a
// single multi-threaded process, so setting the environment would leak into
// the other tests. That part is covered by the BATS tests instead, which run a
// fresh rig process for every case. create_download_dir_checked() takes the
// directory as an argument, so it can be tested here.
#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn creates_a_private_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("rig-42");
        create_download_dir_checked(&dir).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);

        // and it accepts the directory it just created
        create_download_dir_checked(&dir).unwrap();
    }

    #[test]
    fn rejects_a_world_writable_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("rig-42");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        let err = create_download_dir_checked(&dir).unwrap_err().to_string();
        assert!(err.contains("let other users write into it"), "{}", err);
    }

    #[test]
    fn rejects_a_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("elsewhere");
        std::fs::create_dir(&target).unwrap();
        let dir = tmp.path().join("rig-42");
        std::os::unix::fs::symlink(&target, &dir).unwrap();
        let err = create_download_dir_checked(&dir).unwrap_err().to_string();
        assert!(err.contains("not a directory"), "{}", err);
    }
}
