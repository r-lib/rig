use std::collections::BTreeMap;
use std::error::Error;

use log::debug;
use serde_json::Value;

use crate::cache::get_cache_dir;
use crate::dcf::*;
use crate::download::download_if_newer_;
use crate::utils::*;

pub fn get_cran_package_version(
    package: &str,
    version: &str,
) -> Result<BTreeMap<String, String>, Box<dyn Error>> {
    let mut url = "https://crandb.r-pkg.org/".to_string() + &package;
    if version != "latest" {
        url += "/";
        url += version;
    }
    debug!("Fetching package info from {}", url);
    let mut local = get_cache_dir()?;
    local.push("package-metadata");
    local.push("package-".to_string() + &package + "-" + version + ".json");
    debug!("Local cache file: {}", local.display());

    create_parent_dir_if_needed(&local)?;
    let (_downloaded, _etag) = download_if_newer_(&url, &local, None, None)?;

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
) -> Result<Vec<Package>, Box<dyn Error>> {
    let url = "https://crandb.r-pkg.org/".to_string() + &package + "/" + "all";
    let mut local = get_cache_dir()?;
    local.push("packages");
    local.push("package-".to_string() + &package + ".json");

    create_parent_dir_if_needed(&local)?;
    let (_downloaded, _etag) = download_if_newer_(&url, &local, None, client)?;

    let contents: String = read_file_string(&local)?;
    let contents = contents.replace("<U+000a>", " ");
    let json: Value = serde_json::from_str(&contents)?;
    let versions = &json["versions"];

    let mut rows: Vec<Package> = vec![];
    if let Some(versions) = versions.as_object() {
        for (ver, data) in versions {
            let mut pkg_deps = PackageDependencies::new();
            for dep_type in RDepType::all() {
                let dep_type_str = dep_type.to_string();
                if let Some(deps_json) = data.get(&dep_type_str) {
                    pkg_deps.append(&mut parse_crandb_deps(deps_json, &dep_type_str)?);
                }
            }
            pkg_deps.simplify();
            let pver: RPackageVersion = RPackageVersion::from_str(ver)?;
            let pkg = Package::from_crandb(package.to_string(), pver, pkg_deps.dependencies);
            rows.push(pkg);
        }
    }

    Ok(rows)
}

fn parse_crandb_deps(
    deps: &serde_json::Value,
    dep_type: &str,
) -> Result<PackageDependencies, Box<dyn Error>> {
    let mut result: Vec<DepVersionSpec> = Vec::new();

    if let Some(pkgs) = deps.as_object() {
        for (name, ver_spec) in pkgs {
            if ver_spec.is_string() {
                if ver_spec == "*" {
                    result.push(DepVersionSpec {
                        name: name.to_string(),
                        constraints: vec![],
                        types: vec![RDepType::from_str(dep_type)?],
                    });
                } else {
                    result.push(DepVersionSpec::parse(
                        &format!("{} ({})", name, ver_spec.as_str().unwrap()),
                        dep_type,
                    )?);
                }
            }
        }
    }

    let mut pkg_deps = PackageDependencies {
        dependencies: result,
    };
    pkg_deps.simplify();
    Ok(pkg_deps)
}
