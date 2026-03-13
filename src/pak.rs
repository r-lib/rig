use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::proj::BASE_PKGS;
use crate::solver::*;

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all(serialize = "snake_case"))]
pub struct PakLockfilePackage {
    pub r#ref: String,
    pub package: String,
    pub version: String,
    pub r#type: String,
    pub direct: bool,
    pub binary: bool,
    pub dependencies: Vec<String>,
    pub vignettes: bool,
    pub metadata: HashMap<String, String>,
    pub sources: Vec<String>,
    pub target: String,
    pub platform: String,
    pub rversion: String,
    pub directpkg: bool,
    pub license: String,
    pub dep_types: Vec<String>,
    pub params: Vec<String>,
    pub install_args: String,
    pub sysreqs: String,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all(serialize = "snake_case"))]
pub struct PakLockfile {
    pub lockfile_version: usize,
    pub os: String,
    pub r_version: String,
    pub platform: String,
    pub packages: Vec<PakLockfilePackage>,
}

impl PakLockfile {
    pub fn from_solution(
        registry: &RPackageRegistry,
        solution: &HashMap<String, RegistryPackageVersion, rustc_hash::FxBuildHasher>,
    ) -> PakLockfile {
        let mut pkgs = vec![];
        for (k, v) in solution.iter() {
            if k == "R" || k == "_project" || BASE_PKGS.contains(&k.as_str()) {
                continue;
            }
            let filename = format!("{}_{}.tar.gz", k, v);
            let deps = registry
                .get_dependency_summary(k, v)
                .unwrap()
                .into_iter()
                .filter(|dep| dep != "R" && !BASE_PKGS.contains(&dep.as_str()))
                .collect();
            let dl1 = format!("https://cloud.r-project.org/src/contrib/{}", filename);
            let dl2 = format!(
                "https://cloud.r-project.org/src/contrib/Archive/{}/{}",
                k, filename
            );
            pkgs.push(PakLockfilePackage {
                r#ref: k.to_string(),
                package: k.to_string(),
                version: v.to_string(),
                r#type: "standard".to_string(),
                direct: false,
                binary: false,
                dependencies: deps,
                vignettes: false,
                metadata: HashMap::new(),
                sources: vec![dl1, dl2],
                target: "src/contrib/".to_string() + &filename,
                platform: "source".to_string(),
                rversion: "4.5.2".to_string(),
                directpkg: false,
                license: "UNKNOWN".to_string(),
                dep_types: vec![],
                params: vec![],
                install_args: "".to_string(),
                sysreqs: "".to_string(),
            });
        }

        PakLockfile {
            lockfile_version: 1,
            os: std::env::consts::OS.to_string(),
            r_version: "4.5.2".to_string(),
            platform: std::env::consts::ARCH.to_string(),
            packages: pkgs,
        }
    }
}
