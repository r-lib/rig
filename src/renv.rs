use std::collections::HashMap;
use std::error::Error;
use std::path::PathBuf;

use serde_derive::Deserialize;
use serde_derive::Serialize;
use simple_error::*;

use crate::common::*;
use crate::dcf::RPackageVersion;
use crate::proj::BASE_PKGS;
use crate::rversion::*;
use crate::solver::*;
use crate::utils::*;

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
        match ver {
            InstalledVersion {
                name: n,
                version: Some(v),
                path: Some(p),
                binary: Some(b),
                aliases: _,
            } => {
                if let Ok(sv) = semver::Version::parse(v) {
                    ok.push(OKInstalledVersion {
                        name: n.to_string(),
                        version: sv,
                        path: p.to_string(),
                        binary: b.to_string(),
                    });
                }
            }
            _ => {}
        }
    }
    ok
}

pub fn match_r_version(ver: &str) -> Result<OKInstalledVersion, Box<dyn Error>> {
    let allvers = sc_get_list_details()?;
    let mut okvers = filter_ok_versions(allvers);
    okvers.sort();

    let ver = try_with!(
        semver::Version::parse(ver),
        "Invalid R version in renv.lock file: {:?}",
        ver
    );

    // Matching major.minor
    let goodvers: Vec<OKInstalledVersion> = okvers
        .into_iter()
        .filter(|v| v.version.major == ver.major && v.version.minor == ver.minor)
        .collect();

    // If we have a perfect match, then reduce further
    let goodvers2: Vec<OKInstalledVersion>;
    match goodvers.iter().find(|v| v.version.patch == ver.patch) {
        Some(_) => {
            goodvers2 = goodvers
                .into_iter()
                .filter(|v| v.version.patch == ver.patch)
                .collect();
        }
        None => goodvers2 = goodvers,
    };

    // If we have an arm64 version, reduce to those
    let goodvers3: Vec<OKInstalledVersion>;
    match goodvers2.iter().find(|v| v.name.ends_with("-arm64")) {
        Some(_) => {
            goodvers3 = goodvers2
                .into_iter()
                .filter(|v| v.name.ends_with("-arm64"))
                .collect();
        }
        None => goodvers3 = goodvers2,
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
        solution: &HashMap<String, RPackageVersion, rustc_hash::FxBuildHasher>,
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
                    Version: v.to_string(),
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
                Version: solution.get("R").unwrap().to_string(),
                Repositories: vec![REnvLockfileRepository {
                    Name: "CRAN".to_string(),
                    URL: "https://cloud.r-project.org".to_string(),
                }],
            },
            Packages: pkgs,
        }
    }
}
