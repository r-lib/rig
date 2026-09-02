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
use std::error::Error;

use serde::{Deserialize, Serialize};

use crate::dcf::{
    DepVersionSpec, Package as DcfPackage, PackageDependencies, RDepType, VersionConstraint,
    DEP_TYPES_SOFT,
};
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

    /// Merge a DESCRIPTION-derived `Package`'s dependencies into this
    /// manifest, upserting entries (an existing entry for the same package
    /// name is overwritten). `Depends`/`Imports` land in `[dependencies]`
    /// (`Depends` marked with `attach = true`, except for `R` itself, which
    /// stays a plain version string); `LinkingTo` also lands in
    /// `[linking-dependencies]` (a package can be in both tables at once,
    /// e.g. `Rcpp` in both `Imports` and `LinkingTo`); `Suggests`/`Enhances`
    /// land in `[dependency-groups.test]` / `[dependency-groups.enhances]`.
    pub fn merge_description(&mut self, pkg: &DcfPackage) {
        for dep in pkg.dependencies.dependencies.iter() {
            let version_str = format_constraints(&dep.constraints);
            let hard = dep.types.contains(&RDepType::Depends)
                || dep.types.contains(&RDepType::Imports)
                || dep.types.contains(&RDepType::LinkingTo);

            if dep.types.contains(&RDepType::Depends) || dep.types.contains(&RDepType::Imports) {
                let value = if dep.name != "R" && dep.types.contains(&RDepType::Depends) {
                    Dependency::Detailed(DepTable {
                        version: Some(version_str.clone()),
                        attach: Some(true),
                        ..Default::default()
                    })
                } else {
                    Dependency::Version(version_str.clone())
                };
                self.dependencies.insert(dep.name.clone(), value);
            }

            if dep.types.contains(&RDepType::LinkingTo) {
                self.linking_dependencies
                    .insert(dep.name.clone(), Dependency::Version(version_str.clone()));
            }

            if !hard {
                if dep.types.contains(&RDepType::Suggests) {
                    self.dependency_groups
                        .entry("test".to_string())
                        .or_default()
                        .dependencies
                        .insert(dep.name.clone(), Dependency::Version(version_str.clone()));
                }
                if dep.types.contains(&RDepType::Enhances) {
                    self.dependency_groups
                        .entry("enhances".to_string())
                        .or_default()
                        .dependencies
                        .insert(dep.name.clone(), Dependency::Version(version_str.clone()));
                }
            }
        }
    }

    /// The manifest's dependencies as the solver's [`PackageDependencies`], the
    /// inverse of [`Rproj::merge_description`]: `[dependencies]` becomes
    /// `Depends` (entries marked `attach = true`, and `R` itself) or `Imports`,
    /// `[linking-dependencies]` becomes `LinkingTo`, and the `test` / `enhances`
    /// dependency groups become `Suggests` / `Enhances`. Other groups have no
    /// DESCRIPTION dependency type to map to and are left out.
    ///
    /// Soft dependencies are dropped unless `dev`; a package that is also a hard
    /// dependency stays, because it needs to be installed either way.
    pub fn to_dep_version_specs(&self, dev: bool) -> Result<PackageDependencies, Box<dyn Error>> {
        let mut deps: Vec<DepVersionSpec> = Vec::new();

        for (name, dep) in self.dependencies.iter() {
            let dep_type = if name == "R" || dep_attach(dep) {
                RDepType::Depends
            } else {
                RDepType::Imports
            };
            deps.push(dep_spec(name, dep, dep_type)?);
        }

        for (name, dep) in self.linking_dependencies.iter() {
            deps.push(dep_spec(name, dep, RDepType::LinkingTo)?);
        }

        for (group, dep_type) in [
            ("test", RDepType::Suggests),
            ("enhances", RDepType::Enhances),
        ] {
            if let Some(group) = self.dependency_groups.get(group) {
                for (name, dep) in group.dependencies.iter() {
                    deps.push(dep_spec(name, dep, dep_type.clone())?);
                }
            }
        }

        let mut pkg_deps = PackageDependencies { dependencies: deps };
        pkg_deps.simplify();

        if !dev {
            pkg_deps
                .dependencies
                .retain(|dep| !dep.types.iter().all(|t| DEP_TYPES_SOFT.contains(t)));
        }

        Ok(pkg_deps)
    }
}

/// Whether a dependency is attached (`Depends:` rather than `Imports:`).
fn dep_attach(dep: &Dependency) -> bool {
    match dep {
        Dependency::Version(_) => false,
        Dependency::Detailed(t) => t.attach == Some(true),
    }
}

/// One manifest dependency entry as a solver [`DepVersionSpec`]. A dependency
/// with no version (a bare `"*"`, or a table that only names a source, e.g.
/// `git = ...`) has no constraints.
fn dep_spec(
    name: &str,
    dep: &Dependency,
    dep_type: RDepType,
) -> Result<DepVersionSpec, Box<dyn Error>> {
    let version = match dep {
        Dependency::Version(v) => Some(v.as_str()),
        Dependency::Detailed(t) => t.version.as_deref(),
    };
    Ok(DepVersionSpec {
        name: name.to_string(),
        types: vec![dep_type],
        constraints: parse_constraints(version.unwrap_or("*"))?,
    })
}

/// Parse an `rproj.toml` version string, e.g. `">= 1.0, < 2.0"`, into version
/// constraints. The inverse of [`format_constraints`]; `"*"` means no
/// constraint.
fn parse_constraints(version: &str) -> Result<Vec<VersionConstraint>, Box<dyn Error>> {
    let version = version.trim();
    if version.is_empty() || version == "*" {
        return Ok(vec![]);
    }
    version
        .split(',')
        .map(|c| VersionConstraint::from_str(c.trim()))
        .collect()
}

/// Format a dependency's version constraints as an `rproj.toml` version
/// string, e.g. `">= 1.0, < 2.0"`, or `"*"` if there are none.
fn format_constraints(constraints: &[VersionConstraint]) -> String {
    if constraints.is_empty() {
        return "*".to_string();
    }
    constraints
        .iter()
        .map(|c| format!("{} {}", c.constraint_type, c.version))
        .collect::<Vec<_>>()
        .join(", ")
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
    use crate::dcf::{DepVersionSpec, RPackageVersion, VersionConstraintType};
    use std::collections::HashMap;

    fn spec(name: &str, types: &[RDepType], constraints: Vec<VersionConstraint>) -> DepVersionSpec {
        DepVersionSpec {
            name: name.to_string(),
            types: types.to_vec(),
            constraints,
        }
    }

    fn constraint(op: VersionConstraintType, version: &str) -> VersionConstraint {
        VersionConstraint {
            constraint_type: op,
            version: RPackageVersion::from_str(version).unwrap(),
        }
    }

    fn package(deps: Vec<DepVersionSpec>) -> DcfPackage {
        DcfPackage::from_crandb(
            "mypkg".to_string(),
            RPackageVersion::from_str("1.0.0").unwrap(),
            deps,
        )
    }

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

    #[test]
    fn merge_description_imports_go_to_dependencies() {
        let mut m = Rproj::minimal("mypkg");
        m.dependencies.clear();
        let pkg = package(vec![spec(
            "cli",
            &[RDepType::Imports],
            vec![constraint(VersionConstraintType::GreaterOrEqual, "3.6.5")],
        )]);
        m.merge_description(&pkg);
        assert_eq!(m.dependencies.get("cli"), Some(&dep(">= 3.6.5")));
    }

    #[test]
    fn merge_description_depends_sets_attach() {
        let mut m = Rproj::minimal("mypkg");
        m.dependencies.clear();
        let pkg = package(vec![spec("crayon", &[RDepType::Depends], vec![])]);
        m.merge_description(&pkg);
        assert_eq!(
            m.dependencies.get("crayon"),
            Some(&Dependency::Detailed(DepTable {
                version: Some("*".to_string()),
                attach: Some(true),
                ..Default::default()
            }))
        );
    }

    #[test]
    fn merge_description_r_depends_stays_plain() {
        let mut m = Rproj::minimal("mypkg");
        m.dependencies.clear();
        let pkg = package(vec![spec(
            "R",
            &[RDepType::Depends],
            vec![constraint(VersionConstraintType::GreaterOrEqual, "4.1")],
        )]);
        m.merge_description(&pkg);
        assert_eq!(m.dependencies.get("R"), Some(&dep(">= 4.1")));
    }

    #[test]
    fn merge_description_linkingto_lands_in_both_tables() {
        let mut m = Rproj::minimal("mypkg");
        m.dependencies.clear();
        let pkg = package(vec![spec(
            "Rcpp",
            &[RDepType::Imports, RDepType::LinkingTo],
            vec![constraint(VersionConstraintType::GreaterOrEqual, "1.0")],
        )]);
        m.merge_description(&pkg);
        assert_eq!(m.dependencies.get("Rcpp"), Some(&dep(">= 1.0")));
        assert_eq!(m.linking_dependencies.get("Rcpp"), Some(&dep(">= 1.0")));
    }

    #[test]
    fn merge_description_suggests_and_enhances_go_to_groups() {
        let mut m = Rproj::minimal("mypkg");
        m.dependencies.clear();
        let pkg = package(vec![
            spec("testthat", &[RDepType::Suggests], vec![]),
            spec("otherpkg", &[RDepType::Enhances], vec![]),
        ]);
        m.merge_description(&pkg);
        assert_eq!(
            m.dependency_groups
                .get("test")
                .unwrap()
                .dependencies
                .get("testthat"),
            Some(&dep("*"))
        );
        assert_eq!(
            m.dependency_groups
                .get("enhances")
                .unwrap()
                .dependencies
                .get("otherpkg"),
            Some(&dep("*"))
        );
        assert!(m.dependencies.is_empty());
    }

    #[test]
    fn merge_description_upserts_existing_entry() {
        let mut m = Rproj::minimal("mypkg");
        m.dependencies.insert("cli".to_string(), dep(">= 1.0"));
        let pkg = package(vec![spec(
            "cli",
            &[RDepType::Imports],
            vec![constraint(VersionConstraintType::GreaterOrEqual, "3.6.5")],
        )]);
        m.merge_description(&pkg);
        assert_eq!(m.dependencies.get("cli"), Some(&dep(">= 3.6.5")));
    }

    /// The `(types, constraint strings)` of a converted manifest's dependency,
    /// or `None` if the package is not in the solver's dependency list.
    fn converted<'a>(
        deps: &'a PackageDependencies,
        name: &str,
    ) -> Option<(&'a [RDepType], Vec<String>)> {
        deps.dependencies.iter().find(|d| d.name == name).map(|d| {
            (
                d.types.as_slice(),
                d.constraints
                    .iter()
                    .map(|c| format!("{} {}", c.constraint_type, c.version))
                    .collect(),
            )
        })
    }

    #[test]
    fn to_dep_version_specs_plain_entry_is_an_import() {
        let mut m = Rproj::minimal("mypkg");
        m.dependencies.insert("cli".to_string(), dep(">= 3.6.5"));
        let deps = m.to_dep_version_specs(false).unwrap();
        assert_eq!(
            converted(&deps, "cli"),
            Some((&[RDepType::Imports][..], vec![">= 3.6.5".to_string()]))
        );
    }

    #[test]
    fn to_dep_version_specs_attach_is_a_depends() {
        let mut m = Rproj::minimal("mypkg");
        m.dependencies.insert(
            "crayon".to_string(),
            Dependency::Detailed(DepTable {
                version: Some("*".to_string()),
                attach: Some(true),
                ..Default::default()
            }),
        );
        let deps = m.to_dep_version_specs(false).unwrap();
        assert_eq!(
            converted(&deps, "crayon"),
            Some((&[RDepType::Depends][..], vec![]))
        );
    }

    #[test]
    fn to_dep_version_specs_r_is_a_depends_with_its_constraint() {
        let deps = Rproj::minimal("mypkg").to_dep_version_specs(false).unwrap();
        assert_eq!(
            converted(&deps, "R"),
            Some((&[RDepType::Depends][..], vec![">= 4.1".to_string()]))
        );
    }

    #[test]
    fn to_dep_version_specs_versionless_source_has_no_constraints() {
        let mut m = Rproj::minimal("mypkg");
        m.dependencies.insert(
            "ts".to_string(),
            Dependency::Detailed(DepTable {
                git: Some("https://github.com/gaborcsardi/ts".to_string()),
                ..Default::default()
            }),
        );
        let deps = m.to_dep_version_specs(false).unwrap();
        assert_eq!(
            converted(&deps, "ts"),
            Some((&[RDepType::Imports][..], vec![]))
        );
    }

    #[test]
    fn to_dep_version_specs_merges_linkingto_into_one_entry() {
        let mut m = Rproj::minimal("mypkg");
        m.dependencies.insert("Rcpp".to_string(), dep(">= 1.0"));
        m.linking_dependencies
            .insert("Rcpp".to_string(), dep(">= 1.0"));
        let deps = m.to_dep_version_specs(false).unwrap();
        assert_eq!(
            converted(&deps, "Rcpp"),
            Some((
                &[RDepType::Imports, RDepType::LinkingTo][..],
                vec![">= 1.0".to_string()]
            ))
        );
    }

    #[test]
    fn to_dep_version_specs_multiple_constraints_split_on_comma() {
        let mut m = Rproj::minimal("mypkg");
        m.dependencies
            .insert("cli".to_string(), dep(">= 1.0, << 2.0"));
        let deps = m.to_dep_version_specs(false).unwrap();
        assert_eq!(
            converted(&deps, "cli"),
            Some((
                &[RDepType::Imports][..],
                vec![">= 1.0".to_string(), "<< 2.0".to_string()]
            ))
        );
    }

    #[test]
    fn to_dep_version_specs_groups_are_soft_and_need_dev() {
        let mut m = Rproj::minimal("mypkg");
        m.dependency_groups.insert(
            "test".to_string(),
            Group {
                include_groups: vec![],
                dependencies: BTreeMap::from([("testthat".to_string(), dep(">= 3.0"))]),
            },
        );
        m.dependency_groups.insert(
            "enhances".to_string(),
            Group {
                include_groups: vec![],
                dependencies: BTreeMap::from([("otherpkg".to_string(), dep("*"))]),
            },
        );
        // an unknown group has no DESCRIPTION dependency type, and is left out
        m.dependency_groups.insert(
            "docs".to_string(),
            Group {
                include_groups: vec![],
                dependencies: BTreeMap::from([("pkgdown".to_string(), dep("*"))]),
            },
        );

        let deps = m.to_dep_version_specs(false).unwrap();
        assert_eq!(converted(&deps, "testthat"), None);
        assert_eq!(converted(&deps, "otherpkg"), None);
        assert_eq!(converted(&deps, "pkgdown"), None);

        let deps = m.to_dep_version_specs(true).unwrap();
        assert_eq!(
            converted(&deps, "testthat"),
            Some((&[RDepType::Suggests][..], vec![">= 3.0".to_string()]))
        );
        assert_eq!(
            converted(&deps, "otherpkg"),
            Some((&[RDepType::Enhances][..], vec![]))
        );
        assert_eq!(converted(&deps, "pkgdown"), None);
    }

    #[test]
    fn to_dep_version_specs_keeps_a_soft_dep_that_is_also_hard() {
        let mut m = Rproj::minimal("mypkg");
        m.dependencies.insert("cli".to_string(), dep(">= 3.6.5"));
        m.dependency_groups.insert(
            "test".to_string(),
            Group {
                include_groups: vec![],
                dependencies: BTreeMap::from([("cli".to_string(), dep("*"))]),
            },
        );
        let deps = m.to_dep_version_specs(false).unwrap();
        assert_eq!(
            converted(&deps, "cli"),
            Some((
                &[RDepType::Imports, RDepType::Suggests][..],
                vec![">= 3.6.5".to_string()]
            ))
        );
    }

    #[test]
    fn to_dep_version_specs_round_trips_a_description() {
        let mut m = Rproj::minimal("mypkg");
        m.dependencies.clear();
        let pkg = package(vec![
            spec(
                "R",
                &[RDepType::Depends],
                vec![constraint(VersionConstraintType::GreaterOrEqual, "4.1")],
            ),
            spec("crayon", &[RDepType::Depends], vec![]),
            spec(
                "cli",
                &[RDepType::Imports],
                vec![constraint(VersionConstraintType::GreaterOrEqual, "3.6.5")],
            ),
            spec("Rcpp", &[RDepType::Imports, RDepType::LinkingTo], vec![]),
            spec("testthat", &[RDepType::Suggests], vec![]),
            spec("otherpkg", &[RDepType::Enhances], vec![]),
        ]);
        m.merge_description(&pkg);

        let deps = m.to_dep_version_specs(true).unwrap();
        let mut expected = pkg.dependencies.dependencies.clone();
        expected.sort_by(|a, b| a.name.cmp(&b.name));
        let mut got = deps.dependencies.clone();
        got.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(got, expected);
    }

    #[test]
    fn merge_description_multiple_constraints_join_with_comma() {
        let mut m = Rproj::minimal("mypkg");
        m.dependencies.clear();
        let pkg = package(vec![spec(
            "cli",
            &[RDepType::Imports],
            vec![
                constraint(VersionConstraintType::GreaterOrEqual, "1.0"),
                constraint(VersionConstraintType::Less, "2.0"),
            ],
        )]);
        m.merge_description(&pkg);
        assert_eq!(m.dependencies.get("cli"), Some(&dep(">= 1.0, << 2.0")));
    }
}
