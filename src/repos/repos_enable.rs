use std::error::Error;

use clap::ArgMatches;

use crate::output::OUTPUT;

use super::interpret_repos_args::ReposSetupArgs;
use super::setup::{repos_not_applicable, validate_repo_names};
use super::{escalate_if_needed, repo_names_arg, repos_setup, target_versions};

pub fn sc_repos_enable(
    args: &ArgMatches,
    _libargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let names = repo_names_arg(args);
    let vers = target_versions(args)?;
    validate_repo_names(&names, &vers)?;

    // Fail early, before `sudo`, if a repository has no URL for a version.
    for ver in vers.iter() {
        let missing = repos_not_applicable(ver, &names)?;
        if !missing.is_empty() {
            bail!(
                "Repository {} has no URL for R {} (platform, architecture or R version \
                 do not match), see `rig repos available <name>`",
                missing.join(", "),
                ver
            );
        }
    }

    escalate_if_needed(&vers, "enabling package repositories")?;
    repos_setup(
        Some(vers.clone()),
        ReposSetupArgs::Default {
            whitelist: names.clone(),
            blacklist: vec![],
        },
    )?;
    OUTPUT.success(&format!(
        "Enabled {} for R {}",
        names.join(", "),
        vers.join(", ")
    ));
    Ok(())
}

pub fn sc_repos_disable(
    args: &ArgMatches,
    _libargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let names = repo_names_arg(args);
    let vers = target_versions(args)?;
    validate_repo_names(&names, &vers)?;

    escalate_if_needed(&vers, "disabling package repositories")?;
    repos_setup(
        Some(vers.clone()),
        ReposSetupArgs::Default {
            whitelist: vec![],
            blacklist: names.clone(),
        },
    )?;
    OUTPUT.success(&format!(
        "Disabled {} for R {}",
        names.join(", "),
        vers.join(", ")
    ));
    Ok(())
}
