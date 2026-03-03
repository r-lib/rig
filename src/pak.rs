use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::dcf::*;
use crate::proj::BASE_PKGS;
use crate::solver::*;

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all(serialize = "snake_case"))]
pub struct PakLockfilePackage {
    r#ref: String,
    package: String,
    version: String,
    r#type: String,
    direct: bool,
    binary: bool,
    dependencies: Vec<String>,
    vignettes: bool,
    metadata: HashMap<String, String>,
    sources: Vec<String>,
    target: String,
    platform: String,
    rversion: String,
    directpkg: bool,
    license: String,
    dep_types: Vec<String>,
    params: Vec<String>,
    install_args: String,
    sysreqs: String,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all(serialize = "snake_case"))]
pub struct PakLockfile {
    lockfile_version: usize,
    os: String,
    r_version: String,
    platform: String,
    packages: Vec<PakLockfilePackage>,
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
            let deps = registry.get_dependency_summary(k, v).unwrap();
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
                sources: vec![],
                target: "".to_string(),
                platform: std::env::consts::ARCH.to_string(),
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
