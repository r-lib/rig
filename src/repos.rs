use std::error::Error;

use clap::ArgMatches;

pub fn sc_repos(args: &ArgMatches, mainargs: &ArgMatches)
              -> Result<(), Box<dyn Error>> {

    match args.subcommand() {
        Some(("deps", s)) => sc_repos_list_packages(s, args, mainargs),
        _ => Ok(()), // unreachable
    }
}

fn sc_repos_list_packages(
    args: &ArgMatches,
    libargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {

    Ok(())
}
