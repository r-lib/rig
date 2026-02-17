use std::error::Error;

use clap::ArgMatches;

use crate::repos::get_repos_config;

pub fn sc_repos_available(
    args: &ArgMatches,
    _libargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let config = get_repos_config()?;

    if args.get_flag("json") || mainargs.get_flag("json") {
        println!("{}", serde_json::to_string_pretty(&config)?);
    } else {
        for (n, repo) in config.iter().enumerate() {
            if n != 0 {
                println!();
            }
            println!("Name: {}", repo.name);
            if let Some(title) = &repo.title {
                println!("Title: {}", title);
            }
            println!("Enabled: {}", if repo.enabled { "Yes" } else { "No" });
            println!("URLS:");
            for repoentry in repo.repos.iter() {
                let mut extra = "".to_string();
                if let Some(platforms) = &repoentry.platforms {
                    extra = extra + &platforms.clone().join(" | ");
                }
                if let Some(archs) = &repoentry.archs {
                    let comma = if extra != "" { ", " } else { "" };
                    extra = extra + comma + &archs.clone().join(" | ");
                }
                if let Some(rversions) = &repoentry.rversions {
                    let comma = if extra != "" { ", " } else { "" };
                    extra = extra + comma + &rversions.clone().join(" | ");
                }
                if extra != "" {
                    extra = "(".to_string() + &extra + ")";
                }
                println!("  {} {}", repoentry.url, extra);
            }
        }
    }

    Ok(())
}
