use regex::Regex;
use std::collections::BTreeMap;
use std::error::Error;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use clap::ArgMatches;
use csv::ReaderBuilder;
use deb822_fast::Deb822;
use directories::ProjectDirs;
use globset::Glob;
use semver::Op;
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use simple_error::*;
use tabular::*;

use crate::common::*;
use crate::dcf::*;
use crate::download::download_if_newer_;
use crate::hardcoded::*;
use crate::repositories::*;
use crate::solver::RPackageVersion;
use crate::utils::*;

#[cfg(target_os = "macos")]
use crate::macos::*;

#[cfg(target_os = "windows")]
use crate::windows::*;

#[cfg(target_os = "linux")]
use crate::linux::*;

pub fn sc_repos(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        // Some(("add", s)) => sc_repos_add(s, args, mainargs),
        // Some(("disable", s)) => sc_repos_disable(s, args, mainargs),
        // Some(("enable", s)) => sc_repos_enable(s, args, mainargs),
        Some(("list-packages", s)) => sc_repos_list_packages(s, args, mainargs),
        Some(("package-info", s)) => sc_repos_package_info(s, args, mainargs),
        Some(("package-versions", s)) => sc_repos_package_versions(s, args, mainargs),
        // Some(("reset", s)) => sc_repos_reset(s, args, mainargs),
        // Some(("rm", s)) => sc_repos_rm(s, args, mainargs),
        Some(("setup", s)) => sc_repos_setup(s, args, mainargs),
        _ => Ok(()), // unreachable
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RepoEntry {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub url: String,
    pub platforms: Option<Vec<String>>,
    pub archs: Option<Vec<String>>,
    pub rversions: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Repository {
    // E.g. CRAN, BioCsoft, PPPM, etc.
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub enabled: bool,
    pub repos: Vec<RepoEntry>,
}

// pub fn sc_repos_add(
//     args: &ArgMatches,
//     _libargs: &ArgMatches,
//     _mainargs: &ArgMatches,
// ) -> Result<(), Box<dyn Error>> {
//     panic!("TODO: implement sc_repos_add");
//     Ok(())
// }

// pub fn sc_repos_disable(
//     args: &ArgMatches,
//     _libargs: &ArgMatches,
//     _mainargs: &ArgMatches,
// ) -> Result<(), Box<dyn Error>> {
//     panic!("TODO: implement sc_repos_disable");
//     Ok(())
// }

// pub fn sc_repos_enable(
//     args: &ArgMatches,
//     _libargs: &ArgMatches,
//     _mainargs: &ArgMatches,
// ) -> Result<(), Box<dyn Error>> {
//     panic!("TODO: implement sc_repos_enable");
//     Ok(())
// }

// pub fn sc_repos_reset(
//     args: &ArgMatches,
//     _libargs: &ArgMatches,
//     _mainargs: &ArgMatches,
// ) -> Result<(), Box<dyn Error>> {
//     panic!("TODO: implement sc_repos_reset");
//     Ok(())
// }

// pub fn sc_repos_rm(
//     args: &ArgMatches,
//     _libargs: &ArgMatches,
//     _mainargs: &ArgMatches,
// ) -> Result<(), Box<dyn Error>> {
//     panic!("TODO: implement sc_repos_rm");
//     Ok(())
// }

fn get_repos_config() -> Result<Vec<Repository>, Box<dyn Error>> {
    Ok(HC_REPOS.to_vec())
}

#[derive(Debug)]
struct RData {
    pub platform: String,
    pub arch: String,    // x86_64, aarch64
    pub version: String, // 4.5.2, etc.
    pub os: String,      // linux, linux-gnu, windows, macos
    pub distro: Option<String>,
    pub release: Option<String>,
}

fn sc_repos_setup(
    args: &ArgMatches,
    _libargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let vers: Vec<String> = if args.contains_id("r-version") {
        vec![args.get_one::<String>("r-version").unwrap().to_string()]
    } else {
        sc_get_list()?
    };
    repos_setup(Some(vers))
}

pub fn repos_setup(vers: Option<Vec<String>>) -> Result<(), Box<dyn Error>> {
    let vers = match vers {
        Some(v) => v,
        None => sc_get_list()?,
    };
    let config = get_repos_config()?;
    let root: String = get_r_root();

    for ver in vers {
        let ver = check_installed(&ver.to_string())?;
        let repositories = root.clone() + "/" + &R_ETC_PATH.replace("{}", &ver) + "/repositories";

        // if no 'repositories' file, skip. Maybe this happens for very old R versions?
        if !PathBuf::from(&repositories).exists() {
            debug!(
                "repositories file does not exist at {}, skipping",
                repositories
            );
            continue;
        }

        // save a copy of the original file, so we can restore later if needed.
        let orig: String = repositories.clone() + ".orig";
        if !PathBuf::from(&orig).exists() {
            debug!(
                "Original repositories file does not exist at {}, copying from {}",
                orig, repositories
            );
            std::fs::copy(&repositories, &orig)?;
        }

        debug!("Updating repositories file at {}", repositories);
        let mut repos = read_repositories_file(&orig)?;

        let rdata = get_r_data(&ver)?;
        debug!("Detected architecture {:?}", rdata);

        for repo in config.iter() {
            for entry in repo.repos.iter() {
                if !should_activate_repo(repo, entry, &rdata)? {
                    continue;
                }
                add_repository(&mut repos, entry);
            }
        }
        add_repositories_comment(&mut repos, "edited automatically by rig");

        write_repositories_file(repos, &repositories)?;

        let profile = root.clone() + "/" + &get_r_base_profile(&ver);
        debug!("Updating R profile at {}", profile);
        let mut profile_lines = read_lines(&Path::new(&profile))?;

        // maybe already current?
        if grep_lines(
            &Regex::new(&HC_PROFILE_REPOS_MARKERS.current_start.to_string())?,
            &profile_lines,
        )
        .len()
            > 0
        {
            continue;
        }

        // maybe from another version of rig?
        let start = grep_lines(
            &Regex::new(&HC_PROFILE_REPOS_MARKERS.generic_start.to_string())?,
            &profile_lines,
        );
        let end = grep_lines(
            &Regex::new(&HC_PROFILE_REPOS_MARKERS.end.to_string())?,
            &profile_lines,
        );

        if start.len() == 1 && end.len() == 1 {
            // remove old version
            profile_lines.drain(start[0]..=end[0]);
        } else if start.len() == 0 && end.len() == 0 {
            // nothing there, nothing to remove
        } else {
            warn!("Corrupt R profile at {}, try reinstalling R. If the issue perists, report it to rig developers.", profile);
            continue;
        }

        profile_lines.push(HC_PROFILE_REPOS.to_string());
        std::fs::write(&profile, profile_lines.join("\n"))?;
    }

    Ok(())
}

fn should_activate_repo(
    repo: &Repository,
    entry: &RepoEntry,
    rdata: &RData,
) -> Result<bool, Box<dyn Error>> {
    if !repo.enabled {
        return Ok(false);
    }

    debug!(
        "Checking if repo '{}' should be activated for platform '{}', arch '{}', R version '{}'",
        repo.name, rdata.platform, rdata.arch, rdata.version
    );

    // if platforms are present, then they must match the current platform
    if entry.platforms.is_some() {
        let mut ok = false;
        for platform in entry.platforms.as_ref().unwrap().iter() {
            let glob = match Glob::new(platform) {
                Ok(g) => g.compile_matcher(),
                Err(e) => {
                    warn!(
                        "Invalid platform glob '{}' in repo '{}', skipping: {}",
                        platform, repo.name, e
                    );
                    continue;
                }
            };
            if glob.is_match(&rdata.platform) {
                debug!("Repo '{}' matches platform glob '{}'", repo.name, platform);
                ok = true;
                break;
            }
        }
        if !ok {
            return Ok(false);
        }
    }

    // if archs are present, then they must match the current arch
    if entry.archs.is_some() {
        let mut ok = false;
        for arch in entry.archs.as_ref().unwrap().iter() {
            if arch == &rdata.arch {
                debug!("Repo '{}' matches arch '{}'", repo.name, arch);
                ok = true;
                break;
            }
        }
        if !ok {
            return Ok(false);
        }
    }

    // if rversions are present, then one of them must be satisfied by the current R version
    if entry.rversions.is_some() {
        let mut ok = false;
        for constraint in entry.rversions.as_ref().unwrap().iter() {
            let depconstraint = parse_constraint(constraint)?;
            let dep = DepVersionSpec {
                name: "R".to_string(),
                types: vec!["R version constraint".to_string()],
                constraints: vec![depconstraint],
            };
            if dep.satisfies(&rdata.version)? {
                debug!(
                    "Repo '{}' (R {}) matches R version constraint '{}'",
                    repo.name, rdata.version, constraint
                );
                ok = true;
                break;
            }
        }
        if !ok {
            return Ok(false);
        }
    }

    Ok(true)
}

#[cfg(target_os = "macos")]
fn get_r_data(ver: &str) -> Result<RData, Box<dyn Error>> {
    get_r_data_common(ver)
}

#[cfg(target_os = "linux")]
fn get_r_data(ver: &str) -> Result<RData, Box<dyn Error>> {
    let mut data = get_r_data_common(ver)?;
    let os = detect_linux()?;
    data.distro = Some(os.distro);
    data.release = Some(os.version);
    Ok(data)
}

#[cfg(target_os = "windows")]
fn get_r_data(ver: &str) -> Result<RData, Box<dyn Error>> {
    // TODO: this arch does not work on Windows, because of an R bug:
    // https://bugs.r-project.org/show_bug.cgi?id=19003
    // We need to look for "^BINPREF" in a a Makeconf file, in
    // etc/Makeconf, etc/x64/Makeconf or etc/i386/Makeconf.
    // If this has 'aarch64' then it is an aaarch64 R build.
    get_r_data_common(ver)
}

fn get_r_data_common(ver: &str) -> Result<RData, Box<dyn Error>> {
    let root: String = get_r_root();
    let statsdesc = root + "/" + &R_SYSLIBPATH.replace("{}", ver) + "/stats/DESCRIPTION";
    debug!("Getting architectture from {}.", statsdesc);
    let lines = read_lines(Path::new(&statsdesc))?;
    let re = Regex::new("^Built:[ ]?")?;
    let bltidx = grep_lines(&re, &lines);
    if bltidx.len() == 0 {
        bail!(
            "Could not find 'Built' in {}, cannot determine architecture of R installation.",
            statsdesc
        );
    }
    let blt = &lines[bltidx[0]];

    // Remove "Built:" prefix and split by semicolons
    let built = blt.strip_prefix("Built:").unwrap_or(blt).trim();
    let parts: Vec<&str> = built.split(';').collect();

    if parts.len() < 2 {
        bail!("Could not parse 'Built' field in {}: {}", statsdesc, blt);
    }

    let platform = parts[1].trim();
    let parts2: Vec<&str> = platform.splitn(3, '-').collect();
    if parts2.len() < 3 {
        bail!("Could not parse 'Built' field in {}: {}", statsdesc, blt);
    }

    let arch = parts2[0];

    if arch == "" {
        bail!("Could not parse 'Built' field in {}: {}", statsdesc, blt);
    }

    let rver = parts[0].trim();
    let rver = rver.strip_prefix("R").unwrap_or(rver).trim();

    let os = match parts2[2] {
        "linux" => "linux",
        "linux-gnu" => "linux-gnu",
        s if s.starts_with("linux") => "linux",
        s if s.starts_with("darwin") => "macos",
        "mingw32" => "windows",
        _ => {
            bail!("Cannot determine OS from {}", statsdesc)
        }
    };

    Ok(RData {
        platform: platform.to_string(),
        arch: arch.to_string(),
        version: rver.to_string(),
        os: os.to_string(),
        distro: None,
        release: None,
    })
}

pub fn repos_get_packages() -> Result<Vec<Package>, Box<dyn Error>> {
    // TODO: do not hardcode repo URL
    let repo_url = "https://cloud.r-project.org/src/contrib/PACKAGES";
    let repo_local = repo_local_file(repo_url)?;
    create_parent_dir_if_needed(&repo_local)?;
    info!("Updating repo metadata from {}", repo_url);
    let dl_status = download_if_newer_(repo_url, &repo_local, None, None)?;
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

        packages.push(Package {
            name,
            version,
            dependencies,
        });
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
            let deps_str: String = pkg
                .dependencies
                .iter()
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

fn get_cran_package_version(
    package: &str,
    version: &str,
) -> Result<BTreeMap<String, String>, Box<dyn Error>> {
    let mut url = "https://crandb.r-pkg.org/".to_string() + &package;
    if version != "latest" {
        url += "/";
        url += version;
    }
    debug!("Fetching package info from {}", url);
    let mut local = ProjectDirs::from("com", "gaborcsardi", "rig")
        .ok_or("Cannot determine cache directory")?
        .cache_dir()
        .to_path_buf();
    local.push("packages");
    local.push("package-".to_string() + &package + "-" + version + ".json");
    debug!("Local cache file: {}", local.display());

    create_parent_dir_if_needed(&local)?;
    download_if_newer_(&url, &local, None, None)?;

    let contents: String = read_file_string(&local)?;
    let contents = contents.replace("<U+000a>", " ");
    let json: Value = serde_json::from_str(&contents)?;

    let mut result: BTreeMap<String, String> = BTreeMap::new();
    if let Some(json) = json.as_object() {
        for (k, v) in json {
            if v.is_string() {
                result.insert(k.to_string(), v.as_str().unwrap().to_string());
            }
        }
    }

    Ok(result)
}

pub fn get_all_cran_package_versions(
    package: &str,
    client: Option<&reqwest::Client>,
) -> Result<Vec<(RPackageVersion, Vec<DepVersionSpec>)>, Box<dyn Error>> {
    let url = "https://crandb.r-pkg.org/".to_string() + &package + "/" + "all";
    let mut local = ProjectDirs::from("com", "gaborcsardi", "rig")
        .ok_or("Cannot determine cache directory")?
        .cache_dir()
        .to_path_buf();
    local.push("packages");
    local.push("package-".to_string() + &package + ".json");

    create_parent_dir_if_needed(&local)?;
    download_if_newer_(&url, &local, None, client)?;

    let contents: String = read_file_string(&local)?;
    let contents = contents.replace("<U+000a>", " ");
    let json: Value = serde_json::from_str(&contents)?;
    let versions = &json["versions"];

    let mut rows: Vec<(RPackageVersion, Vec<DepVersionSpec>)> = vec![];
    if let Some(versions) = versions.as_object() {
        for (ver, data) in versions {
            let mut deps: Vec<DepVersionSpec> = vec![];
            deps.append(&mut parse_crandb_deps(&data["Depends"], "Depends")?);
            deps.append(&mut parse_crandb_deps(&data["Imports"], "Imports")?);
            deps.append(&mut parse_crandb_deps(&data["LinkingTo"], "LinkingTo")?);
            let pver: RPackageVersion = RPackageVersion::from_str(ver)?;
            rows.push((pver, deps));
        }
    }

    Ok(rows)
}

fn sc_repos_package_info(
    args: &ArgMatches,
    _libargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let package: String =
        require_with!(args.get_one::<String>("package"), "clap error").to_string();
    let ver = if args.contains_id("version") {
        args.get_one::<String>("version").unwrap().to_string()
    } else {
        "latest".to_string()
    };

    let info = get_cran_package_version(&package, &ver)?;
    if args.get_flag("json") {
        let json = serde_json::to_string_pretty(&info)?;
        println!("{}", json);
    } else {
        let mut tab: Table = Table::new("{:<}   {:<}");
        tab.add_row(row!("Field", "Value"));
        tab.add_heading("------------------------------------------------------------------------");
        for (k, v) in info.iter() {
            tab.add_row(row!(k, v));
        }
        print!("{}", tab);
    }

    Ok(())
}

fn sc_repos_package_versions(
    args: &ArgMatches,
    _libargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let package: String =
        require_with!(args.get_one::<String>("package"), "clap error").to_string();

    let mut rows = get_all_cran_package_versions(&package, None)?;
    rows.sort_by(|a, b| a.0.cmp(&b.0)); // assumes RPackageVersion implements Ord

    let mut tab: Table = Table::new("{:<}   {:<}   {:<}");
    tab.add_row(row!("Package", "Version", "Dependencies"));
    tab.add_heading("------------------------------------------------------------------------");
    for row in rows {
        let deps_str: String = row
            .1
            .iter()
            .map(|x| format!("{}", x))
            .collect::<Vec<String>>()
            .join(", ");

        tab.add_row(row!(&package, &row.0, &deps_str));
    }

    print!("{}", tab);

    Ok(())
}

fn parse_crandb_deps(
    deps: &serde_json::Value,
    dep_type: &str,
) -> Result<Vec<DepVersionSpec>, Box<dyn Error>> {
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
