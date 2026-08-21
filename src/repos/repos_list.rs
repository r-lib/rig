use std::error::Error;

use clap::ArgMatches;
use tabular::*;

use crate::repos::configured::configured_repos;

pub fn sc_repos_list(
    args: &ArgMatches,
    _libargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let cfg = configured_repos(
        args.get_one::<String>("r-version").map(|x| x.as_str()),
        args.get_flag("all"),
        !args.get_flag("raw"),
    )?;
    let repos = cfg.repos;

    if args.get_flag("json") || mainargs.get_flag("json") {
        println!("{}", serde_json::to_string_pretty(&repos)?);
    } else {
        let mut tab = Table::new("{:<}  {:<}  {:<}  {:<}");
        tab.add_row(row!["name", "description", "url", "default"]);
        tab.add_heading(
            "-----------------------------------------------------------------------------------",
        );
        for repo in repos.iter() {
            tab.add_row(row![
                repo.name.clone(),
                repo.description.clone(),
                repo.url.clone(),
                if repo.default { "X" } else { "" }
            ]);
        }
        println!("{}", tab);
    }
    Ok(())
}
