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
// `rig proj lock` solves one target per `(R version, platform)` given with
// `--r-version`/`--platform` (repeatable, combined as a cross product), and
// `rig proj sync` picks the one target whose OS matches this machine, using
// the highest R version among the matches if there is more than one -- a
// foreign-OS target is simply inert on this machine, which is what makes
// locking for a Linux deployment target from a macOS laptop work.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::Write as _;

use log::warn;
use serde::{Deserialize, Serialize};
use simple_error::*;

use crate::dcf::{
    DepVersionSpec, Package as DcfPackage, PackageDependencies, RDepType, RPackageVersion,
    VersionConstraint, VersionConstraintType, DEP_TYPES_SOFT,
};
use crate::pak::PakLockfilePackage;
use crate::repos::cranlike_metadata::minor_r_version;

pub const RPROJ_LOCK_VERSION: usize = 1;

// `rproj.toml`: the project/package manifest (see the design doc). This is the
// *requirements* file a human edits, as opposed to `rproj.lock` (the solved
// output above). rig owns the schema; a `DESCRIPTION` can be generated from it
// (follow-up work). For now the model round-trips through TOML and backs
// `rig proj init`.
pub const RPROJ_MANIFEST_FILE: &str = "rproj.toml";

/// The dependency groups that map onto a `DESCRIPTION` dependency field
/// instead of onto a `Config/Needs/*` field: `test` is `Suggests` and
/// `enhances` is `Enhances` (see [`Rproj::merge_description`]). Every other
/// group is a `Config/Needs/<group>` field (see
/// [`Rproj::merge_config_needs`]).
const DESCRIPTION_DEP_GROUPS: [&str; 2] = ["test", "enhances"];

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

impl Author {
    /// Best-effort parser for an `Authors@R` field's R source, e.g.
    /// `c(person("Jane", "Doe", email = "jane@x.com", role = c("aut", "cre")))`.
    /// Returns authors in the order their `person(...)` calls appear in `raw`.
    /// A `person(...)` call that cannot be parsed is skipped (logged), not a
    /// hard error: this is not a real R parser, just a scanner for the
    /// common cases.
    pub fn from_authors_r(raw: &str) -> Vec<Author> {
        extract_calls(raw, "person")
            .iter()
            .filter_map(|args| match parse_person_call(args) {
                Ok(author) => Some(author),
                Err(err) => {
                    warn!("Skipping unparseable Authors@R person(): {}", err);
                    None
                }
            })
            .collect()
    }

    /// Build one `person(...)` call for `Authors@R`, the inverse of
    /// [`Author::from_authors_r`]'s per-call parsing. `name` is written as
    /// the sole positional argument (`given`, with no `family`), since
    /// [`Author`] only stores one combined name; that round-trips through
    /// [`parse_person_call`], which treats a lone positional string as
    /// `given` alone.
    pub fn to_person_r(&self) -> String {
        let mut parts = vec![format!("\"{}\"", escape_r_string(&self.name))];
        if let Some(email) = &self.email {
            parts.push(format!("email = \"{}\"", escape_r_string(email)));
        }
        if !self.roles.is_empty() {
            let roles = self
                .roles
                .iter()
                .map(|r| format!("\"{}\"", escape_r_string(r)))
                .collect::<Vec<_>>()
                .join(", ");
            parts.push(format!("role = c({})", roles));
        }
        let mut comment = vec![];
        if let Some(orcid) = &self.orcid {
            comment.push(format!("ORCID = \"{}\"", escape_r_string(orcid)));
        }
        if let Some(ror) = &self.ror {
            comment.push(format!("ROR = \"{}\"", escape_r_string(ror)));
        }
        if !comment.is_empty() {
            parts.push(format!("comment = c({})", comment.join(", ")));
        }
        format!("person({})", parts.join(", "))
    }

    /// Fallback when `Authors@R` is absent: parses DESCRIPTION's
    /// `Maintainer: Name <email>` into a single author with role `cre`.
    pub fn from_maintainer(raw: &str) -> Option<Author> {
        let raw = raw.trim();
        let (name, email) = match (raw.find('<'), raw.find('>')) {
            (Some(open), Some(close)) if open < close => (
                raw[..open].trim().to_string(),
                Some(raw[open + 1..close].trim().to_string()),
            ),
            _ => (raw.to_string(), None),
        };
        if name.is_empty() {
            return None;
        }
        Some(Author {
            name,
            email,
            roles: vec!["cre".to_string()],
            orcid: None,
            ror: None,
        })
    }
}

/// Find every top-level call `name(...)` in `src` and return each call's
/// argument text (the content between the outer parens), in the order they
/// appear. Quote-aware, so a `(` or `)` inside a string literal does not
/// confuse the paren matching.
fn extract_calls(src: &str, name: &str) -> Vec<String> {
    let chars: Vec<char> = src.chars().collect();
    let needle: Vec<char> = name.chars().collect();
    let mut calls = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let is_match =
            chars[i..].starts_with(&needle[..]) && chars.get(i + needle.len()) == Some(&'(');
        // Skip a suffix match, e.g. "myperson(" should not match "person(".
        let prev_is_ident =
            i > 0 && matches!(chars[i - 1], c if c.is_alphanumeric() || c == '_' || c == '.');
        if is_match && !prev_is_ident {
            let open = i + needle.len();
            if let Some(close) = matching_paren(&chars, open) {
                calls.push(chars[open + 1..close].iter().collect());
                i = close + 1;
                continue;
            }
        }
        i += 1;
    }
    calls
}

/// Index of the `)` matching the `(` at `open`, quote-aware.
fn matching_paren(chars: &[char], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_string: Option<char> = None;
    let mut i = open;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = in_string {
            if c == '\\' {
                i += 1;
            } else if c == q {
                in_string = None;
            }
        } else {
            match c {
                '"' | '\'' => in_string = Some(c),
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// Split `args` on top-level commas (quote/paren/bracket-aware).
fn split_top_level_commas(args: &str) -> Vec<String> {
    let chars: Vec<char> = args.chars().collect();
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut in_string: Option<char> = None;
    let mut start = 0;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = in_string {
            if c == '\\' {
                i += 1;
            } else if c == q {
                in_string = None;
            }
        } else {
            match c {
                '"' | '\'' => in_string = Some(c),
                '(' | '[' => depth += 1,
                ')' | ']' => depth -= 1,
                ',' if depth == 0 => {
                    parts.push(chars[start..i].iter().collect::<String>());
                    start = i + 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
    parts.push(chars[start..].iter().collect::<String>());
    parts
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Split `s` on the first top-level `key = value` assignment (not `==`, and
/// not inside a string literal). Returns `None` if there isn't one.
fn split_top_level_eq(s: &str) -> Option<(String, String)> {
    let chars: Vec<char> = s.chars().collect();
    let mut in_string: Option<char> = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = in_string {
            if c == '\\' {
                i += 1;
            } else if c == q {
                in_string = None;
            }
        } else if c == '"' || c == '\'' {
            in_string = Some(c);
        } else if c == '=' {
            let next_is_eq = chars.get(i + 1) == Some(&'=');
            let prev_is_cmp = i > 0 && matches!(chars[i - 1], '!' | '<' | '>' | '=');
            if !next_is_eq && !prev_is_cmp {
                return Some((chars[..i].iter().collect(), chars[i + 1..].iter().collect()));
            }
        }
        i += 1;
    }
    None
}

/// Unquote a simple R string literal (`"foo"` or `'foo'`), or `None` if `s`
/// (after trimming) isn't one.
fn unquote(s: &str) -> Option<String> {
    let s = s.trim();
    let bytes = s.as_bytes();
    if bytes.len() < 2 {
        return None;
    }
    let q = bytes[0] as char;
    if (q == '"' || q == '\'') && bytes[bytes.len() - 1] as char == q {
        let inner = &s[1..s.len() - 1];
        return Some(
            inner
                .replace(&format!("\\{}", q), &q.to_string())
                .replace("\\\\", "\\"),
        );
    }
    None
}

/// Parse a `role = ...` value: a single quoted string, or `c("aut", "cre")`.
fn parse_string_list(value: &str) -> Vec<String> {
    let value = value.trim();
    if let Some(inner) = value.strip_prefix("c(").and_then(|v| v.strip_suffix(')')) {
        split_top_level_commas(inner)
            .iter()
            .filter_map(|p| unquote(p))
            .collect()
    } else if let Some(s) = unquote(value) {
        vec![s]
    } else {
        vec![]
    }
}

/// Parse a `comment = ...` value for `ORCID`/`ROR`: `c(ORCID = "...")`, or a
/// bare `ORCID = "..."`. Any other key or a plain string comment is ignored.
fn parse_comment(value: &str) -> (Option<String>, Option<String>) {
    let value = value.trim();
    let inner = value
        .strip_prefix("c(")
        .and_then(|v| v.strip_suffix(')'))
        .unwrap_or(value);
    let mut orcid = None;
    let mut ror = None;
    for part in split_top_level_commas(inner) {
        if let Some((key, val)) = split_top_level_eq(&part) {
            if let Some(val) = unquote(&val) {
                match key.trim().to_uppercase().as_str() {
                    "ORCID" => orcid = Some(val),
                    "ROR" => ror = Some(val),
                    _ => {}
                }
            }
        }
    }
    (orcid, ror)
}

/// Parse one `person(...)` call's argument text into an [`Author`].
/// Positional args fill `given`, `family` in that order (R's `person()`
/// signature also has `middle`/`email`/`role`/`comment` positions, but named
/// arguments are the overwhelmingly common style for anything past the
/// name, so only the first two positions are treated positionally here).
fn parse_person_call(args: &str) -> Result<Author, String> {
    let mut given: Option<String> = None;
    let mut family: Option<String> = None;
    let mut email: Option<String> = None;
    let mut roles: Vec<String> = vec![];
    let mut orcid: Option<String> = None;
    let mut ror: Option<String> = None;
    let mut positional: Vec<String> = vec![];

    for part in split_top_level_commas(args) {
        if let Some(s) = unquote(&part) {
            positional.push(s);
            continue;
        }
        if let Some((key, value)) = split_top_level_eq(&part) {
            match key.trim() {
                "given" | "first" => given = unquote(&value),
                "family" | "last" => family = unquote(&value),
                "email" => email = unquote(&value),
                "role" => roles = parse_string_list(&value),
                "comment" => {
                    let (o, r) = parse_comment(&value);
                    orcid = orcid.or(o);
                    ror = ror.or(r);
                }
                _ => {}
            }
        }
    }

    if given.is_none() && !positional.is_empty() {
        given = Some(positional[0].clone());
    }
    if family.is_none() && positional.len() >= 2 {
        family = Some(positional[1].clone());
    }

    let name = match (given, family) {
        (Some(g), Some(f)) => format!("{} {}", g, f).trim().to_string(),
        (Some(g), None) => g,
        (None, Some(f)) => f,
        (None, None) => return Err(format!("no name found in person({})", args)),
    };

    Ok(Author {
        name,
        email,
        roles,
        orcid,
        ror,
    })
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
    // The package reference, kept verbatim, in whatever syntax it was written
    // in (`tidyverse/tidytemplate`, `bioc::S4Vectors`, `url::https://...`).
    // Written by the `Config/Needs/*` import, which has no way to tell what
    // kind of reference an entry is, and written back out unchanged by
    // `rig proj export`.
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub ref_: Option<String>,
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

    /// Same as [`Rproj::minimal`], but with the R requirement taken from the
    /// R version the project is being created for.
    ///
    /// The requirement is `>= <major>.<minor>`, not the patch level: a
    /// project practically never means "at least this patch release", and
    /// `rproj.lock` records the exact version anyway.
    pub fn minimal_for_r(name: &str, r_version: &str) -> Result<Self, Box<dyn Error>> {
        let minor = minor_r_version(r_version)?;
        let mut manifest = Rproj::minimal(name);
        manifest.dependencies.insert(
            "R".to_string(),
            Dependency::Version(format!(">= {}", minor)),
        );
        Ok(manifest)
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

    /// Merge a DESCRIPTION's `Config/Needs/*` fields into this manifest's
    /// dependency groups: `Config/Needs/website` becomes
    /// `[dependency-groups.website]`. `needs` holds one `(group name, raw
    /// field value)` pair per field, the group name being what follows
    /// `Config/Needs/`.
    ///
    /// A field's value is a comma-separated list of package references, and
    /// unlike a `DESCRIPTION` dependency field it is not restricted to
    /// package names: `tidyverse/tidytemplate` and other `pak` reference
    /// syntaxes are common. An entry that is a plain package name (with an
    /// optional version constraint) becomes an ordinary version requirement;
    /// anything else is kept verbatim in [`DepTable::ref_`], so
    /// [`Rproj::to_description`] can write it back unchanged.
    ///
    /// A field with an empty value creates an empty group, so that it, too,
    /// round-trips.
    pub fn merge_config_needs(&mut self, needs: &[(String, String)]) {
        for (group_name, value) in needs.iter() {
            if DESCRIPTION_DEP_GROUPS.contains(&group_name.as_str()) {
                warn!(
                    "Config/Needs/{} is merged into the `{}` dependency group, \
                     which `rig proj export` writes as a DESCRIPTION dependency \
                     field, not as Config/Needs/{}",
                    group_name, group_name, group_name
                );
            }
            let group = self
                .dependency_groups
                .entry(group_name.clone())
                .or_default();
            for entry in value.split(',') {
                let entry = entry.trim();
                if entry.is_empty() {
                    continue;
                }
                let (name, dep) = config_needs_entry(entry);
                group.dependencies.insert(name, dep);
            }
        }
    }

    /// Add a dependency to the manifest, or update it if the manifest lists it
    /// already. `dev` puts it in the `test` dependency group (the group
    /// `rig proj import` imports `Suggests` into) instead of
    /// `[dependencies]`.
    ///
    /// Returns the version requirement the entry had before, if any, so the
    /// caller can tell "added" from "updated". An entry that names a source
    /// (`{ git = ... }`) or sets a flag (`attach = true`) keeps those, only
    /// its version requirement is replaced.
    pub fn add_dependency(&mut self, name: &str, version: &str, dev: bool) -> Option<String> {
        let table = if dev {
            &mut self
                .dependency_groups
                .entry("test".to_string())
                .or_default()
                .dependencies
        } else {
            &mut self.dependencies
        };

        let (previous, value) = match table.get(name) {
            Some(Dependency::Version(old)) => {
                (Some(old.clone()), Dependency::Version(version.to_string()))
            }
            Some(Dependency::Detailed(old)) => {
                let mut new = old.clone();
                new.version = Some(version.to_string());
                (
                    Some(old.version.clone().unwrap_or_else(|| "*".to_string())),
                    Dependency::Detailed(new),
                )
            }
            None => (None, Dependency::Version(version.to_string())),
        };

        table.insert(name.to_string(), value);
        previous
    }

    /// Whether the manifest lists a dependency by this name anywhere:
    /// `[dependencies]`, `[linking-dependencies]`, or any
    /// `[dependency-groups.*]` table.
    pub fn has_dependency(&self, name: &str) -> bool {
        self.dependencies.contains_key(name)
            || self.linking_dependencies.contains_key(name)
            || self
                .dependency_groups
                .values()
                .any(|group| group.dependencies.contains_key(name))
    }

    /// Remove a dependency from the manifest, wherever it is listed:
    /// `[dependencies]`, `[linking-dependencies]`, or any
    /// `[dependency-groups.*]` table.
    ///
    /// Returns the removed entry, if the name was found anywhere.
    pub fn remove_dependency(&mut self, name: &str) -> Option<Dependency> {
        if let Some(dep) = self.dependencies.remove(name) {
            return Some(dep);
        }
        if let Some(dep) = self.linking_dependencies.remove(name) {
            return Some(dep);
        }
        for group in self.dependency_groups.values_mut() {
            if let Some(dep) = group.dependencies.remove(name) {
                return Some(dep);
            }
        }
        None
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

    /// Render this manifest as a `DESCRIPTION` file, the inverse of
    /// [`Rproj::merge_description`] plus the `[project]` metadata mapping
    /// `rig proj import` reads (`Package`/`Version`/`Title`/`Description`/
    /// `License`/`Type`/`Authors@R`/`URL`/`BugReports`). Returns the text
    /// plus the names of any dependencies that had an upper version bound
    /// dropped (DESCRIPTION's `pkg (>= 1.2.3)` syntax has no room for a
    /// two-sided range, so only the lower bound, if any, survives).
    pub fn to_description(&self) -> Result<(String, Vec<String>), Box<dyn Error>> {
        let mut out = String::new();
        let mut dropped: Vec<String> = Vec::new();

        writeln!(out, "Package: {}", self.project.name)?;
        let type_ = self.project.type_.as_deref().unwrap_or("package");
        writeln!(out, "Type: {}", title_case(type_))?;
        if let Some(title) = &self.project.title {
            writeln!(out, "Title: {}", fold_dcf_prose(title, 76))?;
        }
        writeln!(out, "Version: {}", self.project.version)?;
        if !self.project.authors.is_empty() {
            let people = self
                .project
                .authors
                .iter()
                .map(Author::to_person_r)
                .collect::<Vec<_>>()
                .join(", ");
            let raw = if self.project.authors.len() == 1 {
                people
            } else {
                format!("c({})", people)
            };
            writeln!(out, "Authors@R: {}", fold_dcf_prose(&raw, 76))?;
        }
        if let Some(description) = &self.project.description {
            writeln!(out, "Description: {}", fold_dcf_prose(description, 76))?;
        }
        if let Some(license) = &self.project.license {
            writeln!(out, "License: {}", license)?;
        }
        let mut urls: Vec<&str> = vec![];
        if let Some(homepage) = self.project.urls.get("homepage") {
            urls.push(homepage);
        }
        if let Some(source) = self.project.urls.get("source") {
            urls.push(source);
        }
        if !urls.is_empty() {
            writeln!(out, "URL: {}", urls.join(", "))?;
        }
        if let Some(bugreports) = self.project.urls.get("bugreports") {
            writeln!(out, "BugReports: {}", bugreports)?;
        }

        let pkg_deps = self.to_dep_version_specs(true)?;
        for dep_type in RDepType::all() {
            let mut entries: Vec<&DepVersionSpec> = pkg_deps
                .dependencies
                .iter()
                .filter(|d| d.types.contains(dep_type))
                .collect();
            entries.sort_by(|a, b| a.name.cmp(&b.name));
            if entries.is_empty() {
                continue;
            }
            let items: Vec<String> = entries
                .iter()
                .map(|dep| {
                    let (item, was_dropped) = format_dep_entry(dep);
                    if was_dropped {
                        dropped.push(dep.name.clone());
                    }
                    item
                })
                .collect();
            writeln!(out, "{}: {}", dep_type, fold_dcf_list(&items, 76))?;
        }

        for (group_name, group) in self.dependency_groups.iter() {
            if DESCRIPTION_DEP_GROUPS.contains(&group_name.as_str()) {
                continue;
            }
            let mut items: Vec<String> = Vec::new();
            for (name, dep) in group.dependencies.iter() {
                let (item, was_dropped) = format_group_entry(name, dep)?;
                if was_dropped {
                    dropped.push(name.clone());
                }
                items.push(item);
            }
            if items.is_empty() {
                writeln!(out, "Config/Needs/{}:", group_name)?;
            } else {
                writeln!(
                    out,
                    "Config/Needs/{}: {}",
                    group_name,
                    fold_dcf_list(&items, 76)
                )?;
            }
        }

        Ok((out, dropped))
    }
}

/// Title-case a single word, e.g. `"package"` -> `"Package"`, for the
/// `Type:` field (`rproj.toml`'s `[project].type` is lowercase by
/// convention, DESCRIPTION's `Type:` is not).
fn title_case(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Escape a string for use inside an R string literal (`"..."`).
fn escape_r_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Format one dependency as a DCF entry, e.g. `"dplyr"` or
/// `"dplyr (>= 1.1.0)"`. [`parse_constraints`]/[`expand_version_req`] always
/// build the lower bound first, so `constraints[0]` is the lower bound
/// whenever there is one; any further constraint (an upper bound) has no
/// place in DESCRIPTION's single-comparison syntax and is dropped, which the
/// second return value flags.
fn format_dep_entry(dep: &DepVersionSpec) -> (String, bool) {
    match dep.constraints.first() {
        None => (dep.name.clone(), false),
        Some(c) => (
            format!("{} ({} {})", dep.name, c.constraint_type, c.version),
            dep.constraints.len() > 1,
        ),
    }
}

/// Format one dependency-group entry as a `Config/Needs/*` entry. An entry
/// that kept its reference verbatim (see [`Rproj::merge_config_needs`]) is
/// written back as it came in; anything else goes through
/// [`format_dep_entry`], so it looks like a DESCRIPTION dependency entry.
fn format_group_entry(name: &str, dep: &Dependency) -> Result<(String, bool), Box<dyn Error>> {
    if let Dependency::Detailed(table) = dep {
        if let Some(ref_) = &table.ref_ {
            return Ok((ref_.clone(), false));
        }
    }
    let spec = dep_spec(name, dep, RDepType::Suggests)?;
    Ok(format_dep_entry(&spec))
}

/// One entry of a `Config/Needs/*` field as a dependency-group entry: the
/// package name to key it under, and the dependency itself. A plain package
/// name, with an optional version constraint, becomes a version requirement;
/// anything else is a package reference in one of `pak`'s syntaxes, kept
/// verbatim under the package name the reference implies.
fn config_needs_entry(entry: &str) -> (String, Dependency) {
    if let Ok(spec) = DepVersionSpec::parse(entry, "Suggests") {
        if is_r_package_name(&spec.name) {
            return (
                spec.name,
                Dependency::Version(format_constraints(&spec.constraints)),
            );
        }
    }

    let name = match pak_ref_name(entry) {
        Some(name) => name,
        None => {
            warn!(
                "Cannot tell which package `{}` refers to, keeping it as is",
                entry
            );
            entry.to_string()
        }
    };
    (
        name,
        Dependency::Detailed(DepTable {
            ref_: Some(entry.to_string()),
            ..Default::default()
        }),
    )
}

/// The package name a `pak` package reference implies, e.g. `tidytemplate`
/// for `tidyverse/tidytemplate@main`. `pak` reference syntax is
/// `[<name>=][<type>::]<ref>`, so an explicit name wins; otherwise the name
/// is the last path component of the reference, without its `@<tag>` /
/// `#<pull request>` suffix and without a file extension. `None` if that does
/// not leave a valid package name behind.
fn pak_ref_name(entry: &str) -> Option<String> {
    if let Some((name, _)) = entry.split_once('=') {
        let name = name.trim();
        if is_r_package_name(name) {
            return Some(name.to_string());
        }
    }

    let ref_ = match entry.split_once("::") {
        Some((_type, ref_)) => ref_,
        None => entry,
    };
    let ref_ = ref_.split(['@', '#']).next().unwrap_or(ref_);
    let last = ref_.trim_end_matches('/').rsplit('/').next()?;
    let last = last.split('.').next().unwrap_or(last);
    if is_r_package_name(last) {
        Some(last.to_string())
    } else {
        None
    }
}

/// Whether `name` is a valid R package name: a letter, then letters, digits
/// and dots.
fn is_r_package_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '.')
}

/// Greedily wrap comma-joined `items` (each already formatted, e.g.
/// `"dplyr (>= 1.1.0)"`) to at most `width` columns, folding only between
/// items (never inside one, since an item can itself contain a space), with
/// 4-space-indented continuation lines, DCF's folding convention.
fn fold_dcf_list(items: &[String], width: usize) -> String {
    let mut out = String::new();
    let mut col = 0usize;
    for (i, item) in items.iter().enumerate() {
        let piece = if i + 1 < items.len() {
            format!("{},", item)
        } else {
            item.clone()
        };
        if i == 0 {
            out.push_str(&piece);
            col = piece.len();
        } else if col + 1 + piece.len() <= width {
            out.push(' ');
            out.push_str(&piece);
            col += 1 + piece.len();
        } else {
            out.push_str("\n    ");
            out.push_str(&piece);
            col = 4 + piece.len();
        }
    }
    out
}

/// Wrap prose (`Title`/`Description`) to at most `width` columns, joining
/// continuation lines with DCF's 4-space indent.
fn fold_dcf_prose(value: &str, width: usize) -> String {
    crate::textfmt::wrap(value, width).join("\n    ")
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

/// Parse a `rig proj add` package specification into a package name and the
/// version requirement to write into the manifest.
///
/// The syntax is `<package>` or `<package>@<requirement>`: `dplyr`,
/// `dplyr@1.1.0`, `cli@>= 3.6`, `rlang@>= 1.0, < 2.0`. A specification with no
/// requirement means any version (`"*"`), and a bare version is normalized to
/// its explicit caret spelling, so `dplyr@1.1.0` becomes `^1.1.0` in the
/// manifest — the same requirement, but readable without knowing that a bare
/// version means caret.
///
/// The requirement is parsed to validate it, so an unusable one is rejected
/// here rather than written into the manifest.
pub fn parse_add_spec(spec: &str) -> Result<(String, String), Box<dyn Error>> {
    let spec = spec.trim();
    let (name, version) = match spec.split_once('@') {
        Some((name, version)) => (name.trim(), version.trim()),
        None => (spec, "*"),
    };

    if name.is_empty() {
        bail!("Invalid package `{}`: the package name is missing", spec);
    }
    if name.contains(|c: char| c.is_whitespace() || c == '(' || c == ')') {
        bail!(
            "Invalid package name `{}`, expected `<package>` or `<package>@<version>`, \
             e.g. `dplyr@>= 1.1.0`",
            name
        );
    }
    if version.is_empty() {
        bail!(
            "Invalid package `{}`: the version requirement after `@` is missing",
            spec
        );
    }

    let version = match version.strip_prefix('^') {
        Some(_) => version.to_string(),
        None if version.starts_with(|c: char| c.is_ascii_digit()) => format!("^{}", version),
        None => version.to_string(),
    };

    parse_constraints(&version).map_err(|err| {
        SimpleError::new(format!(
            "Invalid version requirement `{}` for package `{}`: {}",
            version, name, err
        ))
    })?;

    Ok((name.to_string(), version))
}

/// Parse an `rproj.toml` version string, e.g. `">= 1.0, < 2.0"`, into version
/// constraints. Comma means AND, `"*"` (or an empty string) means no
/// constraint, and each piece goes through [`expand_version_req`], so the
/// caret/tilde/bare forms are understood as well. The inverse of
/// [`format_constraints`] for the plain-operator forms.
pub fn parse_constraints(version: &str) -> Result<Vec<VersionConstraint>, Box<dyn Error>> {
    let version = version.trim();
    if version.is_empty() || version == "*" {
        return Ok(vec![]);
    }
    let mut constraints = Vec::new();
    for piece in version.split(',') {
        constraints.extend(expand_version_req(piece.trim())?);
    }
    Ok(constraints)
}

/// One version requirement as version constraints. A plain operator form
/// (`">= 1.0"`) is a single constraint; the caret, tilde and bare forms lower
/// onto a pair of them:
///
/// - `^1.2.3` (and the bare `1.2.3`, which means the same) is *compatible
///   with*: `>= 1.2.3, < 2.0.0`.
/// - `~1.2.3` allows the last component to move only: `>= 1.2.3, < 1.3.0`.
///
/// R versions are not semver — they can have any number of components
/// (`1.1`, `1.1.0.9000`) — so both forms are defined over the component
/// vector rather than over major/minor/patch. The upper bound bumps one
/// component and zeroes the ones after it: for a caret the leftmost non-zero
/// component (cargo's zero nuance: `^0.2.3` is `< 0.3.0`, `^0.0.3` is
/// `< 0.0.4`), or the last one if every component is zero; for a tilde the
/// second component, or the first if that is all there is.
fn expand_version_req(req: &str) -> Result<Vec<VersionConstraint>, Box<dyn Error>> {
    let (version_str, tilde) = match req.strip_prefix('^') {
        Some(rest) => (rest.trim(), false),
        None => match req.strip_prefix('~') {
            Some(rest) => (rest.trim(), true),
            // A bare version, i.e. one that starts with a digit, means the
            // same as the caret form. Anything else is an operator form.
            None if req.starts_with(|c: char| c.is_ascii_digit()) => (req, false),
            None => return Ok(vec![VersionConstraint::from_str(req)?]),
        },
    };

    let version = RPackageVersion::from_str(version_str)?;
    if version.components.is_empty() {
        bail!("Invalid version constraint: {}", req);
    }

    let bump = if tilde {
        // `~1.2.3` and `~1.2` bump the second component, `~1` the first.
        std::cmp::min(1, version.components.len() - 1)
    } else {
        // The leftmost non-zero component, or the last one if all are zero.
        version
            .components
            .iter()
            .position(|c| *c > 0)
            .unwrap_or(version.components.len() - 1)
    };

    let mut upper: Vec<u32> = version.components.clone();
    upper[bump] += 1;
    for c in upper.iter_mut().skip(bump + 1) {
        *c = 0;
    }

    Ok(vec![
        VersionConstraint {
            constraint_type: VersionConstraintType::GreaterOrEqual,
            version,
        },
        VersionConstraint {
            constraint_type: VersionConstraintType::Less,
            version: RPackageVersion {
                original: upper
                    .iter()
                    .map(|c| c.to_string())
                    .collect::<Vec<_>>()
                    .join("."),
                components: upper,
            },
        },
    ])
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
    fn minimal_manifest_records_the_projects_r_version() {
        // The patch level is dropped: a project practically never means "at
        // least this patch release".
        let m = Rproj::minimal_for_r("mypkg", "4.6.1").unwrap();
        assert_eq!(
            m.dependencies.get("R"),
            Some(&Dependency::Version(">= 4.6".to_string()))
        );
        // A two-part version is fine, too.
        let m2 = Rproj::minimal_for_r("mypkg", "4.6").unwrap();
        assert_eq!(m2, m);
        // and it round-trips
        let text = toml::to_string_pretty(&m).unwrap();
        assert_eq!(toml::from_str::<Rproj>(&text).unwrap(), m);

        assert!(Rproj::minimal_for_r("mypkg", "devel").is_err());
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
    fn roundtrips_several_targets_through_toml() {
        let mut linux_package = sample_package();
        linux_package.platform = "x86_64-pc-linux-gnu".to_string();
        linux_package.rversion = "4.5".to_string();

        let lock = RprojLock {
            version: RPROJ_LOCK_VERSION,
            targets: vec![
                RprojLockTarget {
                    r_version: "4.5".to_string(),
                    platform: "x86_64-pc-linux-gnu".to_string(),
                    packages: vec![linux_package],
                },
                RprojLockTarget {
                    r_version: "4.6".to_string(),
                    platform: "aarch64-apple-darwin".to_string(),
                    packages: vec![sample_package()],
                },
            ],
        };
        let text = toml::to_string_pretty(&lock).unwrap();
        let parsed: RprojLock = toml::from_str(&text).unwrap();
        assert_eq!(parsed.targets.len(), 2);
        assert_eq!(parsed.targets[0].r_version, "4.5");
        assert_eq!(parsed.targets[0].platform, "x86_64-pc-linux-gnu");
        assert_eq!(parsed.targets[1].r_version, "4.6");
        assert_eq!(parsed.targets[1].platform, "aarch64-apple-darwin");
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

    /// The constraints of a version requirement as `("op", "version")` pairs,
    /// which read more clearly in the expansion tests below than a
    /// `VersionConstraint` literal does.
    fn expanded(version: &str) -> Vec<(String, String)> {
        parse_constraints(version)
            .unwrap()
            .iter()
            .map(|c| (c.constraint_type.to_string(), c.version.to_string()))
            .collect()
    }

    fn range(lower: &str, upper: &str) -> Vec<(String, String)> {
        vec![
            (">=".to_string(), lower.to_string()),
            ("<<".to_string(), upper.to_string()),
        ]
    }

    #[test]
    fn a_caret_requirement_allows_the_leftmost_non_zero_component_to_stay() {
        assert_eq!(expanded("^1.2.3"), range("1.2.3", "2.0.0"));
        assert_eq!(expanded("^0.2.3"), range("0.2.3", "0.3.0"));
        assert_eq!(expanded("^0.0.3"), range("0.0.3", "0.0.4"));
        assert_eq!(expanded("^1.2"), range("1.2", "2.0"));
        assert_eq!(expanded("^1"), range("1", "2"));
        assert_eq!(expanded("^0"), range("0", "1"));
        assert_eq!(expanded("^0.0"), range("0.0", "0.1"));
        // R versions are not semver, they can have any number of components.
        assert_eq!(expanded("^1.1.0.9000"), range("1.1.0.9000", "2.0.0.0"));
    }

    #[test]
    fn a_bare_version_is_a_caret_requirement() {
        assert_eq!(expanded("1.2.3"), expanded("^1.2.3"));
        assert_eq!(expanded("0.0.3"), expanded("^0.0.3"));
    }

    #[test]
    fn a_tilde_requirement_allows_the_last_component_to_move() {
        assert_eq!(expanded("~1.2.3"), range("1.2.3", "1.3.0"));
        assert_eq!(expanded("~1.2"), range("1.2", "1.3"));
        assert_eq!(expanded("~1"), range("1", "2"));
        assert_eq!(expanded("~0.0.3"), range("0.0.3", "0.1.0"));
        assert_eq!(expanded("~1.1.0.9000"), range("1.1.0.9000", "1.2.0.0"));
    }

    #[test]
    fn the_operator_requirements_are_unchanged() {
        assert_eq!(
            expanded(">= 1.2"),
            vec![(">=".to_string(), "1.2".to_string())]
        );
        assert_eq!(
            expanded("= 1.2.3"),
            vec![("=".to_string(), "1.2.3".to_string())]
        );
        assert_eq!(expanded(">= 1.0, < 2.0"), range("1.0", "2.0"));
        assert_eq!(expanded("*"), vec![]);
        assert_eq!(expanded(""), vec![]);
    }

    #[test]
    fn an_unparseable_requirement_is_an_error() {
        assert!(parse_constraints("nope").is_err());
        assert!(parse_constraints("^nope").is_err());
        assert!(parse_constraints("^").is_err());
        assert!(parse_constraints(">= 1.0, nope").is_err());
    }

    #[test]
    fn an_add_spec_without_a_version_means_any_version() {
        assert_eq!(
            parse_add_spec("dplyr").unwrap(),
            ("dplyr".to_string(), "*".to_string())
        );
    }

    #[test]
    fn an_add_specs_bare_version_becomes_an_explicit_caret() {
        assert_eq!(
            parse_add_spec("dplyr@1.1.0").unwrap(),
            ("dplyr".to_string(), "^1.1.0".to_string())
        );
        assert_eq!(
            parse_add_spec("dplyr@^1.1.0").unwrap(),
            ("dplyr".to_string(), "^1.1.0".to_string())
        );
    }

    #[test]
    fn an_add_specs_other_versions_are_kept_as_they_are() {
        assert_eq!(
            parse_add_spec("cli@>= 3.6").unwrap(),
            ("cli".to_string(), ">= 3.6".to_string())
        );
        assert_eq!(
            parse_add_spec(" rlang@>= 1.0, < 2.0 ").unwrap(),
            ("rlang".to_string(), ">= 1.0, < 2.0".to_string())
        );
        assert_eq!(
            parse_add_spec("tidyr@~1.3.0").unwrap(),
            ("tidyr".to_string(), "~1.3.0".to_string())
        );
    }

    #[test]
    fn an_invalid_add_spec_is_an_error() {
        // No version after the `@`, no package name, and a version
        // requirement that does not parse.
        assert!(parse_add_spec("dplyr@").is_err());
        assert!(parse_add_spec("@1.0").is_err());
        assert!(parse_add_spec("").is_err());
        assert!(parse_add_spec("dplyr@nope").is_err());
        assert!(parse_add_spec("dplyr (>= 1.0)").is_err());
    }

    #[test]
    fn add_dependency_adds_a_new_dependency() {
        let mut m = Rproj::minimal("mypkg");
        assert_eq!(m.add_dependency("dplyr", "^1.1.0", false), None);
        assert_eq!(m.dependencies.get("dplyr"), Some(&dep("^1.1.0")));
        assert!(m.dependency_groups.is_empty());
    }

    #[test]
    fn add_dependency_dev_adds_to_the_test_group() {
        let mut m = Rproj::minimal("mypkg");
        assert_eq!(m.add_dependency("testthat", ">= 3.0", true), None);
        assert!(!m.dependencies.contains_key("testthat"));
        assert_eq!(
            m.dependency_groups
                .get("test")
                .unwrap()
                .dependencies
                .get("testthat"),
            Some(&dep(">= 3.0"))
        );
    }

    #[test]
    fn add_dependency_returns_the_previous_requirement() {
        let mut m = Rproj::minimal("mypkg");
        m.add_dependency("dplyr", "^1.0.0", false);
        assert_eq!(
            m.add_dependency("dplyr", "^1.1.0", false),
            Some("^1.0.0".to_string())
        );
        assert_eq!(m.dependencies.get("dplyr"), Some(&dep("^1.1.0")));
    }

    #[test]
    fn add_dependency_keeps_an_existing_entrys_other_fields() {
        let mut m = Rproj::minimal("mypkg");
        m.dependencies.insert(
            "ts".to_string(),
            Dependency::Detailed(DepTable {
                git: Some("https://github.com/gaborcsardi/ts".to_string()),
                attach: Some(true),
                ..Default::default()
            }),
        );
        // The entry had no version requirement, so the previous one reads as
        // "any version".
        assert_eq!(
            m.add_dependency("ts", ">= 1.0", false),
            Some("*".to_string())
        );
        assert_eq!(
            m.dependencies.get("ts"),
            Some(&Dependency::Detailed(DepTable {
                version: Some(">= 1.0".to_string()),
                git: Some("https://github.com/gaborcsardi/ts".to_string()),
                attach: Some(true),
                ..Default::default()
            }))
        );
    }

    #[test]
    fn remove_dependency_removes_a_hard_dependency() {
        let mut m = Rproj::minimal("mypkg");
        m.add_dependency("dplyr", "^1.1.0", false);
        assert_eq!(m.remove_dependency("dplyr"), Some(dep("^1.1.0")));
        assert!(!m.dependencies.contains_key("dplyr"));
    }

    #[test]
    fn remove_dependency_removes_a_dependency_group_entry() {
        let mut m = Rproj::minimal("mypkg");
        m.add_dependency("testthat", ">= 3.0", true);
        assert_eq!(m.remove_dependency("testthat"), Some(dep(">= 3.0")));
        assert!(!m
            .dependency_groups
            .get("test")
            .unwrap()
            .dependencies
            .contains_key("testthat"));
    }

    #[test]
    fn remove_dependency_of_a_name_not_listed_anywhere_is_none() {
        let mut m = Rproj::minimal("mypkg");
        assert_eq!(m.remove_dependency("nosuchpkg"), None);
    }

    #[test]
    fn a_dev_dependency_is_soft_and_needs_dev_to_be_solved() {
        let mut m = Rproj::minimal("mypkg");
        m.add_dependency("testthat", ">= 3.0", true);

        let dev = m.to_dep_version_specs(true).unwrap();
        assert_eq!(
            dev.dependencies
                .iter()
                .find(|d| d.name == "testthat")
                .map(|d| d.types.clone()),
            Some(vec![RDepType::Suggests])
        );

        let nodev = m.to_dep_version_specs(false).unwrap();
        assert!(!nodev.dependencies.iter().any(|d| d.name == "testthat"));
    }

    #[test]
    fn authors_r_parses_a_single_person() {
        let authors = Author::from_authors_r(
            "person(\"Jane\", \"Doe\", email = \"jane@x.com\", role = c(\"aut\", \"cre\"))",
        );
        assert_eq!(
            authors,
            vec![Author {
                name: "Jane Doe".to_string(),
                email: Some("jane@x.com".to_string()),
                roles: vec!["aut".to_string(), "cre".to_string()],
                orcid: None,
                ror: None,
            }]
        );
    }

    #[test]
    fn authors_r_parses_multiple_people_in_order() {
        let authors = Author::from_authors_r(
            "c(\n  person(\"Jane\", \"Doe\", role = c(\"aut\", \"cre\")),\n  \
             person(\"John\", \"Smith\", role = \"ctb\")\n)",
        );
        assert_eq!(authors.len(), 2);
        assert_eq!(authors[0].name, "Jane Doe");
        assert_eq!(authors[1].name, "John Smith");
        assert_eq!(authors[1].roles, vec!["ctb".to_string()]);
    }

    #[test]
    fn authors_r_parses_orcid_from_comment() {
        let authors = Author::from_authors_r(
            "person(\"Jane\", \"Doe\", role = \"aut\", \
             comment = c(ORCID = \"0000-0001-7098-9676\"))",
        );
        assert_eq!(authors[0].orcid, Some("0000-0001-7098-9676".to_string()));
    }

    #[test]
    fn authors_r_skips_a_call_with_no_name() {
        // Only named args, none of which give a name: skipped, not an error.
        let authors = Author::from_authors_r("person(role = \"aut\")");
        assert!(authors.is_empty());
    }

    #[test]
    fn maintainer_parses_name_and_email() {
        let author = Author::from_maintainer("Jane Doe <jane@x.com>").unwrap();
        assert_eq!(author.name, "Jane Doe");
        assert_eq!(author.email, Some("jane@x.com".to_string()));
        assert_eq!(author.roles, vec!["cre".to_string()]);
    }

    #[test]
    fn maintainer_without_email_still_parses() {
        let author = Author::from_maintainer("Jane Doe").unwrap();
        assert_eq!(author.name, "Jane Doe");
        assert_eq!(author.email, None);
    }

    #[test]
    fn to_description_renders_metadata_and_deps_and_reports_dropped_bounds() {
        let mut m = Rproj::minimal("mypkg");
        m.project.title = Some("My Package".to_string());
        m.project.description = Some("Does things.".to_string());
        m.project.license = Some("MIT".to_string());
        m.project.authors.push(Author {
            name: "Jane Doe".to_string(),
            email: Some("jane@x.com".to_string()),
            roles: vec!["aut".to_string(), "cre".to_string()],
            orcid: None,
            ror: None,
        });
        m.project
            .urls
            .insert("homepage".to_string(), "https://x.example/pkg".to_string());
        m.project.urls.insert(
            "bugreports".to_string(),
            "https://x.example/pkg/issues".to_string(),
        );
        // Two-sided range: the upper bound should be dropped and reported.
        m.dependencies.insert(
            "dplyr".to_string(),
            Dependency::Version(">= 1.1.0, < 2.0.0".to_string()),
        );
        // Single-sided: kept as-is.
        m.dependencies.insert(
            "rlang".to_string(),
            Dependency::Version(">= 1.0".to_string()),
        );
        m.add_dependency("testthat", ">= 3.0", true);

        let (desc, dropped) = m.to_description().unwrap();
        assert_eq!(dropped, vec!["dplyr".to_string()]);
        assert!(desc.contains("Package: mypkg\n"));
        assert!(desc.contains("Type: Project\n"));
        assert!(desc.contains("Title: My Package\n"));
        assert!(desc.contains(
            "Authors@R: person(\"Jane Doe\", email = \"jane@x.com\", role = c(\"aut\", \"cre\"))\n"
        ));
        assert!(desc.contains("Description: Does things.\n"));
        assert!(desc.contains("License: MIT\n"));
        assert!(desc.contains("URL: https://x.example/pkg\n"));
        assert!(desc.contains("BugReports: https://x.example/pkg/issues\n"));
        assert!(desc.contains("Depends: R (>= 4.1)\n"));
        assert!(desc.contains("Imports: dplyr (>= 1.1.0), rlang (>= 1.0)\n"));
        assert!(desc.contains("Suggests: testthat (>= 3.0)\n"));
    }

    #[test]
    fn to_description_omits_absent_optional_fields() {
        let m = Rproj::minimal("bare");
        let (desc, dropped) = m.to_description().unwrap();
        assert!(dropped.is_empty());
        assert!(!desc.contains("Title:"));
        assert!(!desc.contains("Authors@R:"));
        assert!(!desc.contains("Description:"));
        assert!(!desc.contains("License:"));
        assert!(!desc.contains("URL:"));
        assert!(!desc.contains("BugReports:"));
        assert!(desc.contains("Depends: R (>= 4.1)\n"));
    }

    fn needs(fields: &[(&str, &str)]) -> Vec<(String, String)> {
        fields
            .iter()
            .map(|(g, v)| (g.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn merge_config_needs_creates_a_group_per_field() {
        let mut m = Rproj::minimal("mypkg");
        m.merge_config_needs(&needs(&[
            ("website", "pkgdown, tidyverse/tidytemplate"),
            ("coverage", "covr (>= 3.6)"),
        ]));

        let website = &m.dependency_groups.get("website").unwrap().dependencies;
        assert_eq!(website.get("pkgdown"), Some(&dep("*")));
        assert_eq!(
            website.get("tidytemplate"),
            Some(&Dependency::Detailed(DepTable {
                ref_: Some("tidyverse/tidytemplate".to_string()),
                ..Default::default()
            }))
        );
        assert_eq!(
            m.dependency_groups
                .get("coverage")
                .unwrap()
                .dependencies
                .get("covr"),
            Some(&dep(">= 3.6"))
        );
    }

    #[test]
    fn merge_config_needs_keeps_an_empty_field_as_an_empty_group() {
        let mut m = Rproj::minimal("mypkg");
        m.merge_config_needs(&needs(&[("website", "")]));
        assert!(m
            .dependency_groups
            .get("website")
            .unwrap()
            .dependencies
            .is_empty());

        let (desc, _) = m.to_description().unwrap();
        assert!(desc.contains("Config/Needs/website:\n"));
    }

    #[test]
    fn merge_config_needs_merges_into_the_description_backed_groups() {
        // `Config/Needs/test` has nowhere else to go, so it lands in the
        // group `Suggests` is imported into, and exports as `Suggests`.
        let mut m = Rproj::minimal("mypkg");
        m.add_dependency("testthat", ">= 3.0", true);
        m.merge_config_needs(&needs(&[("test", "mockery")]));

        let test = &m.dependency_groups.get("test").unwrap().dependencies;
        assert_eq!(test.get("testthat"), Some(&dep(">= 3.0")));
        assert_eq!(test.get("mockery"), Some(&dep("*")));

        let (desc, _) = m.to_description().unwrap();
        assert!(desc.contains("Suggests: mockery, testthat (>= 3.0)\n"));
        assert!(!desc.contains("Config/Needs/"));
    }

    #[test]
    fn to_description_writes_the_other_groups_as_config_needs() {
        let mut m = Rproj::minimal("mypkg");
        m.add_dependency("testthat", "*", true);
        m.merge_config_needs(&needs(&[
            ("website", "pkgdown, tidyverse/tidytemplate"),
            ("coverage", "covr"),
        ]));

        let (desc, dropped) = m.to_description().unwrap();
        assert!(dropped.is_empty());
        assert!(desc.contains("Suggests: testthat\n"));
        assert!(desc.contains("Config/Needs/coverage: covr\n"));
        assert!(desc.contains("Config/Needs/website: pkgdown, tidyverse/tidytemplate\n"));
        // Dependency fields come first, `Config/Needs/*` after them.
        assert!(desc.find("Suggests:").unwrap() < desc.find("Config/Needs/").unwrap());
    }

    #[test]
    fn config_needs_roundtrips_through_the_manifest() {
        let mut m = Rproj::minimal("mypkg");
        let field = "tidyverse/tidytemplate, pkgdown (>= 2.0), \
                     bioc::S4Vectors, jsonlite=jeroen/jsonlite@v1.8.0";
        m.merge_config_needs(&needs(&[("website", field)]));

        // The manifest survives a TOML round trip, `ref` and all.
        let text = toml::to_string_pretty(&m).unwrap();
        assert_eq!(toml::from_str::<Rproj>(&text).unwrap(), m);

        let (desc, _) = m.to_description().unwrap();
        // Entries are sorted by package name, and every reference is written
        // back exactly as it came in.
        assert!(desc.contains(
            "Config/Needs/website: bioc::S4Vectors, jsonlite=jeroen/jsonlite@v1.8.0, \
             pkgdown (>= 2.0),\n    tidyverse/tidytemplate\n"
        ));
    }

    #[test]
    fn config_needs_entry_names_the_package_a_reference_implies() {
        let cases = [
            ("tidyverse/tidytemplate", "tidytemplate"),
            ("tidyverse/tidytemplate@main", "tidytemplate"),
            ("r-lib/pak#123", "pak"),
            ("bioc::S4Vectors", "S4Vectors"),
            ("git::https://github.com/r-lib/cli.git", "cli"),
            ("jsonlite=jeroen/jsonlite", "jsonlite"),
            ("r-lib/usethis/subdir", "subdir"),
        ];
        for (entry, name) in cases {
            let (key, dep) = config_needs_entry(entry);
            assert_eq!(key, name, "{}", entry);
            assert_eq!(
                dep,
                Dependency::Detailed(DepTable {
                    ref_: Some(entry.to_string()),
                    ..Default::default()
                }),
                "{}",
                entry
            );
        }

        // A reference with no package name in it is kept under the reference
        // itself, rather than being dropped.
        let (key, _) = config_needs_entry("url::https://example.org/x?a=1");
        assert_eq!(key, "url::https://example.org/x?a=1");
    }
}
