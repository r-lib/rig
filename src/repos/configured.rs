use std::error::Error;

use crate::common::get_r_version_data_version;
use crate::common::sc_get_default_or_fail;
use crate::repos::r_version_to_bioc_version;
use crate::repositories::{read_repositories_file, RepoFileEntry};

#[cfg(target_os = "macos")]
use crate::macos::*;

#[cfg(target_os = "windows")]
use crate::windows::*;

#[cfg(target_os = "linux")]
use crate::linux::*;

/// The repositories an R installation is configured to use, i.e. the contents
/// of its `etc/repositories` file, in the same order, filtered the way
/// `rig repos list` shows them.
pub(crate) struct ConfiguredRepos {
    /// Installation name, e.g. `4.5.1`, `devel`.
    pub rver: String,
    /// Numeric R version of that installation, e.g. `4.5.1`. Only filled in if
    /// it had to be looked up to resolve a Bioconductor URL; use
    /// [`ConfiguredRepos::numeric_version`] to get it either way.
    pub numver: Option<String>,
    pub repos: Vec<RepoFileEntry>,
}

impl ConfiguredRepos {
    /// The numeric R version of the installation, looked up if it is not known
    /// yet.
    pub(crate) fn numeric_version(&self) -> Result<String, Box<dyn Error>> {
        match &self.numver {
            Some(v) => Ok(v.clone()),
            None => get_r_version_data_version(&self.rver),
        }
    }
}

/// Whether the enabled repositories of an installation include a repository
/// of the Package Manager at `addr`.
fn uses_instance(addr: &str, repos: &[RepoFileEntry]) -> bool {
    let prefix = format!("{}/", addr.trim_end_matches('/'));
    repos
        .iter()
        .any(|r| r.default && r.url.starts_with(&prefix))
}

/// Warn (once per run) if a private Package Manager is configured, but
/// installation `rver` does not use it, typically because it was set up
/// before `PACKAGEMANAGER_ADDRESS` or `ppm-address` was set. No warning if
/// the `PPM` group was turned off for this installation on purpose.
fn warn_if_instance_unused(rver: &str, repos: &[RepoFileEntry]) {
    use crate::repos::binaries::{ppm_is_public_instance, ppm_url};
    static WARNED: std::sync::Once = std::sync::Once::new();
    if ppm_is_public_instance() {
        return;
    }
    let addr = ppm_url();
    if uses_instance(&addr, repos) {
        return;
    }
    let disabled = crate::repos::state::get_setup_state(rver)
        .ok()
        .flatten()
        .is_some_and(|s| s.excludes(crate::repos::instance::INSTANCE_GROUP));
    if disabled {
        return;
    }
    WARNED.call_once(|| {
        crate::output::OUTPUT.warn(&format!(
            "R {} does not use the Package Manager at {} (set by \
             PACKAGEMANAGER_ADDRESS or the ppm-address config key). \
             Run `rig repos setup --r-version {}` to switch to it.",
            rver, addr, rver
        ));
    });
}

/// Read the repositories of an R installation.
///
/// `r_version` is an installation name (or alias); `None` means the default
/// installation. Without `all` only the repositories that are enabled by
/// default are returned. With `resolve_vars` the Bioconductor `%v` and `%bm`
/// variables are substituted, which is required for any URL that is meant to
/// be fetched.
pub(crate) fn configured_repos(
    r_version: Option<&str>,
    all: bool,
    resolve_vars: bool,
) -> Result<ConfiguredRepos, Box<dyn Error>> {
    let rver = match r_version {
        Some(v) => v.to_string(),
        None => sc_get_default_or_fail()?,
    };

    let root: String = get_r_root_for(&rver)?;
    let repositories = root.clone()
        + "/"
        + &get_r_etc_path()?.replace("{}", &version_dir_key(&rver))
        + "/repositories";
    let mut repos = read_repositories_file(&repositories)?.data;
    warn_if_instance_unused(&rver, &repos);

    if !all {
        repos.retain(|x| x.default);
    }

    // Only looked up when a Bioconductor URL needs it: an installation with a
    // missing `base/DESCRIPTION` should still list its repositories.
    let numver = if resolve_vars {
        resolve_bioc_vars(&rver, &mut repos)?
    } else {
        None
    };

    Ok(ConfiguredRepos {
        rver,
        numver,
        repos,
    })
}

/// Substitute the Bioconductor `%v` and `%bm` variables in the URLs of
/// `repos`, entries of the `repositories` file of installation `rver`.
/// Returns the numeric R version of the installation, if it had to be looked
/// up, i.e. if there were any variables.
pub(crate) fn resolve_bioc_vars(
    rver: &str,
    repos: &mut [RepoFileEntry],
) -> Result<Option<String>, Box<dyn Error>> {
    let has_bioc = repos
        .iter()
        .any(|x| x.url.contains("%v") || x.url.contains("%bm"));
    if !has_bioc {
        return Ok(None);
    }
    let ver = get_r_version_data_version(rver)?;
    let biocver = r_version_to_bioc_version(&ver)?;
    let biocmirror = super::bioc_mirror();
    for repo in repos.iter_mut() {
        repo.url = repo.url.replace("%v", &biocver).replace("%bm", &biocmirror);
    }
    Ok(Some(ver))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(url: &str, default: bool) -> RepoFileEntry {
        RepoFileEntry {
            name: "x".to_string(),
            description: String::new(),
            url: url.to_string(),
            default,
            source: true,
            win_binary: false,
            mac_binary: false,
        }
    }

    #[test]
    fn instance_use_is_detected_from_enabled_urls() {
        let addr = "https://ppm.acme.com";
        assert!(uses_instance(
            addr,
            &[entry("https://ppm.acme.com/cran/latest", true)]
        ));
        assert!(uses_instance(
            "https://ppm.acme.com/",
            &[entry("https://ppm.acme.com/cran/latest", true)]
        ));
        // Only enabled entries count.
        assert!(!uses_instance(
            addr,
            &[entry("https://ppm.acme.com/cran/latest", false)]
        ));
        assert!(!uses_instance(
            addr,
            &[entry("https://p3m.dev/cran/latest", true)]
        ));
        // Not a prefix of another host.
        assert!(!uses_instance(
            addr,
            &[entry("https://ppm.acme.com.evil.org/cran/latest", true)]
        ));
    }
}
