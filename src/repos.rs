use std::error::Error;
use std::fs::File;
use std::path::PathBuf;

use clap::ArgMatches;
use deb822_fast::Deb822;
use directories::ProjectDirs;
use serde_json::Value;
use simple_error::*;
use simplelog::info;
use tabular::*;

use crate::dcf::*;
use crate::download::download_if_newer_;
use crate::solver::RPackageVersion;
use crate::utils::*;

pub fn sc_repos(args: &ArgMatches, mainargs: &ArgMatches)
              -> Result<(), Box<dyn Error>> {

    match args.subcommand() {
        Some(("list-packages", s)) => sc_repos_list_packages(s, args, mainargs),
        Some(("package-info", s)) => sc_repos_package_info(s, args, mainargs),
        _ => Ok(()), // unreachable
    }
}

pub fn repos_get_packages() -> Result<Vec<Package>, Box<dyn Error>> {
    // TODO: do not hardcode repo URL
    let repo_url = "https://cloud.r-project.org/src/contrib/PACKAGES";
    let repo_local = repo_local_file(repo_url)?;
    create_parent_dir_if_needed(&repo_local)?;
    info!("Updating repo metadata from {}", repo_url);
    let dl_status = download_if_newer_(repo_url, &repo_local, None)?;
    if dl_status {
        info!("Updated repo metadata at {}", repo_local.display());
    } else {
        info!("Repo metadata is up to date at {}", repo_local.display());
    }

    let df = File::open(&repo_local)?;
    info!("Parsing repo metadata from {}", repo_local.display());
    let desc = Deb822::from_reader(df)?;
    info!("Parsed {} packages from repo metadata", desc.len());

    let mut packages: Vec<Package> = vec![];

    for pkg in desc.iter() {
        let name = match pkg.get("Package") {
            Some(n) => n.to_string(),
            None => continue,
        };
        let version = match pkg.get("Version") {
            Some(v) => v.to_string(),
            None => continue,
        };
        let mut dependencies: Vec<DepVersionSpec> = vec![];

        if let Some(dd) = pkg.get("Depends") {
            dependencies.append(&mut parse_deps(dd, "Depends")?)
        }
        if let Some(di) = pkg.get("Imports") {
            dependencies.append(&mut parse_deps(di, "Imports")?)
        }
        if let Some(dl) = pkg.get("LinkingTo") {
            dependencies.append(&mut parse_deps(dl, "LinkingTo")?);
        }
        // if let Some(ds) = desc0.get("Suggests") {
        //     deps.append(&mut parse_deps(ds)?);
        // }
        // if let Some(de) = desc0.get("Enhances") {
        //     deps.append(&mut parse_deps(de)?);
        // }
        let dependencies = simplify_constraints(dependencies);

        packages.push(Package { name, version, dependencies });
    }

    Ok(packages)
}

fn sc_repos_list_packages(
    args: &ArgMatches,
    _libargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {

    let packages = repos_get_packages()?;

    if args.get_flag("json") || mainargs.get_flag("json") {

    } else {
        let mut tab: Table = Table::new("{:<}   {:<}   {:<}");
        tab.add_row(row!("Package", "Version", "Dependencies"));
        tab.add_heading("------------------------------------------------------------------------");
        for pkg in packages.iter() {
            let deps_str: String = pkg.dependencies.iter()
                .map(|x| format!("{}", x))
                .collect::<Vec<String>>()
                .join(", ");
            tab.add_row(row!(&pkg.name, &pkg.version, deps_str));
        }

        print!("{}", tab);
    }

    Ok(())
}

fn repo_local_file(url: &str) -> Result<PathBuf, Box<dyn Error>> {
    let mut cache = ProjectDirs::from("com", "gaborcsardi", "rig")
        .ok_or("Cannot determine cache directory")?
        .cache_dir()
        .to_path_buf();
    let urlhash = "repo-".to_string() + &calculate_hash(url) + ".dcf";

    cache.push(urlhash);

    Ok(cache)
}

fn sc_repos_package_info(
    args: &ArgMatches,
    _libargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {

    let package: String = require_with!(args.get_one::<String>("package"), "clap error").to_string();

    let url = "https://crandb.r-pkg.org/".to_string() + &package + "/" + "all";
    let mut local = ProjectDirs::from("com", "gaborcsardi", "rig")
        .ok_or("Cannot determine cache directory")?
        .cache_dir()
        .to_path_buf();
    local.push("packages");
    local.push("package-".to_string() + &package + ".json");

    create_parent_dir_if_needed(&local)?;
    download_if_newer_(&url, &local, None);

    let contents: String = read_file_string(&local)?;
    let json: Value = serde_json::from_str(&contents)?;
    let versions = &json["versions"];

    let mut rows: Vec<(RPackageVersion, String)> = vec![];
    if let Some(versions) = versions.as_object() {
        for (ver, data) in versions {
            let mut deps: Vec<DepVersionSpec> = vec![];
            deps.append(&mut parse_crandb_deps(&data["Depends"], "Depends")?);
            deps.append(&mut parse_crandb_deps(&data["Imports"], "Imports")?);
            deps.append(&mut parse_crandb_deps(&data["LinkingTo"], "LinkingTo")?);
            let deps_str: String = deps.iter()
                .map(|x| format!("{}", x))
                .collect::<Vec<String>>()
                .join(", ");
            let pver: RPackageVersion = RPackageVersion::from_str(ver)?;
            rows.push((pver, deps_str));
        }
    }

    rows.sort_by(|a, b| a.0.cmp(&b.0)); // assumes RPackageVersion implements Ord

    let mut tab: Table = Table::new("{:<}   {:<}   {:<}");
    tab.add_row(row!("Package", "Version", "Dependencies"));
    tab.add_heading("------------------------------------------------------------------------");
    for row in rows {
        tab.add_row(row!(&package, &row.0, &row.1));
    }

    print!("{}", tab);

    Ok(())
}

fn parse_crandb_deps(deps: &serde_json::Value, dep_type: &str)
            -> Result<Vec<DepVersionSpec>, Box<dyn Error>> {
    let mut result: Vec<DepVersionSpec> = Vec::new();

    if let Some(pkgs) = deps.as_object() {
        for (name, ver_spec) in pkgs {
            if ver_spec.is_string() {
                if ver_spec == "*" {
                    result.push(DepVersionSpec {
                        name: name.to_string(),
                        constraints: vec![],
                        types: vec![dep_type.to_string()],
                    });
                } else {
                    result.push(parse_dep(
                        &format!("{} ({})", name, ver_spec.as_str().unwrap()),
                        dep_type,
                    )?);
                }
            }
        }
    }

    let result2: Vec<DepVersionSpec> = simplify_constraints(result);
    Ok(result2)
}
