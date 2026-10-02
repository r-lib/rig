//! The r-system-requirements rules database
//! (<https://github.com/r-hub/r-system-requirements>): which OS packages a
//! `SystemRequirements` field asks for, on which Linux distribution.
//!
//! rig keeps a copy of the rules in its cache, refreshed once a day from
//! GitHub, and falls back to the copy embedded at build time
//! (`src/data/sysreqs-rules.json`, see `make sysreqs-rules`) when it cannot
//! download one. Both are one JSON object, keyed by rule name, with each rule
//! as it is in the repository's `rules/<name>.json`.

use std::collections::BTreeMap;
use std::error::Error;
use std::io::Read;
use std::path::PathBuf;

use flate2::read::GzDecoder;
use log::{debug, info};
use regex::Regex;
use serde::Deserialize;

use crate::cache::get_cache_dir;
use crate::download::{fetch_optional_if_modified_, ConditionalFetch};
use crate::utils::{not_too_old, write_atomically};

static EMBEDDED_RULES: &str = include_str!("../data/sysreqs-rules.json");

const DEFAULT_RULES_URL: &str =
    "https://codeload.github.com/r-hub/r-system-requirements/tar.gz/refs/heads/main";

/// One `rules/<name>.json` file.
#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    pub patterns: Vec<String>,
    pub dependencies: Vec<RuleDependency>,
}

/// The OS packages of a rule for some systems, given by `constraints`.
#[derive(Debug, Clone, Deserialize)]
pub struct RuleDependency {
    #[serde(default)]
    pub packages: Vec<String>,
    #[serde(default)]
    pub pre_install: Vec<RuleStep>,
    #[serde(default)]
    pub post_install: Vec<RuleStep>,
    #[serde(default)]
    pub constraints: Vec<RuleConstraint>,
}

/// A command to run before or after installing the packages, e.g. to enable
/// EPEL. A `script` refers to a file in the repository's `scripts/`
/// directory; rig does not run those, and no rule uses one currently.
#[derive(Debug, Clone, Deserialize)]
pub struct RuleStep {
    pub command: Option<String>,
    pub script: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RuleConstraint {
    pub os: String,
    pub distribution: Option<String>,
    /// No versions means every version of the distribution.
    pub versions: Option<Vec<String>>,
}

/// A rule, with its patterns compiled.
#[derive(Debug)]
pub struct CompiledRule {
    pub name: String,
    patterns: Vec<Regex>,
    pub dependencies: Vec<RuleDependency>,
}

impl CompiledRule {
    /// Whether the rule applies to a `SystemRequirements` field.
    pub fn matches(&self, sysreqs: &str) -> bool {
        self.patterns.iter().any(|p| p.is_match(sysreqs))
    }

    /// The dependency entry of the rule for `distribution` `version`.
    ///
    /// The rules list version-specific entries both before and after the
    /// generic entry of the same distribution, so an entry that names the
    /// version wins over one that does not name any, wherever they are.
    pub fn dependency_for(&self, distribution: &str, version: &str) -> Option<&RuleDependency> {
        let for_distro = |dep: &&RuleDependency, exact: bool| {
            dep.constraints.iter().any(|c| {
                c.os == "linux"
                    && c.distribution.as_deref() == Some(distribution)
                    && match &c.versions {
                        Some(versions) => exact && versions.iter().any(|v| v == version),
                        None => !exact,
                    }
            })
        };
        self.dependencies
            .iter()
            .find(|dep| for_distro(dep, true))
            .or_else(|| self.dependencies.iter().find(|dep| for_distro(dep, false)))
    }
}

#[derive(Debug)]
pub struct RuleDb {
    pub rules: Vec<CompiledRule>,
}

impl RuleDb {
    /// Parse the merged rules JSON. A rule with an invalid pattern is
    /// skipped, so that one bad rule does not disable all of them.
    pub fn from_json(text: &str) -> Result<RuleDb, Box<dyn Error>> {
        let parsed: BTreeMap<String, Rule> = serde_json::from_str(text)?;
        let mut rules = Vec::with_capacity(parsed.len());
        for (name, rule) in parsed {
            // The rules are written for R's `grepl(perl = TRUE)`, and match
            // the field case-insensitively.
            let patterns: Result<Vec<Regex>, _> = rule
                .patterns
                .iter()
                .map(|p| Regex::new(&format!("(?i){}", p)))
                .collect();
            match patterns {
                Ok(patterns) => rules.push(CompiledRule {
                    name,
                    patterns,
                    dependencies: rule.dependencies,
                }),
                Err(e) => debug!("Skipping system requirements rule {}: {}", name, e),
            }
        }
        Ok(RuleDb { rules })
    }

    /// The rules embedded in rig.
    pub fn embedded() -> Result<RuleDb, Box<dyn Error>> {
        RuleDb::from_json(EMBEDDED_RULES)
    }

    /// The cached rules, refreshed if older than a day, or the embedded ones
    /// if there are no usable cached rules.
    pub fn load() -> Result<RuleDb, Box<dyn Error>> {
        match cached_rules() {
            Ok(Some(text)) => match RuleDb::from_json(&text) {
                Ok(db) => return Ok(db),
                Err(e) => debug!("Invalid cached system requirements rules: {}", e),
            },
            Ok(None) => {}
            Err(e) => debug!("Cannot use cached system requirements rules: {}", e),
        }
        info!("Using the system requirements rules embedded in rig");
        RuleDb::embedded()
    }
}

/// Where the rules are downloaded from, a gzipped tarball of the repository.
fn rules_url() -> String {
    if let Ok(url) = std::env::var("RIG_SYSREQS_RULES_URL") {
        return url;
    }
    if let Ok(Some(url)) = crate::config::get_global_config_value("sysreqs-rules-url") {
        return url;
    }
    DEFAULT_RULES_URL.to_string()
}

fn cache_paths() -> Result<(PathBuf, PathBuf), Box<dyn Error>> {
    let dir = get_cache_dir()?.join("sysreqs");
    Ok((dir.join("rules.json"), dir.join("rules.etag")))
}

/// The text of the cached rules, refreshing them first if they are older
/// than a day. A failed refresh is not an error: rig uses the old copy, or
/// the embedded one if there is none.
fn cached_rules() -> Result<Option<String>, Box<dyn Error>> {
    let (path, etag_path) = cache_paths()?;
    if not_too_old(&path) {
        return Ok(Some(std::fs::read_to_string(&path)?));
    }

    let url = rules_url();
    let etag = if path.exists() {
        std::fs::read_to_string(&etag_path).ok()
    } else {
        None
    };
    match fetch_optional_if_modified_(&url, etag.as_deref(), None) {
        Ok(ConditionalFetch::Fetched { bytes, etag }) => {
            let text = rules_from_tarball(&bytes)?;
            write_atomically(&path, text.as_bytes())?;
            match etag {
                Some(etag) => write_atomically(&etag_path, etag.as_bytes())?,
                None => {
                    let _ = std::fs::remove_file(&etag_path);
                }
            }
            info!("Downloaded system requirements rules from {}", url);
            Ok(Some(text))
        }
        Ok(ConditionalFetch::NotModified) => {
            // Rewrite the file to reset its age.
            let text = std::fs::read_to_string(&path)?;
            write_atomically(&path, text.as_bytes())?;
            Ok(Some(text))
        }
        Ok(ConditionalFetch::NotFound) => {
            debug!("System requirements rules not found at {}", url);
            Ok(std::fs::read_to_string(&path).ok())
        }
        Err(e) => {
            debug!(
                "Cannot download system requirements rules from {}: {}",
                url, e
            );
            Ok(std::fs::read_to_string(&path).ok())
        }
    }
}

/// Merge the `rules/*.json` files of a repository tarball into one JSON
/// object, the format of the embedded rules.
pub fn rules_from_tarball(bytes: &[u8]) -> Result<String, Box<dyn Error>> {
    let mut archive = tar::Archive::new(GzDecoder::new(bytes));
    let mut rules: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        // `<repo>-<ref>/rules/<name>.json`
        let comps: Vec<String> = path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect();
        if comps.len() != 3 || comps[1] != "rules" || !comps[2].ends_with(".json") {
            continue;
        }
        let name = comps[2].trim_end_matches(".json").to_string();
        let mut text = String::new();
        entry.read_to_string(&mut text)?;
        rules.insert(name, serde_json::from_str(&text)?);
    }
    if rules.is_empty() {
        bail!("No system requirements rules in the downloaded archive");
    }
    Ok(serde_json::to_string_pretty(&rules)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_rules_compile() {
        let text: BTreeMap<String, serde_json::Value> =
            serde_json::from_str(EMBEDDED_RULES).unwrap();
        let db = RuleDb::embedded().unwrap();
        // Every rule's patterns compile with the `regex` crate.
        assert_eq!(db.rules.len(), text.len());
        assert!(db.rules.len() > 100);
    }

    #[test]
    fn patterns_match_case_insensitively() {
        let db = RuleDb::embedded().unwrap();
        let curl = db.rules.iter().find(|r| r.name == "libcurl").unwrap();
        assert!(curl.matches("libcurl: libcurl-devel (rpm) or libcurl4-openssl-dev (deb)"));
        assert!(curl.matches("LibCurl"));
        assert!(!curl.matches("curlish"));
    }

    fn rule(deps: serde_json::Value) -> CompiledRule {
        CompiledRule {
            name: "test".to_string(),
            patterns: vec![],
            dependencies: serde_json::from_value(deps).unwrap(),
        }
    }

    #[test]
    fn version_specific_entry_wins() {
        // Generic entry first, then a version-specific one.
        let r = rule(serde_json::json!([
            { "packages": ["generic"],
              "constraints": [{ "os": "linux", "distribution": "redhat" }] },
            { "packages": ["six"],
              "constraints": [{ "os": "linux", "distribution": "redhat", "versions": ["6"] }] }
        ]));
        assert_eq!(
            r.dependency_for("redhat", "6").unwrap().packages,
            vec!["six"]
        );
        assert_eq!(
            r.dependency_for("redhat", "9").unwrap().packages,
            vec!["generic"]
        );
        assert!(r.dependency_for("ubuntu", "22.04").is_none());

        // A version-specific entry first, then the generic one.
        let r = rule(serde_json::json!([
            { "packages": ["old"],
              "constraints": [{ "os": "linux", "distribution": "ubuntu",
                                "versions": ["14.04", "16.04"] }] },
            { "packages": ["new"],
              "constraints": [{ "os": "linux", "distribution": "ubuntu" },
                              { "os": "linux", "distribution": "debian" }] }
        ]));
        assert_eq!(
            r.dependency_for("ubuntu", "16.04").unwrap().packages,
            vec!["old"]
        );
        assert_eq!(
            r.dependency_for("ubuntu", "24.04").unwrap().packages,
            vec!["new"]
        );
        assert_eq!(
            r.dependency_for("debian", "12").unwrap().packages,
            vec!["new"]
        );
    }

    #[test]
    fn rules_from_tarball_merges_rule_files() {
        let mut builder = tar::Builder::new(Vec::new());
        let mut add = |path: &str, data: &str| {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, path, data.as_bytes())
                .unwrap();
        };
        add(
            "repo-main/rules/zlib.json",
            r#"{"patterns": ["\\bzlib\\b"], "dependencies": []}"#,
        );
        add("repo-main/systems.json", "[]");
        add("repo-main/test/rules/x.json", "{}");
        let tar = builder.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut gz, &tar).unwrap();
        let bytes = gz.finish().unwrap();

        let text = rules_from_tarball(&bytes).unwrap();
        let db = RuleDb::from_json(&text).unwrap();
        assert_eq!(db.rules.len(), 1);
        assert_eq!(db.rules[0].name, "zlib");
        assert!(db.rules[0].matches("zlib"));
    }
}
