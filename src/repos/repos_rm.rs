use std::error::Error;

use clap::ArgMatches;

use crate::common::find_installed;
use crate::output::OUTPUT;

use super::config::{builtin_repo_names, get_custom_repos, save_custom_repos};
use super::interpret_repos_args::ReposSetupArgs;
use super::state::{get_setup_states, save_setup_states};
use super::{escalate_if_needed, repo_names_arg, repos_setup};

pub fn sc_repos_rm(
    args: &ArgMatches,
    _libargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let names = repo_names_arg(args);

    let builtin: Vec<String> = builtin_repo_names()
        .iter()
        .map(|x| x.to_lowercase())
        .collect();
    let mut custom = get_custom_repos()?;
    for name in names.iter() {
        if builtin.contains(name) {
            bail!(
                "Repository '{}' is built into rig and cannot be removed, \
                 use `rig repos disable` instead",
                name
            );
        }
        if !custom.iter().any(|r| &r.name.to_lowercase() == name) {
            bail!("Unknown repository: '{}'", name);
        }
    }

    // The installations that use (or used) these repositories, their
    // repositories files need updating.
    let mut states = get_setup_states()?;
    let mut affected: Vec<String> = vec![];
    for (ver, state) in states.iter_mut() {
        let mut changed = false;
        for name in names.iter() {
            changed |= state.forget(name);
        }
        if changed && find_installed(ver)?.is_some() {
            affected.push(ver.clone());
        }
    }

    escalate_if_needed(&affected, "removing package repositories")?;

    custom.retain(|r| !names.contains(&r.name.to_lowercase()));
    save_custom_repos(&custom)?;
    save_setup_states(&states)?;

    if !affected.is_empty() {
        repos_setup(
            Some(affected),
            ReposSetupArgs::Default {
                whitelist: vec![],
                blacklist: vec![],
            },
        )?;
    }

    OUTPUT.success(&format!("Removed repository {}", names.join(", ")));
    Ok(())
}
