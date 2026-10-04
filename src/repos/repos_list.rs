use std::collections::HashMap;
use std::error::Error;

use clap::ArgMatches;
use serde::Serialize;
use tabular::*;

use crate::repos::configured::configured_repos;
use crate::repos::{get_repos_config, repo_metadata_urls};
use crate::repositories::RepoFileEntry;

/// A repository of `rig repos list`, with the rig repository it belongs to
/// and the base URL of its extended metadata, if it has one.
#[derive(Serialize)]
struct ListedRepo {
    #[serde(flatten)]
    entry: RepoFileEntry,
    /// The name of the rig repository (see `rig repos available`) of this
    /// entry, e.g. `Bioconductor` for `BioCsoft`. `None` if the entry is not
    /// from a rig repository.
    group: Option<String>,
    metadata: Option<String>,
}

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
    let metadata = repo_metadata_urls()?;
    let mut groups: HashMap<String, String> = HashMap::new();
    for repo in get_repos_config()? {
        for entry in repo.repos.iter() {
            groups.insert(entry.name.to_lowercase(), repo.name.clone());
        }
    }
    let repos: Vec<ListedRepo> = cfg
        .repos
        .into_iter()
        .map(|entry| ListedRepo {
            group: groups.get(&entry.name.to_lowercase()).cloned(),
            metadata: metadata.get(&entry.name.to_lowercase()).cloned(),
            entry,
        })
        .collect();

    if args.get_flag("json") || mainargs.get_flag("json") {
        println!("{}", serde_json::to_string_pretty(&repos)?);
    } else {
        let mut tab = Table::new("{:<}  {:<}  {:<}  {:<}  {:<}");
        tab.add_row(row!["name", "repo", "url", "E", "M"]);
        for repo in repos.iter() {
            // The rig repository name, that `rig repos enable` takes, and the
            // entry name only if it is different.
            let (name, entry) = match &repo.group {
                Some(g) if !g.eq_ignore_ascii_case(&repo.entry.name) => {
                    (g.clone(), repo.entry.name.clone())
                }
                Some(g) => (g.clone(), "".to_string()),
                None => (repo.entry.name.clone(), "".to_string()),
            };
            tab.add_row(row![
                name,
                entry,
                repo.entry.url.clone(),
                if repo.entry.default { "X" } else { "" },
                if repo.metadata.is_some() { "X" } else { "" }
            ]);
        }
        // Draw the line under the header as wide as the table.
        let tab = tab.to_string();
        let (header, rows) = tab.split_once('\n').unwrap_or((&tab, ""));
        println!("{}", header);
        println!("{}", "-".repeat(header.trim_end().chars().count()));
        print!("{}", rows);
        println!("\nE: enabled, M: extended metadata");
    }
    Ok(())
}
