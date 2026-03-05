use regex::Regex;
use std::error::Error;
use std::path::{Path, PathBuf};

use globset::Glob;
use log::{debug, warn};
use simple_error::*;

use crate::common::*;
use crate::dcf::*;
use crate::hardcoded::*;
use crate::repositories::*;
use crate::utils::*;

use super::{
    config::{get_repos_config, RepoEntry, Repository},
    interpret_repos_args::ReposSetupArgs,
};

#[cfg(target_os = "macos")]
use crate::macos::*;

#[cfg(target_os = "windows")]
use crate::windows::*;

#[cfg(target_os = "linux")]
use crate::linux::*;

#[cfg(target_os = "linux")]
use crate::platform::*;

#[derive(Debug)]
struct RData {
    pub platform: String,
    pub arch: String,    // x86_64, aarch64
    pub version: String, // 4.5.2, etc.
    pub distro: Option<String>,
    pub release: Option<String>,
}

fn validate_repos_in_setup(
    config: &[Repository],
    setup: &ReposSetupArgs,
) -> Result<(), Box<dyn Error>> {
    let mut valid_repo_names: Vec<String> = config
        .iter()
        .map(|r| r.name.to_lowercase())
        .collect::<Vec<_>>();
    valid_repo_names.sort();

    let mut invalid_repos: Vec<String> = Vec::new();

    match setup {
        ReposSetupArgs::Default {
            whitelist,
            blacklist,
        } => {
            for repo in whitelist.iter().chain(blacklist.iter()) {
                if !valid_repo_names.contains(repo) {
                    invalid_repos.push(repo.clone());
                }
            }
        }
        ReposSetupArgs::Empty { whitelist } => {
            for repo in whitelist.iter() {
                if !valid_repo_names.contains(repo) {
                    invalid_repos.push(repo.clone());
                }
            }
        }
    }

    if !invalid_repos.is_empty() {
        invalid_repos.sort();
        invalid_repos.dedup();
        bail!(
            "Invalid repository name(s): {}. Valid repositories are: {}",
            invalid_repos.join(", "),
            valid_repo_names.join(", ")
        );
    }

    Ok(())
}

pub fn repos_setup(vers: Option<Vec<String>>, setup: ReposSetupArgs) -> Result<(), Box<dyn Error>> {
    let vers = match vers {
        Some(v) => v,
        None => sc_get_list()?,
    };
    let config = get_repos_config()?;

    // Validate that all repositories in whitelist and blacklist exist in config
    validate_repos_in_setup(&config, &setup)?;

    let root: String = get_r_root();
    for ver in vers {
        let ver = check_installed(&ver.to_string())?;
        let repositories = root.clone() + "/" + &R_ETC_PATH.replace("{}", &ver) + "/repositories";

        // if no 'repositories' file, skip. Maybe this happens for very old R versions?
        if !PathBuf::from(&repositories).exists() {
            debug!(
                "repositories file does not exist at {}, skipping",
                repositories
            );
            continue;
        }

        // save a copy of the original file, so we can restore later if needed.
        let orig: String = repositories.clone() + ".orig";
        if !PathBuf::from(&orig).exists() {
            debug!(
                "Original repositories file does not exist at {}, copying from {}",
                orig, repositories
            );
            std::fs::copy(&repositories, &orig)?;
        }

        debug!("Updating repositories file at {}", repositories);
        let mut repos = read_repositories_file(&orig)?;

        let rdata = get_r_data(&ver)?;
        debug!("Detected architecture {:?}", rdata);

        add_repositories_comment(&mut repos, "start added by rig");
        for repo in config.iter() {
            let mut enabled = repo.enabled;
            let in_whitelist = match &setup {
                ReposSetupArgs::Default {
                    whitelist,
                    blacklist,
                } => {
                    whitelist.contains(&repo.name.to_lowercase())
                        && !blacklist.contains(&repo.name.to_lowercase())
                }
                ReposSetupArgs::Empty { whitelist } => {
                    enabled = false;
                    whitelist.contains(&repo.name.to_lowercase())
                }
            };

            for entry in repo.repos.iter() {
                let enabled2 = entry.enabled.unwrap_or(enabled);
                if !enabled2 && !in_whitelist {
                    continue;
                }
                if !should_activate_repo(repo, entry, &rdata)? {
                    continue;
                }
                add_repository(&mut repos, entry);
            }
        }
        add_repositories_comment(&mut repos, "end added by rig");

        write_repositories_file(repos, &repositories)?;

        let profile = root.clone() + "/" + &get_r_base_profile(&ver);
        debug!("Updating R profile at {}", profile);
        let mut profile_lines = read_lines(&Path::new(&profile))?;

        // maybe already current?
        if grep_lines(
            &Regex::new(&HC_PROFILE_REPOS_MARKERS.current_start.to_string())?,
            &profile_lines,
        )
        .len()
            > 0
        {
            continue;
        }

        // maybe from another version of rig?
        let start = grep_lines(
            &Regex::new(&HC_PROFILE_REPOS_MARKERS.generic_start.to_string())?,
            &profile_lines,
        );
        let end = grep_lines(
            &Regex::new(&HC_PROFILE_REPOS_MARKERS.end.to_string())?,
            &profile_lines,
        );

        if start.len() == 1 && end.len() == 1 {
            // remove old version
            profile_lines.drain(start[0]..=end[0]);
        } else if start.len() == 0 && end.len() == 0 {
            // nothing there, nothing to remove
        } else {
            warn!("Corrupt R profile at {}, try reinstalling R. If the issue perists, report it to rig developers.", profile);
            continue;
        }

        profile_lines.push(HC_PROFILE_REPOS.to_string());
        std::fs::write(&profile, profile_lines.join("\n"))?;
    }

    Ok(())
}

fn should_activate_repo(
    repo: &Repository,
    entry: &RepoEntry,
    rdata: &RData,
) -> Result<bool, Box<dyn Error>> {
    debug!(
        "Checking if repo '{}' should be activated for platform '{}', arch '{}', R version '{}'",
        repo.name, rdata.platform, rdata.arch, rdata.version
    );

    // if platforms are present, then they must match the current platform
    if entry.platforms.is_some() {
        let mut ok = false;
        let mut rdata_platform = rdata.platform.clone();
        if let Some(p) = &rdata.distro {
            rdata_platform += "-";
            rdata_platform += &p;
        }
        if let Some(r) = &rdata.release {
            rdata_platform += "-";
            rdata_platform += &r;
        }
        for platform in entry.platforms.as_ref().unwrap().iter() {
            let glob = match Glob::new(platform) {
                Ok(g) => g.compile_matcher(),
                Err(e) => {
                    warn!(
                        "Invalid platform glob '{}' in repo '{}', skipping: {}",
                        platform, repo.name, e
                    );
                    continue;
                }
            };
            if glob.is_match(&rdata_platform) {
                debug!("Repo '{}' matches platform glob '{}'", repo.name, platform);
                ok = true;
                break;
            }
        }
        if !ok {
            debug!(
                "Repo '{}' (platform {}) does not match any platform glob, skipping",
                repo.name, rdata_platform
            );
            return Ok(false);
        }
    }

    // if archs are present, then they must match the current arch
    if entry.archs.is_some() {
        let mut ok = false;
        for arch in entry.archs.as_ref().unwrap().iter() {
            if arch == &rdata.arch {
                debug!("Repo '{}' matches arch '{}'", repo.name, arch);
                ok = true;
                break;
            }
        }
        if !ok {
            return Ok(false);
        }
    }

    // if rversions are present, then one of them must be satisfied by the current R version
    if entry.rversions.is_some() {
        let mut ok = false;
        for constraint in entry.rversions.as_ref().unwrap().iter() {
            let depconstraint = VersionConstraint::from_str(constraint)?;
            let dep = DepVersionSpec {
                name: "R".to_string(),
                types: vec![RDepType::Depends],
                constraints: vec![depconstraint],
            };
            if dep.satisfies(&rdata.version)? {
                debug!(
                    "Repo '{}' (R {}) matches R version constraint '{}'",
                    repo.name, rdata.version, constraint
                );
                ok = true;
                break;
            }
        }
        if !ok {
            return Ok(false);
        }
    }

    Ok(true)
}

#[cfg(target_os = "macos")]
fn get_r_data(ver: &str) -> Result<RData, Box<dyn Error>> {
    get_r_data_common(ver)
}

#[cfg(target_os = "linux")]
fn get_r_data(ver: &str) -> Result<RData, Box<dyn Error>> {
    let mut data = get_r_data_common(ver)?;
    let os = detect_platform()?;
    data.distro = os.distro;
    data.release = os.version;
    Ok(data)
}

#[cfg(target_os = "windows")]
fn get_r_data(ver: &str) -> Result<RData, Box<dyn Error>> {
    // TODO: this arch does not work on Windows, because of an R bug:
    // https://bugs.r-project.org/show_bug.cgi?id=19003
    // We need to look for "^BINPREF" in a a Makeconf file, in
    // etc/Makeconf, etc/x64/Makeconf or etc/i386/Makeconf.
    // If this has 'aarch64' then it is an aaarch64 R build.
    get_r_data_common(ver)
}

fn get_r_data_common(ver: &str) -> Result<RData, Box<dyn Error>> {
    let root: String = get_r_root();
    let statsdesc = root + "/" + &R_SYSLIBPATH.replace("{}", ver) + "/stats/DESCRIPTION";
    debug!("Getting architectture from {}.", statsdesc);
    let lines = read_lines(Path::new(&statsdesc))?;
    let re = Regex::new("^Built:[ ]?")?;
    let bltidx = grep_lines(&re, &lines);
    if bltidx.len() == 0 {
        bail!(
            "Could not find 'Built' in {}, cannot determine architecture of R installation.",
            statsdesc
        );
    }
    let blt = &lines[bltidx[0]];

    // Remove "Built:" prefix and split by semicolons
    let built = blt.strip_prefix("Built:").unwrap_or(blt).trim();
    let parts: Vec<&str> = built.split(';').collect();

    if parts.len() < 2 {
        bail!("Could not parse 'Built' field in {}: {}", statsdesc, blt);
    }

    let platform = parts[1].trim();
    let parts2: Vec<&str> = platform.splitn(3, '-').collect();
    if parts2.len() < 3 {
        bail!("Could not parse 'Built' field in {}: {}", statsdesc, blt);
    }

    let arch = parts2[0];

    if arch == "" {
        bail!("Could not parse 'Built' field in {}: {}", statsdesc, blt);
    }

    let rver = parts[0].trim();
    let rver = rver.strip_prefix("R").unwrap_or(rver).trim();

    Ok(RData {
        platform: platform.to_string(),
        arch: arch.to_string(),
        version: rver.to_string(),
        distro: None,
        release: None,
    })
}
