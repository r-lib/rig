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

use std::error::Error;

use deb822_fast::Deb822;
use log::debug;
use serde_json::{Map, Value};
use simple_error::bail;

use crate::cache::get_cache_dir;
use crate::dcf::RPackageVersion;
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
    let raw_desc = find_raw_desc(&manifest, wanted)?;
    parse_description(&raw_desc)
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
/// Entries are matched on the sha256 of the original CRAN tarball. Rows stored
/// before rig recorded that hash have none, in which case we fall back to the
/// `Version:` field of the DESCRIPTION itself.
fn find_raw_desc(manifest: &Value, wanted: &AllPackagesVersion) -> Result<String, Box<dyn Error>> {
    for key in ["Current", "Archived"] {
        let entries = match manifest.get(key).and_then(|v| v.as_array()) {
            Some(e) => e,
            None => continue,
        };
        for entry in entries {
            let raw_desc = match entry.get("raw_desc").and_then(|v| v.as_str()) {
                Some(d) => d,
                None => continue,
            };
            let matches = match &wanted.sha256sum {
                Some(sha) => entry.get("sha256sum").and_then(|v| v.as_str()) == Some(sha.as_str()),
                None => desc_version(raw_desc).as_deref() == Some(wanted.version.original.as_str()),
            };
            if matches {
                return Ok(raw_desc.to_string());
            }
        }
    }

    bail!(
        "P3M has no DESCRIPTION for version '{}' in its metadata.",
        wanted.version.original
    )
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
        assert!(find_raw_desc(&manifest, &wanted).is_err());
    }

    #[test]
    fn without_a_sha256_the_version_field_is_matched() {
        let manifest = serde_json::json!({
            "Current": [
                { "sha256sum": "aaaa", "raw_desc": "Package: pkg\nVersion: 1.0.0\n" },
            ],
            "Archived": [
                { "sha256sum": "bbbb", "raw_desc": "Package: pkg\nVersion: 0.9.0\n" },
            ],
        });

        let wanted = ver("0.9.0", "2026-01-01", None);
        let desc = find_raw_desc(&manifest, &wanted).unwrap();
        assert_eq!(desc_version(&desc).as_deref(), Some("0.9.0"));
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
