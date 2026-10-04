use std::error::Error;

use clap::ArgMatches;
use regex::Regex;

use crate::output::OUTPUT;

use super::config::{builtin_repo_names, get_custom_repos, save_custom_repos, CustomRepo};
use super::interpret_repos_args::ReposSetupArgs;
use super::{escalate_if_needed, repos_setup, target_versions};

pub fn sc_repos_add(
    args: &ArgMatches,
    _libargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let name = args.get_one::<String>("name").unwrap();
    let url = args.get_one::<String>("url").unwrap();
    let enable = args.get_flag("enable");

    let repo = CustomRepo {
        name: check_repo_name(name)?,
        url: check_repo_url(url)?,
        title: args.get_one::<String>("title").cloned(),
        description: args.get_one::<String>("description").cloned(),
        // `--enable --all-versions`: R versions installed later get it, too.
        default: enable && args.get_flag("all-versions"),
    };

    // Work out the R versions before changing anything, so an invalid
    // `--r-version` does not leave a half-done job behind.
    let vers = if enable {
        Some(target_versions(args)?)
    } else {
        None
    };

    let lname = repo.name.to_lowercase();
    if builtin_repo_names()
        .iter()
        .any(|b| b.to_lowercase() == lname)
    {
        bail!(
            "Repository '{}' is built into rig, use a different name",
            repo.name
        );
    }

    let mut custom = get_custom_repos()?;
    match custom.iter().position(|r| r.name.to_lowercase() == lname) {
        Some(idx) if custom[idx] == repo => {}
        Some(idx) => {
            if !args.get_flag("force") {
                bail!(
                    "Repository '{}' already exists, use `--force` to replace it",
                    custom[idx].name
                );
            }
            custom[idx] = repo.clone();
        }
        None => custom.push(repo.clone()),
    }

    // If this re-runs rig with `sudo`, the repository is saved by that
    // process.
    if let Some(vers) = &vers {
        escalate_if_needed(vers, "enabling package repositories")?;
    }
    save_custom_repos(&custom)?;

    match vers {
        Some(vers) => {
            repos_setup(
                Some(vers.clone()),
                ReposSetupArgs::Default {
                    whitelist: vec![lname],
                    blacklist: vec![],
                },
            )?;
            if repo.default {
                OUTPUT.success(&format!(
                    "Added repository {} and enabled it for all R versions, \
                     including the ones you install later",
                    repo.name
                ));
            } else {
                OUTPUT.success(&format!(
                    "Added repository {} and enabled it for R {}",
                    repo.name,
                    vers.join(", ")
                ));
            }
        }
        None => {
            OUTPUT.success(&format!(
                "Added repository {}, enable it with `rig repos enable {}`",
                repo.name, repo.name
            ));
        }
    }

    Ok(())
}

fn check_repo_name(name: &str) -> Result<String, Box<dyn Error>> {
    let re = Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._/-]*$")?;
    if !re.is_match(name) {
        bail!(
            "Invalid repository name: '{}'. Use letters, numbers and '.', '_', '/', '-', \
             starting with a letter or number",
            name
        );
    }
    Ok(name.to_string())
}

fn check_repo_url(url: &str) -> Result<String, Box<dyn Error>> {
    let url = url.trim();
    let lower = url.to_lowercase();
    if !["http://", "https://", "file://"]
        .iter()
        .any(|p| lower.starts_with(p))
    {
        bail!(
            "Invalid repository URL: '{}'. It must start with 'https://', 'http://' or 'file://'",
            url
        );
    }
    if url.contains(char::is_whitespace) {
        bail!(
            "Invalid repository URL: '{}'. It must not contain spaces",
            url
        );
    }
    Ok(url.trim_end_matches('/').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_names() {
        for name in ["acme", "Acme-CRAN", "r-universe/acme", "a.b_c", "4ever"] {
            assert_eq!(check_repo_name(name).unwrap(), name);
        }
    }

    #[test]
    fn invalid_names() {
        for name in ["", "-acme", "/acme", "ac me", "acme\t", "ac\"me"] {
            assert!(check_repo_name(name).is_err(), "{}", name);
        }
    }

    #[test]
    fn urls_lose_trailing_slash() {
        assert_eq!(
            check_repo_url("https://cran.acme.com/").unwrap(),
            "https://cran.acme.com"
        );
        assert_eq!(
            check_repo_url("file:///srv/cran").unwrap(),
            "file:///srv/cran"
        );
    }

    #[test]
    fn bioconductor_variables_are_allowed() {
        // A URL must still have a scheme.
        assert!(check_repo_url("%bm/packages/%v/bioc").is_err());
        assert!(check_repo_url("https://mirror.acme.com/packages/%v/bioc").is_ok());
    }

    #[test]
    fn invalid_urls() {
        for url in ["cran.acme.com", "ftp://cran.acme.com", "https://a b"] {
            assert!(check_repo_url(url).is_err(), "{}", url);
        }
    }
}
