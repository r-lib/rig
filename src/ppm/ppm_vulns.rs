//! `rig ppm pkg vulns`: known vulnerabilities of the packages of a repository.
//!
//! PPM serves these in the OSV format, keyed by package name. For CRAN the
//! ids are `RSEC-*`, from the R Consortium's advisory database.
//!
//! Without `--repo`, every R and Bioconductor repository is queried. Some do
//! not support vulnerabilities (e.g. Git sources), these are skipped.

use std::collections::BTreeMap;
use std::error::Error;

use clap::ArgMatches;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tabular::{Row, Table};

use crate::output::OUTPUT;
use crate::ppm::api::{self, ApiError};
use crate::ppm::ppm_repos::load_package_repos;
use crate::ppm::{print_table, want_json};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Vuln {
    id: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    ranges: Vec<VulnRange>,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VulnRange {
    #[serde(default)]
    events: Vec<BTreeMap<String, String>>,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

impl Vuln {
    /// The versions that fix the vulnerability, from the OSV ranges.
    fn fixed(&self) -> Vec<String> {
        self.ranges
            .iter()
            .flat_map(|r| r.events.iter())
            .filter_map(|e| e.get("fixed").cloned())
            .collect()
    }
}

type Vulns = BTreeMap<String, Vec<Vuln>>;

/// Vulnerabilities per repository, in the order the repositories were
/// queried.
type RepoVulns = Vec<(String, Vulns)>;

/// One table row: a vulnerability of a package, and the repositories that
/// have it.
struct VulnRow<'a> {
    package: &'a str,
    vuln: &'a Vuln,
    repos: Vec<&'a str>,
}

/// Merge the same vulnerability of the same package in several
/// repositories into one row.
fn merge_rows(all: &RepoVulns) -> Vec<VulnRow<'_>> {
    let mut rows: Vec<VulnRow> = vec![];
    let mut index: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for (repo, vulns) in all {
        for (name, list) in vulns {
            for v in list {
                match index.get(&(name.as_str(), v.id.as_str())) {
                    Some(&i) => rows[i].repos.push(repo),
                    None => {
                        index.insert((name, &v.id), rows.len());
                        rows.push(VulnRow {
                            package: name,
                            vuln: v,
                            repos: vec![repo],
                        });
                    }
                }
            }
        }
    }
    rows.sort_by(|a, b| (a.package, &a.vuln.id).cmp(&(b.package, &b.vuln.id)));
    rows
}

/// Whether an error means that the repository has no vulnerability data:
/// 400 for sources that do not support it, 404 for e.g. a Bioconductor
/// repository without a Bioconductor version.
fn no_vulns(e: &(dyn Error + 'static)) -> bool {
    matches!(e.downcast_ref::<ApiError>(), Some(ae) if ae.status == 400 || ae.status == 404)
}

pub fn sc_ppm_vulns(
    args: &ArgMatches,
    ppmargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let explicit = args.get_one::<String>("repo");
    let repos = match explicit {
        Some(r) => vec![r.clone()],
        None => load_package_repos()?,
    };
    let mut all: RepoVulns = vec![];
    for repo in &repos {
        match api::get::<Vulns>(&["repos", repo, "vulns"], &vec![]) {
            Ok(v) => all.push((repo.clone(), v)),
            // With --repo, every error is an error
            Err(e) if explicit.is_some() => return Err(e),
            Err(e) if no_vulns(e.as_ref()) => {}
            Err(e) => match e.downcast_ref::<ApiError>() {
                Some(_) => OUTPUT.warn(&format!("Skipping repository {}: {}", repo, e)),
                None => return Err(e),
            },
        }
    }
    if let Some(pkgs) = args.get_many::<String>("package") {
        let pkgs: Vec<&String> = pkgs.collect();
        for (_, vulns) in all.iter_mut() {
            vulns.retain(|name, _| pkgs.contains(&name));
        }
    }

    if want_json(args, ppmargs, mainargs) {
        let out: serde_json::Map<String, Value> = all
            .iter()
            .map(|(r, v)| Ok((r.clone(), serde_json::to_value(v)?)))
            .collect::<Result<_, serde_json::Error>>()?;
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    let rows = merge_rows(&all);
    if rows.is_empty() {
        println!("No known vulnerabilities.");
        return Ok(());
    }
    let mut tab = Table::new("{:<}   {:<}   {:<}   {:<}   {:<}");
    tab.add_row(
        Row::new()
            .with_cell("package")
            .with_cell("id")
            .with_cell("fixed")
            .with_cell("repos")
            .with_cell("summary"),
    );
    for r in &rows {
        let fixed = r.vuln.fixed();
        tab.add_row(
            Row::new()
                .with_cell(r.package)
                .with_cell(&r.vuln.id)
                .with_cell(if fixed.is_empty() {
                    "-".to_string()
                } else {
                    fixed.join(", ")
                })
                .with_cell(r.repos.join(", "))
                .with_cell(&r.vuln.summary),
        );
    }
    print_table(&tab);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_vulns() {
        let json = r#"{"haven":[{"id":"RSEC-2023-5","versions":{"0.1.0":{}},"ranges":[{"type":"ECOSYSTEM","events":[{"introduced":"0.1.0"},{"fixed":"1.1.1"}]}],"summary":"Infinite loop","details":"x","modified":"2025-05-19T19:43:47Z"}]}"#;
        let v: Vulns = serde_json::from_str(json).unwrap();
        assert_eq!(v["haven"][0].fixed(), vec!["1.1.1".to_string()]);
        let out = serde_json::to_value(&v).unwrap();
        assert_eq!(out["haven"][0]["details"], "x");
        assert_eq!(out["haven"][0]["ranges"][0]["type"], "ECOSYSTEM");
    }

    #[test]
    fn test_merge_rows() {
        let v = |id: &str| -> Vuln {
            serde_json::from_value(serde_json::json!({"id": id, "summary": "s"})).unwrap()
        };
        let cran: Vulns = [
            ("haven".to_string(), vec![v("RSEC-1")]),
            ("readxl".to_string(), vec![v("RSEC-2")]),
        ]
        .into_iter()
        .collect();
        let curated: Vulns = [("haven".to_string(), vec![v("RSEC-1"), v("RSEC-3")])]
            .into_iter()
            .collect();
        let all: RepoVulns = vec![("cran".to_string(), cran), ("curated".to_string(), curated)];
        let rows = merge_rows(&all);
        let got: Vec<(&str, &str, String)> = rows
            .iter()
            .map(|r| (r.package, r.vuln.id.as_str(), r.repos.join(",")))
            .collect();
        assert_eq!(
            got,
            vec![
                ("haven", "RSEC-1", "cran,curated".to_string()),
                ("haven", "RSEC-3", "curated".to_string()),
                ("readxl", "RSEC-2", "cran".to_string()),
            ]
        );
    }

    #[test]
    fn test_no_vulns() {
        let e = |status| -> Box<dyn Error> {
            Box::new(ApiError {
                status,
                code: None,
                message: "x".to_string(),
            })
        };
        assert!(no_vulns(e(400).as_ref()));
        assert!(no_vulns(e(404).as_ref()));
        assert!(!no_vulns(e(401).as_ref()));
        let other: Box<dyn Error> = "network".into();
        assert!(!no_vulns(other.as_ref()));
    }
}
