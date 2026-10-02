//! Inline metadata of self-contained R scripts, for `rig run script.R`.
//!
//! A script can declare its own dependencies in a comment block, in the
//! format of Python's PEP 723 (`uv run` uses the same one):
//!
//! ```r
//! # /// script
//! # [dependencies]
//! # R = ">= 4.4"
//! # dplyr = ">= 1.1"
//! #
//! # [tool.rig]
//! # exclude-newer = "2026-06-01"
//! # ///
//! ```
//!
//! The body is a subset of `rproj.toml`. `rig run` turns it into a small
//! generated project in the rig cache, one per distinct header, and locks and
//! syncs that with the usual `rig proj` machinery. Scripts with the same
//! header share one environment.

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use clap::ArgMatches;
use log::{error, info, trace};
use serde::{Deserialize, Serialize};
use toml_edit::DocumentMut;

use crate::cache::get_cache_dir;
use crate::common::get_r_version_data_version;
use crate::output::OUTPUT;
use crate::proj::{
    add_spec_to_manifest, is_foreign_arch, lock_file_label, lock_fits_manifest, parse_add_arg,
    parse_upgrade_packages, proj_lock_host, proj_lock_keep_targets, proj_sync,
    requested_r_installation, resolve_project_r_version, rvenv_r_arch, AddSpec, ProjSyncOptions,
};
use crate::repos::cranlike_metadata::minor_r_version;
use crate::rproj::{Dependency, Repository, Rproj, RprojLock, RPROJ_MANIFEST_FILE};
use crate::rvenv::{
    ensure_rvenv_files, project_r_wrapper, read_rvenv_cfg, rvenv_sync_needed, RPROJ_LOCK_FILE,
};
use crate::stdout_redirect::StdoutToStderr;
use crate::utils::{calculate_hash, write_atomically};

/// The cache subdirectory that holds the script environments.
pub const SCRIPTS_CACHE_SUBDIR: &str = "scripts";

const BLOCK_START: &str = "# /// script";

/// The comment markers a block can use. The opening line decides which one,
/// and every other line of the block must use the same one. `##` is common
/// in R code, e.g. for comments that RStudio does not re-indent.
const BLOCK_MARKERS: [&str; 2] = ["##", "#"];

/// The parsed body of a `# /// script` block.
#[derive(Serialize, Deserialize, Debug, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ScriptMeta {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, Dependency>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repository: Vec<Repository>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tool: BTreeMap<String, toml::Table>,
}

/// The `# /// script` block of a script: its comment marker, where it is,
/// and its TOML body.
#[derive(Debug, PartialEq)]
struct ScriptBlock {
    /// `#` or `##`, see [`BLOCK_MARKERS`].
    marker: &'static str,
    /// The index of the opening `# /// script` line.
    start: usize,
    /// The index of the closing `# ///` line.
    end: usize,
    /// The TOML body, with the comment prefixes removed.
    body: String,
}

/// The `# /// script` block in `text`, or `None` if there is no such block.
///
/// The block can also use `##` instead of `#`, i.e. start with `## ///
/// script`, see [`BLOCK_MARKERS`]. Every line between the opening and the
/// closing line must be a comment with the same marker: either the marker
/// alone, or the marker followed by the content, usually after one space,
/// which is removed. A file can have at most one `script` block, and the
/// block must be closed.
fn find_script_block(text: &str) -> Result<Option<ScriptBlock>, Box<dyn Error>> {
    let mut block: Option<ScriptBlock> = None;
    let mut lines = text.lines().enumerate();
    while let Some((startno, line)) = lines.next() {
        let line = line.trim_end();
        let Some(marker) = BLOCK_MARKERS
            .iter()
            .find(|m| line == format!("{} /// script", m))
        else {
            continue;
        };
        if block.is_some() {
            bail!("more than one `{}` block", BLOCK_START);
        }
        let start = format!("{} /// script", marker);
        let end = format!("{} ///", marker);
        let mut content = String::new();
        let mut endno = None;
        for (lineno, line) in lines.by_ref() {
            let line = line.trim_end_matches('\r');
            if line.trim_end() == end {
                endno = Some(lineno);
                break;
            }
            let Some(rest) = line.strip_prefix(marker) else {
                bail!(
                    "line {} is inside the `{}` block, but it does not start \
                     with `{}` (is the closing `{}` line missing?)",
                    lineno + 1,
                    start,
                    marker,
                    end
                );
            };
            content.push_str(rest.strip_prefix(' ').unwrap_or(rest));
            content.push('\n');
        }
        let Some(endno) = endno else {
            bail!("the `{}` block has no closing `{}` line", start, end);
        };
        block = Some(ScriptBlock {
            marker,
            start: startno,
            end: endno,
            body: content,
        });
    }
    Ok(block)
}

/// The TOML body of the `# /// script` block in `text`, with the comment
/// prefixes removed, or `None` if there is no such block. See
/// [`find_script_block`].
fn extract_script_block(text: &str) -> Result<Option<String>, Box<dyn Error>> {
    Ok(find_script_block(text)?.map(|block| block.body))
}

/// A `script` block with the TOML `body`, every line commented out with
/// `marker`, and every line ending in `eol`.
fn render_script_block(marker: &str, body: &str, eol: &str) -> String {
    let mut out = format!("{} /// script{}", marker, eol);
    for line in body.trim_end().lines() {
        if line.is_empty() {
            out.push_str(marker);
        } else {
            out.push_str(&format!("{} {}", marker, line));
        }
        out.push_str(eol);
    }
    out.push_str(&format!("{} ///{}", marker, eol));
    out
}

/// `text` with its `block` replaced by a block with the TOML `body`. Without
/// a `block`, the new block goes at the top, after the `#!` line, if there
/// is one, and before an empty line. The new block keeps the marker of the
/// old one, and the file keeps its line endings.
fn replace_script_block(text: &str, block: Option<&ScriptBlock>, body: &str) -> String {
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let marker = block.map(|b| b.marker).unwrap_or(BLOCK_MARKERS[1]);
    let rendered = render_script_block(marker, body, eol);
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out = String::new();
    match block {
        Some(block) => {
            lines[..block.start].iter().for_each(|l| out.push_str(l));
            out.push_str(&rendered);
            lines[block.end + 1..].iter().for_each(|l| out.push_str(l));
        }
        None => {
            let skip = usize::from(lines.first().is_some_and(|l| l.starts_with("#!")));
            for line in &lines[..skip] {
                out.push_str(line);
                if !line.ends_with('\n') {
                    out.push_str(eol);
                }
            }
            out.push_str(&rendered);
            if lines.len() > skip {
                out.push_str(eol);
            }
            lines[skip..].iter().for_each(|l| out.push_str(l));
        }
    }
    out
}

/// Parse a `script` block body, with an error message that names the
/// block.
fn parse_block_body(body: &str) -> Result<ScriptMeta, Box<dyn Error>> {
    Ok(toml::from_str(body)
        .map_err(|e| format!("invalid TOML in the `{}` block: {}", BLOCK_START, e))?)
}

/// The inline metadata of a script, or `None` if it has none.
pub fn parse_script_metadata(text: &str) -> Result<Option<ScriptMeta>, Box<dyn Error>> {
    let body = match extract_script_block(text)? {
        None => return Ok(None),
        Some(body) => body,
    };
    Ok(Some(parse_block_body(&body)?))
}

impl ScriptMeta {
    /// Make `path = "..."` dependencies absolute, relative to `script_dir`.
    /// The environment lives in the cache, so a relative path would point
    /// somewhere else there. It also makes two scripts in different
    /// directories with the same relative path get different environments.
    pub fn absolutize_paths(&mut self, script_dir: &Path) {
        for dep in self.dependencies.values_mut() {
            if let Dependency::Detailed(table) = dep {
                if let Some(path) = &table.path {
                    if Path::new(path).is_relative() {
                        table.path = Some(script_dir.join(path).display().to_string());
                    }
                }
            }
        }
    }

    /// The `rproj.toml` of the generated project for this script.
    fn to_manifest(&self) -> Rproj {
        let mut manifest = Rproj::minimal("script");
        manifest.dependencies = self.dependencies.clone();
        // R has to be part of the solution, the lock file records the R
        // version from it. Without an `R` entry, it only is if some package
        // depends on R, so add one that allows any R.
        manifest
            .dependencies
            .entry("R".to_string())
            .or_insert_with(|| Dependency::Version("*".to_string()));
        manifest.repository = self.repository.clone();
        manifest.tool = self.tool.clone();
        manifest
    }

    /// The key of the environment of this metadata: the first 16 hex digits
    /// of the hash of its canonical TOML form, plus the R build selected by
    /// `--r-version`, if any, because that changes what gets locked. That is
    /// the R version and architecture of the installation (see
    /// [`requested_r_installation`]), not the argument as given, so `4.6`,
    /// `4.6.1` and `release` share an environment if they select the same R.
    ///
    /// A script with a lock file also has the absolute path of the lock file
    /// in its key: its environment is synced from that lock file, so it must
    /// not be shared with another script that has the same block.
    fn env_key(
        &self,
        r_version: Option<&str>,
        lock: Option<&Path>,
    ) -> Result<String, Box<dyn Error>> {
        let mut canonical = toml::to_string(self)?;
        if let Some(rver) = r_version {
            canonical.push_str(&format!("\n# r-version = {}\n", rver));
        }
        if let Some(lock) = lock {
            canonical.push_str(&format!("\n# lock = {}\n", lock.display()));
        }
        Ok(calculate_hash(&canonical)[..16].to_string())
    }
}

/// The cache directory of the environment of `meta`.
fn script_env_dir(
    meta: &ScriptMeta,
    r_version: Option<&str>,
    lock: Option<&Path>,
) -> Result<PathBuf, Box<dyn Error>> {
    Ok(get_cache_dir()?
        .join(SCRIPTS_CACHE_SUBDIR)
        .join(meta.env_key(r_version, lock)?))
}

/// The lock file of `script`: the script's path with `.lock` appended, e.g.
/// `analysis.R.lock` for `analysis.R`.
pub(crate) fn script_lock_path(script: &Path) -> PathBuf {
    let mut path = script.as_os_str().to_owned();
    path.push(".lock");
    PathBuf::from(path)
}

/// The absolute path of the lock file of `script`, for the key of its
/// environment. The lock file does not have to exist.
fn canonical_lock_path(script: &Path) -> Result<PathBuf, Box<dyn Error>> {
    Ok(script_lock_path(&script.canonicalize()?))
}

/// Set up the environment that `rig proj lock --script` solves the block of
/// `script` in: the same one `rig run` uses for the script once it has a
/// lock file. Returns its directory. The caller puts the script's lock file
/// into it, if there is one.
pub(crate) fn script_lock_env(script: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let (_text, block, mut meta) = read_script(script)?;
    if block.is_none() {
        bail!(
            "{} has no `{}` block, create one with `rig proj init --script {}`",
            script.display(),
            BLOCK_START,
            script.display()
        );
    }
    meta.absolutize_paths(&script_dir_of(script)?);
    let lock = canonical_lock_path(script)?;
    let envdir = script_env_dir(&meta, None, Some(&lock))?;
    write_env_manifest(&envdir, &meta)?;
    Ok(envdir)
}

/// Create the environment directory `envdir` for `meta`, if needed, with its
/// `rproj.toml` and the other files of a project environment.
fn write_env_manifest(envdir: &Path, meta: &ScriptMeta) -> Result<(), Box<dyn Error>> {
    let manifest_path = envdir.join(RPROJ_MANIFEST_FILE);
    if !manifest_path.exists() {
        fs::create_dir_all(envdir)?;
        let manifest = toml::to_string(&meta.to_manifest())?;
        write_atomically(&manifest_path, manifest.as_bytes())?;
    }
    ensure_rvenv_files(envdir)
}

/// Why the synced environment in `envdir` does not use R `version` on
/// `arch`, or `None` if it does, or if it was never synced.
fn env_r_mismatch(
    envdir: &Path,
    version: &str,
    arch: &str,
) -> Result<Option<String>, Box<dyn Error>> {
    let cfg = match read_rvenv_cfg(envdir)? {
        None => return Ok(None),
        Some(cfg) => cfg,
    };
    if cfg.r_arch != arch {
        return Ok(Some(format!(
            "it uses an {} R instead of {}",
            cfg.r_arch, arch
        )));
    }
    // An R that is gone is not a mismatch here, the sync reinstalls it.
    if let Ok(have) = get_r_version_data_version(&cfg.r_version) {
        if have != version {
            return Ok(Some(format!("it uses R {} instead of {}", have, version)));
        }
    }
    Ok(None)
}

/// The R wrapper to run `script` with, if it has inline metadata: the one in
/// the script's own environment, created or synced first if needed. `None` if
/// the script has no metadata block, or cannot be read, in which case it runs
/// the usual way.
pub fn script_r_binary(
    script: &Path,
    args: &ArgMatches,
    dry_run: bool,
) -> Result<Option<String>, Box<dyn Error>> {
    // An unreadable file is not our business here, R reports it the usual
    // way when it tries to run it.
    let text = match fs::read_to_string(script) {
        Ok(text) => text,
        Err(e) => {
            trace!("Could not read {}: {}", script.display(), e);
            return Ok(None);
        }
    };
    let mut meta = match parse_script_metadata(&text) {
        Ok(None) => return Ok(None),
        Ok(Some(meta)) => meta,
        Err(e) => {
            let msg = format!("{}: {}", script.display(), e);
            bail!("{}", msg);
        }
    };

    // Installing R or packages must not write to stdout, which belongs to
    // the script, e.g. `rig run script.R > out.csv`.
    let _stdout_guard = StdoutToStderr::new();

    meta.absolutize_paths(&script_dir_of(script)?);

    let mut created = None;
    let envdir = match prepare_script_env(script, &meta, args, dry_run, true, &mut created)? {
        Some(envdir) => envdir,
        None => {
            let requested = args
                .try_get_one::<String>("r-version")
                .ok()
                .flatten()
                .map(String::as_str)
                .unwrap_or_default();
            return Ok(Some(format!("<R {} environment>", requested)));
        }
    };
    let wrapper = project_r_wrapper(&envdir);

    if !wrapper.exists() && !dry_run {
        let msg = format!(
            "No R wrapper at {} after syncing the environment of {}",
            wrapper.display(),
            script.display()
        );
        bail!("{}", msg);
    }

    Ok(Some(
        wrapper
            .to_str()
            .ok_or("The cache path is not valid Unicode")?
            .to_string(),
    ))
}

/// The directory of `script`, to resolve its relative `path = "..."`
/// dependencies against.
fn script_dir_of(script: &Path) -> Result<PathBuf, Box<dyn Error>> {
    Ok(script
        .canonicalize()?
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default())
}

/// Create, lock and, with `sync`, sync the environment of `script`, which
/// has the metadata `meta`, with its paths already made absolute. Returns
/// the environment's directory, or `None` in a dry run, if the R that
/// `--r-version` selects is not installed. If this call creates the
/// directory, it also puts it into `created`, so a caller can remove it if
/// a later step fails.
fn prepare_script_env(
    script: &Path,
    meta: &ScriptMeta,
    args: &ArgMatches,
    dry_run: bool,
    sync: bool,
    created: &mut Option<PathBuf>,
) -> Result<Option<PathBuf>, Box<dyn Error>> {
    // Not every command that gets here has an `--r-version`.
    let requested = args.try_get_one::<String>("r-version").ok().flatten();

    let lock_path = script_lock_path(script);
    if lock_path.exists() {
        return prepare_locked_script_env(
            script,
            &lock_path,
            meta,
            requested.map(String::as_str),
            args,
            dry_run,
            sync,
            created,
        );
    }

    // Resolve `--r-version` to an installation first, installing it if
    // needed, so the environment is keyed on the R it actually uses.
    let (rver, r_arch) = match requested {
        None => (None, None),
        Some(requested) => match requested_r_installation(requested, dry_run)? {
            Some((version, arch)) => (Some(version), Some(arch)),
            None => {
                let msg = format!("Would create an environment for {}", script.display());
                OUTPUT.info(&msg);
                info!("{}", msg);
                return Ok(None);
            }
        },
    };
    let r_id = rver
        .as_ref()
        .zip(r_arch.as_ref())
        .map(|(version, arch)| format!("{} {}", version, arch));
    let envdir = script_env_dir(meta, r_id.as_deref(), None)?;
    trace!(
        "Environment of {} is at {}",
        script.display(),
        envdir.display()
    );

    // The key says which R the environment is for, but check that it really
    // uses that R, and start over if not: an environment that runs another
    // R build is broken, not merely stale.
    if let (Some(version), Some(arch)) = (&rver, &r_arch) {
        if let Some(why) = env_r_mismatch(&envdir, version, arch)? {
            if dry_run {
                let msg = format!(
                    "Would re-create the environment of {}, because {}",
                    script.display(),
                    why
                );
                OUTPUT.info(&msg);
                info!("{}", msg);
            } else {
                let msg = format!(
                    "Re-creating the environment of {}, because {}",
                    script.display(),
                    why
                );
                OUTPUT.info(&msg);
                info!("{}", msg);
                fs::remove_dir_all(&envdir)?;
            }
        }
    }

    let manifest_path = envdir.join(RPROJ_MANIFEST_FILE);
    if !manifest_path.exists() {
        if dry_run {
            let msg = format!(
                "Would create an environment for {} in {}",
                script.display(),
                envdir.display()
            );
            OUTPUT.info(&msg);
            info!("{}", msg);
            return Ok(Some(envdir));
        }
        if !envdir.exists() {
            *created = Some(envdir.clone());
        }
    }
    write_env_manifest(&envdir, meta)?;

    if !sync {
        if !envdir.join(RPROJ_LOCK_FILE).exists() {
            proj_lock_host(&envdir, rver, r_arch.as_deref(), false, vec![], args)?;
        }
        return Ok(Some(envdir));
    }

    // `--upgrade` / `--upgrade-package`: solve the existing lock again, the
    // sync below then installs what changed. An environment without a lock
    // yet is solved from scratch anyway.
    // Not every command that gets here has these options.
    let upgrade = args
        .try_get_one::<bool>("upgrade")
        .ok()
        .flatten()
        .copied()
        .unwrap_or(false);
    let upgrade_packages = if args.try_contains_id("upgrade-package").is_ok() {
        parse_upgrade_packages(args)?
    } else {
        vec![]
    };
    let relock = upgrade || !upgrade_packages.is_empty();
    let has_lock = envdir.join(RPROJ_LOCK_FILE).exists();
    if relock && has_lock {
        if dry_run {
            let msg = format!("Would re-lock the environment of {}", script.display());
            OUTPUT.info(&msg);
            info!("{}", msg);
        } else {
            let msg = format!("Re-locking the environment of {}", script.display());
            OUTPUT.info(&msg);
            info!("{}", msg);
            proj_lock_host(
                &envdir,
                rver.clone(),
                r_arch.as_deref(),
                upgrade,
                upgrade_packages,
                args,
            )?;
        }
    }

    match rvenv_sync_needed(&envdir)? {
        None => {}
        Some(why) if dry_run => {
            let msg = format!(
                "Would sync the environment of {} first, because {}",
                script.display(),
                why
            );
            OUTPUT.info(&msg);
            info!("{}", msg);
        }
        Some(why) => {
            let msg = format!(
                "Setting up the environment of {}, because {}",
                script.display(),
                why
            );
            OUTPUT.info(&msg);
            info!("{}", msg);
            if !envdir.join(RPROJ_LOCK_FILE).exists() {
                proj_lock_host(&envdir, rver, r_arch.as_deref(), false, vec![], args)?;
            }
            // An R of another architecture than the machine's, e.g.
            // `4.6.1-x86_64` on an arm64 Mac, needs that arch's lock target
            // and R build, not the machine's.
            let sync_opts = ProjSyncOptions {
                arch: r_arch.filter(|arch| is_foreign_arch(arch)),
                ..Default::default()
            };
            proj_sync(&envdir, &sync_opts, args)?;
        }
    }

    Ok(Some(envdir))
}

/// [`prepare_script_env`] for a script that has a lock file, `lock_path`.
///
/// The environment is synced from the lock file, so it installs the R
/// version and the package versions the lock file names. If the lock file
/// changed since the environment last used it, check that it still fits the
/// script's block, and if it does not, lock again, for the same R versions
/// and platforms, and update the lock file. With `--locked`, fail instead.
/// `--upgrade` and `--upgrade-package` also update the lock file.
///
/// `requested` is `--r-version`. It picks the lock file target, so it is a
/// version number like `4.6` or `4.6.1`, optionally with an architecture,
/// like `4.6.1-x86_64`.
#[allow(clippy::too_many_arguments)]
fn prepare_locked_script_env(
    script: &Path,
    lock_path: &Path,
    meta: &ScriptMeta,
    requested: Option<&str>,
    args: &ArgMatches,
    dry_run: bool,
    sync: bool,
    created: &mut Option<PathBuf>,
) -> Result<Option<PathBuf>, Box<dyn Error>> {
    let label = lock_file_label(lock_path);
    let envdir = script_env_dir(meta, requested, Some(&canonical_lock_path(script)?))?;
    trace!(
        "Environment of {} is at {}",
        script.display(),
        envdir.display()
    );

    if dry_run && !envdir.join(RPROJ_MANIFEST_FILE).exists() {
        let msg = format!(
            "Would create an environment for {} from {} in {}",
            script.display(),
            label,
            envdir.display()
        );
        OUTPUT.info(&msg);
        info!("{}", msg);
        return Ok(Some(envdir));
    }
    if !dry_run {
        if !envdir.exists() {
            *created = Some(envdir.clone());
        }
        write_env_manifest(&envdir, meta)?;
    }

    // Not every command that gets here has these options.
    let flag = |name: &str| {
        args.try_get_one::<bool>(name)
            .ok()
            .flatten()
            .copied()
            .unwrap_or(false)
    };
    let upgrade = flag("upgrade");
    let locked = flag("locked");
    let upgrade_packages = if args.try_contains_id("upgrade-package").is_ok() {
        parse_upgrade_packages(args)?
    } else {
        vec![]
    };

    // The environment has a copy of the lock file. If it is the same, the
    // lock file was already checked against this block (the block is part of
    // the environment's key), and there is nothing to check.
    let env_lock = envdir.join(RPROJ_LOCK_FILE);
    let script_lock = fs::read(lock_path)?;
    let lock_changed = fs::read(&env_lock).ok().as_deref() != Some(&script_lock[..]);
    if lock_changed && dry_run {
        let msg = format!(
            "Would check that {} fits the `{}` block of {}",
            label,
            BLOCK_START,
            script.display()
        );
        OUTPUT.info(&msg);
        info!("{}", msg);
    }

    if !dry_run {
        // Until the lock file is checked, and updated if needed, the copy in
        // the environment must not look like it was, so remove it on errors.
        let result = update_script_lock(
            script,
            lock_path,
            &label,
            &envdir,
            &script_lock,
            lock_changed,
            locked,
            upgrade,
            upgrade_packages,
            args,
        );
        if result.is_err() {
            let _ = fs::remove_file(&env_lock);
        }
        result?;
    } else if upgrade || !upgrade_packages.is_empty() {
        let msg = format!("Would re-lock {} and update {}", script.display(), label);
        OUTPUT.info(&msg);
        info!("{}", msg);
    }

    if !sync {
        return Ok(Some(envdir));
    }

    match rvenv_sync_needed(&envdir)? {
        None => {}
        Some(why) if dry_run => {
            let msg = format!(
                "Would sync the environment of {} from {} first, because {}",
                script.display(),
                label,
                why
            );
            OUTPUT.info(&msg);
            info!("{}", msg);
        }
        Some(why) => {
            let msg = format!(
                "Setting up the environment of {} from {}, because {}",
                script.display(),
                label,
                why
            );
            OUTPUT.info(&msg);
            info!("{}", msg);
            // `--r-version` picks the lock file target, and the R build of
            // that target is what the environment uses. An R of another
            // architecture than the machine's, e.g. `4.6.1-x86_64` on an
            // arm64 Mac, needs that arch's target and R build.
            let (r_version, r_arch) = match requested {
                None => (None, None),
                Some(requested) => match requested.rsplit_once('-') {
                    Some((version, arch)) if arch == "arm64" || arch == "x86_64" => {
                        (Some(version.to_string()), Some(rvenv_r_arch(requested)))
                    }
                    _ => (Some(requested.to_string()), None),
                },
            };
            let sync_opts = ProjSyncOptions {
                r_version,
                arch: r_arch.filter(|arch| is_foreign_arch(arch)),
                ..Default::default()
            };
            if let Err(err) = proj_sync(&envdir, &sync_opts, args) {
                // The lock file has no target for this machine or for
                // `--r-version`: say how to add one.
                if err.to_string().starts_with("No target in") {
                    OUTPUT.info(&format!(
                        "Lock {} for this machine with `rig proj lock --script {}{}`.",
                        script.display(),
                        script.display(),
                        lock_r_versions_arg(lock_path, requested)
                    ));
                }
                return Err(err);
            }
        }
    }

    Ok(Some(envdir))
}

/// The `--r-version` argument of `rig proj lock` that keeps the R versions
/// of the lock file at `lock_path` and adds `requested`, or an empty string
/// if nothing was requested. `rig proj lock` drops the R versions it is not
/// given.
fn lock_r_versions_arg(lock_path: &Path, requested: Option<&str>) -> String {
    let Some(requested) = requested else {
        return String::new();
    };
    let mut versions: Vec<String> = fs::read_to_string(lock_path)
        .ok()
        .and_then(|text| toml::from_str::<RprojLock>(&text).ok())
        .map(|lock| lock.targets.into_iter().map(|t| t.r_version).collect())
        .unwrap_or_default();
    versions.push(requested.to_string());
    let mut seen = std::collections::HashSet::new();
    versions.retain(|v| seen.insert(v.clone()));
    format!(" --r-version {}", versions.join(","))
}

/// Put the lock file of `script` into its environment, if it changed, and
/// check that it fits the block. Lock again and update the lock file if it
/// does not, or for `--upgrade` / `--upgrade-package`. With `--locked`, a lock
/// file that does not fit is an error.
#[allow(clippy::too_many_arguments)]
fn update_script_lock(
    script: &Path,
    lock_path: &Path,
    label: &str,
    envdir: &Path,
    script_lock: &[u8],
    lock_changed: bool,
    locked: bool,
    upgrade: bool,
    upgrade_packages: Vec<(String, String)>,
    args: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let env_lock = envdir.join(RPROJ_LOCK_FILE);
    let mut why = None;
    if lock_changed {
        write_atomically(&env_lock, script_lock)?;
        if !lock_fits_manifest(envdir)? {
            if locked {
                bail!(
                    "{} does not fit the `{}` block of {} any more, update it with \
                     `rig proj lock --script {}`",
                    label,
                    BLOCK_START,
                    script.display(),
                    script.display()
                );
            }
            why = Some(format!(
                "Updating {}, because it does not fit the `{}` block of {}",
                label,
                BLOCK_START,
                script.display()
            ));
        }
    }
    if upgrade || !upgrade_packages.is_empty() {
        why = Some(format!(
            "Re-locking {} and updating {}",
            script.display(),
            label
        ));
    }
    if let Some(msg) = why {
        OUTPUT.info(&msg);
        info!("{}", msg);
        proj_lock_keep_targets(envdir, upgrade, upgrade_packages, label, args)?;
        write_atomically(lock_path, &fs::read(&env_lock)?)?;
    }
    Ok(())
}

/// The text of `script`, its `script` block and the parsed block, or empty
/// metadata if it has no block.
fn read_script(script: &Path) -> Result<(String, Option<ScriptBlock>, ScriptMeta), Box<dyn Error>> {
    let text = fs::read_to_string(script)
        .map_err(|e| format!("Cannot read {}: {}", script.display(), e))?;
    let block = find_script_block(&text).map_err(|e| format!("{}: {}", script.display(), e))?;
    let meta = match &block {
        Some(block) => {
            parse_block_body(&block.body).map_err(|e| format!("{}: {}", script.display(), e))?
        }
        None => ScriptMeta::default(),
    };
    Ok((text, block, meta))
}

/// Write the `script` block with the TOML `body` into `script`, which
/// currently has the content `text`, replacing `block`. Checks that the new
/// block is valid first.
fn write_script_block(
    script: &Path,
    text: &str,
    block: Option<&ScriptBlock>,
    body: &str,
) -> Result<(), Box<dyn Error>> {
    parse_block_body(body).map_err(|e| format!("{}: {}", script.display(), e))?;
    // `fs::write` keeps the file's permissions, e.g. the executable bit of
    // a script with a `#!` line.
    fs::write(script, replace_script_block(text, block, body))?;
    Ok(())
}

/// Create a `script` block in a new or existing R script: `rig proj init
/// --script`.
pub fn sc_proj_init_script(script: &Path, args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let exists = script.exists();
    let (text, block, _meta) = if exists {
        read_script(script)?
    } else {
        (String::new(), None, ScriptMeta::default())
    };
    if block.is_some() && !args.get_flag("force") {
        bail!(
            "{} already has a `{}` block, use --force to replace it",
            script.display(),
            BLOCK_START
        );
    }

    let rver = resolve_project_r_version(args)?;
    let body = format!("[dependencies]\nR = \">= {}\"\n", minor_r_version(&rver)?);
    write_script_block(script, &text, block.as_ref(), &body)?;

    let msg = if !exists {
        format!("Created {}", script.display())
    } else if block.is_some() {
        format!(
            "Replaced the `{}` block of {}",
            BLOCK_START,
            script.display()
        )
    } else {
        format!("Added a `{}` block to {}", BLOCK_START, script.display())
    };
    OUTPUT.success(&msg);
    info!("{}", msg);
    OUTPUT.info(&format!(
        "Script set up for R {}. Next: add dependencies with \
         `rig proj add --script {}`, then run it with `rig run {}`.",
        rver,
        script.display(),
        script.display()
    ));
    Ok(())
}

/// Add dependencies to the `script` block of an R script, creating the block
/// if needed, then set up the script's environment: `rig proj add --script`.
pub fn sc_proj_add_script(script: &Path, args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let (text, block, meta) = read_script(script)?;
    let script_dir = script_dir_of(script)?;
    let target = script.display().to_string();

    // Parse (and fetch) every specification before changing anything, same
    // as `rig proj add`. Local paths are relative to the script.
    let mut specs: Vec<AddSpec> = Vec::new();
    for spec in args.get_many::<String>("package").unwrap_or_default() {
        specs.push(parse_add_arg(spec, &script_dir)?);
    }

    // Edit the block's own TOML document, to keep its comments and
    // formatting, and use a manifest with the same dependencies to work out
    // the new entries and the messages.
    let mut doc: DocumentMut = block.as_ref().map_or("", |b| b.body.as_str()).parse()?;
    let mut manifest = Rproj {
        dependencies: meta.dependencies,
        ..Default::default()
    };
    let mut messages: Vec<String> = Vec::new();
    for spec in specs.iter() {
        messages.push(add_spec_to_manifest(&mut manifest, spec, false, &target));
        let value = manifest
            .dependencies
            .get(spec.name())
            .expect("just inserted by add_spec_to_manifest");
        Rproj::doc_set_dependency(&mut doc, &["dependencies"], spec.name(), value)?;
    }

    let body = doc.to_string();
    write_script_block(script, &text, block.as_ref(), &body)?;
    for msg in messages.iter() {
        OUTPUT.success(msg);
        info!("{}", msg);
    }

    update_script_env(script, &text, &body, args)
}

/// Remove dependencies from the `script` block of an R script, then set up
/// the script's new environment: `rig proj remove --script`.
pub fn sc_proj_remove_script(script: &Path, args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let (text, block, meta) = read_script(script)?;
    let Some(block) = block else {
        bail!("{} has no `{}` block", script.display(), BLOCK_START);
    };

    let mut names: Vec<String> = Vec::new();
    for name in args.get_many::<String>("package").unwrap_or_default() {
        if !names.contains(name) {
            names.push(name.clone());
        }
    }
    // All or none, same as `rig proj remove`.
    let missing: Vec<&str> = names
        .iter()
        .filter(|name| !meta.dependencies.contains_key(name.as_str()))
        .map(|name| name.as_str())
        .collect();
    if !missing.is_empty() {
        bail!(
            "Not a {} in {}: {}",
            if missing.len() == 1 {
                "dependency"
            } else {
                "dependencies"
            },
            script.display(),
            missing.join(", ")
        );
    }

    let mut doc: DocumentMut = block.body.parse()?;
    let mut messages: Vec<String> = Vec::new();
    for name in names.iter() {
        Rproj::doc_remove_dependency(&mut doc, name);
        messages.push(format!("Removed {} from {}", name, script.display()));
    }

    let body = doc.to_string();
    write_script_block(script, &text, Some(&block), &body)?;
    for msg in messages.iter() {
        OUTPUT.success(msg);
        info!("{}", msg);
    }

    update_script_env(script, &text, &body, args)
}

/// After `rig proj add --script` or `rig proj remove --script` changed the
/// block of `script` to `body`: lock the script's environment and, without
/// `--no-sync`, sync it, unless `--no-lock`. If locking or syncing fails,
/// put back the `original` content of the script, and remove the
/// environment if it was new, so a failed command leaves no trace.
fn update_script_env(
    script: &Path,
    original: &str,
    body: &str,
    args: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    if args.get_flag("no-lock") {
        OUTPUT.info(&format!(
            "Next: run `rig run {}` to set up its environment.",
            script.display()
        ));
        return Ok(());
    }

    let mut meta = parse_block_body(body)?;
    meta.absolutize_paths(&script_dir_of(script)?);
    let sync = !args.get_flag("no-sync");
    let mut created = None;
    // The lock file of the script, if it has one, is updated, too.
    let lock_path = script_lock_path(script);
    let original_lock = fs::read(&lock_path).ok();
    if let Err(err) = prepare_script_env(script, &meta, args, false, sync, &mut created) {
        fs::write(script, original)?;
        if let Some(lock) = original_lock {
            fs::write(&lock_path, lock)?;
        }
        if let Some(envdir) = created {
            let _ = fs::remove_dir_all(envdir);
        }
        let msg = format!(
            "Could not resolve the dependencies, {} unchanged",
            script.display()
        );
        OUTPUT.error(&msg);
        error!("{}", msg);
        return Err(err);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_block_means_no_metadata() {
        let text = "library(cli)\n# a comment\ncli::cli_text('hi')\n";
        assert_eq!(parse_script_metadata(text).unwrap(), None);
    }

    #[test]
    fn a_valid_block_is_parsed() {
        let text = "\
# /// script
# [dependencies]
# R = \">= 4.4\"
# cli = \"*\"
# glue = { git = \"https://github.com/tidyverse/glue\" }
#
# [tool.rig]
# exclude-newer = \"2026-06-01\"
# ///
library(cli)
";
        let meta = parse_script_metadata(text).unwrap().unwrap();
        assert_eq!(meta.dependencies.len(), 3);
        assert_eq!(
            meta.dependencies.get("R"),
            Some(&Dependency::Version(">= 4.4".to_string()))
        );
        assert!(matches!(
            meta.dependencies.get("glue"),
            Some(Dependency::Detailed(_))
        ));
        assert_eq!(
            meta.tool["rig"]["exclude-newer"].as_str(),
            Some("2026-06-01")
        );
    }

    #[test]
    fn a_shebang_can_come_first() {
        let text =
            "#!/usr/bin/env -S rig run\n# /// script\n# [dependencies]\n# cli = \"*\"\n# ///\n";
        let meta = parse_script_metadata(text).unwrap().unwrap();
        assert!(meta.dependencies.contains_key("cli"));
    }

    #[test]
    fn the_space_after_the_hash_is_optional() {
        let text = "# /// script\n#[dependencies]\n#cli = \"*\"\n# ///\n";
        let meta = parse_script_metadata(text).unwrap().unwrap();
        assert!(meta.dependencies.contains_key("cli"));
    }

    #[test]
    fn crlf_line_endings_work() {
        let text = "# /// script\r\n# [dependencies]\r\n# cli = \"*\"\r\n# ///\r\n";
        let meta = parse_script_metadata(text).unwrap().unwrap();
        assert!(meta.dependencies.contains_key("cli"));
    }

    #[test]
    fn an_empty_block_is_valid() {
        let text = "# /// script\n# ///\n";
        assert_eq!(
            parse_script_metadata(text).unwrap(),
            Some(ScriptMeta::default())
        );
    }

    #[test]
    fn an_unclosed_block_is_an_error() {
        let text = "# /// script\n# [dependencies]\n# cli = \"*\"\n";
        let err = parse_script_metadata(text).unwrap_err().to_string();
        assert!(err.contains("no closing"), "{}", err);
    }

    #[test]
    fn a_code_line_inside_the_block_is_an_error() {
        let text = "# /// script\n# [dependencies]\nlibrary(cli)\n# ///\n";
        let err = parse_script_metadata(text).unwrap_err().to_string();
        assert!(err.contains("line 3"), "{}", err);
    }

    #[test]
    fn a_double_hash_block_is_parsed() {
        let text =
            "## /// script\n## [dependencies]\n##sessioninfo = \"*\"\n##\n## ///\nR.home()\n";
        let meta = parse_script_metadata(text).unwrap().unwrap();
        assert!(meta.dependencies.contains_key("sessioninfo"));
    }

    #[test]
    fn a_double_hash_block_needs_double_hash_lines() {
        let text = "## /// script\n# [dependencies]\n## ///\n";
        let err = parse_script_metadata(text).unwrap_err().to_string();
        assert!(err.contains("does not start with `##`"), "{}", err);
        // and a `#` closing line does not close it
        let text = "## /// script\n## [dependencies]\n# ///\n";
        assert!(parse_script_metadata(text).is_err());
    }

    #[test]
    fn two_blocks_are_an_error() {
        let text = "# /// script\n# ///\n# /// script\n# ///\n";
        let err = parse_script_metadata(text).unwrap_err().to_string();
        assert!(err.contains("more than one"), "{}", err);
    }

    #[test]
    fn an_unknown_key_is_an_error() {
        let text = "# /// script\n# [dependancies]\n# cli = \"*\"\n# ///\n";
        let err = parse_script_metadata(text).unwrap_err().to_string();
        assert!(err.contains("dependancies"), "{}", err);
    }

    #[test]
    fn other_block_types_are_ignored() {
        let text = "# /// other\n# ///\n";
        assert_eq!(parse_script_metadata(text).unwrap(), None);
    }

    #[test]
    fn relative_path_dependencies_are_made_absolute() {
        let text = "# /// script\n# [dependencies]\n# mypkg = { path = \"pkg\" }\n# ///\n";
        let mut meta = parse_script_metadata(text).unwrap().unwrap();
        let dir = std::env::temp_dir();
        meta.absolutize_paths(&dir);
        match meta.dependencies.get("mypkg") {
            Some(Dependency::Detailed(table)) => {
                assert_eq!(table.path, Some(dir.join("pkg").display().to_string()));
            }
            other => panic!("unexpected dependency: {:?}", other),
        }
    }

    #[test]
    fn the_env_key_is_stable_and_depends_on_the_content() {
        let a = parse_script_metadata(
            "# /// script\n# [dependencies]\n# cli = \"*\"\n# glue = \"*\"\n# ///\n",
        )
        .unwrap()
        .unwrap();
        // Same content, different order and formatting.
        let b = parse_script_metadata(
            "# /// script\n# [dependencies]\n# glue   = \"*\"\n# cli = \"*\"\n# ///\n",
        )
        .unwrap()
        .unwrap();
        let c = parse_script_metadata("# /// script\n# [dependencies]\n# cli = \"*\"\n# ///\n")
            .unwrap()
            .unwrap();
        assert_eq!(
            a.env_key(None, None).unwrap(),
            b.env_key(None, None).unwrap()
        );
        assert_ne!(
            a.env_key(None, None).unwrap(),
            c.env_key(None, None).unwrap()
        );
        assert_ne!(
            a.env_key(None, None).unwrap(),
            a.env_key(Some("4.5"), None).unwrap()
        );
        assert_eq!(a.env_key(None, None).unwrap().len(), 16);
    }

    #[test]
    fn a_lock_file_gives_a_separate_env_key() {
        let meta = parse_script_metadata("# /// script\n# [dependencies]\n# cli = \"*\"\n# ///\n")
            .unwrap()
            .unwrap();
        let lock = Path::new("/home/me/a.R.lock");
        let other = Path::new("/home/me/b.R.lock");
        let plain = meta.env_key(None, None).unwrap();
        let locked = meta.env_key(None, Some(lock)).unwrap();
        assert_ne!(plain, locked);
        assert_eq!(locked, meta.env_key(None, Some(lock)).unwrap());
        assert_ne!(locked, meta.env_key(None, Some(other)).unwrap());
        assert_ne!(locked, meta.env_key(Some("4.5"), Some(lock)).unwrap());
    }

    #[test]
    fn the_lock_file_of_a_script_is_next_to_it() {
        assert_eq!(
            script_lock_path(Path::new("analysis.R")),
            PathBuf::from("analysis.R.lock")
        );
        assert_eq!(
            script_lock_path(Path::new("/tmp/dir/run-me")),
            PathBuf::from("/tmp/dir/run-me.lock")
        );
    }

    #[test]
    fn the_manifest_has_only_the_scripts_dependencies() {
        let meta = parse_script_metadata("# /// script\n# [dependencies]\n# cli = \"*\"\n# ///\n")
            .unwrap()
            .unwrap();
        let manifest = meta.to_manifest();
        assert_eq!(manifest.project.name, "script");
        assert_eq!(manifest.dependencies.len(), 2);
        assert!(manifest.dependencies.contains_key("cli"));
        // no R requirement means any R, but R is always there
        assert_eq!(
            manifest.dependencies.get("R"),
            Some(&Dependency::Version("*".to_string()))
        );

        let meta =
            parse_script_metadata("# /// script\n# [dependencies]\n# R = \">= 4.4\"\n# ///\n")
                .unwrap()
                .unwrap();
        assert_eq!(
            meta.to_manifest().dependencies.get("R"),
            Some(&Dependency::Version(">= 4.4".to_string()))
        );
    }

    #[test]
    fn find_script_block_reports_the_lines_and_marker() {
        let text = "#!/usr/bin/env -S rig run\n## /// script\n## [dependencies]\n## ///\n1\n";
        let block = find_script_block(text).unwrap().unwrap();
        assert_eq!(block.marker, "##");
        assert_eq!((block.start, block.end), (1, 3));
        assert_eq!(block.body, "[dependencies]\n");
    }

    #[test]
    fn a_new_block_goes_to_the_top() {
        let body = "[dependencies]\nR = \">= 4.6\"\n";
        assert_eq!(
            replace_script_block("", None, body),
            "# /// script\n# [dependencies]\n# R = \">= 4.6\"\n# ///\n"
        );
        assert_eq!(
            replace_script_block("library(cli)\n", None, body),
            "# /// script\n# [dependencies]\n# R = \">= 4.6\"\n# ///\n\nlibrary(cli)\n"
        );
    }

    #[test]
    fn a_new_block_goes_after_the_shebang() {
        let out = replace_script_block("#!/usr/bin/env -S rig run\n1\n", None, "[dependencies]\n");
        assert_eq!(
            out,
            "#!/usr/bin/env -S rig run\n# /// script\n# [dependencies]\n# ///\n\n1\n"
        );
        // without a newline after the shebang
        let out = replace_script_block("#!/usr/bin/env -S rig run", None, "[dependencies]\n");
        assert_eq!(
            out,
            "#!/usr/bin/env -S rig run\n# /// script\n# [dependencies]\n# ///\n"
        );
    }

    #[test]
    fn replacing_a_block_keeps_the_rest_the_marker_and_the_line_endings() {
        let text = "x <- 1\r\n## /// script\r\n## [dependencies]\r\n## ///\r\ny <- 2\r\n";
        let block = find_script_block(text).unwrap().unwrap();
        let out = replace_script_block(text, Some(&block), "[dependencies]\n\ncli = \"*\"\n");
        assert_eq!(
            out,
            "x <- 1\r\n## /// script\r\n## [dependencies]\r\n##\r\n## cli = \"*\"\r\n## ///\r\ny <- 2\r\n"
        );
        let meta = parse_script_metadata(&out).unwrap().unwrap();
        assert!(meta.dependencies.contains_key("cli"));
    }

    #[test]
    fn editing_the_block_document_keeps_its_comments() {
        let text = "# /// script\n# [dependencies]\n# # for the output\n# cli = \"*\"\n# ///\n";
        let block = find_script_block(text).unwrap().unwrap();
        let mut doc: DocumentMut = block.body.parse().unwrap();
        Rproj::doc_set_dependency(
            &mut doc,
            &["dependencies"],
            "glue",
            &Dependency::Version(">= 1.6".to_string()),
        )
        .unwrap();
        let out = replace_script_block(text, Some(&block), &doc.to_string());
        assert_eq!(
            out,
            "# /// script\n# [dependencies]\n# # for the output\n# cli = \"*\"\n# glue = \">= 1.6\"\n# ///\n"
        );
        assert!(Rproj::doc_remove_dependency(&mut doc, "cli"));
        let out = replace_script_block(text, Some(&block), &doc.to_string());
        assert_eq!(
            out,
            "# /// script\n# [dependencies]\n# glue = \">= 1.6\"\n# ///\n"
        );
    }
}
