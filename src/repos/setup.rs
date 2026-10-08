use regex::Regex;
use std::error::Error;
use std::path::{Path, PathBuf};

use globset::Glob;
use log::{debug, error, warn};

use crate::common::*;
use crate::dcf::*;
use crate::hardcoded::*;
use crate::output::OUTPUT;
use crate::repositories::*;
use crate::utils::*;

use super::{
    config::{get_repos_config, Enabled, RepoEntry, Repository},
    interpret_repos_args::ReposSetupArgs,
    state::{get_setup_state, save_setup_state, SetupState},
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

// Fail if `setup` names a repository that is neither in `config` nor in
// `r_names`, the (lowercase) names of R's own repositories, see
// [`r_own_repo_names`].
fn validate_repos_in_setup(
    config: &[Repository],
    r_names: &[String],
    setup: &ReposSetupArgs,
) -> Result<(), Box<dyn Error>> {
    let mut valid_repo_names: Vec<String> = config
        .iter()
        .map(|r| r.name.to_lowercase())
        .chain(r_names.iter().cloned())
        .collect::<Vec<_>>();
    valid_repo_names.sort();
    valid_repo_names.dedup();

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
        let inv = invalid_repos.join(", ");
        let val = valid_repo_names.join(", ");
        let msg = format!(
            "Invalid repository name(s): {}. Valid repositories are: {}",
            inv, val
        );
        OUTPUT.error(&msg);
        error!("{}", msg);
        bail!("{}", msg);
    }

    Ok(())
}

/// Fail if any of `names` (lowercase) is not a known repository: a rig
/// repository, or one of R's own repositories of an installation in `vers`.
pub fn validate_repo_names(names: &[String], vers: &[String]) -> Result<(), Box<dyn Error>> {
    let config = get_repos_config()?;
    validate_repos_in_setup(
        &config,
        &r_own_repo_names_of(&config, vers)?,
        &ReposSetupArgs::Empty {
            whitelist: names.to_vec(),
        },
    )
}

// The lowercase names of the entries of rig's repositories, e.g. `biocsoft`.
fn rig_entry_names(config: &[Repository]) -> Vec<String> {
    config
        .iter()
        .flat_map(|r| r.repos.iter())
        .map(|e| e.name.to_lowercase())
        .collect()
}

// Whether a `repositories` file entry is one of R's own repositories, i.e.
// not one that rig sets up, e.g. `CRANextra` and `R-Forge`.
fn is_r_own_repo(entry: &RepoFileEntry, rig_names: &[String]) -> bool {
    !rig_names.contains(&entry.name.to_lowercase())
}

/// The lowercase names of R's own repositories of installation `ver`, the
/// entries of its original `repositories` file that rig does not set up
/// itself. These can be enabled and disabled the same way as rig's
/// repositories. Empty if the installation has no `repositories` file.
pub fn r_own_repo_names(config: &[Repository], ver: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let repositories = repositories_file(ver)?;
    let orig = repositories.clone() + ".orig";
    let path = if PathBuf::from(&orig).exists() {
        orig
    } else if PathBuf::from(&repositories).exists() {
        repositories
    } else {
        return Ok(vec![]);
    };
    let rig_names = rig_entry_names(config);
    Ok(read_repositories_file(&path)?
        .data
        .iter()
        .filter(|e| is_r_own_repo(e, &rig_names))
        .map(|e| e.name.to_lowercase())
        .collect())
}

// [`r_own_repo_names`] of all installations in `vers`.
fn r_own_repo_names_of(
    config: &[Repository],
    vers: &[String],
) -> Result<Vec<String>, Box<dyn Error>> {
    let mut names = vec![];
    for ver in vers {
        names.extend(r_own_repo_names(config, &check_installed(ver)?)?);
    }
    names.sort();
    names.dedup();
    Ok(names)
}

// Turn R's own repositories in `repos` (the original `repositories` file) on
// or off, according to `setup`. The ones that are not mentioned keep R's
// setting, unless `setup` starts from no repositories.
fn set_r_own_repos(repos: &mut RepositoriesContents, rig_names: &[String], setup: &ReposSetupArgs) {
    for entry in repos.data.iter_mut() {
        if is_r_own_repo(entry, rig_names) {
            entry.default = should_include_repo(setup, &entry.name, entry.default);
        }
    }
}

/// Set up the repositories of R installations.
///
/// `setup` (from `--with-repos`, `--without-repos`, `rig repos enable`, etc.)
/// is merged into the stored repository choices of each installation, see
/// [`SetupState::merge`], and the result is stored and written to the
/// installation's `repositories` file. Rig's repositories that are selected
/// are added to the file, and R's own repositories, see [`r_own_repo_names`],
/// are turned on or off.
pub fn repos_setup(vers: Option<Vec<String>>, setup: ReposSetupArgs) -> Result<(), Box<dyn Error>> {
    let vers = match vers {
        Some(v) => v,
        None => sc_get_list()?,
    };
    let config = get_repos_config()?;

    // Validate that all repositories in whitelist and blacklist exist
    validate_repos_in_setup(&config, &r_own_repo_names_of(&config, &vers)?, &setup)?;
    let rig_names = rig_entry_names(&config);

    for ver in vers {
        let ver = check_installed(&ver.to_string())?;
        let repositories = repositories_file(&ver)?;

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

        let rdata = get_r_data(&ver)?;
        debug!("Detected architecture {:?}", rdata);

        let stored = stored_setup_state(&ver, &config, &rdata, &repositories, &orig)?;
        let state = stored.merge(&setup);
        save_setup_state(&ver, Some(state.clone()))?;
        let setup = state.to_args();

        debug!("Updating repositories file at {}", repositories);
        let repos = build_repositories(&orig, &config, &rig_names, &rdata, &setup)?;
        write_repositories_file(repos, &repositories)?;

        let profile = profile_file(&ver)?;
        debug!("Updating R profile at {}", profile);
        let mut profile_lines = read_lines(Path::new(&profile))?;

        // maybe already current?
        if profile_is_current(&profile_lines)? {
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
        } else if start.is_empty() && end.is_empty() {
            // nothing there, nothing to remove
        } else {
            OUTPUT.warn(&format!(
                "Corrupt R profile at {}, try reinstalling R. If the issue perists, report it to rig developers.",
                profile
            ));
            warn!("Corrupt R profile at {}, try reinstalling R. If the issue perists, report it to rig developers.", profile);
            continue;
        }

        profile_lines.push(HC_PROFILE_REPOS.to_string());
        std::fs::write(&profile, profile_lines.join("\n"))?;
    }

    Ok(())
}

// The stored repository choices of installation `ver`, or if there are none,
// the ones worked out from its `repositories` file and the original one.
fn stored_setup_state(
    ver: &str,
    config: &[Repository],
    rdata: &RData,
    repositories: &str,
    orig: &str,
) -> Result<SetupState, Box<dyn Error>> {
    Ok(match get_setup_state(ver)? {
        Some(state) => state,
        None => {
            let current = read_repositories_file(repositories)?;
            let original = read_repositories_file(orig)?;
            let state = infer_setup_state(&current, &original, config, rdata)?;
            debug!("Inferred repository setup for {}: {:?}", ver, state);
            state
        }
    })
}

// The contents of the `repositories` file for `setup`: the original file
// `orig`, with R's own repositories turned on or off, and the selected rig
// repositories added. The entries of the original file that are rig
// repositories, e.g. R's `@CRAN@`, are off, so `--without-repos=cran` turns
// off CRAN. If CRAN is selected, rig adds its own entry for it.
fn build_repositories(
    orig: &str,
    config: &[Repository],
    rig_names: &[String],
    rdata: &RData,
    setup: &ReposSetupArgs,
) -> Result<RepositoriesContents, Box<dyn Error>> {
    let mut repos = read_repositories_file(orig)?;
    set_r_own_repos(&mut repos, rig_names, setup);
    for entry in repos.data.iter_mut() {
        if !is_r_own_repo(entry, rig_names) {
            entry.default = false;
        }
    }

    add_repositories_comment(&mut repos, "start added by rig");
    let selected = selected_entries(config, rdata, |repo, enabled_default| {
        should_include_repo(setup, &repo.name, enabled_default)
    })?;
    for (_, entry) in selected {
        add_repository(&mut repos, entry);
    }
    add_repositories_comment(&mut repos, "end added by rig");
    Ok(repos)
}

/// The repositories that installation `ver` would use if `setup` was applied
/// on top of its repository choices, like `rig repos enable` and
/// `rig repos disable` do, but without storing anything or changing its
/// `repositories` file. Only the entries that are on, in the order of the
/// file. URLs are not resolved, see [`super::resolve_bioc_vars`].
///
/// `None` if the installation has no `repositories` file.
pub(crate) fn repos_with_setup(
    ver: &str,
    setup: &ReposSetupArgs,
) -> Result<Option<Vec<RepoFileEntry>>, Box<dyn Error>> {
    let ver = check_installed(&ver.to_string())?;
    let config = get_repos_config()?;
    validate_repos_in_setup(&config, &r_own_repo_names(&config, &ver)?, setup)?;

    let repositories = repositories_file(&ver)?;
    if !PathBuf::from(&repositories).exists() {
        return Ok(None);
    }
    let orig: String = repositories.clone() + ".orig";
    let orig = if PathBuf::from(&orig).exists() {
        orig
    } else {
        repositories.clone()
    };

    let rdata = get_r_data(&ver)?;
    let stored = stored_setup_state(&ver, &config, &rdata, &repositories, &orig)?;
    let setup = stored.merge(setup).to_args();
    let rig_names = rig_entry_names(&config);
    let mut repos = build_repositories(&orig, &config, &rig_names, &rdata, &setup)?.data;
    repos.retain(|x| x.default);
    Ok(Some(repos))
}

// Compose the full platform string that platform globs are matched against,
// e.g. "x86_64-pc-linux-gnu-ubuntu-22.04" or "x86_64-pc-linux-gnu-manylinux-2.34".
fn rdata_platform_string(rdata: &RData) -> String {
    let mut platform = rdata.platform.clone();
    if let Some(p) = &rdata.distro {
        platform += "-";
        platform += p;
    }
    if let Some(r) = &rdata.release {
        platform += "-";
        platform += r;
    }
    platform
}

fn profile_file(ver: &str) -> Result<String, Box<dyn Error>> {
    let root: String = get_r_root_for(ver)?;
    Ok(root + "/" + &get_r_base_profile()?.replace("{}", &version_dir_key(ver)))
}

// Whether the R profile already has the current version of rig's repository
// setup code.
fn profile_is_current(lines: &[String]) -> Result<bool, Box<dyn Error>> {
    let re = Regex::new(&HC_PROFILE_REPOS_MARKERS.current_start.to_string())?;
    Ok(!grep_lines(&re, lines).is_empty())
}

/// The files and directories [`repos_setup`] would write for the R
/// installations `vers`, that the current user cannot write. Empty if it
/// can run without administrator rights.
pub fn repos_setup_unwritable(vers: &[String]) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut result = vec![];
    for ver in vers {
        let ver = check_installed(ver)?;
        let repositories = PathBuf::from(repositories_file(&ver)?);
        if !repositories.exists() {
            continue;
        }
        if !file_is_writable(&repositories) {
            result.push(repositories.clone());
        }
        // The `.orig` copy is created in the same directory, on first use.
        let orig = PathBuf::from(repositories.display().to_string() + ".orig");
        if !orig.exists() {
            if let Some(dir) = repositories.parent() {
                if !dir_is_writable(dir) {
                    result.push(dir.to_path_buf());
                }
            }
        }
        let profile = PathBuf::from(profile_file(&ver)?);
        if !profile_is_current(&read_lines(&profile)?)? && !file_is_writable(&profile) {
            result.push(profile);
        }
    }
    Ok(result)
}

// Opening for writing, without truncating, does not change the file.
fn file_is_writable(path: &Path) -> bool {
    std::fs::OpenOptions::new().write(true).open(path).is_ok()
}

fn dir_is_writable(path: &Path) -> bool {
    tempfile::Builder::new()
        .prefix(".rig-write-test-")
        .tempfile_in(path)
        .is_ok()
}

fn repositories_file(ver: &str) -> Result<String, Box<dyn Error>> {
    let root: String = get_r_root_for(ver)?;
    Ok(root + "/" + &get_r_etc_path()?.replace("{}", &version_dir_key(ver)) + "/repositories")
}

// The repository choices of an installation that was set up before rig stored
// them, worked out from its `repositories` file: the repositories rig added
// that are not defaults were enabled, the defaults it did not add were
// disabled.
fn infer_setup_state(
    current: &RepositoriesContents,
    orig: &RepositoriesContents,
    config: &[Repository],
    rdata: &RData,
) -> Result<SetupState, Box<dyn Error>> {
    let mut state = SetupState::default();
    if !current
        .comments
        .iter()
        .any(|(_, c)| c.contains("added by rig"))
    {
        // Not set up by rig at all.
        return Ok(state);
    }

    let added: Vec<&RepoFileEntry> = current
        .data
        .iter()
        .filter(|e| {
            e.default
                && !orig
                    .data
                    .iter()
                    .any(|o| o.default && o.name == e.name && o.url == e.url)
        })
        .collect();

    let defaults = selected_entries(config, rdata, |_, enabled_default| enabled_default)?;
    for repo in config.iter() {
        let default = defaults.iter().any(|(r, _)| r.name == repo.name);
        let present = repo
            .repos
            .iter()
            .any(|e| added.iter().any(|a| a.name == e.name));
        let name = repo.name.to_lowercase();
        if present && !default {
            state.enable.push(name);
        } else if !present && default {
            state.disable.push(name);
        }
    }

    Ok(state)
}

/// The repositories in `names` (lowercase) that have no URL for the R
/// installation `ver`, i.e. that cannot be enabled for it.
pub fn repos_not_applicable(ver: &str, names: &[String]) -> Result<Vec<String>, Box<dyn Error>> {
    let config = get_repos_config()?;
    let rdata = get_r_data(ver)?;
    let r_names = r_own_repo_names(&config, ver)?;
    let mut result = vec![];
    for name in names {
        let Some(repo) = config.iter().find(|r| &r.name.to_lowercase() == name) else {
            if !r_names.contains(name) {
                result.push(name.clone());
            }
            continue;
        };
        let mut applies = false;
        for entry in repo.repos.iter() {
            if should_activate_repo(repo, entry, &rdata)? {
                applies = true;
                break;
            }
        }
        if !applies {
            result.push(repo.name.clone());
        }
    }
    Ok(result)
}

// The URLs to set up for an installation, in catalog order: the URLs that
// apply to it, of the repositories that `include` selects. `include` gets
// whether the URL is enabled by default; an entry's `enabled` (if present)
// overrides the repo's, and it can depend on the installation's platform
// (e.g. P3M-manylinux is a default only on manylinux). A fallback URL is
// dropped if another selected URL has the same metadata, e.g. P3M's source
// URL if P3M-manylinux is set up.
fn selected_entries<'a>(
    config: &'a [Repository],
    rdata: &RData,
    include: impl Fn(&Repository, bool) -> bool,
) -> Result<Vec<(&'a Repository, &'a RepoEntry)>, Box<dyn Error>> {
    let rdata_platform = rdata_platform_string(rdata);
    let mut selected = vec![];
    for repo in config.iter() {
        for entry in repo.repos.iter() {
            let enabled = entry.enabled.as_ref().unwrap_or(&repo.enabled);
            let enabled_default = enabled_by_default(enabled, &rdata_platform, &repo.name);
            if include(repo, enabled_default) && should_activate_repo(repo, entry, rdata)? {
                selected.push((repo, entry));
            }
        }
    }
    let specific: Vec<String> = selected
        .iter()
        .filter(|(_, e)| !e.fallback)
        .filter_map(|(_, e)| e.metadata.clone())
        .collect();
    Ok(selected
        .into_iter()
        .filter(|(_, e)| !e.fallback || e.metadata.as_ref().is_none_or(|m| !specific.contains(m)))
        .collect())
}

// Whether `rdata_platform` matches any of the given platform globs. Invalid
// globs are warned about and skipped. `ctx` is the repo name, for messages.
fn platform_matches_any(platforms: &[String], rdata_platform: &str, ctx: &str) -> bool {
    for platform in platforms.iter() {
        let glob = match Glob::new(platform) {
            Ok(g) => g.compile_matcher(),
            Err(e) => {
                OUTPUT.warn(&format!(
                    "Invalid platform glob '{}' in repo '{}', skipping: {}",
                    platform, ctx, e
                ));
                warn!(
                    "Invalid platform glob '{}' in repo '{}', skipping: {}",
                    platform, ctx, e
                );
                continue;
            }
        };
        if glob.is_match(rdata_platform) {
            debug!("Repo '{}' matches platform glob '{}'", ctx, platform);
            return true;
        }
    }
    false
}

// Whether a repo/entry with this `enabled` setting is enabled by default for the
// given (composed) platform string.
fn enabled_by_default(enabled: &Enabled, rdata_platform: &str, ctx: &str) -> bool {
    match enabled {
        Enabled::Always(b) => *b,
        Enabled::OnPlatforms { platforms } => platform_matches_any(platforms, rdata_platform, ctx),
    }
}

// Whether `repo_name` should be added at all, given the whitelist/blacklist
// setup and whether it's enabled by default. A blacklisted repo is always
// excluded, even if it would otherwise be enabled by default.
fn should_include_repo(setup: &ReposSetupArgs, repo_name: &str, enabled_default: bool) -> bool {
    let repo_name = repo_name.to_lowercase();
    match setup {
        ReposSetupArgs::Default {
            whitelist,
            blacklist,
        } => {
            if blacklist.contains(&repo_name) {
                return false;
            }
            enabled_default || whitelist.contains(&repo_name)
        }
        ReposSetupArgs::Empty { whitelist } => whitelist.contains(&repo_name),
    }
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
    if let Some(platforms) = &entry.platforms {
        let rdata_platform = rdata_platform_string(rdata);
        if !platform_matches_any(platforms, &rdata_platform, &repo.name) {
            debug!(
                "Repo '{}' (platform {}) does not match any platform glob, skipping",
                repo.name, rdata_platform
            );
            return Ok(false);
        }
    }

    // if archs are present, then they must match the current arch
    if let Some(archs) = &entry.archs {
        let mut ok = false;
        for arch in archs.iter() {
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
    if let Some(rversions) = &entry.rversions {
        let mut ok = false;
        for constraint in rversions.iter() {
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

fn get_r_data_common(ver: &str) -> Result<RData, Box<dyn Error>> {
    let root: String = get_r_root_for(ver)?;
    let statsdesc = root
        + "/"
        + &get_r_syslibpath()?.replace("{}", &version_dir_key(ver))
        + "/stats/DESCRIPTION";
    debug!("Getting architectture from {}.", statsdesc);
    let lines = read_lines(Path::new(&statsdesc))?;
    let re = Regex::new("^Built:[ ]?")?;
    let bltidx = grep_lines(&re, &lines);
    if bltidx.is_empty() {
        OUTPUT.error(&format!(
            "Could not find 'Built' field in {}, cannot determine architecture of R installation.",
            statsdesc
        ));
        error!(
            "Could not find 'Built' field in {}, cannot determine architecture of R installation.",
            statsdesc
        );
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
        OUTPUT.error(&format!(
            "Could not parse 'Built' field in {}, unexpected format: {}",
            statsdesc, blt
        ));
        error!(
            "Could not parse 'Built' field in {}, unexpected format: {}",
            statsdesc, blt
        );
        bail!("Could not parse 'Built' field in {}: {}", statsdesc, blt);
    }

    let platform = parts[1].trim();
    let parts2: Vec<&str> = platform.splitn(3, '-').collect();
    if parts2.len() < 3 {
        OUTPUT.error(&format!(
            "Could not parse 'Built' field in {}, unexpected platform format: {}",
            statsdesc, platform
        ));
        error!(
            "Could not parse 'Built' field in {}, unexpected platform format: {}",
            statsdesc, platform
        );
        bail!("Could not parse 'Built' field in {}: {}", statsdesc, blt);
    }

    let arch = parts2[0];

    if arch.is_empty() {
        OUTPUT.error(&format!(
            "Could not parse 'Built' field in {}, missing architecture: {}",
            statsdesc, blt,
        ));
        error!(
            "Could not parse 'Built' field in {}, missing architecture: {}",
            statsdesc, blt
        );
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

#[cfg(target_os = "linux")]
fn get_r_data(ver: &str) -> Result<RData, Box<dyn Error>> {
    let mut data = get_r_data_common(ver)?;

    let install_dir = PathBuf::from(get_r_root_for(ver)?).join(version_dir_key(ver));
    match read_install_platform(&install_dir) {
        Some(platform) => {
            debug!("Installation {} has recorded platform {}", ver, platform);
            let parts: Vec<&str> = platform.splitn(3, '-').collect();
            if parts.len() == 3 {
                data.distro = Some(parts[1].to_string());
                data.release = Some(parts[2].to_string());
            }
        }
        None => {
            debug!(
                "Installation {} has no metadata.json platform, detecting host distro",
                ver
            );
            let os = detect_platform()?;
            data.distro = os.distro;
            data.release = os.version;
        }
    }
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

#[cfg(test)]
mod tests {
    use super::{
        enabled_by_default, infer_setup_state, rdata_platform_string, selected_entries,
        set_r_own_repos, should_activate_repo, should_include_repo, validate_repos_in_setup, RData,
    };
    use crate::repos::config::{Enabled, RepoEntry, Repository};
    use crate::repos::interpret_repos_args::ReposSetupArgs;
    use crate::repos::state::SetupState;
    use crate::repositories::{RepoFileEntry, RepositoriesContents};

    fn make_repo(name: &str) -> Repository {
        Repository {
            name: name.to_string(),
            title: None,
            description: None,
            enabled: Enabled::Always(true),
            repos: vec![],
            custom: false,
        }
    }

    fn make_entry() -> RepoEntry {
        RepoEntry {
            name: "test".to_string(),
            title: None,
            description: None,
            url: "https://example.com".to_string(),
            metadata: None,
            platforms: None,
            archs: None,
            rversions: None,
            enabled: None,
            fallback: false,
        }
    }

    fn rdata(platform: &str, arch: &str, version: &str) -> RData {
        RData {
            platform: platform.to_string(),
            arch: arch.to_string(),
            version: version.to_string(),
            distro: None,
            release: None,
        }
    }

    // --- validate_repos_in_setup ---

    #[test]
    fn validate_all_valid_names() {
        let config = vec![make_repo("CRAN"), make_repo("P3M")];
        let setup = ReposSetupArgs::Default {
            whitelist: vec!["cran".to_string()],
            blacklist: vec!["p3m".to_string()],
        };
        assert!(validate_repos_in_setup(&config, &[], &setup).is_ok());
    }

    #[test]
    fn validate_invalid_whitelist_errors() {
        let config = vec![make_repo("CRAN")];
        let setup = ReposSetupArgs::Default {
            whitelist: vec!["bioc".to_string()],
            blacklist: vec![],
        };
        assert!(validate_repos_in_setup(&config, &[], &setup).is_err());
    }

    #[test]
    fn validate_invalid_blacklist_errors() {
        let config = vec![make_repo("CRAN")];
        let setup = ReposSetupArgs::Default {
            whitelist: vec![],
            blacklist: vec!["p3m".to_string()],
        };
        assert!(validate_repos_in_setup(&config, &[], &setup).is_err());
    }

    #[test]
    fn validate_empty_lists_ok() {
        let config = vec![make_repo("CRAN")];
        let setup = ReposSetupArgs::Default {
            whitelist: vec![],
            blacklist: vec![],
        };
        assert!(validate_repos_in_setup(&config, &[], &setup).is_ok());
    }

    #[test]
    fn validate_empty_setup_valid_name_ok() {
        let config = vec![make_repo("CRAN")];
        let setup = ReposSetupArgs::Empty {
            whitelist: vec!["cran".to_string()],
        };
        assert!(validate_repos_in_setup(&config, &[], &setup).is_ok());
    }

    #[test]
    fn validate_empty_setup_invalid_name_errors() {
        let config = vec![make_repo("CRAN")];
        let setup = ReposSetupArgs::Empty {
            whitelist: vec!["bioc".to_string()],
        };
        assert!(validate_repos_in_setup(&config, &[], &setup).is_err());
    }

    // --- should_activate_repo ---

    #[test]
    fn activate_no_constraints_returns_true() {
        let repo = make_repo("Test");
        let entry = make_entry();
        assert!(should_activate_repo(&repo, &entry, &rdata("linux", "x86_64", "4.4.0")).unwrap());
    }

    #[test]
    fn activate_platform_glob_matches() {
        let repo = make_repo("Test");
        let mut entry = make_entry();
        entry.platforms = Some(vec!["linux*".to_string()]);
        assert!(should_activate_repo(&repo, &entry, &rdata("linux", "x86_64", "4.4.0")).unwrap());
    }

    #[test]
    fn activate_platform_glob_no_match() {
        let repo = make_repo("Test");
        let mut entry = make_entry();
        entry.platforms = Some(vec!["macos*".to_string()]);
        assert!(!should_activate_repo(&repo, &entry, &rdata("linux", "x86_64", "4.4.0")).unwrap());
    }

    #[test]
    fn activate_platform_with_distro_matches() {
        let repo = make_repo("Test");
        let mut entry = make_entry();
        entry.platforms = Some(vec!["linux-ubuntu*".to_string()]);
        let mut rd = rdata("linux", "x86_64", "4.4.0");
        rd.distro = Some("ubuntu".to_string());
        rd.release = Some("22.04".to_string());
        assert!(should_activate_repo(&repo, &entry, &rd).unwrap());
    }

    #[test]
    fn activate_platform_with_distro_no_match() {
        let repo = make_repo("Test");
        let mut entry = make_entry();
        entry.platforms = Some(vec!["linux-ubuntu*".to_string()]);
        let mut rd = rdata("linux", "x86_64", "4.4.0");
        rd.distro = Some("fedora".to_string());
        rd.release = Some("42".to_string());
        assert!(!should_activate_repo(&repo, &entry, &rd).unwrap());
    }

    #[test]
    fn activate_manylinux_compat_matches_manylinux_install() {
        // The P3M-manylinux repo is compatible with (can be activated on) any
        // glibc Linux install, including manylinux user-mode installs.
        let repo = make_repo("P3M-manylinux");
        let mut entry = make_entry();
        entry.platforms = Some(vec!["*-linux-gnu-*".to_string()]);
        let mut rd = rdata("x86_64-pc-linux-gnu", "x86_64", "4.4.0");
        rd.distro = Some("manylinux".to_string());
        rd.release = Some("2.34".to_string());
        assert!(should_activate_repo(&repo, &entry, &rd).unwrap());
    }

    #[test]
    fn activate_manylinux_compat_matches_distro_install() {
        // It is also compatible with a regular distro install, so that it can be
        // enabled there on request (it is just not a default there).
        let repo = make_repo("P3M-manylinux");
        let mut entry = make_entry();
        entry.platforms = Some(vec!["*-linux-gnu-*".to_string()]);
        let mut rd = rdata("x86_64-pc-linux-gnu", "x86_64", "4.4.0");
        rd.distro = Some("ubuntu".to_string());
        rd.release = Some("22.04".to_string());
        assert!(should_activate_repo(&repo, &entry, &rd).unwrap());
    }

    #[test]
    fn activate_arch_matches() {
        let repo = make_repo("Test");
        let mut entry = make_entry();
        entry.archs = Some(vec!["x86_64".to_string()]);
        assert!(should_activate_repo(&repo, &entry, &rdata("linux", "x86_64", "4.4.0")).unwrap());
    }

    #[test]
    fn activate_arch_no_match() {
        let repo = make_repo("Test");
        let mut entry = make_entry();
        entry.archs = Some(vec!["aarch64".to_string()]);
        assert!(!should_activate_repo(&repo, &entry, &rdata("linux", "x86_64", "4.4.0")).unwrap());
    }

    #[test]
    fn activate_rversion_constraint_satisfied() {
        let repo = make_repo("Test");
        let mut entry = make_entry();
        entry.rversions = Some(vec![">= 4.0".to_string()]);
        assert!(should_activate_repo(&repo, &entry, &rdata("linux", "x86_64", "4.4.0")).unwrap());
    }

    #[test]
    fn activate_rversion_constraint_not_satisfied() {
        let repo = make_repo("Test");
        let mut entry = make_entry();
        entry.rversions = Some(vec!["< 3.5".to_string()]);
        assert!(!should_activate_repo(&repo, &entry, &rdata("linux", "x86_64", "4.4.0")).unwrap());
    }

    #[test]
    fn activate_all_constraints_must_pass() {
        let repo = make_repo("Test");
        let mut entry = make_entry();
        entry.platforms = Some(vec!["linux*".to_string()]);
        entry.archs = Some(vec!["aarch64".to_string()]); // won't match x86_64
        entry.rversions = Some(vec![">= 4.0".to_string()]);
        assert!(!should_activate_repo(&repo, &entry, &rdata("linux", "x86_64", "4.4.0")).unwrap());
    }

    // --- enabled_by_default ---

    fn manylinux_platform() -> String {
        let mut rd = rdata("x86_64-pc-linux-gnu", "x86_64", "4.4.0");
        rd.distro = Some("manylinux".to_string());
        rd.release = Some("2.34".to_string());
        rdata_platform_string(&rd)
    }

    fn ubuntu_platform() -> String {
        let mut rd = rdata("x86_64-pc-linux-gnu", "x86_64", "4.4.0");
        rd.distro = Some("ubuntu".to_string());
        rd.release = Some("22.04".to_string());
        rdata_platform_string(&rd)
    }

    #[test]
    fn enabled_always() {
        assert!(enabled_by_default(
            &Enabled::Always(true),
            &ubuntu_platform(),
            "Test"
        ));
        assert!(!enabled_by_default(
            &Enabled::Always(false),
            &ubuntu_platform(),
            "Test"
        ));
    }

    #[test]
    fn enabled_on_platforms_default_on_manylinux() {
        // P3M-manylinux is a default on manylinux installs ...
        let enabled = Enabled::OnPlatforms {
            platforms: vec!["*-linux-gnu-manylinux-*".to_string()],
        };
        assert!(enabled_by_default(
            &enabled,
            &manylinux_platform(),
            "P3M-manylinux"
        ));
    }

    #[test]
    fn enabled_on_platforms_optional_on_other_linux() {
        // ... but only optional (off by default) on other glibc Linux installs.
        let enabled = Enabled::OnPlatforms {
            platforms: vec!["*-linux-gnu-manylinux-*".to_string()],
        };
        assert!(!enabled_by_default(
            &enabled,
            &ubuntu_platform(),
            "P3M-manylinux"
        ));
    }

    // --- should_include_repo ---
    // Regression tests for https://github.com/r-lib/rig/issues/369:
    // a blacklisted repo (e.g. `--without-p3m`) must be excluded even when
    // it is enabled by default.

    #[test]
    fn blacklisted_repo_excluded_even_if_enabled_by_default() {
        let setup = ReposSetupArgs::Default {
            whitelist: vec![],
            blacklist: vec!["p3m".to_string()],
        };
        assert!(!should_include_repo(&setup, "P3M", true));
    }

    #[test]
    fn blacklisted_repo_excluded_even_if_whitelisted() {
        // whitelist + blacklist for the same repo: blacklist wins.
        let setup = ReposSetupArgs::Default {
            whitelist: vec!["p3m".to_string()],
            blacklist: vec!["p3m".to_string()],
        };
        assert!(!should_include_repo(&setup, "P3M", false));
    }

    #[test]
    fn non_blacklisted_repo_included_if_enabled_by_default() {
        let setup = ReposSetupArgs::Default {
            whitelist: vec![],
            blacklist: vec!["p3m".to_string()],
        };
        assert!(should_include_repo(&setup, "CRAN", true));
    }

    #[test]
    fn non_default_repo_included_if_whitelisted() {
        let setup = ReposSetupArgs::Default {
            whitelist: vec!["bioc".to_string()],
            blacklist: vec![],
        };
        assert!(should_include_repo(&setup, "BioC", false));
    }

    #[test]
    fn non_default_repo_excluded_if_not_whitelisted() {
        let setup = ReposSetupArgs::Default {
            whitelist: vec![],
            blacklist: vec![],
        };
        assert!(!should_include_repo(&setup, "BioC", false));
    }

    #[test]
    fn empty_mode_only_includes_whitelisted() {
        let setup = ReposSetupArgs::Empty {
            whitelist: vec!["cran".to_string()],
        };
        assert!(should_include_repo(&setup, "CRAN", true));
        assert!(!should_include_repo(&setup, "P3M", true));
    }

    // --- selected_entries ---

    // P3M with a binary URL for Ubuntu and a source fallback, and
    // P3M-manylinux, a default only on manylinux, with the same metadata.
    fn fallback_config() -> Vec<Repository> {
        let entry = |name: &str, url: &str, platforms: Option<&str>, fallback: bool| {
            let mut e = make_entry();
            e.name = name.to_string();
            e.url = url.to_string();
            e.metadata = Some("https://ppm.r-pkg.org".to_string());
            e.platforms = platforms.map(|p| vec![p.to_string()]);
            e.fallback = fallback;
            e
        };
        let mut p3m = make_repo("P3M");
        p3m.repos = vec![
            entry("P3M", "https://p3m.dev/cran/latest", None, true),
            entry(
                "P3M",
                "https://p3m.dev/cran/__linux__/jammy/latest",
                Some("*-linux-gnu-ubuntu-22.04"),
                false,
            ),
        ];
        let mut manylinux = make_repo("P3M-manylinux");
        manylinux.enabled = Enabled::OnPlatforms {
            platforms: vec!["*-linux-gnu-manylinux-*".to_string()],
        };
        manylinux.repos = vec![entry(
            "P3M-manylinux",
            "https://p3m.dev/cran/__linux__/manylinux_2_28/latest",
            Some("*-linux-gnu-*"),
            false,
        )];
        vec![p3m, manylinux]
    }

    fn linux_rdata(distro: &str, release: &str) -> RData {
        let mut rd = rdata("x86_64-pc-linux-gnu", "x86_64", "4.5.1");
        rd.distro = Some(distro.to_string());
        rd.release = Some(release.to_string());
        rd
    }

    fn selected_urls(rd: &RData, blacklist: &[&str]) -> Vec<String> {
        let config = fallback_config();
        let setup = ReposSetupArgs::Default {
            whitelist: vec![],
            blacklist: blacklist.iter().map(|s| s.to_string()).collect(),
        };
        selected_entries(&config, rd, |repo, default| {
            should_include_repo(&setup, &repo.name, default)
        })
        .unwrap()
        .into_iter()
        .map(|(_, e)| e.url.clone())
        .collect()
    }

    #[test]
    fn fallback_dropped_if_a_specific_url_applies() {
        assert_eq!(
            selected_urls(&linux_rdata("ubuntu", "22.04"), &[]),
            vec!["https://p3m.dev/cran/__linux__/jammy/latest"]
        );
    }

    #[test]
    fn fallback_used_if_no_specific_url_applies() {
        assert_eq!(
            selected_urls(&linux_rdata("fedora", "43"), &[]),
            vec!["https://p3m.dev/cran/latest"]
        );
        assert_eq!(
            selected_urls(&rdata("aarch64-apple-darwin20", "aarch64", "4.5.1"), &[]),
            vec!["https://p3m.dev/cran/latest"]
        );
    }

    #[test]
    fn fallback_dropped_for_another_repo_with_the_same_metadata() {
        let manylinux = linux_rdata("manylinux", "2.34");
        assert_eq!(
            selected_urls(&manylinux, &[]),
            vec!["https://p3m.dev/cran/__linux__/manylinux_2_28/latest"]
        );
        // Without P3M-manylinux, P3M's fallback is used.
        assert_eq!(
            selected_urls(&manylinux, &["p3m-manylinux"]),
            vec!["https://p3m.dev/cran/latest"]
        );
    }

    // The P3M URL of the built-in catalog, by default, if any.
    fn catalog_p3m_url(rd: &RData) -> Option<String> {
        let config = crate::repos::config::get_repos_config().unwrap();
        selected_entries(&config, rd, |_, default| default)
            .unwrap()
            .into_iter()
            .find(|(r, _)| r.name == "P3M")
            .map(|(_, e)| e.url.clone())
    }

    #[test]
    fn catalog_p3m_defaults() {
        let latest = Some("https://packagemanager.posit.co/cran/latest".to_string());
        // P3M only has x86_64 Windows binaries.
        assert_eq!(
            catalog_p3m_url(&rdata("aarch64-w64-mingw32", "aarch64", "4.5.1")),
            None
        );
        assert_eq!(
            catalog_p3m_url(&rdata("x86_64-w64-mingw32", "x86_64", "4.5.1")),
            latest
        );
        assert_eq!(
            catalog_p3m_url(&rdata("aarch64-apple-darwin20", "aarch64", "4.5.1")),
            latest
        );
        assert_eq!(catalog_p3m_url(&linux_rdata("fedora", "43")), latest);
        assert_eq!(
            catalog_p3m_url(&linux_rdata("ubuntu", "24.04")),
            Some("https://packagemanager.posit.co/cran/__linux__/noble/latest".to_string())
        );
    }

    // --- infer_setup_state ---

    fn file_entry(name: &str, url: &str, default: bool) -> RepoFileEntry {
        RepoFileEntry {
            name: name.to_string(),
            description: name.to_string(),
            url: url.to_string(),
            default,
            source: true,
            win_binary: true,
            mac_binary: true,
        }
    }

    fn orig_file() -> RepositoriesContents {
        RepositoriesContents {
            data: vec![
                file_entry("CRAN", "@CRAN@", true),
                file_entry("BioCsoft", "%bm/packages/%v/bioc", false),
            ],
            comments: vec![],
        }
    }

    fn rig_file(mut data: Vec<RepoFileEntry>) -> RepositoriesContents {
        let mut all = vec![file_entry("BioCsoft", "%bm/packages/%v/bioc", false)];
        all.append(&mut data);
        RepositoriesContents {
            data: all,
            comments: vec![(1, "## start added by rig".to_string())],
        }
    }

    fn infer_config() -> Vec<Repository> {
        let entry = |name: &str, url: &str| {
            let mut e = make_entry();
            e.name = name.to_string();
            e.url = url.to_string();
            e
        };
        let mut cran = make_repo("CRAN");
        cran.repos = vec![entry("CRAN", "https://cloud.r-project.org")];
        let mut p3m = make_repo("P3M");
        p3m.repos = vec![entry("P3M", "https://p3m.dev/cran/latest")];
        let mut bioc = make_repo("Bioconductor");
        bioc.enabled = Enabled::Always(false);
        bioc.repos = vec![entry(
            "BioCsoft",
            "https://bioconductor.org/packages/3.22/bioc",
        )];
        vec![cran, p3m, bioc]
    }

    #[test]
    fn infer_nothing_if_not_set_up_by_rig() {
        let state = infer_setup_state(
            &orig_file(),
            &orig_file(),
            &infer_config(),
            &rdata("x86_64-apple-darwin20", "x86_64", "4.5.1"),
        )
        .unwrap();
        assert_eq!(state, SetupState::default());
    }

    #[test]
    fn infer_defaults() {
        let current = rig_file(vec![
            file_entry("CRAN", "https://cloud.r-project.org", true),
            file_entry("P3M", "https://p3m.dev/cran/latest", true),
        ]);
        let state = infer_setup_state(
            &current,
            &orig_file(),
            &infer_config(),
            &rdata("x86_64-apple-darwin20", "x86_64", "4.5.1"),
        )
        .unwrap();
        assert_eq!(state, SetupState::default());
    }

    #[test]
    fn infer_enabled_and_disabled_repos() {
        // `--with-repos=bioconductor --without-repos=p3m`
        let current = rig_file(vec![
            file_entry("CRAN", "https://cloud.r-project.org", true),
            file_entry(
                "BioCsoft",
                "https://bioconductor.org/packages/3.22/bioc",
                true,
            ),
        ]);
        let state = infer_setup_state(
            &current,
            &orig_file(),
            &infer_config(),
            &rdata("x86_64-apple-darwin20", "x86_64", "4.5.1"),
        )
        .unwrap();
        assert_eq!(state.enable, vec!["bioconductor".to_string()]);
        assert_eq!(state.disable, vec!["p3m".to_string()]);
    }

    #[test]
    fn infer_r_default_cran_is_not_rigs() {
        // `--without-repos=cran` by an older rig: R's own `@CRAN@` entry
        // stayed on in the file.
        let current = rig_file(vec![
            file_entry("CRAN", "@CRAN@", true),
            file_entry("P3M", "https://p3m.dev/cran/latest", true),
        ]);
        let state = infer_setup_state(
            &current,
            &orig_file(),
            &infer_config(),
            &rdata("x86_64-apple-darwin20", "x86_64", "4.5.1"),
        )
        .unwrap();
        assert!(state.enable.is_empty());
        assert_eq!(state.disable, vec!["cran".to_string()]);
    }

    // --- R's own repositories ---

    fn r_orig_file() -> RepositoriesContents {
        RepositoriesContents {
            data: vec![
                file_entry("CRAN", "@CRAN@", true),
                file_entry("BioCsoft", "%bm/packages/%v/bioc", false),
                file_entry("CRANextra", "https://www.stats.ox.ac.uk/pub/RWin", false),
                file_entry("R-Forge", "https://R-Forge.R-project.org", true),
            ],
            comments: vec![],
        }
    }

    fn defaults(repos: &RepositoriesContents) -> Vec<(String, bool)> {
        repos
            .data
            .iter()
            .map(|e| (e.name.clone(), e.default))
            .collect()
    }

    fn rig_names() -> Vec<String> {
        vec![
            "cran".to_string(),
            "p3m".to_string(),
            "biocsoft".to_string(),
        ]
    }

    #[test]
    fn r_own_repos_enable_and_disable() {
        let mut repos = r_orig_file();
        set_r_own_repos(
            &mut repos,
            &rig_names(),
            &ReposSetupArgs::Default {
                whitelist: vec!["cranextra".to_string()],
                blacklist: vec!["r-forge".to_string(), "cran".to_string()],
            },
        );
        // CRAN and BioCsoft are rig's, they are left alone.
        assert_eq!(
            defaults(&repos),
            vec![
                ("CRAN".to_string(), true),
                ("BioCsoft".to_string(), false),
                ("CRANextra".to_string(), true),
                ("R-Forge".to_string(), false),
            ]
        );
    }

    #[test]
    fn r_own_repos_keep_r_setting() {
        let mut repos = r_orig_file();
        set_r_own_repos(
            &mut repos,
            &rig_names(),
            &ReposSetupArgs::Default {
                whitelist: vec![],
                blacklist: vec![],
            },
        );
        assert_eq!(defaults(&repos), defaults(&r_orig_file()));
    }

    #[test]
    fn r_own_repos_without_repos() {
        let mut repos = r_orig_file();
        set_r_own_repos(
            &mut repos,
            &rig_names(),
            &ReposSetupArgs::Empty {
                whitelist: vec!["cranextra".to_string()],
            },
        );
        assert_eq!(
            defaults(&repos),
            vec![
                ("CRAN".to_string(), true),
                ("BioCsoft".to_string(), false),
                ("CRANextra".to_string(), true),
                ("R-Forge".to_string(), false),
            ]
        );
    }

    #[test]
    fn validate_accepts_r_own_repos() {
        let config = vec![make_repo("CRAN")];
        let setup = ReposSetupArgs::Default {
            whitelist: vec!["cranextra".to_string()],
            blacklist: vec![],
        };
        assert!(validate_repos_in_setup(&config, &[], &setup).is_err());
        assert!(validate_repos_in_setup(&config, &["cranextra".to_string()], &setup).is_ok());
    }
}
