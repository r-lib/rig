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
use log::{info, trace};
use serde::{Deserialize, Serialize};

use crate::cache::get_cache_dir;
use crate::common::get_r_version_data_version;
use crate::output::OUTPUT;
use crate::proj::{
    is_foreign_arch, parse_upgrade_packages, proj_lock_host, proj_sync, requested_r_installation,
    ProjSyncOptions,
};
use crate::rproj::{Dependency, Repository, Rproj, RPROJ_MANIFEST_FILE};
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

/// The TOML body of the `# /// script` block in `text`, with the comment
/// prefixes removed, or `None` if there is no such block.
///
/// The block can also use `##` instead of `#`, i.e. start with `## ///
/// script`, see [`BLOCK_MARKERS`]. Every line between the opening and the
/// closing line must be a comment with the same marker: either the marker
/// alone, or the marker followed by the content, usually after one space,
/// which is removed. A file can have at most one `script` block, and the
/// block must be closed.
fn extract_script_block(text: &str) -> Result<Option<String>, Box<dyn Error>> {
    let mut body: Option<String> = None;
    let mut lines = text.lines().enumerate();
    while let Some((_, line)) = lines.next() {
        let line = line.trim_end();
        let Some(marker) = BLOCK_MARKERS
            .iter()
            .find(|m| line == format!("{} /// script", m))
        else {
            continue;
        };
        if body.is_some() {
            bail!("more than one `{}` block", BLOCK_START);
        }
        let start = format!("{} /// script", marker);
        let end = format!("{} ///", marker);
        let mut content = String::new();
        let mut closed = false;
        for (lineno, line) in lines.by_ref() {
            let line = line.trim_end_matches('\r');
            if line.trim_end() == end {
                closed = true;
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
        if !closed {
            bail!("the `{}` block has no closing `{}` line", start, end);
        }
        body = Some(content);
    }
    Ok(body)
}

/// The inline metadata of a script, or `None` if it has none.
pub fn parse_script_metadata(text: &str) -> Result<Option<ScriptMeta>, Box<dyn Error>> {
    let body = match extract_script_block(text)? {
        None => return Ok(None),
        Some(body) => body,
    };
    let meta: ScriptMeta = toml::from_str(&body)
        .map_err(|e| format!("invalid TOML in the `{}` block: {}", BLOCK_START, e))?;
    Ok(Some(meta))
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
        manifest.repository = self
            .repository
            .iter()
            .map(|r| Repository {
                name: r.name.clone(),
                url: r.url.clone(),
            })
            .collect();
        manifest.tool = self.tool.clone();
        manifest
    }

    /// The key of the environment of this metadata: the first 16 hex digits
    /// of the hash of its canonical TOML form, plus the R build selected by
    /// `--r-version`, if any, because that changes what gets locked. That is
    /// the R version and architecture of the installation (see
    /// [`requested_r_installation`]), not the argument as given, so `4.6`,
    /// `4.6.1` and `release` share an environment if they select the same R.
    fn env_key(&self, r_version: Option<&str>) -> Result<String, Box<dyn Error>> {
        let mut canonical = toml::to_string(self)?;
        if let Some(rver) = r_version {
            canonical.push_str(&format!("\n# r-version = {}\n", rver));
        }
        Ok(calculate_hash(&canonical)[..16].to_string())
    }
}

/// The cache directory of the environment of `meta`.
fn script_env_dir(meta: &ScriptMeta, r_version: Option<&str>) -> Result<PathBuf, Box<dyn Error>> {
    Ok(get_cache_dir()?
        .join(SCRIPTS_CACHE_SUBDIR)
        .join(meta.env_key(r_version)?))
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

    let script_dir = script
        .canonicalize()?
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    meta.absolutize_paths(&script_dir);

    // Resolve `--r-version` to an installation first, installing it if
    // needed, so the environment is keyed on the R it actually uses.
    let (rver, r_arch) = match args.get_one::<String>("r-version") {
        None => (None, None),
        Some(requested) => match requested_r_installation(requested, dry_run)? {
            Some((version, arch)) => (Some(version), Some(arch)),
            None => {
                let msg = format!("Would create an environment for {}", script.display());
                OUTPUT.info(&msg);
                info!("{}", msg);
                return Ok(Some(format!("<R {} environment>", requested)));
            }
        },
    };
    let r_id = rver
        .as_ref()
        .zip(r_arch.as_ref())
        .map(|(version, arch)| format!("{} {}", version, arch));
    let envdir = script_env_dir(&meta, r_id.as_deref())?;
    let wrapper = project_r_wrapper(&envdir);
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
            return Ok(Some(wrapper.display().to_string()));
        }
        fs::create_dir_all(&envdir)?;
        let manifest = toml::to_string(&meta.to_manifest())?;
        write_atomically(&manifest_path, manifest.as_bytes())?;
    }
    ensure_rvenv_files(&envdir)?;

    // `--upgrade` / `--upgrade-package`: solve the existing lock again, the
    // sync below then installs what changed. An environment without a lock
    // yet is solved from scratch anyway.
    let upgrade = args.get_flag("upgrade");
    let upgrade_packages = parse_upgrade_packages(args)?;
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
        assert_eq!(a.env_key(None).unwrap(), b.env_key(None).unwrap());
        assert_ne!(a.env_key(None).unwrap(), c.env_key(None).unwrap());
        assert_ne!(a.env_key(None).unwrap(), a.env_key(Some("4.5")).unwrap());
        assert_eq!(a.env_key(None).unwrap().len(), 16);
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
}
