//! Full DESCRIPTION files for arbitrary package versions, from P3M's sync
//! manifests.
//!
//! P3M's own API only serves complete metadata for the *current* version of a
//! package: `/__api__/repos/cran/packages/<pkg>?version=<old>` answers with an
//! empty record. The DESCRIPTION of an older version is however in the manifest
//! files its CRAN sync writes, one per package and snapshot date:
//!
//! ```text
//! https://rspm-sync.rstudio.com/manifest/v4/1/data/<package>_<YYYYMMDD>.json
//! ```
//!
//! Such a file holds the DESCRIPTION (`raw_desc`) of the version that was
//! current on that date (`Current`) plus of every version that was in the CRAN
//! archive at that point (`Archived`). `Current` may hold more than one entry
//! when two versions were published on the same day, so entries are matched by
//! the sha256 of the original CRAN tarball rather than by position.
//!
//! The snapshot date and that sha256 both come from the ALLPACKAGES history in
//! the local metadata database (`Snapshot` / `SHA256Original`), so no extra
//! request is needed to find them.
//!
//! Because the newest manifest of a package carries its whole CRAN archive, one
//! request usually covers every version; only versions that have since been
//! removed from the archive need the manifest of their own snapshot.

use std::collections::HashMap;
use std::error::Error;

use deb822_fast::Deb822;
use log::debug;
use serde_json::{Map, Value};
use simple_error::bail;

use crate::cache::get_cache_dir;
use crate::dcf::{Package, PackageDependencies, RPackageVersion};
use crate::download::download_if_newer_;
use crate::repos::cranlike_metadata::{allpackages_versions, AllPackagesVersion};
use crate::utils::*;

/// Base URL of the P3M sync manifests, overridable via the
/// `RIG_P3M_MANIFEST_URL` env var. `v4` is the manifest layout version and `1`
/// P3M's CRAN source id.
fn manifest_base_url() -> String {
    std::env::var("RIG_P3M_MANIFEST_URL")
        .unwrap_or_else(|_| "https://rspm-sync.rstudio.com/manifest/v4/1/data".to_string())
}

/// The DESCRIPTION of `package` at `version` (`"latest"` for the most recent
/// version), as a JSON object of DCF field name to value.
pub fn get_package_description(package: &str, version: &str) -> Result<Value, Box<dyn Error>> {
    let versions = allpackages_versions(package)?;
    if versions.is_empty() {
        bail!("Could not find package '{}' on CRAN.", package);
    }

    let wanted = select_version(&versions, version)?;
    let snapshot = match wanted.snapshot() {
        Some(s) => s,
        None => bail!(
            "No snapshot date for package '{}' version '{}', cannot look up its DESCRIPTION.",
            package,
            wanted.version.original
        ),
    };

    let manifest = fetch_manifest(package, &snapshot)?;
    let raw_desc = match find_raw_desc(&manifest, wanted) {
        Some(d) => d,
        None => bail!(
            "P3M has no DESCRIPTION for version '{}' in its metadata.",
            wanted.version.original
        ),
    };
    parse_description(&raw_desc)
}

/// One version of a package, as `rig repos package-versions` needs it.
pub struct PackageVersion {
    pub version: RPackageVersion,
    /// All DESCRIPTION fields, for `--json`.
    pub description: Value,
    /// The dependencies from that DESCRIPTION.
    pub dependencies: PackageDependencies,
}

/// The DESCRIPTION of every version of `package` that P3M still has metadata
/// for, oldest version first.
///
/// The newest manifest of the package is fetched first, as it covers the
/// versions that are in the CRAN archive; only versions missing from it need a
/// second manifest, the one of the snapshot they were published in. Versions
/// that are in no manifest at all — deleted from CRAN rather than archived — are
/// left out of the result.
pub fn get_package_versions(package: &str) -> Result<Vec<PackageVersion>, Box<dyn Error>> {
    let versions = allpackages_versions(package)?;
    if versions.is_empty() {
        bail!("Could not find package '{}' on CRAN.", package);
    }
    let versions = latest_per_version(versions);

    // The newest snapshot of any version, i.e. the manifest with the most
    // complete archive listing.
    let newest = versions.iter().filter_map(|v| v.snapshot()).max();

    let mut manifests: HashMap<String, Value> = HashMap::new();
    let mut out: Vec<PackageVersion> = vec![];
    let mut todo: Vec<&AllPackagesVersion> = vec![];

    for date in newest.iter() {
        let manifest = fetch_manifest(package, date)?;
        for version in &versions {
            match find_raw_desc(&manifest, version) {
                Some(raw_desc) => out.push(package_version(version, &raw_desc)?),
                None => todo.push(version),
            }
        }
        manifests.insert(date.to_string(), manifest);
    }

    // A version the newest manifest does not know about is looked up in the
    // manifest of its own snapshot, where it was the current version.
    for version in todo {
        let date = match version.snapshot() {
            Some(d) => d,
            None => continue,
        };
        if !manifests.contains_key(&date) {
            manifests.insert(date.clone(), fetch_manifest(package, &date)?);
        }
        let manifest = &manifests[&date];
        match find_raw_desc(manifest, version) {
            Some(raw_desc) => out.push(package_version(version, &raw_desc)?),
            None => debug!(
                "P3M has no DESCRIPTION for {} {}, skipping it",
                package, version.version.original
            ),
        }
    }

    out.sort_by(|a, b| a.version.cmp(&b.version));
    Ok(out)
}

/// Keep one row per version, the one from the newest snapshot. CRAN re-releases
/// a version now and then (recommended packages especially), which ALLPACKAGES
/// records as several rows with the same version but different tarball hashes.
fn latest_per_version(versions: Vec<AllPackagesVersion>) -> Vec<AllPackagesVersion> {
    let mut best: HashMap<String, AllPackagesVersion> = HashMap::new();
    for version in versions {
        match best.get(&version.version.original) {
            Some(have) if have.snapshot() >= version.snapshot() => {}
            _ => {
                best.insert(version.version.original.clone(), version);
            }
        }
    }
    best.into_values().collect()
}

/// Parse a DESCRIPTION into the fields and the dependencies of one version.
fn package_version(
    version: &AllPackagesVersion,
    raw_desc: &str,
) -> Result<PackageVersion, Box<dyn Error>> {
    Ok(PackageVersion {
        version: version.version.clone(),
        description: parse_description(raw_desc)?,
        dependencies: parse_dependencies(raw_desc)?,
    })
}

/// The dependencies declared by a DESCRIPTION, in one simplified list.
fn parse_dependencies(raw_desc: &str) -> Result<PackageDependencies, Box<dyn Error>> {
    let desc = Deb822::from_reader(raw_desc.as_bytes())?;
    match desc.into_iter().next() {
        Some(para) => Ok(Package::from_dcf_paragraph(&para)?.dependencies),
        None => bail!("Failed to parse the DESCRIPTION of the package."),
    }
}

/// Pick the requested version, or the highest one for `"latest"`.
fn select_version<'a>(
    versions: &'a [AllPackagesVersion],
    version: &str,
) -> Result<&'a AllPackagesVersion, Box<dyn Error>> {
    if version == "latest" {
        return versions
            .iter()
            .max_by(|a, b| a.version.cmp(&b.version))
            .ok_or_else(|| "No versions to choose from".into());
    }

    let wanted = RPackageVersion::from_str(version)?;
    match versions.iter().find(|v| v.version == wanted) {
        Some(v) => Ok(v),
        None => bail!("Could not find version '{}' on CRAN.", version),
    }
}

/// Download (with the usual etag / if-modified-since cache) and parse the
/// manifest file of `package` for the `YYYY-MM-DD` snapshot `date`.
fn fetch_manifest(package: &str, date: &str) -> Result<Value, Box<dyn Error>> {
    let compact: String = date.chars().filter(|c| *c != '-').collect();
    let url = format!("{}/{}_{}.json", manifest_base_url(), package, compact);
    debug!("Fetching package DESCRIPTION from {}", url);

    let mut local = get_cache_dir()?;
    local.push("package-metadata");
    local.push(format!("manifest-{}-{}.json", package, compact));

    create_parent_dir_if_needed(&local)?;
    let (_downloaded, _etag) = download_if_newer_(&url, &local, None, None)?;

    let contents: String = read_file_string(&local)?;
    Ok(serde_json::from_str(&contents)?)
}

/// Find the manifest entry belonging to `wanted` and return its `raw_desc`.
///
/// Entries are first matched on the sha256 of the original CRAN tarball, which
/// identifies exactly the file ALLPACKAGES lists. When CRAN re-releases a
/// version the hashes differ even though the version does not, so a second pass
/// matches on the `Version:` field of the DESCRIPTION.
fn find_raw_desc(manifest: &Value, wanted: &AllPackagesVersion) -> Option<String> {
    let entries = || {
        ["Current", "Archived"]
            .into_iter()
            .filter_map(|key| manifest.get(key))
            .filter_map(|v| v.as_array())
            .flatten()
            .filter_map(|entry| {
                let raw_desc = entry.get("raw_desc")?.as_str()?;
                Some((entry.get("sha256sum").and_then(|v| v.as_str()), raw_desc))
            })
    };

    if let Some(sha) = wanted.sha256sum.as_deref() {
        if let Some((_, raw_desc)) = entries().find(|(entry_sha, _)| *entry_sha == Some(sha)) {
            return Some(raw_desc.to_string());
        }
    }

    let version = wanted.version.original.as_str();
    entries()
        .find(|(_, raw_desc)| desc_version(raw_desc).as_deref() == Some(version))
        .map(|(_, raw_desc)| raw_desc.to_string())
}

/// The `Version:` field of a DESCRIPTION, without parsing the whole file.
fn desc_version(raw_desc: &str) -> Option<String> {
    raw_desc
        .lines()
        .find_map(|l| l.strip_prefix("Version:"))
        .map(|v| v.trim().to_string())
}

/// Parse a DESCRIPTION into a JSON object of field name to value. Field values
/// keep their DCF line wrapping; the printer reflows the ones it shows.
fn parse_description(raw_desc: &str) -> Result<Value, Box<dyn Error>> {
    let desc = Deb822::from_reader(raw_desc.as_bytes())?;
    let para = match desc.into_iter().next() {
        Some(p) => p,
        None => bail!("Failed to parse the DESCRIPTION of the package."),
    };

    let mut map = Map::new();
    for (key, value) in para.iter() {
        map.insert(key.to_string(), Value::String(value.to_string()));
    }

    Ok(Value::Object(map))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ver(version: &str, snapshot: &str, sha: Option<&str>) -> AllPackagesVersion {
        AllPackagesVersion {
            version: RPackageVersion::from_str(version).unwrap(),
            download_url: Some(format!(
                "https://p3m.dev/cran/{}/src/contrib/pkg_{}.tar.gz",
                snapshot, version
            )),
            sha256sum: sha.map(|s| s.to_string()),
        }
    }

    #[test]
    fn snapshot_comes_from_the_download_url() {
        let v = ver("1.0.0", "2026-06-08", None);
        assert_eq!(v.snapshot().as_deref(), Some("2026-06-08"));

        let no_url = AllPackagesVersion {
            version: RPackageVersion::from_str("1.0.0").unwrap(),
            download_url: None,
            sha256sum: None,
        };
        assert_eq!(no_url.snapshot(), None);
    }

    #[test]
    fn latest_is_the_highest_version() {
        let versions = vec![
            ver("0.9.0", "2026-01-01", None),
            ver("0.10.0", "2026-06-08", None),
            ver("0.2.0", "2025-01-01", None),
        ];
        let sel = select_version(&versions, "latest").unwrap();
        assert_eq!(sel.version.original, "0.10.0");

        let sel = select_version(&versions, "0.9.0").unwrap();
        assert_eq!(sel.version.original, "0.9.0");

        assert!(select_version(&versions, "1.0.0").is_err());
    }

    #[test]
    fn entry_is_matched_by_sha256_not_by_position() {
        // Two versions published on the same snapshot day, so `Current` holds
        // both: picking the first entry would return the wrong DESCRIPTION.
        let manifest = serde_json::json!({
            "name": "pkg",
            "Current": [
                { "sha256sum": "aaaa", "raw_desc": "Package: pkg\nVersion: 0.9.3-1\n" },
                { "sha256sum": "bbbb", "raw_desc": "Package: pkg\nVersion: 0.9.3\n" },
            ],
            "Archived": [
                { "sha256sum": "cccc", "raw_desc": "Package: pkg\nVersion: 0.9.2\n" },
            ],
        });

        let wanted = ver("0.9.3", "2026-04-17", Some("bbbb"));
        let desc = find_raw_desc(&manifest, &wanted).unwrap();
        assert_eq!(desc_version(&desc).as_deref(), Some("0.9.3"));

        let wanted = ver("0.9.2", "2026-04-17", Some("cccc"));
        let desc = find_raw_desc(&manifest, &wanted).unwrap();
        assert_eq!(desc_version(&desc).as_deref(), Some("0.9.2"));

        let wanted = ver("0.9.4", "2026-04-18", Some("dddd"));
        assert!(find_raw_desc(&manifest, &wanted).is_none());
    }

    #[test]
    fn a_rereleased_version_falls_back_to_the_version_field() {
        // CRAN re-released 1.0.0, so ALLPACKAGES has a different tarball hash
        // for it than the manifest does. The version still identifies it.
        let manifest = serde_json::json!({
            "Current": [
                { "sha256sum": "aaaa", "raw_desc": "Package: pkg\nVersion: 1.0.0\n" },
            ],
            "Archived": [
                { "sha256sum": "bbbb", "raw_desc": "Package: pkg\nVersion: 0.9.0\n" },
            ],
        });

        let wanted = ver("1.0.0", "2026-01-01", Some("zzzz"));
        let desc = find_raw_desc(&manifest, &wanted).unwrap();
        assert_eq!(desc_version(&desc).as_deref(), Some("1.0.0"));

        // Without a hash the version field is all we have.
        let wanted = ver("0.9.0", "2026-01-01", None);
        let desc = find_raw_desc(&manifest, &wanted).unwrap();
        assert_eq!(desc_version(&desc).as_deref(), Some("0.9.0"));
    }

    #[test]
    fn duplicate_versions_keep_the_newest_snapshot() {
        // The same version released twice: only the newer row survives.
        let versions = vec![
            ver("7.3-49", "2018-03-01", Some("old")),
            ver("7.3-49", "2018-05-01", Some("new")),
            ver("7.3-50", "2018-06-01", Some("other")),
        ];
        let mut kept = latest_per_version(versions);
        kept.sort_by(|a, b| a.version.cmp(&b.version));
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].sha256sum.as_deref(), Some("new"));
        assert_eq!(kept[0].snapshot().as_deref(), Some("2018-05-01"));
        assert_eq!(kept[1].version.original, "7.3-50");
    }

    #[test]
    fn dependencies_are_parsed_from_the_description() {
        let raw_desc = "\
Package: pkg
Version: 1.0.0
Depends: R (>= 3.5)
Imports: cli (>= 3.2.0), utils
Suggests: testthat
";
        let deps = parse_dependencies(raw_desc).unwrap();
        let names: Vec<&str> = deps.dependencies.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"R"));
        assert!(names.contains(&"cli"));
        assert!(names.contains(&"testthat"));
    }

    #[test]
    fn description_is_parsed_into_fields() {
        let raw_desc = "\
Package: pkg
Version: 1.0.0
Title: A Package
Description: Long text
    over two lines.
Depends: R (>= 3.5)
Imports: cli (>= 3.2.0), glue
NeedsCompilation: yes
";
        let json = parse_description(raw_desc).unwrap();
        assert_eq!(json["Package"], "pkg");
        assert_eq!(json["Version"], "1.0.0");
        assert_eq!(json["Imports"], "cli (>= 3.2.0), glue");
        assert_eq!(json["NeedsCompilation"], "yes");
        // Continuation lines are kept as-is; the printer reflows them.
        assert!(json["Description"].as_str().unwrap().contains("over two"));
    }
}
