use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use futures::stream::{FuturesUnordered, StreamExt};
use log::debug;
use tokio::fs::create_dir_all;
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::output::{find_error, ReportedError, OUTPUT};

/// The `DESCRIPTION` field recording which artifact an installed package came
/// from: the sha256 of the upstream CRAN source tarball of its version.
pub const REMOTE_HASH_FIELD: &str = "RemoteHash";

/// The lock metadata field holding when a package of a CRAN-like repository
/// was built, from the `Built` field of the repository's `PACKAGES` file. It
/// is not written into the installed package: R writes its own `Built` field,
/// which is the same for a binary package. `rig pkg install` reinstalls a
/// package if the repository has a newer build of it.
pub const REPO_BUILT_FIELD: &str = "Built";

/// The `DESCRIPTION` field recording what the package was compiled against:
/// its `LinkingTo` dependencies as `pkg@version=sha256`, the same syntax P3M's
/// binary index uses.
pub const REMOTE_LINKINGTO_FIELD: &str = "RemoteLinkingToHashes";

/// `DESCRIPTION` fields recording the provenance of a git/GitHub/url-sourced
/// package, the same field names the R `remotes`/`pak`/`renv` packages use, so
/// that a package rig installed this way is recognizable by other R tooling
/// too. `REMOTE_TYPE_FIELD` is `"github"`, `"git"` or `"url"`;
/// `REMOTE_HOST_FIELD`, `REMOTE_REPO_FIELD` and `REMOTE_USERNAME_FIELD` are
/// only set for a GitHub source (a plain `git::`/`url::` source has no
/// owner/repo structure, only a URL).
pub const REMOTE_TYPE_FIELD: &str = "RemoteType";
pub const REMOTE_URL_FIELD: &str = "RemoteUrl";
pub const REMOTE_HOST_FIELD: &str = "RemoteHost";
pub const REMOTE_REPO_FIELD: &str = "RemoteRepo";
pub const REMOTE_USERNAME_FIELD: &str = "RemoteUsername";
pub const REMOTE_SUBDIR_FIELD: &str = "RemoteSubdir";
pub const REMOTE_REF_FIELD: &str = "RemoteRef";
pub const REMOTE_SHA_FIELD: &str = "RemoteSha";

/// Every `DESCRIPTION` field a git/GitHub-sourced install can write, for
/// [`drop_fields`]'s idempotency pass -- see [`patch_description`].
pub const REMOTE_GIT_FIELDS: &[&str] = &[
    REMOTE_TYPE_FIELD,
    REMOTE_URL_FIELD,
    REMOTE_HOST_FIELD,
    REMOTE_REPO_FIELD,
    REMOTE_USERNAME_FIELD,
    REMOTE_SUBDIR_FIELD,
    REMOTE_REF_FIELD,
    REMOTE_SHA_FIELD,
];

#[derive(Debug, Clone)]
pub struct PackageInfo {
    pub name: String,
    pub version: String,
    /// Whether `file_path` is a built package, which is unpacked into the
    /// library directly, or a source tarball, which needs `R CMD INSTALL`.
    pub binary: bool,
    pub file_path: PathBuf,
    pub dependencies: Vec<String>,
    /// Value for the [`REMOTE_HASH_FIELD`] field, when the metadata knew one.
    pub hash: Option<String>,
    /// Value for the [`REMOTE_LINKINGTO_FIELD`] field, as
    /// `(package, version, sha256)`. Empty for a package without `LinkingTo:`.
    pub linkingto: Vec<(String, String, String)>,
    /// Where a source package's own build is cached, see [`crate::built`]. Read
    /// before compiling and written after, so that a package is only ever
    /// compiled once per build. `None` for a binary, and for a source package
    /// whose build cannot be identified.
    pub built: Option<PathBuf>,
    /// Git/GitHub provenance, i.e. the [`REMOTE_GIT_FIELDS`] the lockfile's
    /// `metadata` carried, if this package came from a git/GitHub source.
    /// Empty for an ordinary CRAN/PPM package.
    pub remote: HashMap<String, String>,
}

/// Install one R package into a library.
///
/// A built package is unpacked into the library and R is never started; only a
/// source tarball goes through `R CMD INSTALL`. A built package whose archive
/// does not have the expected layout falls back to `R CMD INSTALL` too, rather
/// than failing: the repository, not rig, decides what a "binary" is.
///
/// A source package that rig has already compiled once is unpacked from
/// [`crate::built`] instead of compiled again, which is the same operation on
/// the same kind of archive; a cache entry that does not unpack is ignored, and
/// the package compiled.
///
/// # Arguments
/// * `pkg` - The package to install, and the provenance to record in it
/// * `library_path` - Path to the R library directory where the package should be installed
/// * `r_binary` - Path to the R binary to use for source installations
/// * `print_fn` - Optional custom print function (e.g., for progress bars). If None, uses OUTPUT.
pub async fn install_package<F>(
    pkg: &PackageInfo,
    library_path: &Path,
    r_binary: &str,
    print_fn: Option<Arc<F>>,
) -> Result<(), Box<dyn Error>>
where
    F: Fn(&str) + Send + Sync + 'static,
{
    // Guard against malicious package names (e.g. "../victim") from the
    // package's `DESCRIPTION` file.
    if !crate::utils::is_r_package_name(&pkg.name) {
        let msg = format!(
            "Failed to install {}: {:?} is not a valid R package name",
            pkg.file_path.display(),
            pkg.name
        );
        OUTPUT.error(&msg);
        return Err(Box::new(ReportedError(msg)));
    }

    let archive = if pkg.binary {
        Some(pkg.file_path.clone())
    } else {
        pkg.built.clone().filter(|path| path.exists())
    };

    if let Some(archive) = archive {
        // A binary's archives all sit together in one `packages` directory
        // (see `lockfile_package_path`), so the shared unpack cache is keyed
        // on the archive's own file name; a rig-built package already lives
        // in its own hash-keyed directory, so its name alone is unique there.
        let key = if pkg.binary {
            archive
                .file_name()
                .and_then(|x| x.to_str())
                .map(String::from)
                .unwrap_or_else(|| pkg.name.clone())
        } else {
            pkg.name.clone()
        };
        let result = install_from_cache(pkg, &archive, &key, library_path);
        match result {
            Ok(()) => {
                let msg = format!("Installed {} {}", pkg.name, pkg.version);
                match print_fn {
                    Some(ref print) => print(&msg),
                    None => OUTPUT.success(&msg),
                }
                debug!(
                    "Installed built package {} {} from {} into {}",
                    pkg.name,
                    pkg.version,
                    archive.display(),
                    library_path.display()
                );
                return Ok(());
            }
            Err(err) => {
                // Not fatal: `R CMD INSTALL` copes with more layouts than we
                // do, so let it try before giving up on the package.
                debug!(
                    "Cannot unpack {} as a built package ({}), falling back to R CMD INSTALL",
                    pkg.name, err
                );
            }
        }
    }

    // `R CMD INSTALL` failures are shown with the path of their log, but an
    // error before or after it (e.g. a library we cannot write to) would only
    // be logged, so show that here.
    // Show it right away, because the other packages are still installing,
    // and return it as already shown.
    r_cmd_install(pkg, library_path, r_binary, print_fn)
        .await
        .map_err(|err| {
            if find_error::<ReportedError>(err.as_ref()).is_some() {
                return err;
            }
            let msg = format!("Failed to install {}: {}", pkg.name, err);
            OUTPUT.error(&msg);
            Box::new(ReportedError(msg)) as Box<dyn Error>
        })
}

/// Install a built package into the library, without starting R.
///
/// The archive is only ever decompressed once: [`crate::built::ensure_unpacked`]
/// extracts it into a shared cache directory next to it the first time this
/// exact archive is installed anywhere, and every install after that —
/// including into other projects' libraries — [`link_package_tree`]s the
/// package's files out of that shared copy instead of extracting the archive
/// again. The same binary or build is commonly installed into many projects'
/// libraries unchanged, so most of the work of an install — decompressing and
/// writing out every byte — is repeated for nothing without this.
///
/// `archive` is passed separately from `pkg`, because it is either the
/// binary artifact the solve picked or rig's own earlier build of the same
/// package. `key` identifies the shared unpack cache entry, see
/// [`crate::built::ensure_unpacked`].
///
/// Errors if the archive is not a single directory named after the package,
/// which is what a built package always is, and what a source tarball
/// masquerading as one is not.
fn install_from_cache(
    pkg: &PackageInfo,
    archive: &Path,
    key: &str,
    library_path: &Path,
) -> Result<(), Box<dyn Error>> {
    let unpacked = crate::built::ensure_unpacked(archive, key, &pkg.name)?;

    // `staging` must not exist yet: [`link_package_tree`] creates it, since
    // the fastest way to populate it (a whole-directory reflink on macOS)
    // requires the destination to not already exist.
    let temp_lib = TempLibrary::create(library_path)?;
    let staging = &temp_lib.path().join(&pkg.name);
    link_package_tree(&unpacked, staging)?;

    // `DESCRIPTION` always gets a private, patched copy: `patch_description`
    // records per-install provenance, and the destination copy may share
    // bytes (a hard link) or an inode's worth of on-disk blocks (a reflink)
    // with the cache, so it is replaced outright rather than edited in place.
    std::fs::remove_file(staging.join("DESCRIPTION"))?;
    std::fs::copy(unpacked.join("DESCRIPTION"), staging.join("DESCRIPTION"))?;
    patch_description(staging, pkg)?;

    move_into_library(&temp_lib, library_path, &pkg.name)
}

/// Replace the `name` package in `library_path` with the complete package
/// directory `<temp_lib>/<name>`.
///
/// The old package is renamed to `<temp_lib>/.old` first, then the new one is
/// renamed into place, and only then is the old one deleted. Renaming is
/// quick, so the window where the library has no `name` package is only the
/// time between the two renames, not the time it takes to delete every file
/// of the old package. If rig is killed in that window, the old package is
/// still in `.old`. `temp_lib` must be inside `library_path`, so that both
/// are on the same filesystem. The leading `.` keeps `.old` from clashing with
/// a package called `old`.
///
/// Another process (e.g. another rig installing into the same library) may
/// move its own copy of the package in between the two renames. Then the
/// second rename fails, because the target exists again, and this tries again:
/// it moves that copy aside too (to `.old-1`, `.old-2`, ...), and then moves
/// its own in. So the install that finishes last wins, without a lock.
///
/// If the new package cannot be moved in, the most recent old one is moved
/// back, unless another process has already put its own copy in place. If
/// moving it back fails, `temp_lib` is kept, so that the old package is not
/// deleted with it, and the error says where it is.
fn move_into_library(
    temp_lib: &TempLibrary,
    library_path: &Path,
    name: &str,
) -> Result<(), Box<dyn Error>> {
    const ATTEMPTS: usize = 3;
    let staged = temp_lib.path().join(name);
    let target = library_path.join(name);
    // Where the old package(s) went, the most recent one last.
    let mut asides: Vec<PathBuf> = Vec::new();

    let mut attempt = 0;
    let result = loop {
        let aside = match attempt {
            0 => temp_lib.path().join(".old"),
            n => temp_lib.path().join(format!(".old-{}", n)),
        };
        match rename_retrying(&target, &aside) {
            Ok(()) => asides.push(aside),
            // No old package, or another process has just moved it away.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                break Err(format!(
                    "cannot move {} to {}: {}",
                    target.display(),
                    aside.display(),
                    err
                ))
            }
        }
        match rename_retrying(&staged, &target) {
            Ok(()) => break Ok(()),
            // The error kind for renaming onto an existing directory varies
            // between platforms, so look at the target instead.
            Err(_) if attempt + 1 < ATTEMPTS && target.exists() => {
                debug!(
                    "{} appeared while installing it, moving it aside again",
                    target.display()
                );
                attempt += 1;
            }
            Err(err) => {
                break Err(format!(
                    "cannot move {} to {}: {}",
                    staged.display(),
                    target.display(),
                    err
                ))
            }
        }
    };

    if let Err(mut msg) = result {
        // Put the old package back, so a failed install leaves it as it was.
        // If another process has put its copy in place, that is newer anyway.
        if let Some(aside) = asides.last() {
            if !target.exists() {
                if let Err(err) = rename_retrying(aside, &target) {
                    temp_lib.keep();
                    msg.push_str(&format!(
                        "; cannot move the previous version back from {}: {}",
                        aside.display(),
                        err
                    ));
                }
            }
        }
        return Err(msg.into());
    }

    // `temp_lib` is deleted later anyway, but the old package can be large,
    // and this frees the space right away. Not fatal: the new package is in
    // place. On Windows this fails if a running R has a DLL of the old package
    // loaded; the file can be renamed, but not deleted.
    for aside in &asides {
        if let Err(err) = std::fs::remove_dir_all(aside) {
            debug!("Cannot remove {}: {}", aside.display(), err);
        }
    }
    Ok(())
}

/// `std::fs::rename`, but on Windows it tries again for a while if the rename
/// fails because a file is in use. Antivirus software and the search indexer
/// open newly written files to scan them, and while they do, the files and
/// the directories that hold them cannot be renamed. This is common right
/// after a package was built.
///
/// It does not try again if `to` exists: that is not going to go away by
/// waiting, and [`move_into_library`] handles it itself.
fn rename_retrying(from: &Path, to: &Path) -> std::io::Result<()> {
    retry_transient(
        || std::fs::rename(from, to),
        |err| is_file_in_use(err) && !to.exists(),
    )
}

/// Whether `err` is Windows refusing to rename or delete a file because some
/// process has it open. Never true on other systems, where open files do not
/// stop either.
fn is_file_in_use(err: &std::io::Error) -> bool {
    // ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION
    cfg!(windows) && matches!(err.raw_os_error(), Some(5) | Some(32))
}

/// Run `op`, and run it again, with increasing waits in between, as long as
/// it fails with an error that `transient` says may go away. It waits about
/// 1.3 seconds in total before it gives up and returns the last error.
///
/// The waits block the thread. They only happen when something has already
/// gone wrong, and they are short, so this is fine in an async install task.
fn retry_transient<T>(
    mut op: impl FnMut() -> std::io::Result<T>,
    transient: impl Fn(&std::io::Error) -> bool,
) -> std::io::Result<T> {
    const RETRIES: u32 = 7;
    let mut wait = std::time::Duration::from_millis(10);
    let mut retries = 0;
    loop {
        match op() {
            Err(err) if retries < RETRIES && transient(&err) => {
                debug!("{}, trying again in {:?}", err, wait);
                std::thread::sleep(wait);
                wait *= 2;
                retries += 1;
            }
            result => return result,
        }
    }
}

/// A temporary library inside the real one, deleted when dropped. A package
/// is built (`R CMD INSTALL`) or staged (from the cache) into it, then
/// [`move_into_library`] moves it into the real library.
struct TempLibrary {
    path: PathBuf,
    /// Set by [`TempLibrary::keep`]: leave the directory on disk when dropped.
    keep: std::cell::Cell<bool>,
}

impl TempLibrary {
    /// Create `<library_path>/.rig-<pid>-<n>`, where `n` counts the temporary
    /// libraries of this process, since rig installs several packages at once.
    /// The name is short because Windows limits path lengths, and every path
    /// of the package is under it while it is built. A leading `.` keeps `rig
    /// pkg list` and R from reading it as a package.
    fn create(library_path: &Path) -> Result<TempLibrary, Box<dyn Error>> {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = library_path.join(format!(".rig-{}-{}", std::process::id(), n));
        // Left behind by an earlier process with the same pid.
        if path.exists() {
            std::fs::remove_dir_all(&path)
                .map_err(|err| format!("cannot remove {}: {}", path.display(), err))?;
        }
        std::fs::create_dir(&path)
            .map_err(|err| format!("cannot create {}: {}", path.display(), err))?;
        Ok(TempLibrary {
            path,
            keep: std::cell::Cell::new(false),
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    /// Do not delete the directory when dropped, because it holds something
    /// that must not be lost, e.g. the only copy of a previous install.
    fn keep(&self) {
        self.keep.set(true);
    }
}

impl Drop for TempLibrary {
    fn drop(&mut self) {
        if !self.keep.get() {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// The pid of the rig process that created `name` in a library, if `name` is
/// one of rig's temporary directories: a [`TempLibrary`]
/// (`.rig-<pid>-<n>`), or a staging directory of rig 0.10 and 0.11
/// (`.rig-staging-<pkg>-<pid>`; package names cannot contain `-`).
fn temp_library_pid(name: &str) -> Option<u32> {
    if let Some(rest) = name.strip_prefix(".rig-staging-") {
        let (_pkg, pid) = rest.rsplit_once('-')?;
        return pid.parse().ok();
    }
    let (pid, n) = name.strip_prefix(".rig-")?.split_once('-')?;
    n.parse::<usize>().ok()?;
    pid.parse().ok()
}

/// Remove the temporary directories that rig processes that no longer run
/// left in `library_path`, e.g. because they were killed in the middle of an
/// install. Such a directory may hold a complete build, so it can be large.
///
/// A directory of a running process is left alone: it may be installing into
/// the same library right now. A reused pid only means that a directory is
/// removed later. Not fatal: errors are only logged.
fn remove_stale_temp_libraries(library_path: &Path) {
    let entries = match std::fs::read_dir(library_path) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    let me = std::process::id();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let pid = match name.to_str().and_then(temp_library_pid) {
            Some(pid) => pid,
            None => continue,
        };
        if pid == me || process_is_running(pid) {
            continue;
        }
        let path = entry.path();
        match std::fs::remove_dir_all(&path) {
            Ok(()) => debug!("Removed stale temporary library {}", path.display()),
            Err(err) => debug!("Cannot remove {}: {}", path.display(), err),
        }
    }
}

/// Whether a process with `pid` is running. When in doubt, e.g. because the
/// process belongs to another user, it is.
#[cfg(unix)]
fn process_is_running(pid: u32) -> bool {
    let pid = match i32::try_from(pid) {
        Ok(pid) if pid > 0 => pid,
        _ => return true,
    };
    // SAFETY: signal 0 only checks whether the process exists.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

/// Whether a process with `pid` is running. When in doubt, e.g. because the
/// process belongs to another user, it is.
#[cfg(windows)]
fn process_is_running(pid: u32) -> bool {
    use std::ffi::c_void;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const ERROR_INVALID_PARAMETER: i32 = 87;
    const STILL_ACTIVE: u32 = 259;
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn GetExitCodeProcess(process: *mut c_void, code: *mut u32) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    // SAFETY: plain calls on a process handle that is closed before returning.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            // `ERROR_INVALID_PARAMETER` means there is no process with this
            // pid. Anything else, e.g. access denied, means there is one.
            return std::io::Error::last_os_error().raw_os_error() != Some(ERROR_INVALID_PARAMETER);
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        // An exited process can still have a handle open somewhere.
        ok == 0 || code == STILL_ACTIVE
    }
}

/// `R_LIBS` for an `R CMD INSTALL` that builds into a [`TempLibrary`]: the
/// real library first, so that the package's dependencies are found there,
/// then whatever `R_LIBS` already was. `R CMD INSTALL` puts its `-l` library
/// in front of this itself.
fn build_r_libs(library_path: &Path, current: Option<&str>) -> String {
    let sep = if cfg!(windows) { ";" } else { ":" };
    let lib = crate::utils::r_path(library_path).display().to_string();
    match current {
        Some(current) if !current.is_empty() => format!("{}{}{}", lib, sep, current),
        _ => lib,
    }
}

/// Populate `dst`, which must not exist yet, with everything under `src`,
/// using the cheapest mechanism the filesystem allows.
///
/// macOS's `clonefile` reflinks a whole directory tree in one call
/// (`reflink_copy::reflink`'s own docs), so `src` is reflinked directly there.
/// Every other platform's reflink only works file by file, so elsewhere `dst`
/// is built up directory by directory, reflinking (falling back to a hard
/// link, falling back to a plain copy) one file at a time; see [`link_file`].
fn link_package_tree(src: &Path, dst: &Path) -> Result<(), Box<dyn Error>> {
    if cfg!(target_os = "macos") && reflink_copy::reflink(src, dst).is_ok() {
        return Ok(());
    }
    link_tree_entries(src, dst)
}

/// Populate `dst`, which must not exist yet, with everything under `src`,
/// for an out-of-tree build of a local path package.
///
/// Unlike [`link_package_tree`], this never hard-links a file: a hard link
/// shares an inode, so a write to the copy (e.g. a compiler dropping a `.o`
/// file, or `configure` regenerating `src/Makevars`) would land in the user's
/// original file too, which is exactly what staging is meant to avoid. A
/// reflink is fine -- it's copy-on-write, so the two sides diverge on the
/// first write to either. `.git` is skipped: it's never needed to build the
/// package and can be large. So is a top level `.rvenv`: when the package is
/// the project's own package, `rig proj sync` installs the dependencies into
/// `.rvenv/lib` at the same time, and copying files that R creates and
/// deletes there fails.
fn stage_local_source(src: &Path, dst: &Path) -> Result<(), Box<dyn Error>> {
    if cfg!(target_os = "macos")
        && !src.join(crate::rvenv::RVENV_DIR).exists()
        && reflink_copy::reflink(src, dst).is_ok()
    {
        return Ok(());
    }
    stage_local_source_entries(src, dst, true)
}

/// A path from the raw bytes `git ls-files -z` prints. Git allows a file name
/// that is not UTF-8 on Unix.
fn path_from_git(bytes: &[u8]) -> Result<PathBuf, Box<dyn Error>> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
    }
    #[cfg(not(unix))]
    {
        Ok(PathBuf::from(std::str::from_utf8(bytes)?))
    }
}

/// Stage a package directory in a git checkout (the package may be in a
/// subdirectory of the checkout) from the files git tracks, leaving out
/// anything an earlier build left behind.
fn stage_tracked_source(src: &Path, dst: &Path) -> Result<(), Box<dyn Error>> {
    let stdout = crate::pkgsource::git::run_git_raw(src, &["ls-files", "-z", "--cached"])?;
    let tracked = stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(path_from_git)
        .collect::<Result<Vec<_>, _>>()?;
    if !tracked.iter().any(|rel| rel == Path::new("DESCRIPTION")) {
        return Err(format!("git does not track a DESCRIPTION file in {}", src.display()).into());
    }

    std::fs::create_dir_all(dst)?;
    for rel in tracked {
        let from = src.join(&rel);
        let to = dst.join(&rel);
        let file_type = match std::fs::symlink_metadata(&from) {
            Ok(meta) => meta.file_type(),
            Err(_) => continue,
        };
        if file_type.is_dir() {
            continue;
        }
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if file_type.is_symlink() {
            recreate_symlink(&from, &to)?;
        } else if reflink_copy::reflink(&from, &to).is_err() {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn stage_local_source_entries(src: &Path, dst: &Path, top: bool) -> Result<(), Box<dyn Error>> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        if entry.file_name() == ".git" || (top && entry.file_name() == crate::rvenv::RVENV_DIR) {
            continue;
        }
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            stage_local_source_entries(&from, &to, false)?;
        } else if file_type.is_symlink() {
            recreate_symlink(&from, &to)?;
        } else if reflink_copy::reflink(&from, &to).is_err() {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn link_tree_entries(src: &Path, dst: &Path) -> Result<(), Box<dyn Error>> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        // `file_type()` does not follow symlinks, so a symlink is recreated as
        // itself rather than walked into or linked as a regular file.
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            link_tree_entries(&from, &to)?;
        } else if file_type.is_symlink() {
            recreate_symlink(&from, &to)?;
        } else {
            link_file(&from, &to)?;
        }
    }
    Ok(())
}

/// Link (or copy) one regular file: a reflink first (copy-on-write, so a
/// later write to either side never touches the other), falling back to a
/// hard link (no bytes copied, but a shared inode — a write to either side is
/// visible on both, which is why callers never write to a linked file in
/// place), falling back to a plain copy, which always works.
fn link_file(from: &Path, to: &Path) -> Result<(), Box<dyn Error>> {
    if reflink_copy::reflink(from, to).is_ok() {
        return Ok(());
    }
    if std::fs::hard_link(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)?;
    Ok(())
}

#[cfg(unix)]
fn recreate_symlink(from: &Path, to: &Path) -> Result<(), Box<dyn Error>> {
    let target = std::fs::read_link(from)?;
    std::os::unix::fs::symlink(target, to)?;
    Ok(())
}

#[cfg(windows)]
fn recreate_symlink(from: &Path, to: &Path) -> Result<(), Box<dyn Error>> {
    let target = std::fs::read_link(from)?;
    if from.is_dir() {
        std::os::windows::fs::symlink_dir(target, to)?;
    } else {
        std::os::windows::fs::symlink_file(target, to)?;
    }
    Ok(())
}

/// Extract a package archive into `dest`. The format (zip, or a tarball
/// compressed with gzip, zstd or xz) is detected from the file's leading
/// magic bytes, not from its name or the current platform, so a
/// `--platform` other than this machine's still does the right thing.
pub(crate) fn unpack_package(archive: &Path, dest: &Path) -> Result<(), Box<dyn Error>> {
    use std::io::Read;

    let mut header = [0u8; 6];
    let n = std::fs::File::open(archive)?.read(&mut header)?;
    let header = &header[..n];

    let file = std::fs::File::open(archive)?;
    if header.starts_with(&[0x50, 0x4B]) {
        let mut ar = zip::ZipArchive::new(file)?;
        ar.extract(dest)?;
        crate::utils::normalize_modes(dest)?;
    } else if header.starts_with(&[0x1F, 0x8B]) {
        unpack_tar(flate2::read::GzDecoder::new(file), dest)?;
    } else if header.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]) {
        unpack_tar(zstd::stream::Decoder::new(file)?, dest)?;
    } else if header.starts_with(&[0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00]) {
        unpack_tar(xz2::read::XzDecoder::new(file), dest)?;
    } else {
        bail!("unrecognized archive format: {}", archive.display());
    }
    Ok(())
}

/// Unpack a tarball entry by entry, giving each entry the mode R gives an
/// installed package (see [`crate::utils::normalize_modes`]) as it goes, so
/// there is no second pass over the tree. The mask covers the common case of
/// modes that are too wide; only an entry whose mode is too narrow (e.g. a
/// 0600 file) needs a `chmod` afterwards.
fn unpack_tar<R: std::io::Read>(reader: R, dest: &Path) -> Result<(), Box<dyn Error>> {
    let mut ar = tar::Archive::new(reader);
    ar.set_preserve_permissions(true);
    ar.set_mask(0o7022);
    ar.set_overwrite(true);
    std::fs::create_dir_all(dest)?;
    let dest = dest.canonicalize()?;
    for entry in ar.entries()? {
        let mut entry = entry?;
        if !entry.unpack_in(&dest)? {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let kind = entry.header().entry_type();
            let Ok(mode) = entry.header().mode() else {
                continue;
            };
            let want = if kind.is_dir() {
                0o755
            } else if kind.is_file() {
                (mode | 0o644) & 0o755
            } else {
                continue;
            };
            if mode & 0o755 != want {
                // The same path `unpack_in` wrote to: root and `.`
                // components are dropped, and `..` was already refused.
                let mut path = dest.clone();
                for part in entry.path()?.components() {
                    if let std::path::Component::Normal(part) = part {
                        path.push(part);
                    }
                }
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(want))?;
            }
        }
    }
    Ok(())
}

/// The single directory `dir` contains, erroring if it holds anything else.
pub(crate) fn single_subdir(dir: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let entries: Vec<std::fs::DirEntry> = std::fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    if entries.len() != 1 || !entries[0].path().is_dir() {
        bail!(
            "expected a single package directory, found {} entries",
            entries.len()
        );
    }
    Ok(entries[0].path())
}

/// Record in an installed package's `DESCRIPTION` which artifact it came from.
///
/// Writes [`REMOTE_HASH_FIELD`] and, for a package with `LinkingTo:`,
/// [`REMOTE_LINKINGTO_FIELD`]. `rig pkg install` reads these back to decide
/// whether an installed package is still the one the solve asked for: a
/// different hash, or a `LinkingTo` dependency that has been upgraded since,
/// means the installed package has to be replaced.
///
/// The fields are appended as text, and any previous copy of them is dropped,
/// so the rest of the file is left byte for byte as the package built it.
/// `Meta/package.rds` is deliberately not touched — rig reads `DESCRIPTION`
/// itself — which is why `packageDescription()` in R shows these fields but
/// `installed.packages()` does not.
fn patch_description(pkg_dir: &Path, pkg: &PackageInfo) -> Result<(), Box<dyn Error>> {
    let path = pkg_dir.join("DESCRIPTION");
    let text = std::fs::read_to_string(&path)
        .map_err(|err| format!("cannot read {}: {}", path.display(), err))?;
    let mut drop = vec![REMOTE_HASH_FIELD, REMOTE_LINKINGTO_FIELD];
    drop.extend_from_slice(REMOTE_GIT_FIELDS);
    let mut out = drop_fields(&text, &drop);

    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if let Some(hash) = &pkg.hash {
        out.push_str(&format!("{}: {}\n", REMOTE_HASH_FIELD, hash));
    }
    if !pkg.linkingto.is_empty() {
        out.push_str(&format!(
            "{}: {}\n",
            REMOTE_LINKINGTO_FIELD,
            format_linkingto(&pkg.linkingto)
        ));
    }
    for field in REMOTE_GIT_FIELDS {
        if let Some(value) = pkg.remote.get(*field) {
            out.push_str(&format!("{}: {}\n", field, value));
        }
    }

    std::fs::write(&path, out)
        .map_err(|err| format!("cannot write {}: {}", path.display(), err))?;
    Ok(())
}

/// `LinkingTo` provenance as it goes into `DESCRIPTION`: P3M's own
/// `pkg@version=sha256` syntax, comma separated.
pub fn format_linkingto(linkingto: &[(String, String, String)]) -> String {
    linkingto
        .iter()
        .map(|(pkg, ver, sha)| format!("{}@{}={}", pkg, ver, sha))
        .collect::<Vec<String>>()
        .join(", ")
}

/// Read back what [`format_linkingto`] wrote.
///
/// An entry that is not `pkg@version=sha256` is dropped rather than reported.
/// That is the safe direction: a package whose provenance we cannot read looks
/// like one with none, and a package with no provenance gets reinstalled.
pub fn parse_linkingto(value: &str) -> Vec<(String, String, String)> {
    let mut out = vec![];
    for entry in value.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        match entry.split_once('@').and_then(|(pkg, rest)| {
            rest.split_once('=')
                .map(|(ver, sha)| (pkg.to_string(), ver.to_string(), sha.to_string()))
        }) {
            Some(parsed) => out.push(parsed),
            None => debug!(
                "Ignoring unparseable {} entry '{}'",
                REMOTE_LINKINGTO_FIELD, entry
            ),
        }
    }
    out
}

/// Remove whole DCF fields from `text`, continuation lines and all.
///
/// A DCF field runs until the next line that starts in column one, so dropping
/// a field means dropping its indented continuation lines too — otherwise they
/// would be reparsed as part of whichever field happens to precede them.
fn drop_fields(text: &str, fields: &[&str]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut skipping = false;
    for line in text.split_inclusive('\n') {
        let continuation = line.starts_with(' ') || line.starts_with('\t');
        if continuation {
            if !skipping {
                out.push_str(line);
            }
            continue;
        }
        skipping = fields.iter().any(|f| {
            line.len() > f.len()
                && line.as_bytes()[f.len()] == b':'
                && line[..f.len()].eq_ignore_ascii_case(f)
        });
        if !skipping {
            out.push_str(line);
        }
    }
    out
}

/// Install a source package with `R CMD INSTALL`, and record the provenance in
/// the result.
///
/// The package is built into a [`TempLibrary`] and only then replaces any
/// previous install in `library_path`, the same way a built package does.
async fn r_cmd_install<F>(
    pkg: &PackageInfo,
    library_path: &Path,
    r_binary: &str,
    print_fn: Option<Arc<F>>,
) -> Result<(), Box<dyn Error>>
where
    F: Fn(&str) + Send + Sync + 'static,
{
    let package_name: &str = &pkg.name;
    let package_path: &Path = &pkg.file_path;
    debug!(
        "Installing package {} from {} to {}",
        package_name,
        package_path.display(),
        library_path.display()
    );

    let logs_dir = library_path.join("_logs");
    create_dir_all(&logs_dir)
        .await
        .map_err(|err| format!("cannot create {}: {}", logs_dir.display(), err))?;

    let log_file_path = logs_dir.join(format!("{}-install.log", package_name));
    let log_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&log_file_path)
        .map_err(|err| format!("cannot open {}: {}", log_file_path.display(), err))?;

    let log_file_stderr = log_file.try_clone()?;

    // Run from a fresh temporary directory, so R does not pick up a project
    // `.Renviron` (or `.Rprofile`) that could redirect it to a project package
    // library instead of `library_path`. Canonicalize the paths passed on the
    // command line first, since they may otherwise be relative to rig's own
    // working directory rather than `run_dir`.
    let run_dir =
        tempfile::tempdir().map_err(|err| format!("cannot create temporary directory: {}", err))?;
    let library_path_abs = library_path
        .canonicalize()
        .map_err(|err| format!("cannot find {}: {}", library_path.display(), err))?;
    let package_path_abs = package_path
        .canonicalize()
        .map_err(|err| format!("cannot find {}: {}", package_path.display(), err))?;

    // `R CMD INSTALL` builds a source directory in place and leaves the
    // compiled artifacts (`.o`/`.so`, a generated `src/Makevars`, `config.log`,
    // ...) behind, we want to build directories from a copy -- a copy of the
    // tracked sources for a git checkout, and a regular copy for anything else
    // (a local directory, or an extracted `url::` archive in the cache). The
    // copy goes into this build's own temporary directory, so that concurrent
    // builds of the same source never share it.
    //
    // A tarball/zip or an already-built binary is unaffected -- R (or rig's
    // own unpacker) already extracts those into a throwaway location.
    let remote_type = pkg.remote.get(REMOTE_TYPE_FIELD).map(String::as_str);
    let is_git = matches!(remote_type, Some("git") | Some("github"));
    let package_path_abs = if package_path_abs.is_dir() {
        let staged = run_dir.path().join(package_name);
        debug!(
            "Staging package {} from {} to {} for an out-of-tree build",
            package_name,
            package_path_abs.display(),
            staged.display()
        );
        // Running git and copying the files block, so do them off the async
        // runtime, like the build below.
        let copied = {
            let src = package_path_abs.clone();
            let dst = staged.clone();
            tokio::task::spawn_blocking(move || -> Result<(), String> {
                let copied = if is_git {
                    stage_tracked_source(&src, &dst)
                } else {
                    stage_local_source(&src, &dst)
                };
                copied.map_err(|err| err.to_string())
            })
            .await
            .map_err(|err| format!("copying task failed: {}", err))?
        };
        copied.map_err(|err| {
            let msg = format!(
                "Failed to install {}: could not copy {} to {}: {}",
                package_name,
                package_path_abs.display(),
                staged.display(),
                err
            );
            OUTPUT.error(&msg);
            ReportedError(msg)
        })?;
        staged
    } else {
        package_path_abs
    };

    // Build into an empty library and move the package into the real one only
    // once it is complete.
    // `--no-lock`: the build library is private to this install #525
    let build_lib = TempLibrary::create(&library_path_abs)?;
    let build_lib_r = crate::utils::r_path(build_lib.path());
    let r_libs = build_r_libs(&library_path_abs, std::env::var("R_LIBS").ok().as_deref());

    let package_path_r = crate::utils::r_path(&package_path_abs);
    let status = Command::new(r_binary)
        .arg("CMD")
        .arg("INSTALL")
        .arg("--no-lock")
        .arg("-l")
        .arg(&build_lib_r)
        .arg(&package_path_r)
        .env("R_LIBS", &r_libs)
        .current_dir(run_dir.path())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_file_stderr))
        .status()
        .await
        .map_err(|err| format!("cannot run {}: {}", r_binary, err))?;

    if status.success() {
        // Before `patch_description`, so that the cached archive is what the
        // package built, and not what rig recorded in it. The provenance fields
        // are added on every install anyway, including on the installs that come
        // out of this cache.
        cache_build(pkg, build_lib.path()).await;

        let built = build_lib.path().join(package_name);
        patch_description(&built, pkg)?;
        move_into_library(&build_lib, &library_path_abs, package_name)?;

        // User output: Use custom print function if provided, otherwise use OUTPUT
        let msg = format!("Installed {} {}", package_name, pkg.version);
        if let Some(ref print) = print_fn {
            print(&msg);
        } else {
            OUTPUT.success(&msg);
        }

        debug!(
            "Successfully installed package {} to {} (log: {})",
            package_name,
            library_path.display(),
            log_file_path.display()
        );

        Ok(())
    } else {
        // User output: Always use OUTPUT for errors (they should be visible)
        // A missing system library is the most common reason a source
        // package fails to compile.
        let missing = crate::sysreqs::still_missing(package_name);
        let mut hint = if missing.is_empty() {
            String::new()
        } else {
            format!("\n  Missing system packages: {}", missing.join(", "))
        };
        if let Some(long) = crate::utils::long_path_hint(&[&build_lib_r, &package_path_r]) {
            hint.push_str(&long);
        }
        OUTPUT.error(&format!(
            "Failed to install {}\n  See log: {}{}",
            package_name,
            log_file_path.display(),
            hint
        ));

        // Show the build log. In an interactive terminal only its tail,
        // the file is still there to look at. Elsewhere, e.g. on CI and in
        // Docker builds, the log file is gone after the run, so show all of
        // it. `-v` always shows all of it. #535
        let log = std::fs::read(&log_file_path)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_else(|err| format!("cannot read {}: {}", log_file_path.display(), err));
        let max_lines = if OUTPUT.term().animate && !OUTPUT.is_verbose() {
            Some(BUILD_LOG_TAIL_LINES)
        } else {
            None
        };
        OUTPUT.println(&format_build_log(
            &log_file_path.display().to_string(),
            &log,
            max_lines,
        ));

        debug!(
            "Installation failed for {} from {}: exit code {}",
            package_name,
            package_path.display(),
            status.code().unwrap_or(-1)
        );

        Err(Box::new(ReportedError(format!(
            "Installation failed for {}",
            package_name
        ))))
    }
}

/// How many lines of a failed build log to show in an interactive terminal.
const BUILD_LOG_TAIL_LINES: usize = 10;

/// Format the build log of a failed install for the user. With `max_lines`,
/// show only the last that many lines, and point to the log file for the
/// rest. Without it, show the whole log.
fn format_build_log(path: &str, log: &str, max_lines: Option<usize>) -> String {
    let lines: Vec<&str> = log.trim_end().lines().collect();
    match max_lines {
        Some(max) if lines.len() > max => format!(
            "--- last {} lines of {} ---\n{}\n--- see {} for the full log ---",
            max,
            path,
            lines[lines.len() - max..].join("\n"),
            path
        ),
        _ => format!(
            "--- {} ---\n{}\n--- end of {} ---",
            path,
            lines.join("\n"),
            path
        ),
    }
}

/// Put what `R CMD INSTALL` just built into the built-package cache.
///
/// Never fails the install: a package that is installed but not cached is a
/// missed optimization, and the next run compiles it again. Archiving is real
/// CPU work in a runtime that runs several installs at once, so it goes to a
/// blocking thread.
async fn cache_build(pkg: &PackageInfo, library_path: &Path) {
    let dest = match &pkg.built {
        Some(dest) => dest.clone(),
        None => return,
    };
    let cached = pkg.clone();
    let library_path = library_path.to_path_buf();
    // `Box<dyn Error>` is not `Send`, and nothing here looks at the error beyond
    // logging it, so it crosses the thread boundary as its message.
    let result = tokio::task::spawn_blocking(move || {
        crate::built::store(&cached, &library_path, &dest).map_err(|err| err.to_string())
    })
    .await;
    match result {
        Ok(Ok(())) => debug!(
            "Cached the {} build at {}",
            pkg.name,
            pkg.built.as_ref().map(|p| p.display().to_string()).unwrap()
        ),
        Ok(Err(err)) => debug!("Cannot cache the {} build: {}", pkg.name, err),
        Err(err) => debug!("Cannot cache the {} build: {}", pkg.name, err),
    }
}

/// The packages that cannot be installed because `failed` failed: everything
/// that depends on it, directly or through other packages. Packages in `done`
/// (installed, failed, skipped or being installed) are left out. Sorted.
fn blocked_dependents(
    failed: &str,
    packages: &HashMap<String, PackageInfo>,
    done: &HashSet<String>,
) -> Vec<String> {
    let mut blocked: HashSet<&str> = HashSet::from([failed]);
    loop {
        let before = blocked.len();
        for (name, pkg) in packages {
            if !done.contains(name)
                && !blocked.contains(name.as_str())
                && pkg
                    .dependencies
                    .iter()
                    .any(|dep| blocked.contains(dep.as_str()))
            {
                blocked.insert(name);
            }
        }
        if blocked.len() == before {
            break;
        }
    }
    blocked.remove(failed);
    let mut blocked: Vec<String> = blocked.into_iter().map(String::from).collect();
    blocked.sort();
    blocked
}

/// `names` as a comma separated list, cut after the first ten.
fn name_list(names: &[String]) -> String {
    const MAX: usize = 10;
    if names.len() <= MAX {
        names.join(", ")
    } else {
        format!(
            "{}, and {} more",
            names[..MAX].join(", "),
            names.len() - MAX
        )
    }
}

fn packages_word(n: usize) -> &'static str {
    if n == 1 {
        "package"
    } else {
        "packages"
    }
}

/// The final error message of an install where some packages failed, and some
/// others were skipped because they depend on a failed one.
fn failure_summary(failed: &[String], skipped: &[String], installed: usize) -> String {
    let mut msg = format!(
        "Failed to install {} {} ({})",
        failed.len(),
        packages_word(failed.len()),
        name_list(failed)
    );
    if !skipped.is_empty() {
        msg.push_str(&format!(
            ", skipped {} {} that {} on {} ({})",
            skipped.len(),
            packages_word(skipped.len()),
            if skipped.len() == 1 {
                "depends"
            } else {
                "depend"
            },
            if failed.len() == 1 { "it" } else { "them" },
            name_list(skipped)
        ));
    }
    msg.push_str(&format!(", installed {}", installed));
    msg
}

/// Install multiple packages respecting dependency order
///
/// Packages are installed concurrently when possible, but dependencies
/// are always installed before packages that depend on them.
///
/// # Arguments
/// * `packages` - List of packages with their file paths and dependencies
/// * `library_path` - Path to the R library directory
/// * `r_binary` - Path to the R binary to use for installation
/// * `max_concurrent` - Maximum number of packages to install concurrently
/// * `print_fn` - Optional print function for success messages (e.g., progress bar's println)
/// * `progress_callback` - Optional callback called when each package completes installation
///
/// # Returns
/// * `Ok(())` if all installations succeeded
/// * `Err` if any installation failed
pub async fn install_package_tree_with_progress<P, F>(
    packages: Vec<PackageInfo>,
    library_path: &Path,
    r_binary: &str,
    max_concurrent: usize,
    print_fn: Option<Arc<P>>,
    mut progress_callback: Option<F>,
) -> Result<(), Box<dyn Error>>
where
    P: Fn(&str) + Send + Sync + 'static,
    F: FnMut(&str, bool),
{
    let package_count = packages.len();

    OUTPUT.status(&format!("Installing {} packages.", package_count));

    debug!(
        "Installing {} packages in dependency order with max_concurrent={}",
        package_count, max_concurrent
    );

    remove_stale_temp_libraries(library_path);

    let package_map: Arc<HashMap<String, PackageInfo>> = Arc::new(
        packages
            .into_iter()
            .map(|pkg| (pkg.name.clone(), pkg))
            .collect(),
    );

    let installed = Arc::new(Mutex::new(HashSet::new()));
    let failed: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    // Packages not tried because a dependency failed, with the failed package.
    let skipped: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));
    let installing = Arc::new(Mutex::new(HashSet::new()));

    let mut running_tasks = FuturesUnordered::new();

    let library_path = library_path.to_path_buf();
    let r_binary = r_binary.to_string();

    #[allow(clippy::too_many_arguments)]
    async fn try_start_packages<P>(
        package_map: Arc<HashMap<String, PackageInfo>>,
        installed: Arc<Mutex<HashSet<String>>>,
        failed: Arc<Mutex<HashSet<String>>>,
        skipped: Arc<Mutex<HashMap<String, String>>>,
        installing: Arc<Mutex<HashSet<String>>>,
        library_path: PathBuf,
        r_binary: String,
        max_to_start: usize,
        print_fn: Option<Arc<P>>,
    ) -> Vec<tokio::task::JoinHandle<Result<String, (String, String)>>>
    where
        P: Fn(&str) + Send + Sync + 'static,
    {
        let installed_set = installed.lock().await.clone();
        let failed_set = failed.lock().await.clone();
        let skipped_set = skipped.lock().await.clone();
        let mut installing_set = installing.lock().await;

        let mut new_tasks = Vec::new();
        let mut started = 0;

        for (name, pkg) in package_map.iter() {
            if started >= max_to_start {
                break;
            }

            if installed_set.contains(name)
                || failed_set.contains(name)
                || skipped_set.contains_key(name)
                || installing_set.contains(name)
            {
                continue;
            }

            // A package with a failed dependency is already in `skipped`, the
            // main loop puts it there when the dependency fails.
            let all_deps_installed = pkg
                .dependencies
                .iter()
                .all(|dep| installed_set.contains(dep));

            if all_deps_installed {
                installing_set.insert(name.clone());
                started += 1;
                let name_clone = name.clone();
                let pkg_clone = pkg.clone();
                let library_path_clone = library_path.clone();
                let r_binary_clone = r_binary.clone();
                let installed_clone = Arc::clone(&installed);
                let failed_clone = Arc::clone(&failed);
                let installing_clone = Arc::clone(&installing);
                let print_fn_clone = print_fn.clone();

                let task = tokio::spawn(async move {
                    let result = match install_package(
                        &pkg_clone,
                        &library_path_clone,
                        &r_binary_clone,
                        print_fn_clone,
                    )
                    .await
                    {
                        Ok(()) => Ok(()),
                        Err(e) => Err(e.to_string()),
                    };

                    let outcome = match result {
                        Ok(()) => {
                            installed_clone.lock().await.insert(name_clone.clone());
                            Ok(())
                        }
                        Err(err_msg) => {
                            debug!(
                                "Install task completed with error for {}: {}",
                                name_clone, err_msg
                            );
                            failed_clone.lock().await.insert(name_clone.clone());
                            Err(err_msg)
                        }
                    };

                    installing_clone.lock().await.remove(&name_clone);

                    match outcome {
                        Ok(()) => Ok(name_clone),
                        Err(err_msg) => Err((name_clone, err_msg)),
                    }
                });

                new_tasks.push(task);
            }
        }

        new_tasks
    }

    let initial_tasks = try_start_packages(
        Arc::clone(&package_map),
        Arc::clone(&installed),
        Arc::clone(&failed),
        Arc::clone(&skipped),
        Arc::clone(&installing),
        library_path.clone(),
        r_binary.clone(),
        max_concurrent,
        print_fn.clone(),
    )
    .await;

    for task in initial_tasks {
        running_tasks.push(task);
    }

    while let Some(result) = running_tasks.next().await {
        match result? {
            Ok(pkg_name) => {
                if let Some(ref mut callback) = progress_callback {
                    callback(&pkg_name, true);
                }
            }
            Err((pkg_name, _err_msg)) => {
                // The error itself is already shown by `install_package`. Here
                // we skip every package that needs the failed one, and say so.
                let mut skipped_map = skipped.lock().await;
                let mut done: HashSet<String> = installed.lock().await.clone();
                done.extend(failed.lock().await.iter().cloned());
                done.extend(installing.lock().await.iter().cloned());
                done.extend(skipped_map.keys().cloned());
                let blocked = blocked_dependents(&pkg_name, &package_map, &done);
                if !blocked.is_empty() {
                    OUTPUT.warn(&format!(
                        "Not installing {}: {} {}",
                        name_list(&blocked),
                        if blocked.len() == 1 {
                            "it needs"
                        } else {
                            "they need"
                        },
                        pkg_name
                    ));
                    for name in blocked {
                        skipped_map.insert(name, pkg_name.clone());
                    }
                }
            }
        }

        let currently_running = installing.lock().await.len();
        let can_start = max_concurrent.saturating_sub(currently_running);

        if can_start > 0 {
            let new_tasks = try_start_packages(
                Arc::clone(&package_map),
                Arc::clone(&installed),
                Arc::clone(&failed),
                Arc::clone(&skipped),
                Arc::clone(&installing),
                library_path.clone(),
                r_binary.clone(),
                can_start,
                print_fn.clone(),
            )
            .await;

            for task in new_tasks {
                running_tasks.push(task);
            }
        }
    }

    let final_installed = installed.lock().await.len();
    let final_failed = failed.lock().await.len();
    let final_skipped = skipped.lock().await.len();

    if final_installed + final_failed + final_skipped < package_count {
        let installed_set = installed.lock().await.clone();
        let failed_set = failed.lock().await.clone();
        let skipped_set = skipped.lock().await.clone();
        let remaining: Vec<String> = package_map
            .keys()
            .filter(|k| {
                !installed_set.contains(*k)
                    && !failed_set.contains(*k)
                    && !skipped_set.contains_key(*k)
            })
            .cloned()
            .collect();

        let err_msg = format!(
            "Unable to install remaining packages (possible circular dependency): {}",
            remaining.join(", ")
        );

        return Err(err_msg.into());
    }

    if final_failed > 0 {
        let mut failed_names: Vec<String> = failed.lock().await.iter().cloned().collect();
        failed_names.sort();
        let mut skipped_names: Vec<String> = skipped.lock().await.keys().cloned().collect();
        skipped_names.sort();
        let err_msg = failure_summary(&failed_names, &skipped_names, final_installed);

        debug!(
            "Installation completed: {} succeeded, {} failed, {} skipped",
            final_installed, final_failed, final_skipped
        );

        return Err(err_msg.into());
    }

    OUTPUT.success(&format!(
        "Installed all {} packages successfully",
        final_installed
    ));

    Ok(())
}

/// Install a set of packages into a library, with a progress bar, and return how
/// many went in.
///
/// The synchronous entry point both `rig pkg install` and `rig proj sync` use:
/// it owns the tokio runtime and the progress bar, so that the callers only have
/// to decide *what* to install.
pub fn install_packages(
    packages: Vec<PackageInfo>,
    library_path: &Path,
    r_binary: &str,
    max_concurrent: usize,
) -> Result<usize, Box<dyn Error>> {
    let total = packages.len();

    let install_pb = OUTPUT.progress(total as u64, "Installing");

    let installed_count = Cell::new(0);

    // Per-package messages have to go through the bar, or they overwrite it.
    let print_fn = Arc::new({
        let pb = install_pb.clone();
        move |msg: &str| {
            pb.println(msg);
        }
    });

    let rt = tokio::runtime::Runtime::new()?;
    let result = rt.block_on(install_package_tree_with_progress(
        packages,
        library_path,
        r_binary,
        max_concurrent,
        Some(print_fn),
        Some(|_pkg_name: &str, success: bool| {
            if success {
                installed_count.set(installed_count.get() + 1);
                install_pb.inc(1);
            }
        }),
    ));

    install_pb.finish();
    result?;

    Ok(installed_count.get())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_format_build_log() {
        let log: String = (1..=15).map(|i| format!("line {}\n", i)).collect();

        let full = format_build_log("x.log", &log, None);
        assert!(full.starts_with("--- x.log ---\nline 1\n"));
        assert!(full.ends_with("line 15\n--- end of x.log ---"));

        let tail = format_build_log("x.log", &log, Some(10));
        assert_eq!(
            tail,
            "--- last 10 lines of x.log ---\n\
             line 6\nline 7\nline 8\nline 9\nline 10\n\
             line 11\nline 12\nline 13\nline 14\nline 15\n\
             --- see x.log for the full log ---"
        );

        // A short log is shown in full, even with a limit.
        assert_eq!(
            format_build_log("x.log", "a\nb\n", Some(10)),
            "--- x.log ---\na\nb\n--- end of x.log ---"
        );
    }

    /// A `PackageInfo` for a built package at `path`, with the given provenance.
    fn info(
        name: &str,
        path: &Path,
        hash: Option<&str>,
        linkingto: &[(&str, &str, &str)],
    ) -> PackageInfo {
        PackageInfo {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            binary: true,
            file_path: path.to_path_buf(),
            dependencies: vec![],
            hash: hash.map(|x| x.to_string()),
            linkingto: linkingto
                .iter()
                .map(|(p, v, s)| (p.to_string(), v.to_string(), s.to_string()))
                .collect(),
            built: None,
            remote: HashMap::new(),
        }
    }

    /// A gzipped tarball at `path` holding `<top>/DESCRIPTION` with `desc`, plus
    /// any extra files, each given as a path relative to `top`.
    fn tarball(path: &Path, top: &str, desc: &str, extra: &[&str]) {
        let file = std::fs::File::create(path).unwrap();
        let enc = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
        let mut ar = tar::Builder::new(enc);
        let mut add = |name: String, content: &str| {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            ar.append_data(&mut header, name, content.as_bytes())
                .unwrap();
        };
        add(format!("{}/DESCRIPTION", top), desc);
        for name in extra {
            add(format!("{}/{}", top, name), "x\n");
        }
        ar.into_inner().unwrap().finish().unwrap();
    }

    /// A zip archive at `path` holding `<top>/DESCRIPTION` with `desc`.
    fn zipball(path: &Path, top: &str, desc: &str) {
        let file = std::fs::File::create(path).unwrap();
        let mut zw = zip::ZipWriter::new(file);
        zw.start_file(
            format!("{}/DESCRIPTION", top),
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zw.write_all(desc.as_bytes()).unwrap();
        zw.finish().unwrap();
    }

    const DESC: &str = "Package: foo\nVersion: 1.0.0\nBuilt: R 4.5.1\n";

    /// The unpack-cache key `install_package` derives for a binary archive:
    /// its own file name, since a binary's archives share one directory.
    fn key(archive: &Path) -> &str {
        archive.file_name().and_then(|x| x.to_str()).unwrap()
    }

    #[test]
    fn a_binary_tarball_is_unpacked_into_the_library() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = tmp.path().join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        let archive = tmp.path().join("foo_1.0.0.tgz");
        tarball(&archive, "foo", DESC, &["libs/foo.so"]);

        install_from_cache(
            &info("foo", &archive, Some("abc"), &[]),
            &archive,
            key(&archive),
            &lib,
        )
        .unwrap();

        assert!(lib.join("foo/libs/foo.so").exists());
        let desc = std::fs::read_to_string(lib.join("foo/DESCRIPTION")).unwrap();
        assert!(desc.contains("Built: R 4.5.1"));
        assert!(desc.contains("RemoteHash: abc"));
    }

    #[test]
    fn a_binary_zip_is_unpacked_into_the_library() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = tmp.path().join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        let archive = tmp.path().join("foo_1.0.0.zip");
        zipball(&archive, "foo", DESC);

        install_from_cache(
            &info("foo", &archive, Some("abc"), &[]),
            &archive,
            key(&archive),
            &lib,
        )
        .unwrap();

        assert!(lib.join("foo/DESCRIPTION").exists());
    }

    /// A binary archive is decompressed once into the shared unpack cache; a
    /// second install of the same archive into a different library reuses
    /// that cache instead of decompressing again, the same as a rig-built
    /// package (see `a_second_install_of_the_same_build_reuses_the_unpacked_cache_without_the_archive`).
    #[test]
    fn a_second_install_of_the_same_binary_reuses_the_unpacked_cache_without_the_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let lib1 = tmp.path().join("lib1");
        let lib2 = tmp.path().join("lib2");
        std::fs::create_dir_all(&lib1).unwrap();
        std::fs::create_dir_all(&lib2).unwrap();
        let archive = tmp.path().join("foo_1.0.0.tgz");
        tarball(&archive, "foo", DESC, &["libs/foo.so"]);

        install_from_cache(
            &info("foo", &archive, Some("abc"), &[]),
            &archive,
            key(&archive),
            &lib1,
        )
        .unwrap();

        // The archive is gone: only the unpacked cache directory next to it is
        // left, so a second install can only succeed by reusing that.
        std::fs::remove_file(&archive).unwrap();

        install_from_cache(
            &info("foo", &archive, Some("def"), &[]),
            &archive,
            key(&archive),
            &lib2,
        )
        .unwrap();

        assert!(lib2.join("foo/libs/foo.so").exists());
        assert_eq!(
            std::fs::read_to_string(lib1.join("foo/libs/foo.so")).unwrap(),
            std::fs::read_to_string(lib2.join("foo/libs/foo.so")).unwrap(),
        );

        let desc1 = std::fs::read_to_string(lib1.join("foo/DESCRIPTION")).unwrap();
        let desc2 = std::fs::read_to_string(lib2.join("foo/DESCRIPTION")).unwrap();
        assert!(desc1.contains("RemoteHash: abc"), "{}", desc1);
        assert!(desc2.contains("RemoteHash: def"), "{}", desc2);
    }

    /// Installing replaces the whole directory, so a file only the previous
    /// version had is gone afterwards.
    #[test]
    fn an_older_install_is_replaced_wholesale() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = tmp.path().join("lib");
        std::fs::create_dir_all(lib.join("foo")).unwrap();
        std::fs::write(
            lib.join("foo/DESCRIPTION"),
            "Package: foo\nVersion: 0.1.0\n",
        )
        .unwrap();
        std::fs::write(lib.join("foo/stale.txt"), "x").unwrap();
        let archive = tmp.path().join("foo_1.0.0.tgz");
        tarball(&archive, "foo", DESC, &[]);

        install_from_cache(
            &info("foo", &archive, None, &[]),
            &archive,
            key(&archive),
            &lib,
        )
        .unwrap();

        assert!(!lib.join("foo/stale.txt").exists());
        let desc = std::fs::read_to_string(lib.join("foo/DESCRIPTION")).unwrap();
        assert!(desc.contains("Version: 1.0.0"), "{}", desc);
    }

    /// A source tarball named like a binary is not one: its top level directory
    /// is the package name, but the check that catches this is worth having, so
    /// an archive with the wrong top level is rejected, and nothing is left
    /// behind for the next run to trip over.
    #[test]
    fn an_archive_with_the_wrong_layout_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = tmp.path().join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        let archive = tmp.path().join("foo_1.0.0.tgz");
        tarball(&archive, "notfoo", DESC, &[]);

        let err = install_from_cache(
            &info("foo", &archive, None, &[]),
            &archive,
            key(&archive),
            &lib,
        )
        .unwrap_err();
        assert!(err.to_string().contains("top level 'notfoo'"), "{}", err);
        assert!(!lib.join("foo").exists());
        // No staging directory is left behind.
        assert_eq!(std::fs::read_dir(&lib).unwrap().count(), 0);
    }

    /// A failed install must not take the installed version with it.
    #[test]
    fn a_failed_install_leaves_the_old_version_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = tmp.path().join("lib");
        std::fs::create_dir_all(lib.join("foo")).unwrap();
        std::fs::write(
            lib.join("foo/DESCRIPTION"),
            "Package: foo\nVersion: 0.1.0\n",
        )
        .unwrap();
        let archive = tmp.path().join("foo_1.0.0.tgz");
        tarball(&archive, "notfoo", DESC, &[]);

        install_from_cache(
            &info("foo", &archive, None, &[]),
            &archive,
            key(&archive),
            &lib,
        )
        .unwrap_err();

        let desc = std::fs::read_to_string(lib.join("foo/DESCRIPTION")).unwrap();
        assert!(desc.contains("Version: 0.1.0"), "{}", desc);
    }

    // ----------------------------------------------------------------
    // Installing from rig's own build cache, linking rather than extracting

    /// A `PackageInfo` for a package rig built itself, as `install_from_cache`
    /// expects: `binary: false`, with `built` set to the cache archive.
    fn cached_info(name: &str, archive: &Path, hash: Option<&str>) -> PackageInfo {
        PackageInfo {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            binary: false,
            file_path: PathBuf::from("unused"),
            dependencies: vec![],
            hash: hash.map(|x| x.to_string()),
            linkingto: vec![],
            built: Some(archive.to_path_buf()),
            remote: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn a_cached_build_is_installed_like_a_binary_package() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = tmp.path().join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        let archive = tmp.path().join("foo_1.0.0.tgz");
        tarball(&archive, "foo", DESC, &["libs/foo.so"]);

        install_from_cache(
            &cached_info("foo", &archive, Some("abc")),
            &archive,
            "foo",
            &lib,
        )
        .unwrap();

        assert!(lib.join("foo/libs/foo.so").exists());
        let desc = std::fs::read_to_string(lib.join("foo/DESCRIPTION")).unwrap();
        assert!(desc.contains("RemoteHash: abc"), "{}", desc);
    }

    /// The whole point of the cache: a second install of the same build, into
    /// a different library, does not need the archive at all, because it was
    /// unpacked once into a shared cache directory on the first install.
    #[test]
    fn a_second_install_of_the_same_build_reuses_the_unpacked_cache_without_the_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let lib1 = tmp.path().join("lib1");
        let lib2 = tmp.path().join("lib2");
        std::fs::create_dir_all(&lib1).unwrap();
        std::fs::create_dir_all(&lib2).unwrap();
        let archive = tmp.path().join("foo_1.0.0.tgz");
        tarball(&archive, "foo", DESC, &["libs/foo.so"]);

        install_from_cache(
            &cached_info("foo", &archive, Some("abc")),
            &archive,
            "foo",
            &lib1,
        )
        .unwrap();

        // The archive is gone: only the unpacked cache directory next to it is
        // left, so a second install can only succeed by reusing that.
        std::fs::remove_file(&archive).unwrap();

        install_from_cache(
            &cached_info("foo", &archive, Some("def")),
            &archive,
            "foo",
            &lib2,
        )
        .unwrap();

        assert!(lib2.join("foo/libs/foo.so").exists());
        assert_eq!(
            std::fs::read_to_string(lib1.join("foo/libs/foo.so")).unwrap(),
            std::fs::read_to_string(lib2.join("foo/libs/foo.so")).unwrap(),
        );

        // DESCRIPTION is never linked: each library's copy is its own file,
        // independently patched with that install's own provenance.
        let desc1 = std::fs::read_to_string(lib1.join("foo/DESCRIPTION")).unwrap();
        let desc2 = std::fs::read_to_string(lib2.join("foo/DESCRIPTION")).unwrap();
        assert!(desc1.contains("RemoteHash: abc"), "{}", desc1);
        assert!(desc2.contains("RemoteHash: def"), "{}", desc2);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_in_a_cached_build_is_recreated_as_a_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = tmp.path().join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        let archive = tmp.path().join("foo_1.0.0.tgz");
        tarball(&archive, "foo", DESC, &["libs/real.so"]);

        // `tar` does not have a convenient symlink-adding helper here, so the
        // unpacked cache is populated directly instead of round tripping a
        // symlink through an archive.
        let unpacked = crate::built::ensure_unpacked(&archive, "foo", "foo").unwrap();
        std::os::unix::fs::symlink("real.so", unpacked.join("libs/link.so")).unwrap();

        install_from_cache(
            &cached_info("foo", &archive, Some("abc")),
            &archive,
            "foo",
            &lib,
        )
        .unwrap();

        let link = lib.join("foo/libs/link.so");
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_link(&link).unwrap(), Path::new("real.so"));
    }

    #[test]
    fn stage_local_source_copies_git_dir_aside() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("pkg");
        std::fs::create_dir_all(src.join(".git")).unwrap();
        std::fs::write(src.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::create_dir_all(src.join("R")).unwrap();
        std::fs::write(src.join("R/foo.R"), "foo <- function() 1\n").unwrap();
        std::fs::write(src.join("DESCRIPTION"), "Package: foo\n").unwrap();

        let dst = tmp.path().join("staged");
        stage_local_source_entries(&src, &dst, true).unwrap();

        assert!(!dst.join(".git").exists());
        assert_eq!(
            std::fs::read_to_string(dst.join("R/foo.R")).unwrap(),
            "foo <- function() 1\n"
        );
        assert_eq!(
            std::fs::read_to_string(dst.join("DESCRIPTION")).unwrap(),
            "Package: foo\n"
        );
    }

    #[test]
    fn stage_local_source_skips_the_project_environment() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("pkg");
        std::fs::create_dir_all(src.join(".rvenv/lib/cli")).unwrap();
        std::fs::create_dir_all(src.join("inst/.rvenv")).unwrap();
        std::fs::write(src.join("DESCRIPTION"), "Package: foo\n").unwrap();

        let dst = tmp.path().join("staged");
        stage_local_source(&src, &dst).unwrap();

        assert!(dst.join("DESCRIPTION").exists());
        assert!(!dst.join(".rvenv").exists());
        // only the top level one is the project environment
        assert!(dst.join("inst/.rvenv").exists());
    }

    #[test]
    fn stage_local_source_copy_is_independent_of_the_original() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("pkg");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("DESCRIPTION"), "Package: foo\n").unwrap();

        let dst = tmp.path().join("staged");
        stage_local_source_entries(&src, &dst, true).unwrap();

        // A write to the staged copy (standing in for a build writing a
        // compiled artifact) must never show up in the user's original
        // directory -- which would happen if the copy were a hard link.
        std::fs::write(dst.join("DESCRIPTION"), "Package: foo\nBuilt: yes\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(src.join("DESCRIPTION")).unwrap(),
            "Package: foo\n"
        );
    }

    // ----------------------------------------------------------------
    // Building a source package

    /// A stand-in for `R` that handles `R CMD INSTALL ... -l <lib> <pkg>` by
    /// running the shell snippet `body` with `$lib` set to the `-l` library,
    /// `$pkg` to the package (the last argument) and `$args` to all
    /// arguments, so that tests can see what rig asks of a build without a
    /// real R.
    #[cfg(unix)]
    fn fake_r(dir: &Path, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("R");
        let script = format!(
            r#"#!/bin/sh
args="$*"
while [ $# -gt 0 ]; do
  if [ "$1" = "-l" ]; then lib="$2"; fi
  pkg="$1"
  shift
done
{}
"#,
            body
        );
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.display().to_string()
    }

    /// A source package `foo` and an empty library, both under `tmp`.
    #[cfg(unix)]
    fn source_setup(tmp: &Path) -> (PackageInfo, PathBuf) {
        let tarball = tmp.join("foo_1.0.0.tar.gz");
        std::fs::write(&tarball, "").unwrap();
        let mut pkg = info("foo", &tarball, Some("abc"), &[]);
        pkg.binary = false;
        let lib = tmp.join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        (pkg, lib.canonicalize().unwrap())
    }

    #[cfg(unix)]
    fn leftovers(lib: &Path) -> Vec<String> {
        std::fs::read_dir(lib)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with(".rig-"))
            .collect()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_source_package_is_built_in_its_own_library_then_moved_in() {
        let tmp = tempfile::tempdir().unwrap();
        let (pkg, lib) = source_setup(tmp.path());
        let r = fake_r(
            tmp.path(),
            r#"mkdir -p "$lib/foo"
printf 'Package: foo\nVersion: 1.0.0\n' > "$lib/foo/DESCRIPTION"
echo "$lib" > "$lib/foo/built-in"
echo "$R_LIBS" > "$lib/foo/r-libs"
echo "$args" > "$lib/foo/args""#,
        );

        r_cmd_install::<fn(&str)>(&pkg, &lib, &r, None)
            .await
            .unwrap();

        let built_in = std::fs::read_to_string(lib.join("foo/built-in")).unwrap();
        let built_in = Path::new(built_in.trim());
        assert_eq!(built_in.parent(), Some(lib.as_path()));
        assert_ne!(built_in, lib);
        let r_libs = std::fs::read_to_string(lib.join("foo/r-libs")).unwrap();
        assert_eq!(r_libs.trim().split(':').next(), lib.to_str());
        let args = std::fs::read_to_string(lib.join("foo/args")).unwrap();
        assert!(
            args.split_whitespace().any(|a| a == "--no-lock"),
            "{}",
            args
        );
        let desc = std::fs::read_to_string(lib.join("foo/DESCRIPTION")).unwrap();
        assert!(desc.contains("RemoteHash: abc\n"), "{}", desc);
        assert!(leftovers(&lib).is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_failed_source_build_leaves_the_previous_install_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let (pkg, lib) = source_setup(tmp.path());
        std::fs::create_dir_all(lib.join("foo")).unwrap();
        std::fs::write(
            lib.join("foo/DESCRIPTION"),
            "Package: foo\nVersion: 0.9.0\n",
        )
        .unwrap();
        let r = fake_r(
            tmp.path(),
            r#"mkdir -p "$lib/foo"
echo partial > "$lib/foo/DESCRIPTION"
exit 1"#,
        );

        assert!(r_cmd_install::<fn(&str)>(&pkg, &lib, &r, None)
            .await
            .is_err());

        let desc = std::fs::read_to_string(lib.join("foo/DESCRIPTION")).unwrap();
        assert_eq!(desc, "Package: foo\nVersion: 0.9.0\n");
        assert!(leftovers(&lib).is_empty());
    }

    /// A library `lib` under `tmp`, and an empty temporary library in it.
    fn move_setup(tmp: &Path) -> (PathBuf, TempLibrary) {
        let lib = tmp.join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        let temp_lib = TempLibrary::create(&lib).unwrap();
        (lib, temp_lib)
    }

    #[test]
    fn move_into_library_replaces_the_old_package() {
        let tmp = tempfile::tempdir().unwrap();
        let (lib, temp_lib) = move_setup(tmp.path());
        std::fs::create_dir_all(lib.join("foo")).unwrap();
        std::fs::write(lib.join("foo/old"), "").unwrap();
        std::fs::create_dir_all(temp_lib.path().join("foo")).unwrap();
        std::fs::write(temp_lib.path().join("foo/new"), "").unwrap();

        move_into_library(&temp_lib, &lib, "foo").unwrap();

        assert!(lib.join("foo/new").exists());
        assert!(!lib.join("foo/old").exists());
        assert!(!temp_lib.path().join("foo").exists());
        assert!(!temp_lib.path().join(".old").exists());
    }

    #[test]
    fn move_into_library_works_without_an_old_package() {
        let tmp = tempfile::tempdir().unwrap();
        let (lib, temp_lib) = move_setup(tmp.path());
        std::fs::create_dir_all(temp_lib.path().join("foo")).unwrap();
        std::fs::write(temp_lib.path().join("foo/new"), "").unwrap();

        move_into_library(&temp_lib, &lib, "foo").unwrap();

        assert!(lib.join("foo/new").exists());
        assert!(!temp_lib.path().join(".old").exists());
    }

    #[test]
    fn move_into_library_puts_the_old_package_back_on_failure() {
        let tmp = tempfile::tempdir().unwrap();
        // No `<temp_lib>/foo`, so the second rename fails.
        let (lib, temp_lib) = move_setup(tmp.path());
        std::fs::create_dir_all(lib.join("foo")).unwrap();
        std::fs::write(lib.join("foo/old"), "").unwrap();

        assert!(move_into_library(&temp_lib, &lib, "foo").is_err());

        assert!(lib.join("foo/old").exists());
        assert!(!temp_lib.path().join(".old").exists());
    }

    /// Two installs of the same package that finish at the same time both
    /// succeed, and one of them ends up in the library. Racy by nature, so it
    /// tries many times; without the retry in `move_into_library` some of the
    /// moves fail.
    #[test]
    fn concurrent_moves_into_the_library_both_succeed() {
        use std::sync::{Arc, Barrier};
        let tmp = tempfile::tempdir().unwrap();
        let lib = tmp.path().join("lib");
        std::fs::create_dir_all(lib.join("foo")).unwrap();
        for round in 0..200 {
            let barrier = Arc::new(Barrier::new(2));
            let threads: Vec<_> = ["a", "b"]
                .into_iter()
                .map(|who| {
                    let lib = lib.clone();
                    let barrier = barrier.clone();
                    std::thread::spawn(move || {
                        let temp_lib = TempLibrary::create(&lib).unwrap();
                        let staged = temp_lib.path().join("foo");
                        std::fs::create_dir(&staged).unwrap();
                        std::fs::write(staged.join(who), "").unwrap();
                        barrier.wait();
                        move_into_library(&temp_lib, &lib, "foo").map_err(|e| e.to_string())
                    })
                })
                .collect();
            for t in threads {
                t.join()
                    .unwrap()
                    .unwrap_or_else(|e| panic!("round {}: {}", round, e));
            }
            let files: Vec<_> = std::fs::read_dir(lib.join("foo")).unwrap().collect();
            assert_eq!(files.len(), 1, "round {}", round);
            let entries: Vec<_> = std::fs::read_dir(&lib).unwrap().collect();
            assert_eq!(entries.len(), 1, "round {}: leftovers", round);
        }
    }

    fn in_use() -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::PermissionDenied, "in use")
    }

    #[test]
    fn retry_transient_tries_again_until_it_works() {
        let mut calls = 0;
        let result = retry_transient(
            || {
                calls += 1;
                if calls < 3 {
                    Err(in_use())
                } else {
                    Ok(calls)
                }
            },
            |_| true,
        );
        assert_eq!(result.unwrap(), 3);
    }

    #[test]
    fn retry_transient_gives_up_eventually() {
        let mut calls = 0;
        let result: std::io::Result<()> = retry_transient(
            || {
                calls += 1;
                Err(in_use())
            },
            |_| true,
        );
        assert!(result.is_err());
        assert_eq!(calls, 8);
    }

    #[test]
    fn retry_transient_does_not_retry_other_errors() {
        let mut calls = 0;
        let result: std::io::Result<()> = retry_transient(
            || {
                calls += 1;
                Err(in_use())
            },
            |_| false,
        );
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }

    #[test]
    fn is_file_in_use_is_windows_only() {
        let denied = std::io::Error::from_raw_os_error(5);
        let sharing = std::io::Error::from_raw_os_error(32);
        let missing = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        assert_eq!(is_file_in_use(&denied), cfg!(windows));
        assert_eq!(is_file_in_use(&sharing), cfg!(windows));
        assert!(!is_file_in_use(&missing));
    }

    #[test]
    fn temp_library_pid_parses_rigs_temporary_directories() {
        assert_eq!(temp_library_pid(".rig-123-0"), Some(123));
        assert_eq!(temp_library_pid(".rig-123-45"), Some(123));
        assert_eq!(temp_library_pid(".rig-staging-foo-123"), Some(123));
        assert_eq!(temp_library_pid(".rig-staging-foo.bar-123"), Some(123));
        assert_eq!(temp_library_pid("foo"), None);
        assert_eq!(temp_library_pid(".rig-123"), None);
        assert_eq!(temp_library_pid(".rig-abc-0"), None);
        assert_eq!(temp_library_pid(".rig-123-x"), None);
        assert_eq!(temp_library_pid(".rig-staging-foo"), None);
        assert_eq!(temp_library_pid("_logs"), None);
    }

    /// The pid of a process that has exited.
    fn dead_pid() -> u32 {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--list")
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        pid
    }

    #[test]
    fn process_is_running_knows_running_and_exited_processes() {
        assert!(process_is_running(std::process::id()));
        assert!(!process_is_running(dead_pid()));
    }

    #[test]
    fn stale_temp_libraries_are_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = tmp.path();
        let dead = dead_pid();
        let me = std::process::id();
        let stale = [
            format!(".rig-{}-0", dead),
            format!(".rig-staging-foo-{}", dead),
        ];
        let kept = [
            format!(".rig-{}-0", me),
            "foo".to_string(),
            "_logs".to_string(),
            ".rig-other".to_string(),
        ];
        for name in stale.iter().chain(kept.iter()) {
            std::fs::create_dir_all(lib.join(name).join("sub")).unwrap();
        }

        remove_stale_temp_libraries(lib);

        for name in &stale {
            assert!(!lib.join(name).exists(), "{}", name);
        }
        for name in &kept {
            assert!(lib.join(name).exists(), "{}", name);
        }
    }

    #[test]
    fn temp_libraries_get_short_distinct_names() {
        let tmp = tempfile::tempdir().unwrap();
        let a = TempLibrary::create(tmp.path()).unwrap();
        let b = TempLibrary::create(tmp.path()).unwrap();
        assert_ne!(a.path(), b.path());
        let prefix = format!(".rig-{}-", std::process::id());
        for lib in [&a, &b] {
            assert_eq!(lib.path().parent(), Some(tmp.path()));
            let name = lib.path().file_name().unwrap().to_str().unwrap();
            let n = name.strip_prefix(&prefix).expect(name);
            assert!(n.parse::<usize>().is_ok(), "{}", name);
            assert!(lib.path().is_dir());
        }
    }

    #[test]
    fn a_temp_library_is_deleted_unless_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let (_lib, temp_lib) = move_setup(tmp.path());
        let path = temp_lib.path().to_path_buf();
        drop(temp_lib);
        assert!(!path.exists());

        let (_lib, temp_lib) = move_setup(tmp.path());
        let path = temp_lib.path().to_path_buf();
        temp_lib.keep();
        drop(temp_lib);
        assert!(path.exists());
    }

    #[test]
    fn build_r_libs_puts_the_library_first() {
        let lib = Path::new("/lib");
        let sep = if cfg!(windows) { ";" } else { ":" };
        assert_eq!(
            build_r_libs(lib, None),
            crate::utils::r_path(lib).display().to_string()
        );
        assert_eq!(build_r_libs(lib, Some("")), build_r_libs(lib, None));
        assert_eq!(
            build_r_libs(lib, Some("/other")),
            format!("{}{}/other", build_r_libs(lib, None), sep)
        );
    }

    /// A source package of `remote_type` at `checkout`, and an empty library.
    #[cfg(unix)]
    fn dir_source_setup(tmp: &Path, checkout: &Path, remote_type: &str) -> (PackageInfo, PathBuf) {
        let (mut pkg, lib) = source_setup(tmp);
        pkg.file_path = checkout.to_path_buf();
        pkg.remote
            .insert(REMOTE_TYPE_FIELD.to_string(), remote_type.to_string());
        (pkg, lib)
    }

    /// An `R` that builds the directory it is given, recording where it built
    /// from and whether it found an object file there.
    #[cfg(unix)]
    fn fake_r_building_a_directory(dir: &Path) -> String {
        fake_r(
            dir,
            r#"test -f "$pkg/src/foo.c" || exit 1
if test -f "$pkg/src/foo.o"; then o=yes; else o=no; fi
touch "$pkg/src/foo.o"
mkdir -p "$lib/foo"
cp "$pkg/DESCRIPTION" "$lib/foo/DESCRIPTION"
echo "$pkg" > "$lib/foo/built-from"
echo "$o" > "$lib/foo/had-object""#,
        )
    }

    /// Every build of a local directory gets a copy of its own, so that
    /// concurrent builds of the same directory cannot get in each other's
    /// way, and nothing of a build is left behind.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_local_source_directory_is_built_from_a_copy_of_its_own() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        std::fs::create_dir_all(source.join("src")).unwrap();
        std::fs::write(source.join("DESCRIPTION"), DESC).unwrap();
        std::fs::write(source.join("src/foo.c"), "").unwrap();
        let (pkg, lib) = dir_source_setup(tmp.path(), &source, "local");
        let r = fake_r_building_a_directory(tmp.path());

        let mut built_from = vec![];
        for _ in 0..2 {
            r_cmd_install::<fn(&str)>(&pkg, &lib, &r, None)
                .await
                .unwrap();
            let from = std::fs::read_to_string(lib.join("foo/built-from")).unwrap();
            assert!(!Path::new(from.trim()).exists());
            built_from.push(from);
        }

        assert!(!source.join("src/foo.o").exists());
        assert_ne!(built_from[0], built_from[1]);
    }

    /// An extracted `url::` archive in the cache is shared by every build of
    /// it, so a build must not leave its object files in it. It is not a git
    /// checkout, so it is copied without git.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_url_source_directory_is_built_from_a_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let checkout = tmp.path().join("checkout");
        std::fs::create_dir_all(checkout.join("src")).unwrap();
        std::fs::write(checkout.join("DESCRIPTION"), DESC).unwrap();
        std::fs::write(checkout.join("src/foo.c"), "").unwrap();
        let (pkg, lib) = dir_source_setup(tmp.path(), &checkout, "url");
        let r = fake_r_building_a_directory(tmp.path());

        r_cmd_install::<fn(&str)>(&pkg, &lib, &r, None)
            .await
            .unwrap();

        assert!(!checkout.join("src/foo.o").exists());
        let built_from = std::fs::read_to_string(lib.join("foo/built-from")).unwrap();
        assert_ne!(
            Path::new(built_from.trim()),
            checkout.canonicalize().unwrap()
        );
        assert!(!Path::new(built_from.trim()).exists());
    }

    /// The name is checked before anything touches the file system or runs
    /// R, so this needs no fake R and runs on every platform.
    #[tokio::test]
    async fn a_package_name_that_is_not_a_name_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = tmp.path().join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        let archive = tmp.path().join("foo_1.0.0.tgz");
        std::fs::write(&archive, "").unwrap();
        for name in [
            "../victim",
            "..\\victim",
            "/abs",
            "C:\\abs",
            "C:abs",
            "a/b",
            "a\\b",
            ".hidden",
            "",
            "1abc",
        ] {
            for binary in [true, false] {
                let mut pkg = info(name, &archive, None, &[]);
                pkg.binary = binary;
                let err = install_package::<fn(&str)>(&pkg, &lib, "no-such-R", None)
                    .await
                    .unwrap_err();
                assert!(
                    err.to_string().contains("not a valid R package name"),
                    "{}",
                    err
                );
            }
        }
        assert!(!tmp.path().join("victim").exists());
        assert_eq!(std::fs::read_dir(&lib).unwrap().count(), 0);
    }

    /// A git source whose `DESCRIPTION` git does not track is an error, not
    /// a silent fallback to copying everything.
    #[test]
    fn an_untracked_git_source_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let status = std::process::Command::new(crate::pkgsource::git::tests::git_binary())
            .args(["init", "--quiet"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        assert!(status.success());
        let src = tmp.path().join("pkg");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("DESCRIPTION"), DESC).unwrap();

        let dst = tmp.path().join("staged");
        assert!(stage_tracked_source(&src, &dst).is_err());
        assert!(!dst.exists());
    }

    /// A checkout an earlier rig built in place holds object files, which a
    /// new build must not start from.
    #[cfg(unix)]
    #[tokio::test]
    async fn artifacts_in_a_git_checkout_are_not_built_from() {
        let tmp = tempfile::tempdir().unwrap();
        let checkout = tmp.path().join("checkout");
        std::fs::create_dir_all(checkout.join("src")).unwrap();
        std::fs::write(checkout.join("DESCRIPTION"), DESC).unwrap();
        std::fs::write(checkout.join("src/foo.c"), "").unwrap();
        for args in [
            vec!["init", "--quiet"],
            vec!["add", "."],
            vec![
                "-c",
                "user.name=rig",
                "-c",
                "user.email=rig@example.com",
                "commit",
                "--quiet",
                "-m",
                "init",
            ],
        ] {
            let status = std::process::Command::new(crate::pkgsource::git::tests::git_binary())
                .args(&args)
                .current_dir(&checkout)
                .status()
                .unwrap();
            assert!(status.success());
        }
        std::fs::write(checkout.join("src/foo.o"), "stale").unwrap();
        let (pkg, lib) = dir_source_setup(tmp.path(), &checkout, "git");
        let r = fake_r_building_a_directory(tmp.path());

        r_cmd_install::<fn(&str)>(&pkg, &lib, &r, None)
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(lib.join("foo/had-object")).unwrap(),
            "no\n"
        );
    }

    // ----------------------------------------------------------------
    // Recording the provenance

    fn patched(desc: &str, hash: Option<&str>, linkingto: &[(&str, &str, &str)]) -> String {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("DESCRIPTION"), desc).unwrap();
        let pkg = info("foo", Path::new("unused"), hash, linkingto);
        patch_description(tmp.path(), &pkg).unwrap();
        std::fs::read_to_string(tmp.path().join("DESCRIPTION")).unwrap()
    }

    #[test]
    fn the_provenance_is_appended() {
        let out = patched(DESC, Some("abc"), &[("cpp11", "0.5.0", "def")]);
        assert_eq!(
            out,
            "Package: foo\nVersion: 1.0.0\nBuilt: R 4.5.1\n\
             RemoteHash: abc\nRemoteLinkingToHashes: cpp11@0.5.0=def\n"
        );
    }

    /// A package without `LinkingTo` gets no `LinkingTo` field, rather than an
    /// empty one.
    #[test]
    fn no_linkingto_means_no_linkingto_field() {
        let out = patched(DESC, Some("abc"), &[]);
        assert!(!out.contains("RemoteLinkingToHashes"));
    }

    #[test]
    fn a_description_without_a_trailing_newline_is_handled() {
        let out = patched("Package: foo\nVersion: 1.0.0", Some("abc"), &[]);
        assert_eq!(out, "Package: foo\nVersion: 1.0.0\nRemoteHash: abc\n");
    }

    /// Reinstalling replaces the previous provenance instead of adding a second
    /// copy of it, continuation lines and all.
    #[test]
    fn an_old_provenance_is_replaced_not_duplicated() {
        let desc = "Package: foo\nVersion: 1.0.0\n\
                    RemoteHash: old\n\
                    RemoteLinkingToHashes: cpp11@0.1.0=old,\n  tzdb@0.1.0=old\n\
                    License: MIT\n";
        let out = patched(desc, Some("new"), &[("cpp11", "0.5.0", "def")]);
        assert_eq!(out.matches("RemoteHash").count(), 1);
        assert!(!out.contains("old"), "{}", out);
        assert!(out.contains("License: MIT"), "{}", out);
        assert!(
            out.contains("RemoteLinkingToHashes: cpp11@0.5.0=def"),
            "{}",
            out
        );
    }

    /// Everything the package itself wrote is left exactly as it was, including
    /// the continuation lines of a multi-line field.
    #[test]
    fn the_rest_of_the_file_is_untouched() {
        let desc = "Package: foo\nVersion: 1.0.0\n\
                    Authors@R: c(\n    person(\"A\"),\n    person(\"B\")\n  )\n\
                    Description: One\n  two.\n";
        let out = patched(desc, Some("abc"), &[]);
        assert!(out.starts_with(desc), "{}", out);
    }

    #[test]
    fn linkingto_round_trips() {
        let value = format_linkingto(&[
            ("cpp11".to_string(), "0.5.0".to_string(), "aa".to_string()),
            ("tzdb".to_string(), "0.4.0".to_string(), "bb".to_string()),
        ]);
        assert_eq!(value, "cpp11@0.5.0=aa, tzdb@0.4.0=bb");
        assert_eq!(
            parse_linkingto(&value),
            vec![
                ("cpp11".to_string(), "0.5.0".to_string(), "aa".to_string()),
                ("tzdb".to_string(), "0.4.0".to_string(), "bb".to_string()),
            ]
        );
    }

    /// An entry rig cannot read is dropped, so the package looks like one with
    /// no provenance and is reinstalled, rather than being trusted.
    #[test]
    fn an_unparseable_linkingto_entry_is_dropped() {
        assert_eq!(parse_linkingto("cpp11, tzdb@0.4.0=bb").len(), 1);
        assert!(parse_linkingto("").is_empty());
    }

    // ----------------------------------------------------------------
    // Modes

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
    }

    /// A gzipped tarball of `pkg` with a `DESCRIPTION`, a `libs/pkg.so`, an
    /// `R/pkg.rdb` and their directories, with the given modes.
    #[cfg(unix)]
    fn tarball_with_modes(path: &Path, pkg: &str, dir: u32, exe: u32, data: u32) {
        let file = std::fs::File::create(path).unwrap();
        let enc = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
        let mut ar = tar::Builder::new(enc);
        let mut add = |name: String, content: &str, mode: u32, is_dir: bool| {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(mode);
            if is_dir {
                header.set_entry_type(tar::EntryType::Directory);
            }
            header.set_cksum();
            ar.append_data(&mut header, name, content.as_bytes())
                .unwrap();
        };
        add(format!("{}/", pkg), "", dir, true);
        add(
            format!("{}/DESCRIPTION", pkg),
            &format!("Package: {}\nVersion: 1.0.0\nBuilt: R 4.4.0\n", pkg),
            data,
            false,
        );
        add(format!("{}/libs/", pkg), "", dir, true);
        add(format!("{}/libs/{}.so", pkg, pkg), "so\n", exe, false);
        add(format!("{}/R/", pkg), "", dir, true);
        add(format!("{}/R/{}.rdb", pkg, pkg), "rdb\n", data, false);
        ar.into_inner().unwrap().finish().unwrap();
    }

    /// A package comes out with the modes R installs it with, whatever the
    /// archive says.
    #[cfg(unix)]
    #[test]
    fn an_unpacked_tarball_has_the_modes_r_gives_a_package() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("foo_1.0.0.tar.gz");
        tarball_with_modes(&archive, "foo", 0o775, 0o777, 0o664);
        let out = tmp.path().join("out");
        std::fs::create_dir(&out).unwrap();
        unpack_package(&archive, &out).unwrap();

        assert_eq!(mode_of(&out.join("foo")), 0o755);
        assert_eq!(mode_of(&out.join("foo/libs")), 0o755);
        assert_eq!(mode_of(&out.join("foo/libs/foo.so")), 0o755);
        assert_eq!(mode_of(&out.join("foo/DESCRIPTION")), 0o644);
        assert_eq!(mode_of(&out.join("foo/R/foo.rdb")), 0o644);

        // Modes that are too narrow are widened, but execute bits are kept
        // as they are.
        let archive = tmp.path().join("bar_1.0.0.tar.gz");
        tarball_with_modes(&archive, "bar", 0o700, 0o750, 0o400);
        unpack_package(&archive, &out).unwrap();
        assert_eq!(mode_of(&out.join("bar")), 0o755);
        assert_eq!(mode_of(&out.join("bar/libs/bar.so")), 0o754);
        assert_eq!(mode_of(&out.join("bar/DESCRIPTION")), 0o644);
    }

    #[cfg(unix)]
    #[test]
    fn an_unpacked_tarball_has_no_setuid_or_sticky_bits() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("foo_1.0.0.tar.gz");
        tarball_with_modes(&archive, "foo", 0o1755, 0o4755, 0o644);
        let out = tmp.path().join("out");
        std::fs::create_dir(&out).unwrap();
        unpack_package(&archive, &out).unwrap();
        assert_eq!(mode_of(&out.join("foo/libs/foo.so")), 0o755);
        assert_eq!(mode_of(&out.join("foo")), 0o755);
    }

    #[cfg(unix)]
    #[test]
    fn an_unpacked_zip_has_the_modes_r_gives_a_package() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("foo_1.0.0.zip");
        let mut zw = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        for (name, mode) in [("foo/DESCRIPTION", 0o664), ("foo/libs/foo.so", 0o775)] {
            zw.start_file(
                name,
                zip::write::SimpleFileOptions::default().unix_permissions(mode),
            )
            .unwrap();
            zw.write_all(b"x\n").unwrap();
        }
        zw.finish().unwrap();

        let out = tmp.path().join("out");
        std::fs::create_dir(&out).unwrap();
        unpack_package(&archive, &out).unwrap();
        assert_eq!(mode_of(&out.join("foo/DESCRIPTION")), 0o644);
        assert_eq!(mode_of(&out.join("foo/libs/foo.so")), 0o755);
        assert_eq!(mode_of(&out.join("foo/libs")), 0o755);
    }

    /// The library's files are links to the unpacked cache, so they have its
    /// modes.
    #[cfg(unix)]
    #[test]
    fn an_install_from_the_cache_has_the_modes_r_gives_a_package() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = tmp.path().join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        let archive = tmp.path().join("foo_1.0.0.tgz");
        tarball_with_modes(&archive, "foo", 0o775, 0o777, 0o666);

        install_from_cache(&info("foo", &archive, None, &[]), &archive, "foo", &lib).unwrap();

        assert_eq!(mode_of(&lib.join("foo/libs/foo.so")), 0o755);
        assert_eq!(mode_of(&lib.join("foo/R/foo.rdb")), 0o644);
        assert_eq!(mode_of(&lib.join("foo/DESCRIPTION")), 0o644);
    }

    fn with_deps(name: &str, deps: &[&str]) -> (String, PackageInfo) {
        let mut pkg = info(name, Path::new("x"), None, &[]);
        pkg.dependencies = deps.iter().map(|d| d.to_string()).collect();
        (name.to_string(), pkg)
    }

    #[test]
    fn blocked_dependents_follows_the_dependency_chain() {
        // c -> b -> a, d -> c and e, e and f are independent of a.
        let packages: HashMap<String, PackageInfo> = [
            with_deps("a", &[]),
            with_deps("b", &["a"]),
            with_deps("c", &["b"]),
            with_deps("d", &["c", "e"]),
            with_deps("e", &[]),
            with_deps("f", &["e"]),
        ]
        .into_iter()
        .collect();

        let done: HashSet<String> = ["a".to_string()].into();
        assert_eq!(
            blocked_dependents("a", &packages, &done),
            vec!["b", "c", "d"]
        );

        // Packages already done are left out, but their dependents are not.
        let done: HashSet<String> = ["a".to_string(), "b".to_string()].into();
        assert_eq!(
            blocked_dependents("a", &packages, &done),
            Vec::<String>::new()
        );
        assert_eq!(blocked_dependents("e", &packages, &done), vec!["d", "f"]);
    }

    #[test]
    fn failure_summary_lists_failed_and_skipped_packages() {
        let names = |xs: &[&str]| xs.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(
            failure_summary(&names(&["foo"]), &[], 79),
            "Failed to install 1 package (foo), installed 79"
        );
        assert_eq!(
            failure_summary(&names(&["foo"]), &names(&["bar"]), 79),
            "Failed to install 1 package (foo), skipped 1 package that depends on it (bar), installed 79"
        );
        assert_eq!(
            failure_summary(&names(&["foo", "qux"]), &names(&["bar", "baz"]), 3),
            "Failed to install 2 packages (foo, qux), skipped 2 packages that depend on them (bar, baz), installed 3"
        );
        let many: Vec<String> = (1..=12).map(|i| format!("p{}", i)).collect();
        assert_eq!(
            name_list(&many),
            "p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, and 2 more"
        );
    }
}
