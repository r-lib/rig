// `rproj.lock`: the multi-target project lockfile written by `rig proj lock`
// and read by `rig proj sync`. TOML, unlike the JSON `pkg.lock` written by
// `rig pkg install` (`src/pak.rs`), which stays as-is because it mirrors the R
// `pak` package's own lockfile schema for interop with `pak::lockfile_*()`.
//
// `rproj.lock` is not interop with anything external; it is rig's own format,
// designed to hold the solve for *several* `(R version, platform)` targets in
// one file — e.g. solving once on a laptop for both macOS and a Linux
// deployment target. Each target's package list reuses `PakLockfilePackage`
// as-is (verified it round-trips cleanly through the `toml` crate, table
// fields and all), so a target's dependency data is exactly what `pkg.lock`
// would have recorded for that one target, just nested under it instead of
// being the whole file.
//
// For now (first implementation slice) `rig proj lock` only ever writes one
// target, and `rig proj sync` always installs `targets[0]`; the multi-target
// solve loop and the "pick the entry matching this machine" logic in `sync`
// are follow-up work.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::pak::PakLockfilePackage;

pub const RPROJ_LOCK_VERSION: usize = 1;

// `rproj.toml`: the project/package manifest (see the design doc). This is the
// *requirements* file a human edits, as opposed to `rproj.lock` (the solved
// output above). rig owns the schema; a `DESCRIPTION` can be generated from it
// (follow-up work). For now the model round-trips through TOML and backs
// `rig proj init`.
pub const RPROJ_MANIFEST_FILE: &str = "rproj.toml";

/// A parsed `rproj.toml` manifest.
///
/// Key ordering is not significant, so dependency tables use `BTreeMap` (they
/// round-trip deterministically, sorted). All top-level fields are tables or
/// arrays-of-tables, so TOML's "values before tables" rule is never at risk.
#[derive(Serialize, Deserialize, Debug, Default, PartialEq)]
pub struct Rproj {
    pub project: Project,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, Dependency>,
    #[serde(
        rename = "linking-dependencies",
        default,
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub linking_dependencies: BTreeMap<String, Dependency>,
    #[serde(
        rename = "optional-dependencies",
        default,
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub optional_dependencies: BTreeMap<String, BTreeMap<String, Dependency>>,
    #[serde(
        rename = "dependency-groups",
        default,
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub dependency_groups: BTreeMap<String, Group>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repository: Vec<Repository>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<Build>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bin: Vec<Bin>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub config: BTreeMap<String, toml::Table>,
    // `[description]` escape hatch: raw DESCRIPTION fields with no structured
    // home (e.g. `License_is_FOSS`), passed through verbatim.
    #[serde(default, skip_serializing_if = "toml::Table::is_empty")]
    pub description: toml::Table,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<Workspace>,
}

/// `[project]` — identity/metadata. Scalar fields serialize before `urls`
/// (a sub-table), keeping TOML happy.
#[derive(Serialize, Deserialize, Debug, Default, PartialEq)]
pub struct Project {
    pub name: String,
    pub version: String,
    // `Type:` in DESCRIPTION. Manifest default is "project" (not built).
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<Author>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub urls: BTreeMap<String, String>,
}

/// One `authors = [...]` entry; generates a `person()` in `Authors@R`.
#[derive(Serialize, Deserialize, Debug, Default, PartialEq)]
pub struct Author {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orcid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ror: Option<String>,
}

/// A dependency value: either a bare version string (`"^1.2"`) or a table with
/// a source/flags. Untagged so both spellings parse.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(untagged)]
pub enum Dependency {
    Version(String),
    Detailed(DepTable),
}

/// The table form of a dependency (`{ version = ..., git = ..., attach = ... }`).
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct DepTable {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attach: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enhances: Option<bool>,
    #[serde(
        rename = "vignette-builder",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub vignette_builder: Option<bool>,
}

/// A `[dependency-groups.<name>]` entry: package specs plus an optional
/// `include-groups` list that pulls in other groups.
#[derive(Serialize, Deserialize, Debug, Default, PartialEq)]
pub struct Group {
    #[serde(
        rename = "include-groups",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub include_groups: Vec<String>,
    #[serde(flatten)]
    pub dependencies: BTreeMap<String, Dependency>,
}

/// One `[[repository]]`. Array order is precedence (first = highest).
#[derive(Serialize, Deserialize, Debug, Default, PartialEq)]
pub struct Repository {
    pub name: String,
    pub url: String,
}

/// `[build]` — package build flags.
#[derive(Serialize, Deserialize, Debug, Default, PartialEq)]
pub struct Build {
    #[serde(
        rename = "byte-compile",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub byte_compile: Option<bool>,
    #[serde(
        rename = "needs-compilation",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub needs_compilation: Option<bool>,
    #[serde(rename = "lazy-data", default, skip_serializing_if = "Option::is_none")]
    pub lazy_data: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub biarch: Option<bool>,
}

/// One `[[bin]]` — a named entry-point script run via `rig run <name>`.
#[derive(Serialize, Deserialize, Debug, Default, PartialEq)]
pub struct Bin {
    pub name: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// `[workspace]` — a cargo-style monorepo of member manifests.
#[derive(Serialize, Deserialize, Debug, Default, PartialEq)]
pub struct Workspace {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, Dependency>,
}

impl Rproj {
    /// The minimal skeleton written by `rig proj init`: a `[project]` with the
    /// given name and a single R dependency.
    pub fn minimal(name: &str) -> Self {
        let mut dependencies = BTreeMap::new();
        dependencies.insert("R".to_string(), Dependency::Version(">= 4.1".to_string()));
        Rproj {
            project: Project {
                name: name.to_string(),
                version: "0.1.0".to_string(),
                type_: Some("project".to_string()),
                ..Default::default()
            },
            dependencies,
            ..Default::default()
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct RprojLock {
    pub version: usize,
    pub targets: Vec<RprojLockTarget>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct RprojLockTarget {
    pub r_version: String,
    pub platform: String,
    pub packages: Vec<PakLockfilePackage>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn sample_package() -> PakLockfilePackage {
        PakLockfilePackage {
            r#ref: "cli".to_string(),
            package: "cli".to_string(),
            version: "3.6.0".to_string(),
            r#type: "standard".to_string(),
            direct: true,
            binary: true,
            dependencies: vec!["rlang".to_string()],
            vignettes: false,
            metadata: HashMap::from([("RemoteSha".to_string(), "abc123".to_string())]),
            sources: vec!["https://example.com/cli.tgz".to_string()],
            target: "cli.tgz".to_string(),
            platform: "aarch64-apple-darwin".to_string(),
            rversion: "4.6".to_string(),
            directpkg: true,
            license: "MIT".to_string(),
            dep_types: vec!["Imports".to_string()],
            params: vec![],
            install_args: "".to_string(),
            sysreqs: "".to_string(),
        }
    }

    fn dep(v: &str) -> Dependency {
        Dependency::Version(v.to_string())
    }

    #[test]
    fn minimal_manifest_serializes_expected() {
        let text = toml::to_string_pretty(&Rproj::minimal("mypkg")).unwrap();
        assert_eq!(
            text,
            "[project]\n\
             name = \"mypkg\"\n\
             version = \"0.1.0\"\n\
             type = \"project\"\n\
             \n\
             [dependencies]\n\
             R = \">= 4.1\"\n"
        );
        // and it parses back to the same value
        let parsed: Rproj = toml::from_str(&text).unwrap();
        assert_eq!(parsed, Rproj::minimal("mypkg"));
    }

    #[test]
    fn full_manifest_roundtrips_through_toml() {
        let mut m = Rproj::minimal("mypkg");
        m.project.type_ = Some("package".to_string());
        m.project.title = Some("A Modern Thing".to_string());
        m.project.license = Some("MIT + file LICENSE".to_string());
        m.project.keywords = Some(vec!["cli".to_string()]);
        m.project.authors = vec![Author {
            name: "Gábor Csárdi".to_string(),
            email: Some("gabor@posit.co".to_string()),
            roles: vec!["aut".to_string(), "cre".to_string()],
            orcid: Some("0000-0001-7098-9676".to_string()),
            ror: None,
        }];
        m.project
            .urls
            .insert("homepage".to_string(), "https://example.org".to_string());

        m.dependencies.insert("cli".to_string(), dep(">= 3.6.5"));
        m.dependencies.insert(
            "ts".to_string(),
            Dependency::Detailed(DepTable {
                git: Some("https://github.com/gaborcsardi/ts".to_string()),
                branch: Some("main".to_string()),
                ..Default::default()
            }),
        );
        m.linking_dependencies
            .insert("Rcpp".to_string(), dep(">= 1.0"));

        let mut viz = BTreeMap::new();
        viz.insert("ggplot2".to_string(), dep("*"));
        m.optional_dependencies.insert("viz".to_string(), viz);

        let mut test_deps = BTreeMap::new();
        test_deps.insert("testthat".to_string(), dep(">= 3.0"));
        m.dependency_groups.insert(
            "test".to_string(),
            Group {
                include_groups: vec![],
                dependencies: test_deps,
            },
        );
        m.dependency_groups.insert(
            "dev".to_string(),
            Group {
                include_groups: vec!["test".to_string()],
                dependencies: BTreeMap::from([("lintr".to_string(), dep("*"))]),
            },
        );

        m.repository = vec![Repository {
            name: "CRAN".to_string(),
            url: "https://cran.r-project.org".to_string(),
        }];
        m.build = Some(Build {
            byte_compile: Some(true),
            needs_compilation: Some(true),
            lazy_data: None,
            biarch: None,
        });
        m.bin = vec![Bin {
            name: "report".to_string(),
            path: "scripts/report.R".to_string(),
            description: Some("Build the report".to_string()),
        }];
        m.config.insert(
            "testthat".to_string(),
            toml::Table::from_iter([("edition".to_string(), toml::Value::Integer(3))]),
        );
        m.description.insert(
            "License_is_FOSS".to_string(),
            toml::Value::String("yes".to_string()),
        );
        m.workspace = Some(Workspace {
            members: vec!["packages/*".to_string()],
            exclude: vec![],
            dependencies: BTreeMap::from([("cli".to_string(), dep(">= 3.6.5"))]),
        });

        let text = toml::to_string_pretty(&m).unwrap();
        let parsed: Rproj = toml::from_str(&text).unwrap();
        assert_eq!(parsed, m);
    }

    #[test]
    fn roundtrips_through_toml() {
        let lock = RprojLock {
            version: RPROJ_LOCK_VERSION,
            targets: vec![RprojLockTarget {
                r_version: "4.6".to_string(),
                platform: "aarch64-apple-darwin".to_string(),
                packages: vec![sample_package()],
            }],
        };
        let text = toml::to_string_pretty(&lock).unwrap();
        let parsed: RprojLock = toml::from_str(&text).unwrap();
        assert_eq!(parsed.version, 1);
        assert_eq!(parsed.targets.len(), 1);
        assert_eq!(parsed.targets[0].r_version, "4.6");
        assert_eq!(parsed.targets[0].packages[0].r#ref, "cli");
        assert_eq!(
            parsed.targets[0].packages[0].metadata.get("RemoteSha"),
            Some(&"abc123".to_string())
        );
    }
}
