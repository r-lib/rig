//! `rig ppm pkg list`, `rig ppm pkg info` and `rig ppm pkg history`: the
//! packages of a Package Manager repository.

use std::error::Error;

use clap::ArgMatches;
use owo_colors::OwoColorize;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tabular::{Row, Table};

use crate::ppm::api::{self, Query};
use crate::ppm::ppm_repos::{
    bioc_query, first_repo_with, limit_arg, load_sources, query_repos, with_bioc_version,
};
use crate::ppm::{print_table, use_color, want_json, yesno};
use crate::repos::binaries::validate_package_name;
use crate::textfmt::print_field;

/// A package record. PPM sends many more fields, they are kept in `extra`
/// for `--json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Package {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl Package {
    /// A field as text, `""` for a missing or `null` one.
    fn field(&self, name: &str) -> String {
        match self.extra.get(name) {
            None | Some(Value::Null) => String::new(),
            Some(Value::String(s)) => s.clone(),
            Some(v) => v.to_string(),
        }
    }
}

// ------------------------------------------------------------------------
// packages

/// The fields `rig ppm pkg list` shows by default.
const DEFAULT_FIELDS: &[&str] = &["name", "version", "repo", "title"];

pub fn sc_ppm_packages(
    args: &ArgMatches,
    ppmargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let limit = limit_arg(args, 100);
    let mut q: Query = vec![];
    if let Some(s) = args.get_one::<String>("search") {
        q.push(("name_like".to_string(), s.to_string()));
        q.push(("exact_first".to_string(), "true".to_string()));
    }
    let pkgs: Vec<Package> = match args.get_one::<String>("source") {
        Some(source) => {
            bioc_query(args, &mut q);
            // The source endpoint wants an id. Names are looked up among all
            // sources, which needs a token.
            let id = if source.chars().all(|c| c.is_ascii_digit()) {
                source.clone()
            } else {
                load_sources(None)?
                    .into_iter()
                    .find(|s| &s.name == source)
                    .ok_or_else(|| format!("No source named '{}'", source))?
                    .id
                    .to_string()
            };
            api::get_paged(&["sources", &id, "packages"], &q, limit)?
        }
        None => {
            // At most `limit` packages in total, from the repositories in
            // order.
            let mut left = limit;
            let found = query_repos(args, |repo| {
                if left == 0 {
                    return Ok(vec![]);
                }
                let (mut pkgs, _) = with_bioc_version(args, repo, &q, |q| {
                    api::get_paged::<Package>(&["repos", repo, "packages"], q, left)
                })?;
                pkgs.truncate(left);
                left -= pkgs.len();
                Ok(pkgs)
            })?;
            found
                .into_iter()
                .flat_map(|(repo, pkgs)| {
                    pkgs.into_iter().map(move |mut p| {
                        p.extra
                            .entry("repo")
                            .or_insert_with(|| Value::String(repo.clone()));
                        p
                    })
                })
                .collect()
        }
    };

    let fields: Vec<String> = match args.get_many::<String>("fields") {
        Some(f) => f
            .flat_map(|s| s.split(','))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        None => DEFAULT_FIELDS.iter().map(|s| s.to_string()).collect(),
    };

    if want_json(args, ppmargs, mainargs) {
        // Only the selected fields: the full records are long.
        let out: Vec<serde_json::Map<String, Value>> = pkgs
            .iter()
            .map(|p| {
                let full = serde_json::to_value(p).unwrap_or(Value::Null);
                fields
                    .iter()
                    .map(|f| (f.clone(), full.get(f).cloned().unwrap_or(Value::Null)))
                    .collect()
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    let spec = vec!["{:<}"; fields.len()].join("   ");
    let mut tab = Table::new(&spec);
    let mut header = Row::new();
    for f in &fields {
        header = header.with_cell(f);
    }
    tab.add_row(header);
    for p in &pkgs {
        let mut row = Row::new();
        for f in &fields {
            let v = match f.as_str() {
                "name" => p.name.clone(),
                "version" => p.version.clone(),
                other => p.field(other),
            };
            // One line per package
            row = row.with_cell(v.split_whitespace().collect::<Vec<_>>().join(" "));
        }
        tab.add_row(row);
    }
    print_table(&tab);
    Ok(())
}

// ------------------------------------------------------------------------
// package

/// Fields of the package record that are too big to print, even in JSON:
/// `links` lists every dependency and reverse dependency (almost 1 MB for a
/// popular package), `readme_data` is the README.
const BIG_FIELDS: &[&str] = &["links", "readme_data"];

pub fn sc_ppm_package(
    args: &ArgMatches,
    ppmargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let name = args.get_one::<String>("package").unwrap();
    validate_package_name(name)?;
    let (repo, (mut pkg, bioc)) = first_repo_with(args, name, |repo| {
        with_bioc_version(args, repo, &vec![], |q| {
            api::get::<Package>(&["repos", repo, "packages", name], q)
        })
    })?;
    for f in BIG_FIELDS {
        pkg.extra.remove(*f);
    }
    pkg.extra
        .entry("repo")
        .or_insert_with(|| Value::String(repo.clone()));
    if let Some(v) = &bioc {
        pkg.extra
            .entry("bioc_version")
            .or_insert_with(|| Value::String(v.clone()));
    }

    if want_json(args, ppmargs, mainargs) {
        println!("{}", serde_json::to_string_pretty(&pkg)?);
        return Ok(());
    }

    let color = use_color();
    let title = format!("{} {}", pkg.name, pkg.version);
    if color {
        println!("{}   {}", title.bold(), pkg.field("title").dimmed());
    } else {
        println!("{}   {}", title, pkg.field("title"));
    }
    println!();
    let archived: Vec<String> = pkg
        .extra
        .get("archived")
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.get("version").and_then(|v| v.as_str()))
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();
    let vulns = pkg
        .extra
        .get("vulns")
        .and_then(|v| v.as_array())
        .map(|v| {
            v.iter()
                .filter_map(|x| x.get("id").and_then(|i| i.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let size = pkg
        .extra
        .get("package_size")
        .and_then(|s| s.as_u64())
        .map(human_size)
        .unwrap_or_default();
    let blocked = pkg
        .extra
        .get("blocked")
        .and_then(|b| b.as_bool())
        .unwrap_or(false);
    let rows: Vec<(&str, String)> = vec![
        ("repo", repo),
        ("bioc_version", bioc.unwrap_or_default()),
        ("source", pkg.field("source")),
        ("published", pkg.field("date_publication")),
        ("license", pkg.field("license")),
        ("maintainer", pkg.field("maintainer")),
        ("url", pkg.field("url")),
        ("bug_reports", pkg.field("bug_reports")),
        ("depends", pkg.field("depends")),
        ("imports", pkg.field("imports")),
        ("linking_to", pkg.field("linking_to")),
        ("suggests", pkg.field("suggests")),
        ("needs_compilation", pkg.field("needs_compilation")),
        ("system_requirements", pkg.field("system_requirements")),
        ("size", size),
        ("checksum", pkg.field("checksum")),
        ("blocked", yesno(blocked).to_string()),
        ("vulnerabilities", vulns),
        ("archived_versions", archived.join(", ")),
    ];
    for (label, value) in rows {
        let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
        if !value.is_empty() {
            print_field(label, &value, 20, color);
        }
    }
    Ok(())
}

fn human_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1024.0 * 1024.0 {
        format!("{:.1} MB", b / 1024.0 / 1024.0)
    } else if b >= 1024.0 {
        format!("{:.1} kB", b / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

// ------------------------------------------------------------------------
// history

#[derive(Debug, Clone, Serialize, Deserialize)]
struct History {
    name: String,
    #[serde(default)]
    snapshots: Vec<HistorySnapshot>,
    #[serde(default)]
    page: i64,
    #[serde(default)]
    pages: i64,
    #[serde(default)]
    total: i64,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HistorySnapshot {
    date: String,
    #[serde(default)]
    snapshot_url: String,
    #[serde(default)]
    change_desc: String,
    #[serde(default)]
    latest_version: Option<HistoryVersion>,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HistoryVersion {
    version: String,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

pub fn sc_ppm_history(
    args: &ArgMatches,
    ppmargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let name = args.get_one::<String>("package").unwrap();
    validate_package_name(name)?;
    let limit = limit_arg(args, 100);

    // The endpoint is paged, but returns an object, not a list.
    let page_size = limit.clamp(1, 100);
    let page_query = |page: usize| -> Query {
        vec![
            ("_page".to_string(), page.to_string()),
            ("_limit".to_string(), page_size.to_string()),
        ]
    };
    let (repo, (first, bioc)) = first_repo_with(args, name, |repo| {
        with_bioc_version(args, repo, &page_query(1), |q| {
            api::get::<History>(&["repos", repo, "packages", name, "history"], q)
        })
    })?;
    let fetch = |page: usize| -> Result<History, Box<dyn Error>> {
        let mut q = page_query(page);
        if let Some(v) = &bioc {
            q.push(("bioc_version".to_string(), v.clone()));
        }
        api::get(&["repos", &repo, "packages", name, "history"], &q)
    };
    let mut snapshots: Vec<HistorySnapshot> = vec![];
    let mut page = 1;
    let mut h = first;
    loop {
        let n = h.snapshots.len();
        let last = h.pages <= page as i64 || n < page_size;
        snapshots.append(&mut h.snapshots);
        if last || snapshots.len() >= limit {
            break;
        }
        page += 1;
        h = fetch(page)?;
    }
    let mut hist = h;
    snapshots.truncate(limit);
    hist.snapshots = snapshots;
    hist.extra
        .entry("repo")
        .or_insert_with(|| Value::String(repo.clone()));
    if let Some(v) = &bioc {
        hist.extra
            .entry("bioc_version")
            .or_insert_with(|| Value::String(v.clone()));
    }

    if want_json(args, ppmargs, mainargs) {
        println!("{}", serde_json::to_string_pretty(&hist)?);
        return Ok(());
    }
    print_field("repo", &repo, 12, use_color());
    if let Some(v) = &bioc {
        print_field("bioc_version", v, 12, use_color());
    }
    println!();
    let mut tab = Table::new("{:<}   {:<}   {:<}");
    tab.add_row(
        Row::new()
            .with_cell("snapshot")
            .with_cell("version")
            .with_cell("change"),
    );
    for s in &hist.snapshots {
        tab.add_row(
            Row::new()
                .with_cell(&s.date)
                .with_cell(
                    s.latest_version
                        .as_ref()
                        .map(|v| v.version.as_str())
                        .unwrap_or(""),
                )
                .with_cell(&s.change_desc),
        );
    }
    print_table(&tab);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_package_fields() {
        let json = r#"{"name":"dplyr","version":"1.2.1","title":"A Grammar\n of Data","license":"MIT","linking_to":null,"package_size":921588,"blocked":false}"#;
        let p: Package = serde_json::from_str(json).unwrap();
        assert_eq!(p.field("license"), "MIT");
        assert_eq!(p.field("linking_to"), "");
        assert_eq!(p.field("nope"), "");
        assert_eq!(p.field("package_size"), "921588");
        assert_eq!(p.field("blocked"), "false");
    }

    #[test]
    fn test_human_size() {
        assert_eq!(human_size(10), "10 B");
        assert_eq!(human_size(2048), "2.0 kB");
        assert_eq!(human_size(921588), "900.0 kB");
        assert_eq!(human_size(3 * 1024 * 1024), "3.0 MB");
    }

    #[test]
    fn test_parse_history() {
        let json = r#"{"name":"dplyr","repo_type":"R","snapshots":[{"date":"2026-04-04","snapshot_url":"https://p3m.dev/cran/2026-04-04","change_desc":"v1.2.1 added from cran","latest_version":{"version":"1.2.1","is_latest":true,"sources":[]},"other_versions":[]}],"page":1,"pages":15,"size":2,"total":29}"#;
        let h: History = serde_json::from_str(json).unwrap();
        assert_eq!(h.pages, 15);
        assert_eq!(
            h.snapshots[0].latest_version.as_ref().unwrap().version,
            "1.2.1"
        );
        let out = serde_json::to_value(&h).unwrap();
        assert_eq!(out["repo_type"], "R");
        assert_eq!(out["snapshots"][0]["other_versions"], serde_json::json!([]));
    }
}
