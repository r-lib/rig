use std::error::Error;

use clap::ArgMatches;
use tabular::*;

use crate::common::get_r_version_data_version;
use crate::common::sc_get_default_or_fail;
use crate::repos::*;

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

    let has_bioc = repos
        .iter()
        .any(|x| x.url.contains("%v") || x.url.contains("%bm"));
    if !args.get_flag("raw") && has_bioc {
        let numver = get_r_version_data_version(&rver)?;
        let biocver = r_version_to_bioc_version(&numver)?;
        let biocmirror = match env::var("R_BIOC_MIRROR") {
            Ok(v) => v,
            Err(_) => "https://bioconductor.org".to_string(),
        };
        for repo in repos.iter_mut() {
            repo.url = repo.url.replace("%v", &biocver).replace("%bm", &biocmirror);
        }
    }

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
