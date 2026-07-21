use std::error::Error;

use log::debug;
use serde_json::Value;

use crate::cache::get_cache_dir;
use crate::dcf::*;
use crate::download::download_if_newer_;
use crate::proj::BASE_PKGS;
use crate::utils::*;

pub fn get_cran_package_version(package: &str, version: &str) -> Result<Value, Box<dyn Error>> {
    let mut url = "https://crandb.r-pkg.org/".to_string() + package;
    if version != "latest" {
        url += "/";
        url += version;
    }
    debug!("Fetching package info from {}", url);
    let mut local = get_cache_dir()?;
    local.push("package-metadata");
    local.push("package-".to_string() + package + "-" + version + ".json");
    debug!("Local cache file: {}", local.display());

    create_parent_dir_if_needed(&local)?;
    let (_downloaded, _etag) = download_if_newer_(&url, &local, None, None)?;

    let contents: String = read_file_string(&local)?;
    let contents = contents.replace("<U+000a>", " ");
    let json: Value = serde_json::from_str(&contents)?;

    Ok(json)
}

/// Download and parse the crandb `<package>/all` record, which lists every
/// (current and archived) version of a package together with a `latest`
/// pointer and a `timeline` of publication dates.
pub fn fetch_crandb_all(
    package: &str,
    client: Option<&reqwest::Client>,
) -> Result<Value, Box<dyn Error>> {
    let url = "https://crandb.r-pkg.org/".to_string() + package + "/" + "all";
    let mut local = get_cache_dir()?;
    local.push("packages");
    local.push("package-".to_string() + package + ".json");

    create_parent_dir_if_needed(&local)?;
    let (_downloaded, _etag) = download_if_newer_(&url, &local, None, client)?;

    let contents: String = read_file_string(&local)?;
    let contents = contents.replace("<U+000a>", " ");
    let json: Value = serde_json::from_str(&contents)?;
    Ok(json)
}

/// Combine every dependency field of a single crandb version record into one
/// simplified [`PackageDependencies`].
fn crandb_version_deps(data: &Value) -> Result<PackageDependencies, Box<dyn Error>> {
    let mut pkg_deps = PackageDependencies::new();
    for dep_type in RDepType::all() {
        let dep_type_str = dep_type.to_string();
        if let Some(deps_json) = data.get(&dep_type_str) {
            pkg_deps.append(&mut parse_crandb_deps(deps_json, &dep_type_str)?);
        }
    }
    pkg_deps.simplify();
    Ok(pkg_deps)
}

/// A single row of `rig repos package-versions` output: a version, when it was
/// published, its R version requirement and how many hard dependencies it has.
#[derive(Debug)]
pub struct CranVersionRow {
    pub version: RPackageVersion,
    /// Publication date as `YYYY-MM-DD`, if crandb knows it.
    pub date: Option<String>,
    /// R version requirement (e.g. `>= 3.5.0`), or `None` when unconstrained.
    pub r_requirement: Option<String>,
    /// Number of hard dependencies (Depends / Imports / LinkingTo), excluding R
    /// and the base packages.
    pub num_deps: usize,
}

/// All versions of a package known to crandb, plus which one is the latest and
/// whether the package is archived.
#[derive(Debug)]
pub struct CranVersions {
    pub name: String,
    pub latest: Option<String>,
    pub archived: bool,
    pub rows: Vec<CranVersionRow>,
}

/// Summarize a single crandb version record into a [`CranVersionRow`]: its R
/// requirement (the constraints on the `R` dependency, if any) and the number
/// of hard dependencies (`Depends` / `Imports` / `LinkingTo`, excluding R and
/// the base packages).
fn crandb_version_row(
    ver: &str,
    data: &Value,
    date: Option<String>,
) -> Result<CranVersionRow, Box<dyn Error>> {
    let pkg_deps = crandb_version_deps(data)?;

    let r_requirement = pkg_deps
        .dependencies
        .iter()
        .find(|d| d.name == "R")
        .filter(|d| !d.constraints.is_empty())
        .map(|d| {
            d.constraints
                .iter()
                .map(|c| format!("{} {}", c.constraint_type, c.version))
                .collect::<Vec<_>>()
                .join(", ")
        });

    let num_deps = pkg_deps
        .dependencies
        .iter()
        .filter(|d| {
            d.name != "R"
                && !BASE_PKGS.contains(&d.name.as_str())
                && d.types.iter().any(|t| {
                    matches!(
                        t,
                        RDepType::Depends | RDepType::Imports | RDepType::LinkingTo
                    )
                })
        })
        .count();

    Ok(CranVersionRow {
        version: RPackageVersion::from_str(ver)?,
        date,
        r_requirement,
        num_deps,
    })
}

/// Collect a per-version summary of a package for `rig repos package-versions`.
pub fn get_cran_package_versions_info(
    package: &str,
    client: Option<&reqwest::Client>,
) -> Result<CranVersions, Box<dyn Error>> {
    let json = fetch_crandb_all(package, client)?;
    let latest = json["latest"].as_str().map(|s| s.to_string());
    let archived = json["archived"].as_bool().unwrap_or(false);
    let timeline = &json["timeline"];

    let mut rows: Vec<CranVersionRow> = vec![];
    if let Some(versions) = json["versions"].as_object() {
        for (ver, data) in versions {
            let date = timeline
                .get(ver)
                .and_then(|v| v.as_str())
                .or_else(|| data.get("Date/Publication").and_then(|v| v.as_str()))
                .map(|s| s.chars().take(10).collect::<String>());
            rows.push(crandb_version_row(ver, data, date)?);
        }
    }

    Ok(CranVersions {
        name: package.to_string(),
        latest,
        archived,
        rows,
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dcf::{RDepType, VersionConstraintType};

    #[test]
    fn parse_wildcard_produces_no_constraints() {
        let deps = serde_json::json!({ "R": "*" });
        let result = parse_crandb_deps(&deps, "Depends").unwrap();
        assert_eq!(result.dependencies.len(), 1);
        assert_eq!(result.dependencies[0].name, "R");
        assert!(result.dependencies[0].constraints.is_empty());
        assert_eq!(result.dependencies[0].types, vec![RDepType::Depends]);
    }

    #[test]
    fn parse_version_constraint() {
        let deps = serde_json::json!({ "ggplot2": ">= 3.0.0" });
        let result = parse_crandb_deps(&deps, "Imports").unwrap();
        assert_eq!(result.dependencies.len(), 1);
        let dep = &result.dependencies[0];
        assert_eq!(dep.name, "ggplot2");
        assert_eq!(dep.types, vec![RDepType::Imports]);
        assert_eq!(dep.constraints.len(), 1);
        assert_eq!(
            dep.constraints[0].constraint_type,
            VersionConstraintType::GreaterOrEqual
        );
    }

    #[test]
    fn parse_empty_object_produces_no_deps() {
        let deps = serde_json::json!({});
        let result = parse_crandb_deps(&deps, "Depends").unwrap();
        assert!(result.dependencies.is_empty());
    }

    #[test]
    fn parse_multiple_packages() {
        let deps = serde_json::json!({ "R": "*", "methods": "*" });
        let result = parse_crandb_deps(&deps, "Depends").unwrap();
        assert_eq!(result.dependencies.len(), 2);
        assert!(result.dependencies.iter().any(|d| d.name == "R"));
        assert!(result.dependencies.iter().any(|d| d.name == "methods"));
    }

    #[test]
    fn parse_invalid_dep_type_errors() {
        let deps = serde_json::json!({ "R": "*" });
        assert!(parse_crandb_deps(&deps, "InvalidType").is_err());
    }

    #[test]
    fn version_row_extracts_r_requirement_and_counts_hard_deps() {
        let data = serde_json::json!({
            "Depends": { "R": ">= 3.5.0" },
            "Imports": { "cli": "*", "glue": ">= 1.0", "utils": "*" },
            "LinkingTo": { "cpp11": "*" },
            "Suggests": { "testthat": "*", "covr": "*" },
        });
        let row = crandb_version_row("1.2.3", &data, Some("2024-01-15".to_string())).unwrap();
        assert_eq!(row.version.original, "1.2.3");
        assert_eq!(row.date.as_deref(), Some("2024-01-15"));
        assert_eq!(row.r_requirement.as_deref(), Some(">= 3.5.0"));
        // cli, glue, cpp11 count; R, the base package utils and the Suggests
        // are excluded.
        assert_eq!(row.num_deps, 3);
    }

    #[test]
    fn version_row_no_r_constraint_is_none() {
        let data = serde_json::json!({
            "Depends": { "R": "*" },
            "Imports": { "utils": "*", "rlang": "*" },
        });
        let row = crandb_version_row("1.0", &data, None).unwrap();
        assert_eq!(row.r_requirement, None);
        assert_eq!(row.date, None);
        // utils is a base package; only rlang counts.
        assert_eq!(row.num_deps, 1);
    }
}
