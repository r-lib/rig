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

/// Archive suffixes a repository serves packages as. Matched whole, because a
/// package version contains dots (`pak_0.9.5.tgz`), so neither the first nor
/// the last `.` of a file name marks where its extension starts.
const ARCHIVE_SUFFIXES: [&str; 4] = [".tar.gz", ".tgz", ".zip", ".tar.bz2"];

/// Short identity of one artifact, for the cache file name.
///
/// `sha256` is the upstream CRAN source hash, which is the same on *every*
/// build of a version, so it identifies a version rather than a build; what
/// tells two builds of one version apart is `linkingto`, the dependency
/// versions the binary was compiled against. Both together identify the
/// artifact, and neither the P3M snapshot date nor the repository URL is part
/// of it, so the same build served at several snapshot dates shares one cache
/// entry.
///
/// Pass `linkingto` for a *binary* only. A source artifact also has a
/// `LinkingTo` provenance, but it describes what the tarball will be compiled
/// against later, not what the file is, and keying on it would cache a
/// separate copy of one tarball per solve.
pub(crate) fn artifact_cache_key(sha256: Option<&str>, linkingto: Option<&str>) -> Option<String> {
    let sha256 = sha256?;
    let hash = crate::utils::calculate_hash(&format!("{}\n{}", sha256, linkingto.unwrap_or("")));
    Some(hash[..8].to_string())
}

/// `name` with `key` inserted before its archive suffix.
fn keyed_file_name(name: &str, key: &str) -> String {
    match ARCHIVE_SUFFIXES.iter().find(|s| name.ends_with(**s)) {
        Some(suffix) => format!("{}-{}{}", &name[..name.len() - suffix.len()], key, suffix),
        None => format!("{}-{}", name, key),
    }
}

/// Where a downloaded artifact is put, relative to the download directory.
///
/// P3M URLs carry the repository layout the file belongs to
/// (`.../bin/macosx/big-sur-arm64/contrib/4.5/pak_0.9.5.tgz`), and the path
/// from the `src/` or `bin/` component onwards is the basis for the cache path.
/// Anything we cannot read that way falls back to `fallback`, a bare file name.
///
/// Two adjustments to that path:
///
/// * The `contrib` components are CRAN repository boilerplate and carry no
///   information, as does the second `src` that a Linux binary URL has after
///   the R version, so everything after the leading `src`/`bin` that is one of
///   those is dropped. What is left is what actually distinguishes targets:
///   OS, arch, R version.
/// * `key` goes into the file name, because the repository path is *not* unique
///   on its own: several binary builds share one `(version, platform, arch,
///   r_version)` and therefore one URL path, differing only in the snapshot
///   date that this path drops. See [`artifact_cache_key`].
pub(crate) fn target_path(url: &str, fallback: &str, key: Option<&str>) -> String {
    let path = url.split('?').next().unwrap_or(url);
    let mut pieces = path.split('/');
    let mut rest: Vec<&str> = vec![];
    for piece in pieces.by_ref() {
        if piece == "src" || piece == "bin" {
            rest.push(piece);
            break;
        }
    }
    if rest.is_empty() {
        return match key {
            Some(key) => keyed_file_name(fallback, key),
            None => fallback.to_string(),
        };
    }
    rest.extend(pieces.filter(|p| *p != "contrib" && *p != "src"));
    let keyed = match (key, rest.last()) {
        (Some(key), Some(file)) => Some(keyed_file_name(file, key)),
        _ => None,
    };
    if let Some(keyed) = &keyed {
        *rest.last_mut().unwrap() = keyed;
    }
    rest.join("/")
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

#[cfg(test)]
mod artifact_path_tests {
    use super::*;

    /// The `linkingto` of two real `dplyr 0.7.4` xenial rows, which differ in
    /// nothing else: same version, platform, arch, R version and `sha256`.
    const DPLYR_SHA: &str = "7b1fc90750fbb46483423da6721832c545d37b157f4f3355784a65e50fada8c2";
    const DPLYR_PLOGR_01: &str = "BH@1.66.0-1=17d9eb5512d74aa7dd02ec98953408422e728b01ce63493a6a473070b9596a92,Rcpp@0.12.16=d4e1636e53e2b656e173b49085b7abbb627981787cd63d63df325c713c83a8e6,bindrcpp@0.2=d0efa1313cb8148880f7902a4267de1dcedae916f28d9a0ef5911f44bf103450,plogr@0.1-1=22755c93c76c26252841f43195df31681ea865e91aa89726010bd1b9288ef48f";
    const DPLYR_PLOGR_02: &str = "BH@1.66.0-1=17d9eb5512d74aa7dd02ec98953408422e728b01ce63493a6a473070b9596a92,Rcpp@0.12.16=d4e1636e53e2b656e173b49085b7abbb627981787cd63d63df325c713c83a8e6,bindrcpp@0.2=d0efa1313cb8148880f7902a4267de1dcedae916f28d9a0ef5911f44bf103450,plogr@0.2.0=0e63ba2e1f624005fe25c67cdd403636a912e063d682eca07f2f1d65e9870d29";

    #[test]
    fn target_path_follows_the_repository_layout() {
        // `contrib` is dropped: it says nothing about which build this is.
        assert_eq!(
            target_path(
                "https://p3m.dev/cran/2026-04-27/bin/macosx/big-sur-arm64/contrib/4.5/pak_0.9.5.tgz",
                "fallback.tgz",
                None
            ),
            "bin/macosx/big-sur-arm64/4.5/pak_0.9.5.tgz"
        );
        assert_eq!(
            target_path(
                "https://p3m.dev/cran/2026-04-27/bin/windows/contrib/4.5/pak_0.9.5.zip",
                "fallback.zip",
                None
            ),
            "bin/windows/4.5/pak_0.9.5.zip"
        );
        assert_eq!(
            target_path(
                "https://p3m.dev/cran/2026-04-27/src/contrib/pak_0.9.5.tar.gz",
                "fallback.tar.gz",
                None
            ),
            "src/pak_0.9.5.tar.gz"
        );
        // Linux binaries live under a second `src/contrib`, and the first
        // component we recognise is the right one; the second one goes away
        // along with the `contrib`.
        assert_eq!(
            target_path(
                "https://p3m.dev/cran/2026-04-27/bin/linux/jammy-x86_64/4.5/src/contrib/pak_0.9.5.tar.gz",
                "fallback.tar.gz",
                None
            ),
            "bin/linux/jammy-x86_64/4.5/pak_0.9.5.tar.gz"
        );
        assert_eq!(
            target_path("https://example.com/pak.tgz", "fallback.tgz", None),
            "fallback.tgz"
        );
    }

    #[test]
    fn target_path_puts_the_key_in_the_file_name() {
        assert_eq!(
            target_path(
                "https://p3m.dev/cran/2026-04-27/bin/macosx/big-sur-arm64/contrib/4.5/pak_0.9.5.tgz",
                "fallback.tgz",
                Some("3f9a1c2e")
            ),
            "bin/macosx/big-sur-arm64/4.5/pak_0.9.5-3f9a1c2e.tgz"
        );
        // The fallback is a file name, so it is keyed too.
        assert_eq!(
            target_path(
                "https://example.com/pak.tgz",
                "pak_0.9.5.tar.gz",
                Some("3f9a1c2e")
            ),
            "pak_0.9.5-3f9a1c2e.tar.gz"
        );
    }

    #[test]
    fn builds_of_one_version_get_different_targets() {
        let url =
            "https://p3m.dev/cran/2018-03-15/bin/linux/xenial-x86_64/3.4/src/contrib/dplyr_0.7.4.tar.gz";
        let one = target_path(
            url,
            "fallback.tar.gz",
            artifact_cache_key(Some(DPLYR_SHA), Some(DPLYR_PLOGR_01)).as_deref(),
        );
        let two = target_path(
            url,
            "fallback.tar.gz",
            artifact_cache_key(Some(DPLYR_SHA), Some(DPLYR_PLOGR_02)).as_deref(),
        );
        assert_ne!(one, two);
    }

    #[test]
    fn one_build_at_two_snapshots_gets_one_target() {
        let key = artifact_cache_key(Some(DPLYR_SHA), Some(DPLYR_PLOGR_01));
        assert_eq!(
            target_path(
                "https://p3m.dev/cran/2018-03-15/bin/linux/xenial-x86_64/3.4/src/contrib/dplyr_0.7.4.tar.gz",
                "fallback.tar.gz",
                key.as_deref()
            ),
            target_path(
                "https://p3m.dev/cran/2018-03-27/bin/linux/xenial-x86_64/3.4/src/contrib/dplyr_0.7.4.tar.gz",
                "fallback.tar.gz",
                key.as_deref()
            )
        );
    }

    #[test]
    fn a_source_artifact_is_keyed_on_its_hash_alone() {
        // `from_solution` passes no `linkingto` for a source artifact, so every
        // solve of one version reuses the one cached tarball.
        assert_eq!(
            artifact_cache_key(Some(DPLYR_SHA), None),
            artifact_cache_key(Some(DPLYR_SHA), None)
        );
        assert_ne!(
            artifact_cache_key(Some(DPLYR_SHA), None),
            artifact_cache_key(Some(DPLYR_SHA), Some(DPLYR_PLOGR_01))
        );
    }

    #[test]
    fn without_a_hash_there_is_no_key() {
        assert_eq!(artifact_cache_key(None, Some(DPLYR_PLOGR_01)), None);
    }

    #[test]
    fn keyed_file_name_keeps_the_archive_suffix() {
        assert_eq!(
            keyed_file_name("pak_0.9.5.tar.gz", "3f9a1c2e"),
            "pak_0.9.5-3f9a1c2e.tar.gz"
        );
        assert_eq!(
            keyed_file_name("pak_0.9.5.tgz", "3f9a1c2e"),
            "pak_0.9.5-3f9a1c2e.tgz"
        );
        assert_eq!(
            keyed_file_name("pak_0.9.5.zip", "3f9a1c2e"),
            "pak_0.9.5-3f9a1c2e.zip"
        );
        // Nothing recognisable to cut before: the key is appended.
        assert_eq!(keyed_file_name("pak", "3f9a1c2e"), "pak-3f9a1c2e");
    }
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
