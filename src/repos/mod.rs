use std::env;
use std::error::Error;

use clap::ArgMatches;

use crate::common::{check_installed, sc_get_default_or_fail};
use crate::escalate::escalate;
use crate::hardcoded::*;
use log::debug;

#[cfg(target_os = "macos")]
use crate::macos::*;

#[cfg(target_os = "windows")]
use crate::windows::*;

#[cfg(target_os = "linux")]
use crate::linux::*;

mod config;
pub use config::{get_repos_config, RepoEntry, Repository};
mod configured;
mod interpret_repos_args;
mod repos_add;
use repos_add::sc_repos_add;
mod repos_enable;
use repos_enable::{sc_repos_disable, sc_repos_enable};
mod repos_rm;
use repos_rm::sc_repos_rm;
pub mod state;
pub use interpret_repos_args::interpret_repos_args;
mod repos_available;
use repos_available::sc_repos_available;
mod repos_list;
use repos_list::sc_repos_list;
mod repos_status;
use repos_status::sc_repos_status;
pub mod cranlike_metadata;
pub mod feed;
pub use cranlike_metadata::DbSourcePackageLoader;
pub mod binaries;
mod setup;
pub use setup::repos_setup;

pub fn sc_repos(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        Some(("add", s)) => sc_repos_add(s, args, mainargs),
        Some(("available", s)) => sc_repos_available(s, args, mainargs),
        Some(("disable", s)) => sc_repos_disable(s, args, mainargs),
        Some(("enable", s)) => sc_repos_enable(s, args, mainargs),
        Some(("list", s)) => sc_repos_list(s, args, mainargs),
        Some(("rm", s)) => sc_repos_rm(s, args, mainargs),
        Some(("setup", s)) => sc_repos_setup(s, args, mainargs),
        Some(("status", s)) => sc_repos_status(s, args, mainargs),
        _ => Ok(()), // unreachable
    }
}

pub fn r_version_to_bioc_version(rver: &str) -> Result<String, Box<dyn Error>> {
    match env::var("R_BIOC_VERSION") {
        Ok(biocver) => Ok(biocver),
        Err(_) => match bioc_version_for_r_minor(rver, &today()) {
            Some(biocver) => Ok(biocver),
            None => {
                bail!(
                    "Cannot determine Bioconductor version for R version {}, \n\
                    set R_BIOC_VERSION environment variable to override.",
                    rver
                );
            }
        },
    }
}

/// The Bioconductor version to use with R version `rver`: `R_BIOC_VERSION`
/// if set, else `pinned` (the manifest's choice), else the release of the
/// hard-coded mapping, see [`bioc_version_for_r_minor`]. `cutoff` is the
/// `--exclude-newer` day, `YYYY-MM-DD`, it defaults to today. `None` if R
/// version has no Bioconductor release.
pub fn bioc_version_for(rver: &str, pinned: Option<&str>, cutoff: Option<&str>) -> Option<String> {
    if let Ok(biocver) = env::var("R_BIOC_VERSION") {
        return Some(biocver);
    }
    if let Some(biocver) = pinned {
        return Some(biocver.to_string());
    }
    let date = cutoff.map(|c| c.to_string()).unwrap_or_else(today);
    bioc_version_for_r_minor(rver, &date)
}

/// The Bioconductor release for the minor version of `rver`, as of `date`
/// (`YYYY-MM-DD`). The date only chooses between the releases of the same R
/// minor version: the newest one released by `date`, or the oldest one, if
/// none of them is. It never picks the release of another R minor version.
fn bioc_version_for_r_minor(rver: &str, date: &str) -> Option<String> {
    let minor = r_minor(rver)?;
    let releases: Vec<&BiocVersionMapping> = HC_BIOC_VERSIONS
        .iter()
        .filter(|m| m.r_version == minor)
        .collect();
    let released = releases
        .iter()
        .rev()
        .find(|m| m.release_date.as_deref().is_some_and(|d| d <= date));
    released
        .or(releases.first())
        .map(|m| m.bioc_version.clone())
}

/// The newest Bioconductor release, as of today. For commands that have no
/// R version to go by.
pub fn latest_bioc_release() -> Option<String> {
    let date = today();
    HC_BIOC_VERSIONS
        .iter()
        .rev()
        .find(|m| {
            m.release_date
                .as_deref()
                .is_some_and(|d| d <= date.as_str())
        })
        .map(|m| m.bioc_version.clone())
}

/// `major.minor` of an R version, also of an installed R name like
/// `4.5-arm64`.
fn r_minor(rver: &str) -> Option<String> {
    lazy_static::lazy_static! {
        static ref MINOR: regex::Regex = regex::Regex::new(r"^(\d+)\.(\d+)").unwrap();
    }
    let caps = MINOR.captures(rver)?;
    Some(format!("{}.{}", &caps[1], &caps[2]))
}

fn today() -> String {
    jiff::Zoned::now().date().to_string()
}

/// The repository names given on the command line, lowercase, the way
/// `--with-repos` matches them.
fn repo_names_arg(args: &ArgMatches) -> Vec<String> {
    let mut names: Vec<String> = vec![];
    for name in args.get_many::<String>("name").into_iter().flatten() {
        let name = name.trim().to_lowercase();
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// The R installations a command applies to: `--all-versions`, the
/// `--r-version` options, or the default installation.
fn target_versions(args: &ArgMatches) -> Result<Vec<String>, Box<dyn Error>> {
    if args.get_flag("all-versions") {
        return sc_get_list();
    }
    match args.get_many::<String>("r-version") {
        Some(vers) => {
            let mut result: Vec<String> = vec![];
            for ver in vers {
                let ver = check_installed(ver)?;
                if !result.contains(&ver) {
                    result.push(ver);
                }
            }
            Ok(result)
        }
        None => Ok(vec![sc_get_default_or_fail()?]),
    }
}

/// Ask for administrator rights if the R installations `vers` have files
/// that [`repos_setup`] needs to update, but the current user cannot write.
/// This is typical for admin mode installations on Linux and Windows. On
/// macOS admin users can usually update them without `sudo`.
fn escalate_if_needed(vers: &[String], task: &str) -> Result<(), Box<dyn Error>> {
    let unwritable = setup::repos_setup_unwritable(vers)?;
    if !unwritable.is_empty() {
        for path in unwritable.iter() {
            debug!("Cannot write {}", path.display());
        }
        escalate(task)?;
    }
    Ok(())
}

fn sc_repos_setup(
    args: &ArgMatches,
    _libargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let vers: Vec<String> = if args.contains_id("r-version") {
        vec![args.get_one::<String>("r-version").unwrap().to_string()]
    } else {
        sc_get_list()?
    };

    let setup = interpret_repos_args(args, false);
    escalate_if_needed(&vers, "setting up package repositories")?;
    repos_setup(Some(vers), setup)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bioc_version_is_the_newest_release_of_the_r_minor() {
        assert_eq!(
            bioc_version_for_r_minor("4.5.1", "2026-01-01").as_deref(),
            Some("3.22")
        );
        assert_eq!(
            bioc_version_for_r_minor("4.4", "2026-01-01").as_deref(),
            Some("3.20")
        );
    }

    #[test]
    fn unreleased_bioc_versions_are_skipped() {
        // 3.24 maps to R 4.6 too, but it has no release date yet.
        assert_eq!(
            bioc_version_for_r_minor("4.6.0", "2026-10-01").as_deref(),
            Some("3.23")
        );
    }

    #[test]
    fn the_date_chooses_between_releases_of_the_same_r_minor() {
        assert_eq!(
            bioc_version_for_r_minor("4.5.0", "2025-06-01").as_deref(),
            Some("3.21")
        );
        assert_eq!(
            bioc_version_for_r_minor("4.5.0", "2025-10-30").as_deref(),
            Some("3.22")
        );
    }

    #[test]
    fn the_date_never_moves_to_another_r_minor() {
        // Before 3.17 was released, but 3.17 is still the oldest for R 4.3.
        assert_eq!(
            bioc_version_for_r_minor("4.3.2", "2020-01-01").as_deref(),
            Some("3.17")
        );
    }

    #[test]
    fn installed_r_names_map_to_their_minor_version() {
        assert_eq!(
            bioc_version_for_r_minor("4.5-arm64", "2026-01-01").as_deref(),
            Some("3.22")
        );
    }

    #[test]
    fn unknown_r_versions_have_no_bioc_version() {
        assert_eq!(bioc_version_for_r_minor("4.9.0", "2026-01-01"), None);
        assert_eq!(bioc_version_for_r_minor("devel", "2026-01-01"), None);
    }

    #[test]
    fn a_pinned_version_wins() {
        if env::var("R_BIOC_VERSION").is_ok() {
            return;
        }
        assert_eq!(
            bioc_version_for("4.6.0", Some("3.24"), None).as_deref(),
            Some("3.24")
        );
    }
}
