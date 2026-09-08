use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use clap::ArgMatches;
use log::{error, info};
use serde_derive::Deserialize;
use serde_derive::Serialize;
use simple_error::*;

use crate::common::*;
use crate::dcf::{DepVersionSpec, RDepType};
use crate::output::OUTPUT;
use crate::proj::{
    check_project_conflicts, init_rvenv_for_manifest, proj_binary_target, proj_lock_r_version,
    proj_read_manifest_deps, sc_proj_solve_deps, BASE_PKGS,
};
use crate::repos::cranlike_metadata::minor_r_version;
use crate::rproj::{Rproj, RPROJ_MANIFEST_FILE};
use crate::rversion::*;
use crate::solver::*;
use crate::utils::*;

pub fn sc_renv(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        Some(("export", s)) => sc_renv_export(s, args, mainargs),
        Some(("import", s)) => sc_renv_import(s, args, mainargs),
        _ => Ok(()), // unreachable
    }
}

/// Solve the project's dependencies (`rproj.toml`) for one `(R version,
/// platform)` target and write the result as `renv.lock`: `rig proj renv export`.
///
/// Always a single target -- `renv.lock` has no multi-target concept, unlike
/// `rproj.lock`. `--r-version` defaults to the same logic `rig proj lock`
/// uses (the default R version if it satisfies the manifest, else the
/// newest installed one that does, else the current release); `--platform`
/// defaults to this machine.
fn sc_renv_export(
    args: &ArgMatches,
    _renvargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let root = Path::new(".");
    let (_name, _version, mut pkg_deps) = proj_read_manifest_deps(root, true)?;
    pkg_deps.dependencies.push(DepVersionSpec {
        name: "renv".to_string(),
        constraints: vec![],
        types: vec![RDepType::Depends],
    });

    let rver = match args.get_one::<String>("r-version") {
        Some(rv) => rv.clone(),
        None => proj_lock_r_version(&pkg_deps, args)?,
    };
    let platform = args.get_one::<String>("platform").cloned();
    let target = proj_binary_target(platform.as_ref(), &rver)?;

    let (registry, solution) = sc_proj_solve_deps(&rver, &pkg_deps, target, None, true)?;
    OUTPUT.success("Solved dependencies");
    info!("Solved dependencies");

    let lockfile = REnvLockfile::from_solution(&registry, &solution);
    fs::write(
        root.join("renv.lock"),
        serde_json::to_string_pretty(&lockfile)?,
    )?;
    OUTPUT.success("Written renv lockfile to renv.lock");
    info!("Written renv lockfile to renv.lock");
    Ok(())
}

/// Import an existing `renv.lock` into `rproj.toml`: `rig proj renv import`.
///
/// Mirrors `rig proj import`'s DESCRIPTION import: by default creates a new
/// `rproj.toml` (refusing to run if one already exists), `--dependencies`
/// merges into an existing one instead. Only package-level information
/// makes it across -- `renv.lock` has no project metadata (name, title,
/// authors, ...) to import, so a fresh manifest is named after the current
/// directory.
fn sc_renv_import(
    args: &ArgMatches,
    _renvargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let default_input = "renv.lock".to_string();
    let input: &String = args.get_one::<String>("input").unwrap_or(&default_input);
    let dependencies_only = args.get_flag("dependencies");
    let root = std::env::current_dir()?;
    let path = root.join(RPROJ_MANIFEST_FILE);
    let path = path.as_path();

    if !dependencies_only && path.exists() {
        let msg = format!(
            "{} already exists; import would only overwrite dependencies, not \
             merge full metadata. Use --dependencies to merge into it, or \
             remove it first.",
            RPROJ_MANIFEST_FILE
        );
        OUTPUT.error(&msg);
        error!("{}", msg);
        bail!("{}", msg);
    }

    // A full import also creates the `.rvenv` layout, so check for conflicts
    // before writing anything. `--dependencies` only touches the manifest.
    if !dependencies_only && !args.get_flag("force") {
        check_project_conflicts(&root)?;
    }

    OUTPUT.status(&format!("Reading dependencies from {}", input));
    info!("Reading dependencies from {}", input);
    let contents = fs::read_to_string(input).map_err(|e| {
        OUTPUT.error(&format!("Cannot read {}: {}", input, e));
        error!("Cannot read {}: {}", input, e);
        e
    })?;
    let lockfile: REnvLockfile = serde_json::from_str(&contents)?;
    let dep_count = lockfile.Packages.len();

    let default_name = || -> String {
        std::env::current_dir()
            .ok()
            .and_then(|cwd| cwd.file_name().map(|n| n.to_string_lossy().into_owned()))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "myproject".to_string())
    };

    let mut manifest = if dependencies_only {
        if path.exists() {
            toml::from_str::<Rproj>(&fs::read_to_string(path)?)?
        } else {
            OUTPUT.status(&format!(
                "{} does not exist, creating a new one",
                RPROJ_MANIFEST_FILE
            ));
            info!("{} does not exist, creating a new one", RPROJ_MANIFEST_FILE);
            Rproj::minimal(&default_name())
        }
    } else {
        Rproj::minimal(&default_name())
    };

    let minor = minor_r_version(&lockfile.R.Version)?;
    manifest.add_dependency("R", &format!(">= {}", minor), false);
    for (name, pkg) in lockfile.Packages.iter() {
        manifest.add_dependency(name, &format!("^{}", pkg.Version), false);
    }

    fs::write(path, toml::to_string_pretty(&manifest)?)?;

    let msg = format!(
        "Imported R {} and {} dependencies from {} into {}",
        minor, dep_count, input, RPROJ_MANIFEST_FILE
    );
    OUTPUT.success(&msg);
    info!("{}", msg);

    // A full import sets up a whole project, not just its manifest, so it
    // creates the same `.rvenv` layout as `rig proj init`. The lockfile
    // records the exact R version it was created with, so use that.
    if !dependencies_only {
        init_rvenv_for_manifest(args, &root, path, Some(&lockfile.R.Version))?;
    }

    Ok(())
}

#[derive(Serialize, Deserialize, Debug)]
#[allow(non_snake_case)]
struct REnvLockfileSimpleR {
    Version: String,
}

#[derive(Serialize, Deserialize, Debug)]
#[allow(non_snake_case)]
struct REnvLockfileSimple {
    R: REnvLockfileSimpleR,
}

// -------------------------------------------------------------------------------------

pub fn parse_r_version(lockfile: PathBuf) -> Result<String, Box<dyn Error>> {
    let contents = read_file_string(&lockfile)?;
    let lockf: REnvLockfileSimple = serde_json::from_str(&contents)?;
    Ok(lockf.R.Version.to_string())
}

fn filter_ok_versions(all: Vec<InstalledVersion>) -> Vec<OKInstalledVersion> {
    let mut ok: Vec<OKInstalledVersion> = vec![];
    for ver in all.iter() {
        if let InstalledVersion {
            name: n,
            version: Some(v),
            path: Some(p),
            binary: Some(b),
            aliases: _,
        } = ver
        {
            if let Ok(sv) = semver::Version::parse(v) {
                ok.push(OKInstalledVersion {
                    name: n.to_string(),
                    version: sv,
                    path: p.to_string(),
                    binary: b.to_string(),
                });
            }
        }
    }
    ok
}

pub fn match_r_version(ver: &str) -> Result<OKInstalledVersion, Box<dyn Error>> {
    let allvers = sc_get_list_details()?;
    let mut okvers = filter_ok_versions(allvers);
    okvers.sort();

    let ver = match semver::Version::parse(ver) {
        Ok(v) => v,
        Err(_) => {
            OUTPUT.error(&format!("Invalid R version in renv.lock file: {:?}", ver));
            error!("Invalid R version in renv.lock file: {:?}", ver);
            bail!("Invalid R version in renv.lock file: {:?}", ver);
        }
    };

    // Matching major.minor
    let goodvers: Vec<OKInstalledVersion> = okvers
        .into_iter()
        .filter(|v| v.version.major == ver.major && v.version.minor == ver.minor)
        .collect();

    // If we have a perfect match, then reduce further
    let goodvers2: Vec<OKInstalledVersion> =
        match goodvers.iter().find(|v| v.version.patch == ver.patch) {
            Some(_) => goodvers
                .into_iter()
                .filter(|v| v.version.patch == ver.patch)
                .collect(),
            None => goodvers,
        };

    // If we have an arm64 version, reduce to those
    let goodvers3: Vec<OKInstalledVersion> =
        match goodvers2.iter().find(|v| v.name.ends_with("-arm64")) {
            Some(_) => goodvers2
                .into_iter()
                .filter(|v| v.name.ends_with("-arm64"))
                .collect(),
            None => goodvers2,
        };

    // Choose the latest one of these (there is surely at least on left)
    match goodvers3.last() {
        Some(v) => Ok(v.to_owned()),
        None => bail!(
            "Cannot find any R version close to R {}, \
                       required by renv lock file.",
            ver
        ),
    }
}

// -------------------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug)]
#[allow(non_snake_case)]
struct REnvLockfileRepository {
    Name: String,
    URL: String,
}

#[derive(Serialize, Deserialize, Debug)]
#[allow(non_snake_case)]
struct REnvLockfileR {
    Version: String,
    Repositories: Vec<REnvLockfileRepository>,
}

#[derive(Serialize, Deserialize, Debug)]
#[allow(non_snake_case)]
struct REnvLockfilePackage {
    Package: String,
    Version: String,
    Source: String,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    Repository: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    Depends: Option<Vec<String>>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    Imports: Option<Vec<String>>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    LinkingTo: Option<Vec<String>>,
}

type REnvLockfilePackages = HashMap<String, REnvLockfilePackage>;

#[derive(Serialize, Deserialize, Debug)]
#[allow(non_snake_case)]
pub struct REnvLockfile {
    R: REnvLockfileR,
    Packages: REnvLockfilePackages,
}

impl REnvLockfile {
    pub fn from_solution(
        registry: &RPackageRegistry,
        solution: &HashMap<String, RegistryPackageVersion, rustc_hash::FxBuildHasher>,
    ) -> REnvLockfile {
        let mut pkgs = REnvLockfilePackages::new();
        for (k, v) in solution.iter() {
            if k == "R" || k == "_project" || BASE_PKGS.contains(&k.as_str()) {
                continue;
            }
            let deps = registry.get_dependency_summary(k, v).unwrap();
            pkgs.insert(
                k.to_string(),
                REnvLockfilePackage {
                    Package: k.to_string(),
                    Version: v.version.to_string(),
                    Source: "Repository".to_string(),
                    Repository: Some("CRAN".to_string()),
                    Depends: Some(deps),
                    Imports: None,
                    LinkingTo: None,
                },
            );
        }
        REnvLockfile {
            R: REnvLockfileR {
                Version: solution.get("R").unwrap().version.to_string(),
                Repositories: vec![REnvLockfileRepository {
                    Name: "CRAN".to_string(),
                    URL: "https://cloud.r-project.org".to_string(),
                }],
            },
            Packages: pkgs,
        }
    }
}
