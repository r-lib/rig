use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use futures::stream::{FuturesUnordered, StreamExt};
use indicatif::{ProgressBar, ProgressStyle};
use log::{debug, error, info};
use simple_error::bail;
use tokio::fs::create_dir_all;
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::output::OUTPUT;

/// The `DESCRIPTION` field recording which artifact an installed package came
/// from: the sha256 of the upstream CRAN source tarball of its version.
pub const REMOTE_HASH_FIELD: &str = "RemoteHash";

/// The `DESCRIPTION` field recording what the package was compiled against:
/// its `LinkingTo` dependencies as `pkg@version=sha256`, the same syntax P3M's
/// binary index uses.
pub const REMOTE_LINKINGTO_FIELD: &str = "RemoteLinkingToHashes";

/// `DESCRIPTION` fields recording the provenance of a git/GitHub-sourced
/// package, the same field names the R `remotes`/`pak`/`renv` packages use, so
/// that a package rig installed this way is recognizable by other R tooling
/// too. `REMOTE_TYPE_FIELD` is `"github"` or `"git"`; `REMOTE_HOST_FIELD`,
/// `REMOTE_REPO_FIELD` and `REMOTE_USERNAME_FIELD` are only set for a GitHub
/// source (a plain `git::` source has no owner/repo structure, only a URL).
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
    let archive = if pkg.binary {
        Some(pkg.file_path.clone())
    } else {
        pkg.built.clone().filter(|path| path.exists())
    };

    if let Some(archive) = archive {
        // A binary's archives all sit together in one `packages` directory
        // (see `lockfile_package_info`), so the shared unpack cache is keyed
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
                info!(
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

    r_cmd_install(pkg, library_path, r_binary, print_fn).await
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
    // A leading `.` keeps `rig pkg list` from reading the staging directory as
    // a half-installed package while another rig is working in the library.
    let staging = library_path.join(format!(".rig-staging-{}-{}", pkg.name, std::process::id()));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }

    let result = stage_cached_package(pkg, archive, key, &staging, library_path);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

/// The body of [`install_from_cache`], so that its caller can clean the
/// staging directory up on every error path.
///
/// `staging` must not exist yet: [`link_package_tree`] creates it, since the
/// fastest way to populate it (a whole-directory reflink on macOS) requires
/// the destination to not already exist.
fn stage_cached_package(
    pkg: &PackageInfo,
    archive: &Path,
    key: &str,
    staging: &Path,
    library_path: &Path,
) -> Result<(), Box<dyn Error>> {
    let unpacked = crate::built::ensure_unpacked(archive, key, &pkg.name)?;

    link_package_tree(&unpacked, staging)?;

    // `DESCRIPTION` always gets a private, patched copy: `patch_description`
    // records per-install provenance, and the destination copy may share
    // bytes (a hard link) or an inode's worth of on-disk blocks (a reflink)
    // with the cache, so it is replaced outright rather than edited in place.
    std::fs::remove_file(staging.join("DESCRIPTION"))?;
    std::fs::copy(unpacked.join("DESCRIPTION"), staging.join("DESCRIPTION"))?;
    patch_description(staging, pkg)?;

    // Replacing the old directory and moving the new one in is not atomic, so
    // this is the one window where an interrupted install leaves the library
    // without the package. It is as small as we can make it, and `rig pkg
    // install` recovers on the next run: a package without a DESCRIPTION is not
    // installed as far as `rig pkg list` is concerned.
    let target = library_path.join(&pkg.name);
    if target.exists() {
        std::fs::remove_dir_all(&target)?;
    }
    std::fs::rename(staging, &target)?;
    Ok(())
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

/// Extract a package archive into `dest`: a `.zip` on Windows, a gzipped
/// tarball everywhere else. The extension decides, not the platform, so that a
/// `--platform` other than this machine's still does the right thing.
pub(crate) fn unpack_package(archive: &Path, dest: &Path) -> Result<(), Box<dyn Error>> {
    let is_zip = archive
        .extension()
        .and_then(|x| x.to_str())
        .map(|x| x.eq_ignore_ascii_case("zip"))
        .unwrap_or(false);

    let file = std::fs::File::open(archive)?;
    if is_zip {
        let mut ar = zip::ZipArchive::new(file)?;
        ar.extract(dest)?;
    } else {
        let decoder = flate2::read::GzDecoder::new(file);
        let mut ar = tar::Archive::new(decoder);
        ar.set_preserve_permissions(true);
        ar.set_overwrite(true);
        ar.unpack(dest)?;
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
    let text = std::fs::read_to_string(&path)?;
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

    std::fs::write(&path, out)?;
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
    info!(
        "Installing package {} from {} to {}",
        package_name,
        package_path.display(),
        library_path.display()
    );

    let logs_dir = library_path.join("_logs");
    create_dir_all(&logs_dir).await?;

    let log_file_path = logs_dir.join(format!("{}-install.log", package_name));
    let log_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&log_file_path)?;

    let log_file_stderr = log_file.try_clone()?;

    // Run from a fresh temporary directory, so R does not pick up a project
    // `.Renviron` (or `.Rprofile`) that could redirect it to a project package
    // library instead of `library_path`. Canonicalize the paths passed on the
    // command line first, since they may otherwise be relative to rig's own
    // working directory rather than `run_dir`.
    let run_dir = tempfile::tempdir()?;
    let library_path_abs = library_path.canonicalize()?;
    let package_path_abs = package_path.canonicalize()?;

    let status = Command::new(r_binary)
        .arg("CMD")
        .arg("INSTALL")
        .arg("-l")
        .arg(&library_path_abs)
        .arg(&package_path_abs)
        .current_dir(run_dir.path())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_file_stderr))
        .status()
        .await?;

    if status.success() {
        // Before `patch_description`, so that the cached archive is what the
        // package built, and not what rig recorded in it. The provenance fields
        // are added on every install anyway, including on the installs that come
        // out of this cache.
        cache_build(pkg, library_path).await;

        patch_description(&library_path.join(package_name), pkg)?;

        // User output: Use custom print function if provided, otherwise use OUTPUT
        let msg = format!("Installed {} {}", package_name, pkg.version);
        if let Some(ref print) = print_fn {
            print(&msg);
        } else {
            OUTPUT.success(&msg);
        }

        info!(
            "Successfully installed package {} to {} (log: {})",
            package_name,
            library_path.display(),
            log_file_path.display()
        );

        Ok(())
    } else {
        // User output: Always use OUTPUT for errors (they should be visible)
        OUTPUT.error(&format!(
            "Failed to install {}\n  See log: {}",
            package_name,
            log_file_path.display()
        ));

        error!(
            "Installation failed for {} from {}: exit code {}",
            package_name,
            package_path.display(),
            status.code().unwrap_or(-1)
        );

        bail!("Installation failed for {}", package_name);
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

    info!(
        "Installing {} packages in dependency order with max_concurrent={}",
        package_count, max_concurrent
    );

    let package_map: Arc<HashMap<String, PackageInfo>> = Arc::new(
        packages
            .into_iter()
            .map(|pkg| (pkg.name.clone(), pkg))
            .collect(),
    );

    let installed = Arc::new(Mutex::new(HashSet::new()));
    let failed: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    let installing = Arc::new(Mutex::new(HashSet::new()));

    let mut running_tasks = FuturesUnordered::new();

    let library_path = library_path.to_path_buf();
    let r_binary = r_binary.to_string();

    #[allow(clippy::too_many_arguments)]
    async fn try_start_packages<P>(
        package_map: Arc<HashMap<String, PackageInfo>>,
        installed: Arc<Mutex<HashSet<String>>>,
        failed: Arc<Mutex<HashSet<String>>>,
        installing: Arc<Mutex<HashSet<String>>>,
        library_path: PathBuf,
        r_binary: String,
        max_to_start: usize,
        print_fn: Option<Arc<P>>,
    ) -> Vec<tokio::task::JoinHandle<Result<String, String>>>
    where
        P: Fn(&str) + Send + Sync + 'static,
    {
        let installed_set = installed.lock().await.clone();
        let failed_set = failed.lock().await.clone();
        let mut installing_set = installing.lock().await;

        let mut new_tasks = Vec::new();
        let mut started = 0;

        for (name, pkg) in package_map.iter() {
            if started >= max_to_start {
                break;
            }

            if installed_set.contains(name)
                || failed_set.contains(name)
                || installing_set.contains(name)
            {
                continue;
            }

            let all_deps_installed = pkg
                .dependencies
                .iter()
                .all(|dep| installed_set.contains(dep));

            let any_dep_failed = pkg.dependencies.iter().any(|dep| failed_set.contains(dep));

            if any_dep_failed {
                drop(installing_set);
                failed.lock().await.insert(name.clone());
                installing_set = installing.lock().await;
                // TODO: can this happen? We quit on the first failure, no?
                error!("Skipping package {} because a dependency failed", name);
            } else if all_deps_installed {
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
                        Err(err_msg) => Err(err_msg),
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
            Err(_err_msg) => {
                // Error already logged in the task
                // We can't get the package name easily here since it's in the error context
                // The error will be caught at the end anyway
            }
        }

        let currently_running = installing.lock().await.len();
        let can_start = max_concurrent.saturating_sub(currently_running);

        if can_start > 0 {
            let new_tasks = try_start_packages(
                Arc::clone(&package_map),
                Arc::clone(&installed),
                Arc::clone(&failed),
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

    if final_installed + final_failed < package_count {
        let installed_set = installed.lock().await.clone();
        let failed_set = failed.lock().await.clone();
        let remaining: Vec<String> = package_map
            .keys()
            .filter(|k| !installed_set.contains(*k) && !failed_set.contains(*k))
            .cloned()
            .collect();

        let err_msg = format!(
            "Unable to install remaining packages (possible circular dependency): {}",
            remaining.join(", ")
        );

        OUTPUT.error(&err_msg);
        error!("{}: {:?}", err_msg, remaining);

        return Err(err_msg.into());
    }

    if final_failed > 0 {
        let err_msg = format!(
            "Installation completed with {} failures ({}  succeeded)",
            final_failed, final_installed
        );

        OUTPUT.error(&err_msg);
        error!(
            "Installation completed: {} succeeded, {} failed",
            final_installed, final_failed
        );

        return Err(err_msg.into());
    }

    OUTPUT.success(&format!(
        "Installed all {} packages successfully",
        final_installed
    ));

    info!("Successfully installed all {} packages", final_installed);

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

    let install_pb = ProgressBar::new(total as u64);
    install_pb.set_style(
        ProgressStyle::default_bar()
            .template("{msg} [{bar:40.green/blue}] {pos}/{len} packages")
            .unwrap()
            .progress_chars("=>-"),
    );
    install_pb.set_message("Installing");

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

    install_pb.finish_and_clear();
    result?;

    Ok(installed_count.get())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

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
}
