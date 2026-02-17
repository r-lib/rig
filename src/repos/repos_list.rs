use std::error::Error;

use clap::ArgMatches;
use tabular::*;

use crate::common::sc_get_default_or_fail;
use crate::repositories::*;

#[cfg(target_os = "macos")]
use crate::macos::*;

#[cfg(target_os = "windows")]
use crate::windows::*;

#[cfg(target_os = "linux")]
use crate::linux::*;

pub fn sc_repos_list(
    args: &ArgMatches,
    _libargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let rver = match args.get_one::<String>("r-version") {
        Some(v) => v.to_string(),
        None => sc_get_default_or_fail()?,
    };
    let all = args.get_flag("all");

    let root: String = get_r_root();
    let repositories = root.clone() + "/" + &R_ETC_PATH.replace("{}", &rver) + "/repositories";
    let mut repos = read_repositories_file(&repositories)?.data;

    if !all {
        repos = repos.into_iter().filter(|x| x.default).collect();
    }
    repos.sort_by(|a, b| b.default.cmp(&a.default));

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
