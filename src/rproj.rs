// `rproj.lock`: the multi-target project lockfile written by `rig proj lock`
// and read by `rig proj sync`. TOML.
//
// `rproj.lock` is not interop with anything external; it is rig's own format,
// designed to hold the solve for *several* `(R version, platform)` targets in
// one file — e.g. solving once on a laptop for both macOS and a Linux
// deployment target. A target's package list is `RprojLockPackage` entries,
// which record only what installing needs: what the package is, whether it is
// a source or a binary build, where it is downloaded from, where it is cached,
// and the provenance an installed package's `DESCRIPTION` gets.
//
// `rig proj lock` solves one target per `(R version, platform)` given with
// `--r-version`/`--platform` (repeatable, combined as a cross product), and
// `rig proj sync` picks the one target whose OS matches this machine, using
// the highest R version among the matches if there is more than one -- a
// foreign-OS target is simply inert on this machine, which is what makes
// locking for a Linux deployment target from a macOS laptop work.

use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::fmt::Write as _;
use std::path::Path;

use log::warn;
use serde::{Deserialize, Serialize};
use simple_error::*;

use crate::cache::{artifact_cache_key, target_path};
use crate::dcf::{
    DepVersionSpec, Package as DcfPackage, PackageDependencies, RDepType, RPackageVersion,
    VersionConstraint, VersionConstraintType, DEP_TYPES_SOFT,
};
use crate::install::{
    format_linkingto, REMOTE_HASH_FIELD, REMOTE_HOST_FIELD, REMOTE_LINKINGTO_FIELD,
    REMOTE_REF_FIELD, REMOTE_REPO_FIELD, REMOTE_SHA_FIELD, REMOTE_SUBDIR_FIELD, REMOTE_TYPE_FIELD,
    REMOTE_URL_FIELD, REMOTE_USERNAME_FIELD,
};
use crate::proj::BASE_PKGS;
use crate::repos::cranlike_metadata::minor_r_version;
use crate::rvenv::RPROJ_LOCK_FILE;
use crate::solver::{RPackageRegistry, RegistryPackageVersion};

pub const RPROJ_LOCK_VERSION: usize = 4;

// `rproj.toml`: the project/package manifest (see the design doc). This is the
// *requirements* file a human edits, as opposed to `rproj.lock` (the solved
// output above). rig owns the schema; a `DESCRIPTION` can be generated from it
// (follow-up work). For now the model round-trips through TOML and backs
// `rig proj init`.
pub const RPROJ_MANIFEST_FILE: &str = "rproj.toml";

/// The DCF field rig writes into every generated `DESCRIPTION` (see
/// [`Rproj::to_description`]) to mark the file as rig-generated. `rig proj
/// export` uses its presence to overwrite such a file without `--force`.
pub const DESCRIPTION_RIG_NOTE_FIELD: &str = "Config/rig/note";

/// The dependency groups that map onto a `DESCRIPTION` dependency field
/// instead of onto a `Config/Needs/*` field: `dev` is `Suggests` and
/// `enhances` is `Enhances` (see [`Rproj::merge_description`]). Every other
/// group is a `Config/Needs/<group>` field (see
/// [`Rproj::merge_config_needs`]).
const DESCRIPTION_DEP_GROUPS: [&str; 2] = ["dev", "enhances"];

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

impl Project {
    /// Whether this project builds an installable R package, i.e. its `Type:`
    /// in DESCRIPTION would be "Package". An absent `type_` defaults to
    /// "package", same as [`Rproj::to_description`].
    pub fn is_package(&self) -> bool {
        self.type_
            .as_deref()
            .unwrap_or("package")
            .eq_ignore_ascii_case("package")
    }
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

/// Split `args` on top-level commas (quote/paren/bracket-aware), dropping
/// empty arguments. Use [`split_top_level_commas_keep_empty`] when the
/// position of each argument matters.
fn split_top_level_commas(args: &str) -> Vec<String> {
    split_top_level_commas_keep_empty(args)
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect()
}

/// Split `args` on top-level commas (quote/paren/bracket-aware), keeping
/// empty arguments. R allows an argument to be left out entirely, e.g. the
/// `middle` in `person("Jane", "Doe", , "jane@x.com")`, and such a hole still
/// counts when matching the remaining arguments to `person()`'s parameters.
fn split_top_level_commas_keep_empty(args: &str) -> Vec<String> {
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
    parts.into_iter().map(|s| s.trim().to_string()).collect()
}

/// Split `s` on the first top-level `key = value` assignment (not `==`, and
/// not inside a string literal, nor inside parens or brackets, so a nested
/// call's own named arguments do not count). Returns `None` if there isn't
/// one.
fn split_top_level_eq(s: &str) -> Option<(String, String)> {
    let chars: Vec<char> = s.chars().collect();
    let mut in_string: Option<char> = None;
    let mut depth = 0i32;
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
        } else if c == '(' || c == '[' {
            depth += 1;
        } else if c == ')' || c == ']' {
            depth -= 1;
        } else if c == '=' && depth == 0 {
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

/// R's `person()` parameters, in signature order. Positional arguments are
/// matched against the ones that were not supplied by name, so
/// `person("Jane", "Doe", , "jane@x.com", role = "cre")` puts `"jane@x.com"`
/// in `email`: `role` is named, and the empty third argument uses up
/// `middle`.
const PERSON_PARAMS: [&str; 8] = [
    "given", "family", "middle", "email", "role", "comment", "first", "last",
];

/// Parse one `person(...)` call's argument text into an [`Author`].
fn parse_person_call(args: &str) -> Result<Author, String> {
    let mut given: Option<String> = None;
    let mut family: Option<String> = None;
    let mut middle: Option<String> = None;
    let mut email: Option<String> = None;
    let mut roles: Vec<String> = vec![];
    let mut orcid: Option<String> = None;
    let mut ror: Option<String> = None;
    let mut named: Vec<(String, String)> = vec![];
    // `None` for an argument that was left out, e.g. the `middle` in
    // `person("Jane", "Doe", , "jane@x.com")`. It still uses up a position.
    let mut positional: Vec<Option<String>> = vec![];

    for part in split_top_level_commas_keep_empty(args) {
        if part.is_empty() {
            positional.push(None);
        } else if let Some((key, value)) = split_top_level_eq(&part) {
            named.push((key.trim().to_string(), value.trim().to_string()));
        } else {
            positional.push(Some(part));
        }
    }

    // Match the positional arguments to the parameters that no named
    // argument claimed, in signature order.
    let mut matched = named.clone();
    let mut positional = positional.into_iter();
    for param in PERSON_PARAMS {
        if named.iter().any(|(key, _)| key == param) {
            continue;
        }
        match positional.next() {
            Some(Some(value)) => matched.push((param.to_string(), value)),
            Some(None) => {}
            None => break,
        }
    }

    for (key, value) in matched {
        match key.as_str() {
            "given" | "first" => given = unquote(&value),
            "family" | "last" => family = unquote(&value),
            "middle" => middle = unquote(&value),
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

    let name = [given, middle, family]
        .into_iter()
        .flatten()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if name.is_empty() {
        return Err(format!("no name found in person({})", args));
    }

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
    Detailed(Box<DepTable>),
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
    // A direct http(s) link to a package source archive (`.tar.gz`/`.tgz`/
    // `.zip`), downloaded and extracted instead of cloned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    // The expected sha256 of a `url` dependency's downloaded archive, for
    // integrity verification and reproducibility -- a URL, unlike a git
    // commit, is not inherently content-addressed. Optional; when absent,
    // whatever the URL currently serves is trusted and its sha256 is
    // recorded in the lockfile.
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
    // A subdirectory of `git`/`url` the package lives in, e.g. a monorepo
    // package at `<repo>/subdir`, or a package archive wrapped in a
    // differently-named top-level directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subdir: Option<String>,
    // A GitHub pull request number (`owner/repo#41`). GitHub sources only,
    // resolved to a commit at fetch time; kept here so a later re-lock can
    // re-resolve the PR's current head instead of the sha it last resolved to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<u32>,
    // `owner/repo@*release`: track the repository's latest release instead of
    // a fixed ref. GitHub sources only, same re-resolution reasoning as `pr`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<bool>,
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
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
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
    /// land in `[dependency-groups.dev]` / `[dependency-groups.enhances]`.
    pub fn merge_description(&mut self, pkg: &DcfPackage) {
        for dep in pkg.dependencies.dependencies.iter() {
            let version_str = format_constraints(&dep.constraints);
            let hard = dep.types.contains(&RDepType::Depends)
                || dep.types.contains(&RDepType::Imports)
                || dep.types.contains(&RDepType::LinkingTo);

            if dep.types.contains(&RDepType::Depends) || dep.types.contains(&RDepType::Imports) {
                let value = if dep.name != "R" && dep.types.contains(&RDepType::Depends) {
                    Dependency::Detailed(Box::new(DepTable {
                        version: Some(version_str.clone()),
                        attach: Some(true),
                        ..Default::default()
                    }))
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
                        .entry("dev".to_string())
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
    /// optional version constraint) becomes an ordinary version requirement.
    /// A `git`/GitHub reference (the same syntax `Remotes:` uses, see
    /// [`crate::pkgsource::parse_pkg_source`]) becomes a [`DepTable`] with a
    /// `git` field, exactly like [`crate::proj::dep_table_from_remote`] builds
    /// for a `Remotes:` entry. Anything else (`bioc::`, `bitbucket::`,
    /// `gitlab::`, ...) is not a reference this crate resolves, so it is kept
    /// verbatim in [`DepTable::ref_`], and [`Rproj::to_description`] writes it
    /// back unchanged.
    ///
    /// A field with an empty value creates an empty group, so that it, too,
    /// round-trips.
    ///
    /// This only ever handles a plain `Config/Needs/<name>` field, i.e. one
    /// that maps to `[dependency-groups.<name>]`. `Config/Needs/Optional/<name>`
    /// -- which maps to `[optional-dependencies.<name>]` instead -- is
    /// handled separately by [`Rproj::merge_optional_dependencies`].
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

    /// Merge a DESCRIPTION's `Config/Needs/Optional/*` fields into this
    /// manifest's `[optional-dependencies.*]` extras: `Config/Needs/Optional/viz`
    /// becomes `[optional-dependencies.viz]`. Otherwise identical to
    /// [`Rproj::merge_config_needs`] -- see its docs for `needs`'s shape and
    /// how each entry's value is parsed -- just targeting
    /// `optional_dependencies` instead of `dependency_groups`.
    pub fn merge_optional_dependencies(&mut self, needs: &[(String, String)]) {
        for (group_name, value) in needs.iter() {
            if DESCRIPTION_DEP_GROUPS.contains(&group_name.as_str()) {
                warn!(
                    "Config/Needs/Optional/{} is merged into the `{}` \
                     dependency group, which `rig proj export` writes as a \
                     DESCRIPTION dependency field, not as \
                     Config/Needs/Optional/{}",
                    group_name, group_name, group_name
                );
            }
            let group = self
                .optional_dependencies
                .entry(group_name.clone())
                .or_default();
            for entry in value.split(',') {
                let entry = entry.trim();
                if entry.is_empty() {
                    continue;
                }
                let (name, dep) = config_needs_entry(entry);
                group.insert(name, dep);
            }
        }
    }

    /// Merge a DESCRIPTION's `Config/<group>/<key>` fields, other than
    /// `Config/Needs/*` (handled by [`Rproj::merge_config_needs`] instead),
    /// into `[config.<group>]` tables: `Config/testthat/edition: 3` becomes
    /// `edition = 3` under `[config.testthat]`. `entries` holds one `(group,
    /// key, raw field value)` triple per field.
    ///
    /// A value that parses as an integer or as `true`/`false` becomes a TOML
    /// integer or boolean; anything else is kept as a string.
    pub fn merge_config(&mut self, entries: &[(String, String, String)]) {
        for (group, key, value) in entries.iter() {
            self.config
                .entry(group.clone())
                .or_default()
                .insert(key.clone(), parse_config_value(value));
        }
    }

    /// Add a dependency to the manifest, or update it if the manifest lists it
    /// already. `dev` puts it in the `dev` dependency group (the group
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
                .entry("dev".to_string())
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

    /// Add (or replace) a git/GitHub/url-sourced dependency: a [`DepTable`]
    /// with `git` or `url` set, pinned by commit or archive rather than by
    /// version range. Mirrors [`Rproj::add_dependency`]'s dev/group
    /// placement.
    ///
    /// If the manifest already lists this package, its version requirement
    /// (e.g. `Imports: pkgcache (>= 2.2.0)`) and, for a `Dependency::Detailed`
    /// entry (e.g. `merge_description` set `attach = true` for a `Depends`
    /// entry), its `attach`/`enhances`/`vignette-builder` flags are kept --
    /// switching a dependency's source shouldn't drop its version constraint
    /// or reset those flags.
    pub fn add_remote_dependency(&mut self, name: &str, mut table: DepTable, dev: bool) {
        let group = if dev {
            &mut self
                .dependency_groups
                .entry("dev".to_string())
                .or_default()
                .dependencies
        } else {
            &mut self.dependencies
        };
        match group.get(name) {
            Some(Dependency::Detailed(old)) => {
                table.version = old.version.clone();
                table.attach = old.attach;
                table.enhances = old.enhances;
                table.vignette_builder = old.vignette_builder;
            }
            Some(Dependency::Version(old)) => {
                table.version = Some(old.clone());
            }
            None => {}
        }
        group.insert(name.to_string(), Dependency::Detailed(Box::new(table)));
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

    /// Returns the table at `path` inside `doc` (e.g. `&["dependencies"]`,
    /// `&["dependency-groups", "dev"]`, `&["config", "testthat"]`),
    /// creating any missing table along the way. A newly created table that
    /// isn't the last path segment -- a grouping table like
    /// `dependency-groups` or `config`, which only ever holds named
    /// sub-tables -- is marked `set_implicit(true)` so it doesn't print its
    /// own redundant header, matching how `toml::to_string_pretty` already
    /// renders those tables.
    fn doc_get_or_create_table<'a>(
        doc: &'a mut toml_edit::DocumentMut,
        path: &[&str],
    ) -> &'a mut toml_edit::Table {
        let mut table: &mut toml_edit::Table = doc.as_table_mut();
        for (i, segment) in path.iter().enumerate() {
            let is_new = !table.contains_key(segment);
            let item = table
                .entry(segment)
                .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
            table = item
                .as_table_mut()
                .expect("dependency/config table path segments are always tables");
            if is_new && i + 1 < path.len() {
                table.set_implicit(true);
            }
        }
        table
    }

    /// Render a resolved [`Dependency`] the way [`Rproj::inline_dependencies`]
    /// renders one when regenerating the whole file: a bare value for
    /// `Version`, a one-line inline table for `Detailed`.
    fn dependency_to_item(dep: &Dependency) -> Result<toml_edit::Item, Box<dyn Error>> {
        match dep {
            Dependency::Version(v) => Ok(toml_edit::value(v.clone())),
            Dependency::Detailed(table) => {
                let mut tmp: toml_edit::DocumentMut = toml::to_string(table)?.parse()?;
                let mut inline = tmp.as_table_mut().clone().into_inline_table();
                inline.decor_mut().clear();
                for (mut k, v) in inline.iter_mut() {
                    k.leaf_decor_mut().clear();
                    v.decor_mut().clear();
                }
                Ok(toml_edit::value(inline))
            }
        }
    }

    /// Convert a scalar `[config.*]` value (string/int/bool; see
    /// [`parse_config_value`]) into a `toml_edit::Item`.
    fn config_value_to_item(value: &toml::Value) -> toml_edit::Item {
        match value {
            toml::Value::Integer(i) => toml_edit::value(*i),
            toml::Value::Boolean(b) => toml_edit::value(*b),
            toml::Value::String(s) => toml_edit::value(s.clone()),
            other => toml_edit::value(other.to_string()),
        }
    }

    /// Insert or update `name` in the document-level table at `path`
    /// (`&["dependencies"]`, `&["dependency-groups", "dev"]`, ...),
    /// mirroring [`Rproj::add_dependency`]/[`Rproj::add_remote_dependency`]
    /// on the ORIGINAL on-disk document, so any comment, blank-line
    /// grouping, or unmodeled table elsewhere in the file survives. A
    /// brand-new key is appended at the end of its table rather than
    /// inserted alphabetically (matching `BTreeMap` order would require
    /// moving existing keys, risking a misattached comment).
    pub fn doc_set_dependency(
        doc: &mut toml_edit::DocumentMut,
        path: &[&str],
        name: &str,
        value: &Dependency,
    ) -> Result<(), Box<dyn Error>> {
        let item = Self::dependency_to_item(value)?;
        Self::doc_get_or_create_table(doc, path).insert(name, item);
        Ok(())
    }

    /// Insert or update `key` in `[config.<group>]` of the ORIGINAL on-disk
    /// document, mirroring [`Rproj::merge_config`].
    pub fn doc_set_config(
        doc: &mut toml_edit::DocumentMut,
        group: &str,
        key: &str,
        value: &toml::Value,
    ) {
        let item = Self::config_value_to_item(value);
        Self::doc_get_or_create_table(doc, &["config", group]).insert(key, item);
    }

    /// Remove `name` from whichever of `[dependencies]`,
    /// `[linking-dependencies]`, or any `[dependency-groups.*]` table has it
    /// in the ORIGINAL on-disk document, mirroring
    /// [`Rproj::remove_dependency`]. Returns whether it was found and
    /// removed. A comment directly above the removed key is that key's
    /// leading decor in `toml_edit` and is removed along with it.
    pub fn doc_remove_dependency(doc: &mut toml_edit::DocumentMut, name: &str) -> bool {
        let root = doc.as_table_mut();
        if let Some(deps) = root.get_mut("dependencies").and_then(|t| t.as_table_mut()) {
            if deps.remove(name).is_some() {
                return true;
            }
        }
        if let Some(deps) = root
            .get_mut("linking-dependencies")
            .and_then(|t| t.as_table_mut())
        {
            if deps.remove(name).is_some() {
                return true;
            }
        }
        if let Some(groups) = root
            .get_mut("dependency-groups")
            .and_then(|t| t.as_table_mut())
        {
            for (_, group) in groups.iter_mut() {
                if let Some(group) = group.as_table_mut() {
                    if group.remove(name).is_some() {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Every dependency, anywhere in the manifest (`[dependencies]`,
    /// `[linking-dependencies]`, any `[dependency-groups.*]`), that has
    /// `git`, `url` or `path` set: the git/GitHub/url/local-sourced packages,
    /// for the pre-solve fetch that registers their real name/version/deps
    /// with the solver (see `crate::proj::register_git_sources`).
    ///
    /// A `path` is stored in `rproj.toml` relative to `root` (the manifest's
    /// own directory), so that the file stays portable when committed and
    /// checked out elsewhere -- see `crate::proj::relativize_to_root`. It is
    /// resolved back to an absolute path here, against `root`, before
    /// anything downstream (`crate::proj::resolve_git_sources`) reads it, so
    /// that code never has to know rig's working directory either.
    pub fn git_dependencies(&self, root: &Path) -> Vec<(String, DepTable)> {
        let mut out = vec![];
        let tables = std::iter::once(&self.dependencies)
            .chain(std::iter::once(&self.linking_dependencies))
            .chain(self.dependency_groups.values().map(|g| &g.dependencies))
            .chain(self.optional_dependencies.values());
        for table in tables {
            for (name, dep) in table.iter() {
                if let Dependency::Detailed(t) = dep {
                    if t.git.is_some() || t.url.is_some() || t.path.is_some() {
                        let mut t = (**t).clone();
                        if let Some(path) = &t.path {
                            let path = Path::new(path);
                            if path.is_relative() {
                                t.path = Some(root.join(path).display().to_string());
                            }
                        }
                        out.push((name.clone(), t));
                    }
                }
            }
        }
        out
    }

    /// The git/GitHub/url/local-sourced dependencies that end up in a
    /// DESCRIPTION dependency field (`Depends`/`Imports`/`LinkingTo`/
    /// `Suggests`/`Enhances`), for [`Rproj::to_description`]'s `Remotes:`
    /// field. Scoped the same way as [`Rproj::to_dep_version_specs`] --
    /// `[dependencies]`, `[linking-dependencies]`, the `dev`/`enhances`
    /// dependency groups, and the `dev`/`enhances` `[optional-dependencies.*]`
    /// extras -- unlike [`Rproj::git_dependencies`], which also sweeps
    /// arbitrary `Config/Needs/*` groups that already carry their own pak-ref
    /// entries and must not duplicate into `Remotes:`. A non-reserved
    /// `[optional-dependencies.*]` extra is excluded the same way a
    /// non-reserved `[dependency-groups.*]` one is: its git/url/path-sourced
    /// entries are written into their own `Config/Needs/Optional/<name>`
    /// field instead (see [`Rproj::to_description`]), so must not also show
    /// up under `Remotes:`. Unlike `git_dependencies`, a `path` here is kept
    /// exactly as stored in the manifest (relative to its directory), since
    /// `DESCRIPTION` is written into that same directory, so the relative
    /// path is exactly as usable from there.
    fn description_git_dependencies(&self) -> Vec<(String, DepTable)> {
        let mut out = vec![];
        let tables = std::iter::once(&self.dependencies)
            .chain(std::iter::once(&self.linking_dependencies))
            .chain(DESCRIPTION_DEP_GROUPS.iter().filter_map(|group_name| {
                self.dependency_groups
                    .get(*group_name)
                    .map(|g| &g.dependencies)
            }))
            .chain(DESCRIPTION_DEP_GROUPS.iter().filter_map(|group_name| {
                self.optional_dependencies.get(*group_name)
            }));
        for table in tables {
            for (name, dep) in table.iter() {
                if let Dependency::Detailed(t) = dep {
                    if t.git.is_some() || t.url.is_some() || t.path.is_some() {
                        out.push((name.clone(), (**t).clone()));
                    }
                }
            }
        }
        out
    }

    /// The manifest's dependencies as the solver's [`PackageDependencies`], the
    /// inverse of [`Rproj::merge_description`]: `[dependencies]` becomes
    /// `Depends` (entries marked `attach = true`, and `R` itself) or `Imports`,
    /// `[linking-dependencies]` becomes `LinkingTo`, and the `dev` / `enhances`
    /// dependency groups become `Suggests` / `Enhances`. Every other
    /// `[dependency-groups.*]` table -- an arbitrary `Config/Needs/*` list,
    /// see [`Rproj::merge_config_needs`] -- is folded in as `Suggests` too,
    /// the same as every `[optional-dependencies.*]` extra: none of these
    /// have their own DESCRIPTION dependency type, but they are still solved
    /// alongside everything else rather than left out or solved on their own.
    ///
    /// Soft dependencies are dropped unless `dev`; a package that is also a hard
    /// dependency stays, because it needs to be installed either way.
    pub fn to_dep_version_specs(&self, dev: bool) -> Result<PackageDependencies, Box<dyn Error>> {
        self.to_dep_version_specs_impl(dev, true)
    }

    /// [`Rproj::to_dep_version_specs`], but with a switch for whether
    /// `[dependency-groups.*]` groups other than `dev`/`enhances` are folded
    /// in as `Suggests`. [`Rproj::to_description`] needs that switched off:
    /// those groups are rendered into their own `Config/Needs/<group>` field
    /// instead (see its loop over [`Rproj::dependency_groups`]), and must not
    /// also show up under `Suggests:`, or they would be listed, and
    /// installed, twice over. Every other caller solves and installs the
    /// manifest's full dependency set, so [`Rproj::to_dep_version_specs`]
    /// leaves the switch on.
    ///
    /// The switch does *not* apply to `[optional-dependencies.*]` extras:
    /// those always fold into `Suggests`, even from [`Rproj::to_description`]
    /// (which also writes each one into its own
    /// `Config/Needs/Optional/<name>` field, see
    /// [`Rproj::merge_optional_dependencies`]) -- an optional dependency has
    /// to be `Suggests`-listed for `R CMD check` to allow using it
    /// conditionally, unlike an arbitrary `Config/Needs/*` group, which has
    /// no such requirement.
    fn to_dep_version_specs_impl(
        &self,
        dev: bool,
        other_groups: bool,
    ) -> Result<PackageDependencies, Box<dyn Error>> {
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

        let resolved_groups = self.resolved_dependency_groups()?;
        // A group can also declare packages by naming, not owning, them --
        // via `include-groups` -- so build a lookup from package name to
        // whichever group actually owns its `Dependency`/version spec, for
        // the inherited names below.
        let mut owner: HashMap<&str, &Dependency> = HashMap::new();
        for group in self.dependency_groups.values() {
            for (name, dep) in group.dependencies.iter() {
                owner.insert(name.as_str(), dep);
            }
        }
        for (group_name, group) in self.dependency_groups.iter() {
            let dep_type = match group_name.as_str() {
                "dev" => RDepType::Suggests,
                "enhances" => RDepType::Enhances,
                _ if other_groups => RDepType::Suggests,
                _ => continue,
            };
            for (name, dep) in group.dependencies.iter() {
                deps.push(dep_spec(name, dep, dep_type.clone())?);
            }
            // Packages inherited through `include-groups`, not declared
            // directly in this group: same dep_type as this group's own
            // packages, since they are just as much this group's concern.
            if let Some(resolved) = resolved_groups.get(group_name) {
                for name in resolved {
                    if group.dependencies.contains_key(name) {
                        continue;
                    }
                    if let Some(dep) = owner.get(name.as_str()) {
                        deps.push(dep_spec(name, dep, dep_type.clone())?);
                    }
                }
            }
        }

        for extra in self.optional_dependencies.values() {
            for (name, dep) in extra.iter() {
                deps.push(dep_spec(name, dep, RDepType::Suggests)?);
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

    /// One `[dependency-groups.<name>]` table's effective package names:
    /// its own `dependencies`, plus every group named in its
    /// `include-groups`, resolved the same way (recursively). `visiting`
    /// tracks the names currently being resolved, to catch a group that
    /// includes itself, directly or through others, as an error rather
    /// than infinite recursion. `cache` memoizes groups already resolved,
    /// since the same group can be included from several places.
    fn resolve_group(
        &self,
        name: &str,
        visiting: &mut Vec<String>,
        cache: &mut HashMap<String, Vec<String>>,
    ) -> Result<Vec<String>, Box<dyn Error>> {
        if let Some(resolved) = cache.get(name) {
            return Ok(resolved.clone());
        }
        if visiting.iter().any(|n| n == name) {
            let mut path = visiting.clone();
            path.push(name.to_string());
            bail!("dependency group cycle: {}", path.join(" -> "));
        }
        let group = match self.dependency_groups.get(name) {
            Some(group) => group,
            None => bail!(
                "`include-groups` names \"{}\", which is not a\n\
                `[dependency-groups.{}]` table",
                name,
                name
            ),
        };

        visiting.push(name.to_string());
        let mut resolved: Vec<String> = group.dependencies.keys().cloned().collect();
        for included in group.include_groups.iter() {
            resolved.extend(self.resolve_group(included, visiting, cache)?);
        }
        visiting.pop();

        resolved.sort();
        resolved.dedup();
        cache.insert(name.to_string(), resolved.clone());
        Ok(resolved)
    }

    /// Every `[dependency-groups.*]` table's effective package names,
    /// `include-groups` resolved all the way through: a group's set is its
    /// own packages plus every included group's set, recursively. Errors on
    /// a cycle, or on `include-groups` naming a group that does not exist.
    pub fn resolved_dependency_groups(
        &self,
    ) -> Result<HashMap<String, Vec<String>>, Box<dyn Error>> {
        let mut cache = HashMap::new();
        let mut out = HashMap::new();
        for name in self.dependency_groups.keys() {
            let mut visiting = vec![];
            let resolved = self.resolve_group(name, &mut visiting, &mut cache)?;
            out.insert(name.clone(), resolved);
        }
        Ok(out)
    }

    /// Every `[optional-dependencies.*]` extra's package names, under its
    /// own name. Unlike [`Rproj::resolved_dependency_groups`], extras have no
    /// `include-groups` of their own, so this is a direct lookup.
    pub fn optional_dependency_roots(&self) -> HashMap<String, Vec<String>> {
        self.optional_dependencies
            .iter()
            .map(|(name, extra)| (name.clone(), extra.keys().cloned().collect()))
            .collect()
    }

    /// `"main"`, the hard `[dependencies]`/`[linking-dependencies]` names --
    /// shared by [`Rproj::dependency_group_roots`] and
    /// [`Rproj::main_and_group_roots`].
    fn main_roots(&self) -> Vec<String> {
        self.dependencies
            .keys()
            .chain(self.linking_dependencies.keys())
            .cloned()
            .collect()
    }

    /// `"main"` plus every `[dependency-groups.*]` table under its own name
    /// (`include-groups` resolved, see [`Rproj::resolved_dependency_groups`]),
    /// without the `[optional-dependencies.*]` extras --
    /// [`rig proj sync`](crate::proj)'s `--group`/`--all-groups`/`--no-dev`
    /// selection is scoped to this set, kept apart from extras so
    /// `--all-groups` and `--all-extras` mean different things.
    pub fn main_and_group_roots(&self) -> Result<HashMap<String, Vec<String>>, Box<dyn Error>> {
        let mut roots: HashMap<String, Vec<String>> = HashMap::new();
        roots.insert("main".to_string(), self.main_roots());
        roots.extend(self.resolved_dependency_groups()?);
        Ok(roots)
    }

    /// The manifest's solvable dependency groups, as direct dependency names,
    /// for classifying a solved package graph by which group(s) need it:
    /// `"main"` for the hard `[dependencies]`/`[linking-dependencies]`, every
    /// `[dependency-groups.*]` table under its own name (`include-groups`
    /// resolved, see [`Rproj::resolved_dependency_groups`]), and every
    /// `[optional-dependencies.*]` extra under its own name -- the same set
    /// [`Rproj::to_dep_version_specs`] solves for, so a package this returns
    /// can always be found among that method's output. For solving/tagging
    /// purposes where groups and extras must stay distinguishable, use
    /// [`Rproj::main_and_group_roots`] and [`Rproj::optional_dependency_roots`]
    /// instead; this merged view is for display (`rig proj status`).
    pub fn dependency_group_roots(&self) -> Result<HashMap<String, Vec<String>>, Box<dyn Error>> {
        let mut roots = self.main_and_group_roots()?;
        roots.extend(self.optional_dependency_roots());
        Ok(roots)
    }

    /// Replace every `{ workspace = true }` dependency with the workspace
    /// root's `[workspace.dependencies]` entry of the same name, so that the
    /// rest of the code never has to know the constraint was inherited.
    ///
    /// This is a pass over a member manifest, not part of
    /// [`Rproj::to_dep_version_specs`], because that one is also called for a
    /// single project (`rig proj deps`, `rig proj tree`, `rig proj export`,
    /// `rig proj renv import`), which has no workspace to inherit from.
    ///
    /// `origin` is the member's manifest path, for the error message when the
    /// name is not in `[workspace.dependencies]`.
    pub fn inherit_workspace_deps(
        &mut self,
        ws: &Workspace,
        origin: &Path,
    ) -> Result<(), Box<dyn Error>> {
        let mut tables: Vec<&mut BTreeMap<String, Dependency>> =
            vec![&mut self.dependencies, &mut self.linking_dependencies];
        tables.extend(self.optional_dependencies.values_mut());
        tables.extend(
            self.dependency_groups
                .values_mut()
                .map(|group| &mut group.dependencies),
        );

        for table in tables {
            for (name, dep) in table.iter_mut() {
                let local = match dep {
                    Dependency::Detailed(t) if t.workspace == Some(true) => (**t).clone(),
                    _ => continue,
                };
                let shared = match ws.dependencies.get(name) {
                    Some(shared) => shared,
                    None => bail!(
                        "{} = {{ workspace = true }} in {}, but there is no \
                         [workspace.dependencies] entry for {}",
                        name,
                        origin.display(),
                        name
                    ),
                };
                *dep = inherit_dep(shared, &local);
            }
        }

        Ok(())
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

        writeln!(
            out,
            "{}: This file was created by rig, do not edit it manually",
            DESCRIPTION_RIG_NOTE_FIELD
        )?;
        writeln!(out, "Package: {}", self.project.name)?;
        if !self.project.is_package() {
            writeln!(
                out,
                "Type: {}",
                title_case(self.project.type_.as_deref().unwrap_or("package"))
            )?;
        }
        if let Some(title) = &self.project.title {
            writeln!(out, "{}", fold_dcf_prose("Title", title, 75))?;
        }
        writeln!(out, "Version: {}", self.project.version)?;
        if !self.project.authors.is_empty() {
            writeln!(
                out,
                "Authors@R: {}",
                format_authors_r(&self.project.authors)
            )?;
        }
        if let Some(description) = &self.project.description {
            writeln!(
                out,
                "{}",
                crate::textfmt::text_to_dcf_field("Description", description)
            )?;
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
        let pkg_deps = self.to_dep_version_specs_impl(true, false)?;
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
            writeln!(out, "{}:{}", dep_type, fold_dcf_list(&items))?;
        }

        let mut git_deps = self.description_git_dependencies();
        if !git_deps.is_empty() {
            git_deps.sort_by(|a, b| a.0.cmp(&b.0));
            let items: Vec<String> = git_deps
                .iter()
                .map(|(name, table)| dep_table_to_pak_ref(name, table))
                .collect();
            writeln!(out, "Remotes:{}", fold_dcf_list(&items))?;
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
                writeln!(out, "Config/Needs/{}:{}", group_name, fold_dcf_list(&items))?;
            }
        }

        for (group_name, extra) in self.optional_dependencies.iter() {
            if DESCRIPTION_DEP_GROUPS.contains(&group_name.as_str()) {
                continue;
            }
            let mut items: Vec<String> = Vec::new();
            for (name, dep) in extra.iter() {
                let (item, was_dropped) = format_group_entry(name, dep)?;
                if was_dropped {
                    dropped.push(name.clone());
                }
                items.push(item);
            }
            if items.is_empty() {
                writeln!(out, "Config/Needs/Optional/{}:", group_name)?;
            } else {
                writeln!(
                    out,
                    "Config/Needs/Optional/{}:{}",
                    group_name,
                    fold_dcf_list(&items)
                )?;
            }
        }

        for (group, table) in self.config.iter() {
            for (key, value) in table.iter() {
                writeln!(
                    out,
                    "Config/{}/{}: {}",
                    group,
                    key,
                    format_config_value(value)
                )?;
            }
        }

        // The `[description]` escape hatch has no structured place in
        // DESCRIPTION's field order, so it goes last.
        for (key, value) in self.description.iter() {
            writeln!(out, "{}: {}", key, format_config_value(value))?;
        }

        Ok((out, dropped))
    }

    /// Render the manifest as the TOML text of `Rproj.toml`.
    ///
    /// A dependency with extra fields (`git`, `path`, ...) is a `DepTable`,
    /// which the plain serializer writes as its own `[dependencies.pkg]`
    /// section. Writing it as an inline table instead keeps one dependency to
    /// one line, the same trick already used for `metadata` in
    /// [`RprojLock::to_toml`].
    pub fn to_toml(&self) -> Result<String, Box<dyn Error>> {
        let mut doc: toml_edit::DocumentMut = toml::to_string_pretty(self)?.parse()?;

        if let Some(deps) = doc.get_mut("dependencies").and_then(|t| t.as_table_mut()) {
            Self::inline_dependencies(deps);
        }
        if let Some(deps) = doc
            .get_mut("linking-dependencies")
            .and_then(|t| t.as_table_mut())
        {
            Self::inline_dependencies(deps);
        }
        if let Some(groups) = doc
            .get_mut("optional-dependencies")
            .and_then(|t| t.as_table_mut())
        {
            for (_, group) in groups.iter_mut() {
                if let Some(group) = group.as_table_mut() {
                    Self::inline_dependencies(group);
                }
            }
        }
        if let Some(groups) = doc
            .get_mut("dependency-groups")
            .and_then(|t| t.as_table_mut())
        {
            for (_, group) in groups.iter_mut() {
                if let Some(group) = group.as_table_mut() {
                    Self::inline_dependencies(group);
                }
            }
        }
        if let Some(deps) = doc
            .get_mut("workspace")
            .and_then(|t| t.as_table_mut())
            .and_then(|t| t.get_mut("dependencies"))
            .and_then(|t| t.as_table_mut())
        {
            Self::inline_dependencies(deps);
        }
        if let (Some(description), Some(project)) = (
            self.project.description.as_deref(),
            doc.get_mut("project").and_then(|t| t.as_table_mut()),
        ) {
            // A multi-paragraph description needs real line breaks in the
            // file, not `toml`'s default `"line1\nline2"` escaping; a TOML
            // literal string (`'''...'''`) renders those verbatim. Skip the
            // (very unlikely) case where the text itself contains `'''`,
            // which can't be represented that way, and fall back to the
            // default escaped single-line string.
            if description.contains('\n') && !description.contains("'''") {
                // A newline right after the opening delimiter is trimmed by
                // the TOML parser, but nothing trims one before the closing
                // delimiter, so the closing `'''` goes straight after the
                // text to avoid adding a trailing blank line.
                let literal = format!("'''\n{}'''", description);
                if let Ok(value) = literal.parse::<toml_edit::Value>() {
                    project.insert("description", toml_edit::Item::Value(value));
                }
            }
        }

        Ok(doc.to_string())
    }

    /// Turn every package entry of `table` that serialized as a full
    /// `[table.pkg]` section into an inline table.
    fn inline_dependencies(table: &mut toml_edit::Table) {
        let keys: Vec<String> = table
            .iter()
            .filter(|(_, item)| item.is_table())
            .map(|(key, _)| key.to_string())
            .collect();
        for key in keys {
            let Some(toml_edit::Item::Table(dep)) = table.remove(&key) else {
                continue;
            };
            let mut dep = dep.into_inline_table();
            // `into_inline_table()` keeps the decorations of the section the
            // table came from, i.e. the blank line before its header.
            dep.decor_mut().clear();
            for (mut k, v) in dep.iter_mut() {
                k.leaf_decor_mut().clear();
                v.decor_mut().clear();
            }
            table.insert(&key, toml_edit::value(dep));
        }
    }
}

/// Format `Authors@R`'s value: one `person(...)` call per line when there is
/// more than one author, indented 4 spaces under `c(` and closed with `)` on
/// its own line, e.g.:
///
/// ```text
/// Authors@R: c(
///     person("Jane Doe", email = "jane@x.com", role = c("aut", "cre")),
///     person("Rich Contributor", role = "ctb")
///   )
/// ```
///
/// A single author is written as a bare call with no `c(...)`. Either way,
/// each call that would otherwise exceed 75 columns wraps its arguments
/// across further lines (see [`wrap_person_call`]).
fn format_authors_r(authors: &[Author]) -> String {
    let calls: Vec<String> = authors.iter().map(Author::to_person_r).collect();
    if calls.len() == 1 {
        return wrap_person_call(&calls[0], "Authors@R: ".len(), 4);
    }
    let last = calls.len() - 1;
    let mut out = "c(".to_string();
    for (i, call) in calls.iter().enumerate() {
        out.push_str("\n    ");
        out.push_str(&wrap_person_call(call, 4, 11));
        if i != last {
            out.push(',');
        }
    }
    out.push_str("\n  )");
    out
}

/// Wrap one `person(...)` call (as built by [`Author::to_person_r`]) so no
/// line exceeds 75 columns, breaking at top-level commas between its
/// arguments. `first_line_indent` is the number of columns already used
/// before the call starts (e.g. the length of `"Authors@R: "`, or of the
/// 4-space indent before a call in a `c(...)` list); continuation lines are
/// indented by `continuation_indent` spaces, which
/// [`format_authors_r`] sets to align under the call's opening paren.
fn wrap_person_call(call: &str, first_line_indent: usize, continuation_indent: usize) -> String {
    let (Some(open), Some(close)) = (call.find('('), call.rfind(')')) else {
        return call.to_string();
    };
    let prefix = &call[..=open];
    let inner = &call[open + 1..close];
    let args = split_top_level_commas(inner);

    let mut lines: Vec<String> = Vec::new();
    let mut cur = prefix.to_string();
    let mut indent = first_line_indent;
    let mut cur_has_arg = false;
    for (i, arg) in args.iter().enumerate() {
        let comma = if i + 1 < args.len() { "," } else { "" };
        let piece = format!("{}{}", arg.trim(), comma);
        if cur_has_arg {
            let tentative_len = indent + cur.chars().count() + 1 + piece.chars().count();
            if tentative_len > 75 {
                lines.push(cur);
                indent = continuation_indent;
                cur = piece;
                continue;
            }
            cur.push(' ');
            cur.push_str(&piece);
        } else {
            cur.push_str(&piece);
            cur_has_arg = true;
        }
    }
    cur.push(')');
    lines.push(cur);

    lines.join(&format!("\n{}", " ".repeat(continuation_indent)))
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
/// written back as it came in; a `git`/`url`/`path`-sourced entry goes
/// through [`dep_table_to_pak_ref`], which rebuilds a `pak` reference from
/// its `DepTable` fields; anything else goes through [`format_dep_entry`],
/// so it looks like a DESCRIPTION dependency entry.
fn format_group_entry(name: &str, dep: &Dependency) -> Result<(String, bool), Box<dyn Error>> {
    if let Dependency::Detailed(table) = dep {
        if let Some(ref_) = &table.ref_ {
            return Ok((ref_.clone(), false));
        }
        if table.git.is_some() || table.url.is_some() || table.path.is_some() {
            return Ok((dep_table_to_pak_ref(name, table), false));
        }
    }
    let spec = dep_spec(name, dep, RDepType::Suggests)?;
    Ok(format_dep_entry(&spec))
}

/// The inverse of [`crate::proj::dep_table_from_remote`]/
/// [`crate::proj::dep_table_from_url`]/[`crate::proj::dep_table_from_local`]:
/// rebuild a `pak` package reference from a `git`-, `url`- or `path`-sourced
/// [`DepTable`], for writing a `Remotes:`/`Config/Needs/*` entry back to
/// `DESCRIPTION`.
///
/// If `table.ref_` is set (the normal case: it is filled in by
/// [`crate::proj::dep_table_from_remote`] with the original reference text),
/// that text is written back verbatim -- this is what makes a `gitlab::`
/// reference (any host, with a subdir) and a GitHub reference's original
/// spelling round-trip losslessly, since a git URL alone cannot always be
/// reconstructed back into its source syntax. Otherwise (a `DepTable` built
/// by hand, e.g. a `git = "..."`/`url = "..."` entry written directly into
/// `rproj.toml`, which never went through `dep_table_from_remote`), fall
/// back to rebuilding a reference from the structured fields: `<owner>/<repo>
/// [/<subdir>][@<ref>|#<pr>|@*release]` for a GitHub URL, `git::<url>[@<rev>]`
/// for another git URL, or `url::<url>` for a `url` dependency. Either way,
/// `name` is prefixed on with `<name>=` only when it does not match the name
/// the reference itself implies (see [`pak_ref_name`]).
fn dep_table_to_pak_ref(name: &str, table: &DepTable) -> String {
    if let Some(entry) = &table.ref_ {
        return match pak_ref_name(entry) {
            Some(implied) if implied == name => entry.clone(),
            _ => format!("{}={}", name, entry),
        };
    }

    if let Some(url) = &table.url {
        let entry = format!("url::{}", url);
        return format!("{}={}", name, entry);
    }

    if let Some(path) = &table.path {
        let entry = format!("local::{}", path);
        return match pak_ref_name(&entry) {
            Some(implied) if implied == name => entry,
            _ => format!("{}={}", name, entry),
        };
    }

    let git_url = table.git.as_deref().unwrap_or_default();

    let detail = if let Some(pr) = table.pr {
        format!("#{}", pr)
    } else if table.release == Some(true) {
        "@*release".to_string()
    } else if let Some(r) = table
        .rev
        .as_deref()
        .or(table.branch.as_deref())
        .or(table.tag.as_deref())
    {
        format!("@{}", r)
    } else {
        String::new()
    };

    let path = match crate::proj::github_owner_repo(git_url) {
        Some((owner, repo)) => {
            let mut path = format!("{}/{}", owner, repo);
            if let Some(subdir) = &table.subdir {
                path.push('/');
                path.push_str(subdir);
            }
            path
        }
        None => format!("git::{}", git_url),
    };

    let entry = format!("{}{}", path, detail);
    match pak_ref_name(&entry) {
        Some(implied) if implied == name => entry,
        _ => format!("{}={}", name, entry),
    }
}

/// One entry of a `Config/Needs/*` field as a dependency-group entry: the
/// package name to key it under, and the dependency itself. A plain package
/// name, with an optional version constraint, becomes a version requirement;
/// anything else is a package reference in one of `pak`'s syntaxes, kept
/// verbatim under the package name the reference implies.
/// Parse a `Config/<group>/<key>` DESCRIPTION field value for
/// [`Rproj::merge_config`]: an integer or `true`/`false` string becomes the
/// matching TOML type, anything else stays a string.
fn parse_config_value(value: &str) -> toml::Value {
    if let Ok(i) = value.parse::<i64>() {
        toml::Value::Integer(i)
    } else if value.eq_ignore_ascii_case("true") {
        toml::Value::Boolean(true)
    } else if value.eq_ignore_ascii_case("false") {
        toml::Value::Boolean(false)
    } else {
        toml::Value::String(value.to_string())
    }
}

/// Format a `[config.<group>]` value back as a DESCRIPTION field value, the
/// inverse of [`parse_config_value`].
fn format_config_value(value: &toml::Value) -> String {
    match value {
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn config_needs_entry(entry: &str) -> (String, Dependency) {
    if let Ok(spec) = DepVersionSpec::parse(entry, "Suggests") {
        if is_r_package_name(&spec.name) {
            return (
                spec.name,
                Dependency::Version(format_constraints(&spec.constraints)),
            );
        }
    }

    match crate::pkgsource::parse_pkg_source(entry) {
        Ok(crate::pkgsource::PkgSource::Remote(r)) => {
            if let Some(name) = pak_ref_name(entry) {
                return (
                    name,
                    Dependency::Detailed(Box::new(crate::proj::dep_table_from_remote(&r, entry))),
                );
            }
        }
        Ok(crate::pkgsource::PkgSource::Url(u)) => {
            // `pak_ref_name`'s "last path segment, minus one extension"
            // heuristic misreads a versioned archive file name (e.g.
            // `mypkg_1.0.0.tar.gz` -> `mypkg_1`), so a `url::` entry always
            // needs its own `<name>=` override to be usable here.
            if let Some(name) = &u.name_override {
                return (
                    name.clone(),
                    Dependency::Detailed(Box::new(crate::proj::dep_table_from_url(&u))),
                );
            }
        }
        Ok(crate::pkgsource::PkgSource::Cran)
        | Ok(crate::pkgsource::PkgSource::Local(_))
        | Err(_) => {}
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
        Dependency::Detailed(Box::new(DepTable {
            ref_: Some(entry.to_string()),
            ..Default::default()
        })),
    )
}

/// The package name a `pak` package reference implies, e.g. `tidytemplate`
/// for `tidyverse/tidytemplate@main`. `pak` reference syntax is
/// `[<name>=][<type>::]<ref>`, so an explicit name wins; otherwise the name
/// is the last path component of the reference, without its `@<tag>` /
/// `#<pull request>` suffix and without a file extension. `None` if that does
/// not leave a valid package name behind.
pub(crate) fn pak_ref_name(entry: &str) -> Option<String> {
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

/// Write comma-joined `items` (each already formatted, e.g.
/// `"dplyr (>= 1.1.0)"`) one per line, on DCF continuation lines indented by
/// four spaces. The returned string starts with a newline, so the caller
/// writes it right after the field name and colon.
fn fold_dcf_list(items: &[String]) -> String {
    let mut out = String::new();
    for (i, item) in items.iter().enumerate() {
        out.push_str("\n    ");
        out.push_str(item);
        if i + 1 < items.len() {
            out.push(',');
        }
    }
    out
}

/// Wrap a prose field (`Title`/`Description`) to at most `width` columns,
/// counting the `Key: ` prefix on the first line and the DCF 4-space
/// continuation indent on the rest, so no rendered line is longer than
/// `width`. Returns the whole field, prefix included.
fn fold_dcf_prose(key: &str, value: &str, width: usize) -> String {
    const INDENT: &str = "    ";
    let first_width = width.saturating_sub(key.len() + 2);
    let rest_width = width.saturating_sub(INDENT.len());

    let mut out = format!("{}:", key);
    let mut line = String::new();
    let mut first = true;
    let flush = |out: &mut String, line: &mut String, first: &mut bool| {
        if *first {
            out.push(' ');
            *first = false;
        } else {
            out.push('\n');
            out.push_str(INDENT);
        }
        out.push_str(line);
        line.clear();
    };
    for word in value.split_whitespace() {
        let avail = if first { first_width } else { rest_width };
        if line.is_empty() {
            line.push_str(word);
        } else if line.len() + 1 + word.len() <= avail {
            line.push(' ');
            line.push_str(word);
        } else {
            flush(&mut out, &mut line, &mut first);
            line.push_str(word);
        }
    }
    if !line.is_empty() || first {
        flush(&mut out, &mut line, &mut first);
    }
    out
}

/// Whether a dependency is attached (`Depends:` rather than `Imports:`).
fn dep_attach(dep: &Dependency) -> bool {
    match dep {
        Dependency::Version(_) => false,
        Dependency::Detailed(t) => t.attach == Some(true),
    }
}

/// One `{ workspace = true }` entry resolved against the workspace root's
/// `[workspace.dependencies]` entry of the same name, for
/// [`Rproj::inherit_workspace_deps`].
///
/// The version and the source come from the shared entry, which is the point
/// of inheriting it. The `attach` / `enhances` / `vignette-builder` flags stay
/// the member's own: how a member uses a package says nothing about how the
/// workspace pins it, and the shared entry has no business attaching a package
/// in a member that only imports it.
fn inherit_dep(shared: &Dependency, local: &DepTable) -> Dependency {
    let mut table = match shared {
        Dependency::Version(v) => DepTable {
            version: Some(v.clone()),
            ..Default::default()
        },
        Dependency::Detailed(t) => (**t).clone(),
    };
    table.workspace = None;
    table.attach = local.attach.or(table.attach);
    table.enhances = local.enhances.or(table.enhances);
    table.vignette_builder = local.vignette_builder.or(table.vignette_builder);
    Dependency::Detailed(Box::new(table))
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
pub(crate) fn format_constraints(constraints: &[VersionConstraint]) -> String {
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

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RprojLockTarget {
    pub r_version: String,
    pub platform: String,
    /// Fingerprint of the manifest's own direct dependency requirements this
    /// target was solved against: one entry per name in `solve.merged`
    /// (minus `"R"` and the base packages, same as `packages` below), each
    /// with its version requirement formatted the same way
    /// [`format_constraints`] writes it into `rproj.toml`. Filled in by
    /// `proj_lock` after the solve, the same way `groups` is on
    /// [`RprojLockPackage`] -- it needs the manifest's roots, not just the
    /// solution, so [`RprojLockTarget::from_solution`] leaves it empty.
    ///
    /// This is what lets a later `rig proj lock` run tell whether this target
    /// still satisfies the manifest without re-solving: compare this list's
    /// names against the manifest's current direct dependencies (catches an
    /// added or removed dependency), and each entry's requirement against the
    /// version actually pinned in `packages` (catches a tightened
    /// constraint) -- see `lock_target_satisfies` in `src/proj.rs`.
    pub direct_dependencies: Vec<LockDirectDependency>,
    pub packages: Vec<RprojLockPackage>,
}

/// One entry of [`RprojLockTarget::direct_dependencies`]: a manifest direct
/// dependency's name and the version requirement it was solved against, e.g.
/// `{ name: "dplyr", constraint: ">= 1.1.0" }`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct LockDirectDependency {
    pub name: String,
    pub constraint: String,
}

/// One solved package of one target, i.e. one file to install.
///
/// Every field here is read when installing; nothing is recorded for the
/// record's sake. `platform` is the binary platform the file was built for, or
/// `"source"`, which is not the same thing as the target's `platform`: a target
/// can mix binary and source packages.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RprojLockPackage {
    pub package: String,
    pub version: String,
    pub binary: bool,
    pub platform: String,
    pub dependencies: Vec<String>,
    /// `RemoteHash` and `RemoteLinkingToHashes`, i.e. what a direct install
    /// writes into the installed package's `DESCRIPTION`.
    pub metadata: HashMap<String, String>,
    /// Where the file is downloaded from, first one that works.
    pub sources: Vec<String>,
    /// Where the file is cached, relative to the package cache.
    pub target: String,
    /// Which dependency group(s) require this package: `"main"` for a hard
    /// `[dependencies]`/`[linking-dependencies]` requirement, plus the name
    /// of every `[dependency-groups.*]` table that (transitively) needs it.
    /// Filled in by `proj_lock` after the solve, not by [`Self::from_solution`]
    /// itself, since it needs the whole package graph, not just one entry.
    /// Kept separate from [`Self::extra_groups`] so `rig proj sync`'s
    /// `--group`/`--all-groups` and `--extra`/`--all-extras` mean different
    /// things.
    pub groups: Vec<String>,
    /// Which `[optional-dependencies.*]` extra(s) (transitively) need this
    /// package. See [`Self::groups`]. Absent (empty) in a lockfile written
    /// before this field existed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_groups: Vec<String>,
    /// Whether this is the project's own package (`type = "package"` in
    /// `rproj.toml`), installed from the project root itself -- see
    /// `ProjectSolve::self_alias` in `src/proj.rs` -- rather than downloaded.
    /// Lets `rig proj sync --no-install-project` find it, and tells it apart
    /// from an ordinary same-named `path` dependency. Absent (false) in a
    /// lockfile written before this existed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_project: bool,
}

impl RprojLockTarget {
    /// Turn one solve into one lockfile target.
    pub fn from_solution(
        registry: &RPackageRegistry,
        solution: &HashMap<String, RegistryPackageVersion, rustc_hash::FxBuildHasher>,
    ) -> RprojLockTarget {
        let r_version = solution
            .get("R")
            .map(|r| r.version.to_string())
            .unwrap_or_default();
        let platform = registry.binary_target();
        let mut pkgs = vec![];
        for (k, v) in solution.iter() {
            // A local package -- a workspace member, or the synthetic solve
            // root -- has no artifact to install, and R and the base packages
            // come with R itself.
            if k == "R" || registry.is_local(k) || BASE_PKGS.contains(&k.as_str()) {
                continue;
            }
            let deps: Vec<String> = registry
                .get_dependency_summary(k, v)
                .unwrap()
                .into_iter()
                .filter(|dep| dep != "R" && !BASE_PKGS.contains(&dep.as_str()))
                .collect();

            // A git/GitHub/url-sourced package has no repository artifact at
            // all: record its `Remote*` provenance instead of a CRAN download
            // URL, and skip the source/binary-artifact bookkeeping below
            // entirely.
            if let Some(git) = registry.git_source(k, v) {
                let mut metadata: HashMap<String, String> = HashMap::new();
                metadata.insert(REMOTE_TYPE_FIELD.to_string(), git.remote_type.to_string());
                metadata.insert(REMOTE_URL_FIELD.to_string(), git.url.clone());
                if let Some(host) = &git.host {
                    metadata.insert(REMOTE_HOST_FIELD.to_string(), host.clone());
                }
                if let Some(repo) = &git.repo {
                    metadata.insert(REMOTE_REPO_FIELD.to_string(), repo.clone());
                }
                if let Some(username) = &git.username {
                    metadata.insert(REMOTE_USERNAME_FIELD.to_string(), username.clone());
                }
                if let Some(subdir) = &git.subdir {
                    metadata.insert(REMOTE_SUBDIR_FIELD.to_string(), subdir.clone());
                }
                if let Some(ref_) = &git.ref_ {
                    metadata.insert(REMOTE_REF_FIELD.to_string(), ref_.clone());
                }
                // A local source has no commit and no content hash: the path
                // it was installed from is its whole provenance.
                if !git.sha.is_empty() {
                    metadata.insert(REMOTE_SHA_FIELD.to_string(), git.sha.clone());
                }

                // Both are directories: a github tarball is unpacked, and a
                // git:: clone is a worktree checkout. `sources` still carries
                // the real download URL for the github case, so the existing
                // HTTP downloader can fetch it unchanged; `RemoteType` is what
                // tells `download_lockfile_packages` these need extra
                // handling instead of "download this URL to this file path".
                let (sources, target) = if git.remote_type == "github" {
                    let repo = git.repo.clone().unwrap_or_default();
                    (
                        vec![format!(
                            "https://codeload.github.com/{}/tar.gz/{}",
                            repo, git.sha
                        )],
                        format!("git/github/{}/{}", repo, git.sha),
                    )
                } else if git.remote_type == "url" {
                    (vec![git.url.clone()], format!("url/{}", git.sha))
                } else if git.remote_type == "local" {
                    // Nothing to download, and nothing in the cache: the
                    // installer reads `RemoteUrl` (the absolute path) instead
                    // of a `target` under the cache directory, see
                    // `lockfile_package_info`.
                    (vec![], String::new())
                } else {
                    (
                        vec![format!("git+{}#{}", git.url, git.sha)],
                        format!("git/git/{}", git.sha),
                    )
                };

                pkgs.push(RprojLockPackage {
                    package: k.to_string(),
                    version: v.version.to_string(),
                    binary: git.binary,
                    platform: if git.binary {
                        platform.clone().unwrap_or_else(|| "source".to_string())
                    } else {
                        "source".to_string()
                    },
                    dependencies: deps,
                    metadata,
                    sources,
                    target,
                    groups: vec![],
                    extra_groups: vec![],
                    is_project: false,
                });
                continue;
            }

            let binary = v.artifact.is_binary();
            // Provenance of the artifact, so that a lockfile install records the
            // same `RemoteHash` / `RemoteLinkingToHashes` a direct install does.
            // A binary knows what it was compiled against; a source build is
            // compiled against whatever the solve picked, so its provenance is
            // read off the solution.
            let mut metadata: HashMap<String, String> = HashMap::new();
            if let Some(sha) = registry.artifact_sha256(k, v) {
                metadata.insert(REMOTE_HASH_FIELD.to_string(), sha);
            }
            let linkingto = if binary {
                registry.artifact_linkingto(k, v)
            } else {
                registry
                    .linkingto_names(k, v)
                    .into_iter()
                    .filter_map(|dep| {
                        let dv = solution.get(&dep)?;
                        let sha = registry.artifact_sha256(&dep, dv)?;
                        Some((dep, dv.version.to_string(), sha))
                    })
                    .collect()
            };
            if !linkingto.is_empty() {
                metadata.insert(
                    REMOTE_LINKINGTO_FIELD.to_string(),
                    format_linkingto(&linkingto),
                );
            }
            // The index's URL is snapshot-pinned; the CRAN ones are guesses, and
            // there are two of them because a version that has been superseded
            // has moved into the archive.
            let filename = format!("{}_{}.tar.gz", k, v.version);
            let sources = match registry.artifact_url(k, v) {
                Some(url) => vec![url],
                None => vec![
                    format!("https://cloud.r-project.org/src/contrib/{}", filename),
                    format!(
                        "https://cloud.r-project.org/src/contrib/Archive/{}/{}",
                        k, filename
                    ),
                ],
            };
            // The cache file name has to tell two builds of one version apart,
            // and the repository path does not: several binaries share it.
            let key = artifact_cache_key(
                metadata.get(REMOTE_HASH_FIELD).map(|s| s.as_str()),
                if binary {
                    metadata.get(REMOTE_LINKINGTO_FIELD).map(|s| s.as_str())
                } else {
                    None
                },
            );
            let target = target_path(&sources[0], &format!("src/{}", filename), key.as_deref());
            pkgs.push(RprojLockPackage {
                package: k.to_string(),
                version: v.version.to_string(),
                binary,
                platform: if binary {
                    platform.clone().unwrap_or_else(|| "source".to_string())
                } else {
                    "source".to_string()
                },
                dependencies: deps,
                metadata,
                sources,
                target,
                groups: vec![],
                extra_groups: vec![],
                is_project: false,
            });
        }

        RprojLockTarget {
            r_version,
            platform: platform.unwrap_or_else(|| std::env::consts::ARCH.to_string()),
            direct_dependencies: vec![],
            packages: pkgs,
        }
    }
}

/// Just the `version` of a lockfile, to read it without the rest.
#[derive(Deserialize)]
struct RprojLockVersion {
    version: usize,
}

impl RprojLock {
    /// Fail on lockfile text that is not the version rig reads.
    ///
    /// Every field of a package entry is required, so a lockfile of another
    /// version usually fails to deserialize anyway, but with a serde message
    /// about a missing field that says nothing about what to do. The version
    /// is read on its own, and first, so that the message can.
    pub fn check_version(text: &str) -> Result<(), Box<dyn Error>> {
        let found: RprojLockVersion = toml::from_str(text)?;
        if found.version > RPROJ_LOCK_VERSION {
            bail!(
                "This {} is version {}, and this rig reads version {}. \
                 Update rig to use it.",
                RPROJ_LOCK_FILE,
                found.version,
                RPROJ_LOCK_VERSION
            );
        }
        if found.version < RPROJ_LOCK_VERSION {
            bail!(
                "This {} is version {}, written by an older rig, and this rig \
                 reads version {}. Run `rig proj lock` to write it again.",
                RPROJ_LOCK_FILE,
                found.version,
                RPROJ_LOCK_VERSION
            );
        }
        Ok(())
    }

    /// Render the lockfile as the TOML text of `rproj.lock`.
    ///
    /// A package's `metadata` map is a table, so the plain serializer writes it
    /// as a `[targets.packages.metadata]` section of its own, which pushes a
    /// handful of `Remote*` fields into a block as tall as the package entry
    /// itself. Writing it as an inline table keeps one package to one block,
    /// and its keys are sorted, because the map is a `HashMap` and would
    /// otherwise land in a different order in every rewrite.
    pub fn to_toml(&self) -> Result<String, Box<dyn Error>> {
        let mut doc: toml_edit::DocumentMut = toml::to_string_pretty(self)?.parse()?;
        let targets = doc
            .get_mut("targets")
            .and_then(|t| t.as_array_of_tables_mut());
        for target in targets.into_iter().flat_map(|ts| ts.iter_mut()) {
            let packages = target
                .get_mut("packages")
                .and_then(|p| p.as_array_of_tables_mut());
            for package in packages.into_iter().flat_map(|ps| ps.iter_mut()) {
                let Some(toml_edit::Item::Table(metadata)) = package.remove("metadata") else {
                    continue;
                };
                let mut metadata = metadata.into_inline_table();
                metadata.sort_values();
                // `into_inline_table()` keeps the decorations of the section
                // the table came from, i.e. the blank line before its header.
                metadata.decor_mut().clear();
                for (mut key, value) in metadata.iter_mut() {
                    key.leaf_decor_mut().clear();
                    value.decor_mut().clear();
                }
                package.insert("metadata", toml_edit::value(metadata));
            }
        }
        Ok(doc.to_string())
    }
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

    fn sample_package() -> RprojLockPackage {
        RprojLockPackage {
            package: "cli".to_string(),
            version: "3.6.0".to_string(),
            binary: true,
            platform: "aarch64-apple-darwin".to_string(),
            dependencies: vec!["rlang".to_string()],
            metadata: HashMap::from([("RemoteSha".to_string(), "abc123".to_string())]),
            sources: vec!["https://example.com/cli.tgz".to_string()],
            target: "cli.tgz".to_string(),
            groups: vec!["main".to_string()],
            extra_groups: vec![],
            is_project: false,
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
            Dependency::Detailed(Box::new(DepTable {
                git: Some("https://github.com/gaborcsardi/ts".to_string()),
                branch: Some("main".to_string()),
                ..Default::default()
            })),
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
            "extra".to_string(),
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
                direct_dependencies: vec![],
                packages: vec![sample_package()],
            }],
        };
        let text = lock.to_toml().unwrap();
        let parsed: RprojLock = toml::from_str(&text).unwrap();
        assert_eq!(parsed.version, RPROJ_LOCK_VERSION);
        assert_eq!(parsed.targets.len(), 1);
        assert_eq!(parsed.targets[0].r_version, "4.6");
        assert_eq!(parsed.targets[0].packages[0].package, "cli");
        assert_eq!(
            parsed.targets[0].packages[0].metadata.get("RemoteSha"),
            Some(&"abc123".to_string())
        );
    }

    #[test]
    fn roundtrips_several_targets_through_toml() {
        let mut linux_package = sample_package();
        linux_package.platform = "x86_64-pc-linux-gnu".to_string();

        let lock = RprojLock {
            version: RPROJ_LOCK_VERSION,
            targets: vec![
                RprojLockTarget {
                    r_version: "4.5".to_string(),
                    platform: "x86_64-pc-linux-gnu".to_string(),
                    direct_dependencies: vec![],
                    packages: vec![linux_package],
                },
                RprojLockTarget {
                    r_version: "4.6".to_string(),
                    platform: "aarch64-apple-darwin".to_string(),
                    direct_dependencies: vec![],
                    packages: vec![sample_package()],
                },
            ],
        };
        let text = lock.to_toml().unwrap();
        let parsed: RprojLock = toml::from_str(&text).unwrap();
        assert_eq!(parsed.targets.len(), 2);
        assert_eq!(parsed.targets[0].r_version, "4.5");
        assert_eq!(parsed.targets[0].platform, "x86_64-pc-linux-gnu");
        assert_eq!(parsed.targets[1].r_version, "4.6");
        assert_eq!(parsed.targets[1].platform, "aarch64-apple-darwin");
    }

    #[test]
    fn package_metadata_is_written_as_a_sorted_inline_table() {
        let mut package = sample_package();
        package.metadata = HashMap::from([
            ("RemoteType".to_string(), "github".to_string()),
            ("RemoteSha".to_string(), "abc123".to_string()),
            ("RemoteRepo".to_string(), "cli".to_string()),
        ]);
        let lock = RprojLock {
            version: RPROJ_LOCK_VERSION,
            targets: vec![RprojLockTarget {
                r_version: "4.6".to_string(),
                platform: "aarch64-apple-darwin".to_string(),
                direct_dependencies: vec![],
                packages: vec![package],
            }],
        };
        let text = lock.to_toml().unwrap();
        assert!(text.contains(
            "metadata = { RemoteRepo = \"cli\", RemoteSha = \"abc123\", RemoteType = \"github\" }\n"
        ));
        assert!(!text.contains("[targets.packages.metadata]"));

        let parsed: RprojLock = toml::from_str(&text).unwrap();
        assert_eq!(
            parsed.targets[0].packages[0].metadata.get("RemoteType"),
            Some(&"github".to_string())
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
            Some(&Dependency::Detailed(Box::new(DepTable {
                version: Some("*".to_string()),
                attach: Some(true),
                ..Default::default()
            })))
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
                .get("dev")
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
            Dependency::Detailed(Box::new(DepTable {
                version: Some("*".to_string()),
                attach: Some(true),
                ..Default::default()
            })),
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
            Dependency::Detailed(Box::new(DepTable {
                git: Some("https://github.com/gaborcsardi/ts".to_string()),
                ..Default::default()
            })),
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
            "dev".to_string(),
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
        // an unknown group has no DESCRIPTION dependency type, but is still
        // solved, as a `Suggests`
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
            converted(&deps, "pkgdown"),
            Some((&[RDepType::Suggests][..], vec![]))
        );
        assert_eq!(
            converted(&deps, "otherpkg"),
            Some((&[RDepType::Enhances][..], vec![]))
        );
    }

    #[test]
    fn to_dep_version_specs_optional_dependencies_are_soft_and_need_dev() {
        let mut m = Rproj::minimal("mypkg");
        m.optional_dependencies.insert(
            "viz".to_string(),
            BTreeMap::from([("ggplot2".to_string(), dep(">= 3.4"))]),
        );

        let deps = m.to_dep_version_specs(false).unwrap();
        assert_eq!(converted(&deps, "ggplot2"), None);

        let deps = m.to_dep_version_specs(true).unwrap();
        assert_eq!(
            converted(&deps, "ggplot2"),
            Some((&[RDepType::Suggests][..], vec![">= 3.4".to_string()]))
        );
    }

    #[test]
    fn dependency_group_roots_includes_optional_dependency_extras() {
        let mut m = Rproj::minimal("mypkg");
        m.dependencies.insert("cli".to_string(), dep("*"));
        m.optional_dependencies.insert(
            "viz".to_string(),
            BTreeMap::from([("ggplot2".to_string(), dep("*"))]),
        );

        let roots = m.dependency_group_roots().unwrap();
        assert!(roots.get("main").unwrap().contains(&"cli".to_string()));
        assert_eq!(roots.get("viz"), Some(&vec!["ggplot2".to_string()]));
    }

    #[test]
    fn dependency_group_roots_includes_arbitrary_dependency_groups() {
        let mut m = Rproj::minimal("mypkg");
        m.dependency_groups.insert(
            "docs".to_string(),
            Group {
                include_groups: vec![],
                dependencies: BTreeMap::from([("pkgdown".to_string(), dep("*"))]),
            },
        );

        let roots = m.dependency_group_roots().unwrap();
        assert_eq!(roots.get("docs"), Some(&vec!["pkgdown".to_string()]));
    }

    #[test]
    fn include_groups_pulls_in_the_included_groups_packages() {
        let mut m = Rproj::minimal("mypkg");
        m.dependency_groups.insert(
            "test".to_string(),
            Group {
                include_groups: vec![],
                dependencies: BTreeMap::from([("testthat".to_string(), dep("*"))]),
            },
        );
        m.dependency_groups.insert(
            "dev".to_string(),
            Group {
                include_groups: vec!["test".to_string()],
                dependencies: BTreeMap::from([("devtools".to_string(), dep("*"))]),
            },
        );

        let roots = m.dependency_group_roots().unwrap();
        let mut dev = roots.get("dev").unwrap().clone();
        dev.sort();
        assert_eq!(dev, vec!["devtools".to_string(), "testthat".to_string()]);
    }

    #[test]
    fn include_groups_is_recursive() {
        let mut m = Rproj::minimal("mypkg");
        m.dependency_groups.insert(
            "c".to_string(),
            Group {
                include_groups: vec![],
                dependencies: BTreeMap::from([("pkgc".to_string(), dep("*"))]),
            },
        );
        m.dependency_groups.insert(
            "b".to_string(),
            Group {
                include_groups: vec!["c".to_string()],
                dependencies: BTreeMap::new(),
            },
        );
        m.dependency_groups.insert(
            "a".to_string(),
            Group {
                include_groups: vec!["b".to_string()],
                dependencies: BTreeMap::new(),
            },
        );

        let roots = m.dependency_group_roots().unwrap();
        assert_eq!(roots.get("a"), Some(&vec!["pkgc".to_string()]));
    }

    #[test]
    fn include_groups_detects_a_cycle() {
        let mut m = Rproj::minimal("mypkg");
        m.dependency_groups.insert(
            "a".to_string(),
            Group {
                include_groups: vec!["b".to_string()],
                dependencies: BTreeMap::new(),
            },
        );
        m.dependency_groups.insert(
            "b".to_string(),
            Group {
                include_groups: vec!["a".to_string()],
                dependencies: BTreeMap::new(),
            },
        );

        assert!(m.dependency_group_roots().is_err());
    }

    #[test]
    fn include_groups_errors_on_an_unknown_group_name() {
        let mut m = Rproj::minimal("mypkg");
        m.dependency_groups.insert(
            "dev".to_string(),
            Group {
                include_groups: vec!["nope".to_string()],
                dependencies: BTreeMap::new(),
            },
        );

        assert!(m.dependency_group_roots().is_err());
    }

    #[test]
    fn to_dep_version_specs_include_groups_are_soft_and_need_dev() {
        let mut m = Rproj::minimal("mypkg");
        m.dependency_groups.insert(
            "test".to_string(),
            Group {
                include_groups: vec![],
                dependencies: BTreeMap::from([("testthat".to_string(), dep("*"))]),
            },
        );
        m.dependency_groups.insert(
            "dev".to_string(),
            Group {
                include_groups: vec!["test".to_string()],
                dependencies: BTreeMap::new(),
            },
        );

        let nodev = m.to_dep_version_specs(false).unwrap();
        assert!(converted(&nodev, "testthat").is_none());

        let dev = m.to_dep_version_specs(true).unwrap();
        assert_eq!(
            converted(&dev, "testthat"),
            Some((&[RDepType::Suggests][..], vec![]))
        );
    }

    #[test]
    fn to_dep_version_specs_keeps_a_soft_dep_that_is_also_hard() {
        let mut m = Rproj::minimal("mypkg");
        m.dependencies.insert("cli".to_string(), dep(">= 3.6.5"));
        m.dependency_groups.insert(
            "dev".to_string(),
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
    fn add_dependency_dev_adds_to_the_dev_group() {
        let mut m = Rproj::minimal("mypkg");
        assert_eq!(m.add_dependency("testthat", ">= 3.0", true), None);
        assert!(!m.dependencies.contains_key("testthat"));
        assert_eq!(
            m.dependency_groups
                .get("dev")
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
            Dependency::Detailed(Box::new(DepTable {
                git: Some("https://github.com/gaborcsardi/ts".to_string()),
                attach: Some(true),
                ..Default::default()
            })),
        );
        // The entry had no version requirement, so the previous one reads as
        // "any version".
        assert_eq!(
            m.add_dependency("ts", ">= 1.0", false),
            Some("*".to_string())
        );
        assert_eq!(
            m.dependencies.get("ts"),
            Some(&Dependency::Detailed(Box::new(DepTable {
                version: Some(">= 1.0".to_string()),
                git: Some("https://github.com/gaborcsardi/ts".to_string()),
                attach: Some(true),
                ..Default::default()
            })))
        );
    }

    /// A `{ workspace = true }` entry, as a member manifest spells it.
    fn inherited() -> Dependency {
        Dependency::Detailed(Box::new(DepTable {
            workspace: Some(true),
            ..Default::default()
        }))
    }

    /// A workspace root's `[workspace.dependencies]` with one entry.
    fn workspace_with(name: &str, dep: Dependency) -> Workspace {
        let mut dependencies = BTreeMap::new();
        dependencies.insert(name.to_string(), dep);
        Workspace {
            dependencies,
            ..Default::default()
        }
    }

    #[test]
    fn inherit_workspace_deps_takes_the_roots_constraint() {
        let ws = workspace_with("cli", dep(">= 3.6.5"));
        let mut m = Rproj::minimal("member");
        m.dependencies.insert("cli".to_string(), inherited());
        m.inherit_workspace_deps(&ws, Path::new("a/rproj.toml"))
            .unwrap();
        let deps = m.to_dep_version_specs(false).unwrap();
        assert_eq!(
            converted(&deps, "cli"),
            Some((&[RDepType::Imports][..], vec![">= 3.6.5".to_string()]))
        );
    }

    #[test]
    fn inherit_workspace_deps_takes_the_roots_source() {
        let ws = workspace_with(
            "ts",
            Dependency::Detailed(Box::new(DepTable {
                git: Some("https://github.com/gaborcsardi/ts".to_string()),
                ..Default::default()
            })),
        );
        let mut m = Rproj::minimal("member");
        m.dependencies.insert("ts".to_string(), inherited());
        m.inherit_workspace_deps(&ws, Path::new("a/rproj.toml"))
            .unwrap();
        let Some(Dependency::Detailed(t)) = m.dependencies.get("ts") else {
            panic!("not a table: {:?}", m.dependencies.get("ts"));
        };
        assert_eq!(t.git.as_deref(), Some("https://github.com/gaborcsardi/ts"));
        assert_eq!(t.workspace, None);
    }

    #[test]
    fn inherit_workspace_deps_keeps_the_members_own_attach_flag() {
        let ws = workspace_with("crayon", dep(">= 1.5"));
        let mut m = Rproj::minimal("member");
        m.dependencies.insert(
            "crayon".to_string(),
            Dependency::Detailed(Box::new(DepTable {
                workspace: Some(true),
                attach: Some(true),
                ..Default::default()
            })),
        );
        m.inherit_workspace_deps(&ws, Path::new("a/rproj.toml"))
            .unwrap();
        let deps = m.to_dep_version_specs(false).unwrap();
        assert_eq!(
            converted(&deps, "crayon"),
            Some((&[RDepType::Depends][..], vec![">= 1.5".to_string()]))
        );
    }

    #[test]
    fn inherit_workspace_deps_reaches_every_dependency_table() {
        let mut ws = workspace_with("cli", dep(">= 3.6.5"));
        ws.dependencies.insert("cpp11".to_string(), dep(">= 0.4"));
        ws.dependencies
            .insert("testthat".to_string(), dep(">= 3.0"));
        ws.dependencies.insert("curl".to_string(), dep(">= 5.0"));

        let mut m = Rproj::minimal("member");
        m.linking_dependencies
            .insert("cpp11".to_string(), inherited());
        m.dependency_groups.insert(
            "dev".to_string(),
            Group {
                dependencies: BTreeMap::from([("testthat".to_string(), inherited())]),
                ..Default::default()
            },
        );
        m.optional_dependencies.insert(
            "web".to_string(),
            BTreeMap::from([("curl".to_string(), inherited())]),
        );

        m.inherit_workspace_deps(&ws, Path::new("a/rproj.toml"))
            .unwrap();

        let deps = m.to_dep_version_specs(true).unwrap();
        assert_eq!(
            converted(&deps, "cpp11"),
            Some((&[RDepType::LinkingTo][..], vec![">= 0.4".to_string()]))
        );
        assert_eq!(
            converted(&deps, "testthat"),
            Some((&[RDepType::Suggests][..], vec![">= 3.0".to_string()]))
        );
        // Optional dependencies have no DESCRIPTION type and so are not
        // solved, but the entry still has to be resolved, not left inherited.
        let Some(Dependency::Detailed(t)) = m.optional_dependencies.get("web").unwrap().get("curl")
        else {
            panic!("not a table");
        };
        assert_eq!(t.version.as_deref(), Some(">= 5.0"));
    }

    #[test]
    fn inherit_workspace_deps_without_a_root_entry_is_an_error() {
        let ws = workspace_with("cli", dep(">= 3.6.5"));
        let mut m = Rproj::minimal("member");
        m.dependencies.insert("dplyr".to_string(), inherited());
        let err = m
            .inherit_workspace_deps(&ws, Path::new("packages/a/rproj.toml"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("packages/a/rproj.toml"), "{}", err);
        assert!(err.contains("[workspace.dependencies]"), "{}", err);
        assert!(err.contains("dplyr"), "{}", err);
    }

    #[test]
    fn workspace_dependencies_nobody_inherits_are_dormant() {
        let ws = workspace_with("cli", dep(">= 3.6.5"));
        let mut m = Rproj::minimal("member");
        m.inherit_workspace_deps(&ws, Path::new("a/rproj.toml"))
            .unwrap();
        let deps = m.to_dep_version_specs(true).unwrap();
        assert_eq!(converted(&deps, "cli"), None);
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
            .get("dev")
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
    fn doc_set_dependency_preserves_comments_and_unmodeled_table() {
        let text = r#"[project]
name = "mypkg"
version = "0.1.0"

[dependencies]
# a comment about R
R = ">= 4.1"

[tool.foo]
bar = 1
"#;
        let mut doc: toml_edit::DocumentMut = text.parse().unwrap();
        Rproj::doc_set_dependency(&mut doc, &["dependencies"], "dplyr", &dep("^1.1.0")).unwrap();
        let out = doc.to_string();
        assert!(out.contains("# a comment about R"), "{}", out);
        assert!(out.contains("[tool.foo]"), "{}", out);
        assert!(out.contains("bar = 1"), "{}", out);
        assert!(out.contains("dplyr = \"^1.1.0\""), "{}", out);
    }

    #[test]
    fn doc_set_dependency_creates_missing_dependencies_table() {
        let text = "[project]\nname = \"mypkg\"\nversion = \"0.1.0\"\n";
        let mut doc: toml_edit::DocumentMut = text.parse().unwrap();
        Rproj::doc_set_dependency(&mut doc, &["dependencies"], "R", &dep(">= 4.1")).unwrap();
        let out = doc.to_string();
        assert!(out.contains("[dependencies]"), "{}", out);
        assert!(out.contains("R = \">= 4.1\""), "{}", out);
    }

    #[test]
    fn doc_set_dependency_remote_renders_inline_table() {
        let text = "[project]\nname = \"mypkg\"\nversion = \"0.1.0\"\n\n[dependencies]\n";
        let mut doc: toml_edit::DocumentMut = text.parse().unwrap();
        let value = Dependency::Detailed(Box::new(DepTable {
            git: Some("https://github.com/gaborcsardi/ts".to_string()),
            ..Default::default()
        }));
        Rproj::doc_set_dependency(&mut doc, &["dependencies"], "ts", &value).unwrap();
        let out = doc.to_string();
        assert!(
            out.contains("ts = { git = \"https://github.com/gaborcsardi/ts\" }"),
            "{}",
            out
        );
        assert!(!out.contains("[dependencies.ts]"), "{}", out);
    }

    #[test]
    fn doc_set_dependency_creates_missing_dependency_group() {
        let text = "[project]\nname = \"mypkg\"\nversion = \"0.1.0\"\n";
        let mut doc: toml_edit::DocumentMut = text.parse().unwrap();
        Rproj::doc_set_dependency(
            &mut doc,
            &["dependency-groups", "dev"],
            "testthat",
            &dep(">= 3.0"),
        )
        .unwrap();
        let out = doc.to_string();
        assert!(out.contains("[dependency-groups.dev]"), "{}", out);
        assert!(!out.contains("[dependency-groups]\n"), "{}", out);
        assert!(out.contains("testthat = \">= 3.0\""), "{}", out);
    }

    #[test]
    fn doc_remove_dependency_preserves_comments_and_unmodeled_table() {
        let text = r#"[project]
name = "mypkg"
version = "0.1.0"

[dependencies]
# about dplyr
dplyr = "^1.1.0"
# about R
R = ">= 4.1"

[tool.foo]
bar = 1
"#;
        let mut doc: toml_edit::DocumentMut = text.parse().unwrap();
        assert!(Rproj::doc_remove_dependency(&mut doc, "dplyr"));
        let out = doc.to_string();
        assert!(!out.contains("dplyr"), "{}", out);
        assert!(!out.contains("# about dplyr"), "{}", out);
        assert!(out.contains("# about R"), "{}", out);
        assert!(out.contains("R = \">= 4.1\""), "{}", out);
        assert!(out.contains("[tool.foo]"), "{}", out);
        assert!(out.contains("bar = 1"), "{}", out);
    }

    #[test]
    fn doc_remove_dependency_finds_dependency_in_linking_and_group_tables() {
        let text = r#"[project]
name = "mypkg"
version = "0.1.0"

[linking-dependencies]
Rcpp = ">= 1.0"

[dependency-groups.website]
pkgdown = "*"
"#;
        let mut doc: toml_edit::DocumentMut = text.parse().unwrap();
        assert!(Rproj::doc_remove_dependency(&mut doc, "Rcpp"));
        assert!(Rproj::doc_remove_dependency(&mut doc, "pkgdown"));
        let out = doc.to_string();
        assert!(!out.contains("Rcpp"), "{}", out);
        assert!(!out.contains("pkgdown"), "{}", out);
        assert!(out.contains("[project]"), "{}", out);
    }

    #[test]
    fn doc_set_config_preserves_comments_and_unmodeled_table() {
        let text = r#"[project]
name = "mypkg"
version = "0.1.0"

[config.testthat]
# existing setting
parallel = true

[config.other]
foo = "bar"
"#;
        let mut doc: toml_edit::DocumentMut = text.parse().unwrap();
        Rproj::doc_set_config(&mut doc, "testthat", "edition", &toml::Value::Integer(3));
        let out = doc.to_string();
        assert!(out.contains("# existing setting"), "{}", out);
        assert!(out.contains("parallel = true"), "{}", out);
        assert!(out.contains("edition = 3"), "{}", out);
        assert!(out.contains("[config.other]"), "{}", out);
        assert!(out.contains("foo = \"bar\""), "{}", out);
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
    fn authors_r_parses_a_positional_email() {
        // `middle` is left out, so the fourth argument is `email`.
        let authors = Author::from_authors_r(
            "person(\"Gábor\", \"Csárdi\", , \"gabor@posit.co\", role = c(\"aut\", \"cre\"))",
        );
        assert_eq!(
            authors,
            vec![Author {
                name: "Gábor Csárdi".to_string(),
                email: Some("gabor@posit.co".to_string()),
                roles: vec!["aut".to_string(), "cre".to_string()],
                orcid: None,
                ror: None,
            }]
        );
    }

    #[test]
    fn authors_r_parses_all_arguments_positionally() {
        let authors = Author::from_authors_r(
            "person(\"Jane\", \"Doe\", \"Q\", \"jane@x.com\", \"cre\", \
             c(ORCID = \"0000-0001-7098-9676\"))",
        );
        assert_eq!(
            authors,
            vec![Author {
                name: "Jane Q Doe".to_string(),
                email: Some("jane@x.com".to_string()),
                roles: vec!["cre".to_string()],
                orcid: Some("0000-0001-7098-9676".to_string()),
                ror: None,
            }]
        );
    }

    #[test]
    fn authors_r_matches_positional_args_around_named_ones() {
        // `family` is named, so the second positional argument is `middle`,
        // and the third is `email`.
        let authors = Author::from_authors_r(
            "person(\"Jane\", family = \"Doe\", \"Q\", \"jane@x.com\", role = \"aut\")",
        );
        assert_eq!(
            authors,
            vec![Author {
                name: "Jane Q Doe".to_string(),
                email: Some("jane@x.com".to_string()),
                roles: vec!["aut".to_string()],
                orcid: None,
                ror: None,
            }]
        );
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
        assert!(desc.contains("Depends:\n    R (>= 4.1)\n"));
        assert!(desc.contains("Imports:\n    dplyr (>= 1.1.0),\n    rlang (>= 1.0)\n"));
        assert!(desc.contains("Suggests:\n    testthat (>= 3.0)\n"));
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
        assert!(desc.contains("Depends:\n    R (>= 4.1)\n"));
    }

    #[test]
    fn to_description_omits_type_for_package_projects() {
        let mut m = Rproj::minimal("mypkg");
        m.project.type_ = Some("package".to_string());
        let (desc, _) = m.to_description().unwrap();
        assert!(!desc.contains("Type:"));
    }

    #[test]
    fn to_description_wraps_title_at_75_columns() {
        let mut m = Rproj::minimal("mypkg");
        let words: Vec<String> = (0..40).map(|i| format!("word{}", i)).collect();
        m.project.title = Some(words.join(" "));
        let (desc, _) = m.to_description().unwrap();

        for line in desc.lines() {
            assert!(line.len() <= 75, "line too long: {:?}", line);
        }
        // The prefix counts towards the first line's width, the 4-space DCF
        // indent towards the continuation lines'.
        let prose: Vec<&str> = desc
            .lines()
            .skip_while(|l| !l.starts_with("Title:"))
            .take_while(|l| l.starts_with("Title:") || l.starts_with("    "))
            .collect();
        assert!(prose.len() > 1);
        assert!(prose[0].starts_with("Title: word0 "));
        // Reflowing the folded field gives the value back unchanged.
        let joined = prose.join(" ");
        assert_eq!(
            crate::textfmt::reflow(joined.trim_start_matches("Title:")),
            words.join(" ")
        );
    }

    #[test]
    fn to_description_keeps_description_line_breaks_literal() {
        let mut m = Rproj::minimal("mypkg");
        m.project.description = Some("First paragraph.\n\nSecond paragraph.".to_string());
        let (desc, _) = m.to_description().unwrap();
        assert!(desc.contains("Description: First paragraph.\n    .\n    Second paragraph.\n"));
    }

    #[test]
    fn to_toml_writes_multiline_description_as_literal_string() {
        let mut m = Rproj::minimal("mypkg");
        m.project.description = Some("First paragraph.\n\nSecond paragraph.".to_string());
        let toml_text = m.to_toml().unwrap();
        assert!(
            toml_text.contains("'''\nFirst paragraph.\n\nSecond paragraph.'''"),
            "{}",
            toml_text
        );

        let round_tripped: Rproj = toml::from_str(&toml_text).unwrap();
        assert_eq!(round_tripped.project.description, m.project.description);
    }

    #[test]
    fn to_toml_leaves_single_line_description_as_a_plain_string() {
        let mut m = Rproj::minimal("mypkg");
        m.project.description = Some("Does things.".to_string());
        let toml_text = m.to_toml().unwrap();
        assert!(toml_text.contains("description = \"Does things.\""));
    }

    fn author(name: &str, roles: &[&str]) -> Author {
        Author {
            name: name.to_string(),
            email: None,
            roles: roles.iter().map(|r| r.to_string()).collect(),
            orcid: None,
            ror: None,
        }
    }

    #[test]
    fn to_description_writes_one_author_as_a_bare_call() {
        let mut m = Rproj::minimal("mypkg");
        m.project.authors.push(author("Jane Doe", &["aut", "cre"]));
        let (desc, _) = m.to_description().unwrap();
        assert!(desc.contains("Authors@R: person(\"Jane Doe\", role = c(\"aut\", \"cre\"))\n"));
    }

    #[test]
    fn to_description_writes_multiple_authors_one_per_line() {
        let mut m = Rproj::minimal("mypkg");
        m.project.authors.push(author("Jane Doe", &["aut", "cre"]));
        m.project.authors.push(author("Rich Contributor", &["ctb"]));
        let (desc, _) = m.to_description().unwrap();
        assert!(desc.contains(
            "Authors@R: c(\n    \
             person(\"Jane Doe\", role = c(\"aut\", \"cre\")),\n    \
             person(\"Rich Contributor\", role = c(\"ctb\"))\n  \
             )\n"
        ));
    }

    #[test]
    fn to_description_wraps_a_long_person_call_across_lines() {
        let mut m = Rproj::minimal("mypkg");
        m.project.authors.push(Author {
            name: "Salim Brüggemann".to_string(),
            email: Some("salim-b@pm.me".to_string()),
            roles: vec!["ctb".to_string()],
            orcid: Some("0000-0002-5329-5987".to_string()),
            ror: None,
        });
        m.project.authors.push(author("Rich Contributor", &["ctb"]));
        let (desc, _) = m.to_description().unwrap();

        // The first author's call is too long for one line, so it wraps,
        // aligned under its own opening paren; the second author still
        // fits on a single line.
        assert!(desc.contains(
            "Authors@R: c(\n    \
             person(\"Salim Brüggemann\", email = \"salim-b@pm.me\", role = c(\"ctb\"),\n           \
             comment = c(ORCID = \"0000-0002-5329-5987\")),\n    \
             person(\"Rich Contributor\", role = c(\"ctb\"))\n  \
             )\n"
        ));
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
            Some(&Dependency::Detailed(Box::new(DepTable {
                git: Some("https://github.com/tidyverse/tidytemplate.git".to_string()),
                ref_: Some("tidyverse/tidytemplate".to_string()),
                ..Default::default()
            })))
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
        // `Config/Needs/dev` has nowhere else to go, so it lands in the
        // group `Suggests` is imported into, and exports as `Suggests`.
        let mut m = Rproj::minimal("mypkg");
        m.add_dependency("testthat", ">= 3.0", true);
        m.merge_config_needs(&needs(&[("dev", "mockery")]));

        let dev = &m.dependency_groups.get("dev").unwrap().dependencies;
        assert_eq!(dev.get("testthat"), Some(&dep(">= 3.0")));
        assert_eq!(dev.get("mockery"), Some(&dep("*")));

        let (desc, _) = m.to_description().unwrap();
        assert!(desc.contains("Suggests:\n    mockery,\n    testthat (>= 3.0)\n"));
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
        assert!(desc.contains("Suggests:\n    testthat\n"));
        assert!(desc.contains("Config/Needs/coverage:\n    covr\n"));
        assert!(desc.contains("Config/Needs/website:\n    pkgdown,\n    tidyverse/tidytemplate\n"));
        // Dependency fields come first, `Config/Needs/*` after them.
        assert!(desc.find("Suggests:").unwrap() < desc.find("Config/Needs/").unwrap());
    }

    #[test]
    fn to_description_writes_remotes_for_git_sourced_dependencies() {
        let mut m = Rproj::minimal("mypkg");
        m.add_remote_dependency(
            "tidytemplate",
            DepTable {
                git: Some("https://github.com/tidyverse/tidytemplate".to_string()),
                branch: Some("main".to_string()),
                ..Default::default()
            },
            false,
        );
        m.add_remote_dependency(
            "jsonlite",
            DepTable {
                git: Some("https://github.com/jeroen/jsonlite".to_string()),
                rev: Some("v1.8.0".to_string()),
                ..Default::default()
            },
            true,
        );

        let (desc, dropped) = m.to_description().unwrap();
        assert!(dropped.is_empty());
        assert!(desc.contains("Imports:\n    tidytemplate\n"));
        assert!(desc.contains("Suggests:\n    jsonlite\n"));
        // Entries are sorted by package name, rebuilt as pak references.
        assert!(desc
            .contains("Remotes:\n    jeroen/jsonlite@v1.8.0,\n    tidyverse/tidytemplate@main\n"));
        // `Remotes:` comes after the dependency fields.
        assert!(desc.find("Suggests:").unwrap() < desc.find("Remotes:").unwrap());
    }

    #[test]
    fn to_description_writes_config_needs_optional_for_git_sourced_optional_dependencies() {
        let mut m = Rproj::minimal("mypkg");
        m.optional_dependencies.insert(
            "viz".to_string(),
            BTreeMap::from([(
                "tidytemplate".to_string(),
                Dependency::Detailed(Box::new(DepTable {
                    git: Some("https://github.com/tidyverse/tidytemplate".to_string()),
                    branch: Some("main".to_string()),
                    ..Default::default()
                })),
            )]),
        );

        let (desc, dropped) = m.to_description().unwrap();
        assert!(dropped.is_empty());
        // Still `Suggests`-listed, so `R CMD check` allows using it
        // conditionally, but its pak reference lives only in
        // `Config/Needs/Optional/viz` now, not also duplicated into
        // `Remotes:`.
        assert!(desc.contains("Suggests:\n    tidytemplate\n"));
        assert!(!desc.contains("Remotes:"));
        assert!(desc.contains("Config/Needs/Optional/viz:\n    tidyverse/tidytemplate@main\n"));
    }

    #[test]
    fn to_description_writes_optional_dependencies_as_config_needs_optional() {
        let mut m = Rproj::minimal("mypkg");
        m.optional_dependencies.insert(
            "viz".to_string(),
            BTreeMap::from([
                ("ggplot2".to_string(), dep(">= 3.4")),
                ("plotly".to_string(), dep("*")),
            ]),
        );

        let (desc, dropped) = m.to_description().unwrap();
        assert!(dropped.is_empty());
        assert!(desc.contains("Suggests:\n    ggplot2 (>= 3.4),\n    plotly\n"));
        assert!(desc.contains("Config/Needs/Optional/viz:\n    ggplot2 (>= 3.4),\n    plotly\n"));
    }

    #[test]
    fn merge_optional_dependencies_creates_a_group_per_field() {
        let mut m = Rproj::minimal("mypkg");
        m.merge_optional_dependencies(&needs(&[
            ("viz", "ggplot2, tidyverse/tidytemplate"),
            ("docs", "pkgdown (>= 2.0)"),
        ]));

        let viz = m.optional_dependencies.get("viz").unwrap();
        assert_eq!(viz.get("ggplot2"), Some(&dep("*")));
        assert_eq!(
            viz.get("tidytemplate"),
            Some(&Dependency::Detailed(Box::new(DepTable {
                git: Some("https://github.com/tidyverse/tidytemplate.git".to_string()),
                ref_: Some("tidyverse/tidytemplate".to_string()),
                ..Default::default()
            })))
        );
        assert_eq!(
            m.optional_dependencies.get("docs").unwrap().get("pkgdown"),
            Some(&dep(">= 2.0"))
        );
    }

    #[test]
    fn merge_optional_dependencies_keeps_an_empty_field_as_an_empty_group() {
        let mut m = Rproj::minimal("mypkg");
        m.merge_optional_dependencies(&needs(&[("viz", "")]));
        assert!(m.optional_dependencies.get("viz").unwrap().is_empty());

        let (desc, _) = m.to_description().unwrap();
        assert!(desc.contains("Config/Needs/Optional/viz:\n"));
    }

    #[test]
    fn optional_dependencies_roundtrip_through_description() {
        let mut m = Rproj::minimal("mypkg");
        m.merge_config_needs(&needs(&[("dev", "mockery")]));
        m.merge_optional_dependencies(&needs(&[(
            "viz",
            "ggplot2 (>= 3.4), tidyverse/tidytemplate",
        )]));

        let (desc, _) = m.to_description().unwrap();
        let paragraph =
            crate::proj::parse_description_paragraph(std::io::Cursor::new(desc)).unwrap();
        let pkg = DcfPackage::from_dcf_paragraph(&paragraph).unwrap();

        let mut m2 = Rproj::minimal("mypkg");
        m2.merge_description(&pkg);
        let optional_needs: Vec<(String, String)> = paragraph
            .iter()
            .filter_map(|(key, value)| {
                key.strip_prefix("Config/Needs/Optional/")
                    .map(|group| (group.to_string(), value.to_string()))
            })
            .collect();
        m2.merge_optional_dependencies(&optional_needs);

        assert_eq!(m2.optional_dependencies, m.optional_dependencies);
        // `ggplot2`/`tidytemplate` are `Suggests`-listed too, so they also
        // land in `dependency_groups["dev"]`, alongside the unrelated
        // `mockery` -- no dedup against `optional_dependencies`.
        let dev = &m2.dependency_groups.get("dev").unwrap().dependencies;
        assert!(dev.contains_key("mockery"));
        assert!(dev.contains_key("ggplot2"));
        assert!(dev.contains_key("tidytemplate"));
    }

    #[test]
    fn to_description_writes_remotes_verbatim_for_gitlab_sourced_dependencies() {
        // `dep_table_from_remote` (the only real producer of these tables)
        // always fills in `ref_` with the original reference text; a plain
        // git URL alone cannot always be reconstructed back into gitlab::
        // syntax (or preserve a subdir, which pak's bare `git::` syntax has
        // no field for), so `dep_table_to_pak_ref` must prefer `ref_`.
        let mut m = Rproj::minimal("mypkg");
        m.add_remote_dependency(
            "pkg",
            DepTable {
                git: Some("https://gitlab.com/group/subgroup/pkg.git".to_string()),
                rev: Some("main".to_string()),
                subdir: Some("pkg".to_string()),
                ref_: Some("gitlab::group/subgroup/pkg/-/pkg@main".to_string()),
                ..Default::default()
            },
            false,
        );

        let (desc, dropped) = m.to_description().unwrap();
        assert!(dropped.is_empty());
        assert!(desc.contains("Remotes:\n    gitlab::group/subgroup/pkg/-/pkg@main\n"));
    }

    #[test]
    fn to_description_writes_remotes_verbatim_for_self_hosted_gitlab_dependencies() {
        let mut m = Rproj::minimal("mypkg");
        m.add_remote_dependency(
            "pkg",
            DepTable {
                git: Some("https://gitlab.example.com/group/pkg.git".to_string()),
                ref_: Some("gitlab::https://gitlab.example.com/group/pkg".to_string()),
                ..Default::default()
            },
            false,
        );

        let (desc, _) = m.to_description().unwrap();
        assert!(desc.contains("Remotes:\n    gitlab::https://gitlab.example.com/group/pkg\n"));
    }

    #[test]
    fn to_description_writes_remotes_for_url_sourced_dependencies() {
        let mut m = Rproj::minimal("mypkg");
        m.add_remote_dependency(
            "otherpkg",
            DepTable {
                url: Some("https://example.com/otherpkg_1.0.0.tar.gz".to_string()),
                ..Default::default()
            },
            false,
        );

        let (desc, dropped) = m.to_description().unwrap();
        assert!(dropped.is_empty());
        assert!(desc.contains("Imports:\n    otherpkg\n"));
        assert!(desc
            .contains("Remotes:\n    otherpkg=url::https://example.com/otherpkg_1.0.0.tar.gz\n"));
    }

    #[test]
    fn to_description_omits_remotes_for_config_needs_git_dependencies() {
        let mut m = Rproj::minimal("mypkg");
        m.merge_config_needs(&needs(&[("website", "tidyverse/tidytemplate")]));

        let (desc, _) = m.to_description().unwrap();
        // The `Config/Needs/website` entry already carries the pak
        // reference itself; it must not also produce a `Remotes:` field.
        assert!(!desc.contains("Remotes:"));
        assert!(desc.contains("Config/Needs/website:\n    tidyverse/tidytemplate\n"));
    }

    #[test]
    fn config_needs_roundtrips_through_the_manifest() {
        let mut m = Rproj::minimal("mypkg");
        let field = "tidyverse/tidytemplate, pkgdown (>= 2.0), \
                     bioc::S4Vectors, jsonlite=jeroen/jsonlite@v1.8.0";
        m.merge_config_needs(&needs(&[("website", field)]));

        // The manifest survives a TOML round trip, `git`/`ref` and all.
        let text = toml::to_string_pretty(&m).unwrap();
        assert_eq!(toml::from_str::<Rproj>(&text).unwrap(), m);

        let (desc, _) = m.to_description().unwrap();
        // Entries are sorted by package name, each written back verbatim as
        // it came in (`ref_`), `bioc::S4Vectors` and `jsonlite=...`'s
        // redundant name override alike.
        assert!(desc.contains(
            "Config/Needs/website:\n    bioc::S4Vectors,\n    \
             jsonlite=jeroen/jsonlite@v1.8.0,\n    pkgdown (>= 2.0),\n    \
             tidyverse/tidytemplate\n"
        ));
    }

    #[test]
    fn merge_config_infers_integer_and_boolean_types() {
        let mut m = Rproj::minimal("mypkg");
        m.merge_config(&[
            (
                "testthat".to_string(),
                "edition".to_string(),
                "3".to_string(),
            ),
            (
                "testthat".to_string(),
                "parallel".to_string(),
                "true".to_string(),
            ),
            (
                "Roxygen".to_string(),
                "roclets".to_string(),
                "list".to_string(),
            ),
        ]);

        let testthat = m.config.get("testthat").unwrap();
        assert_eq!(testthat.get("edition"), Some(&toml::Value::Integer(3)));
        assert_eq!(testthat.get("parallel"), Some(&toml::Value::Boolean(true)));
        assert_eq!(
            m.config.get("Roxygen").unwrap().get("roclets"),
            Some(&toml::Value::String("list".to_string()))
        );
    }

    #[test]
    fn config_roundtrips_through_description() {
        let mut m = Rproj::minimal("mypkg");
        m.merge_config(&[(
            "testthat".to_string(),
            "edition".to_string(),
            "3".to_string(),
        )]);

        let (desc, _) = m.to_description().unwrap();
        assert!(desc.contains("Config/testthat/edition: 3\n"));

        // The manifest survives a TOML round trip.
        let text = toml::to_string_pretty(&m).unwrap();
        assert_eq!(toml::from_str::<Rproj>(&text).unwrap(), m);
    }

    #[test]
    fn description_escape_hatch_roundtrips_through_description() {
        let mut m = Rproj::minimal("mypkg");
        m.description.insert(
            "Encoding".to_string(),
            toml::Value::String("UTF-8".to_string()),
        );

        let (desc, _) = m.to_description().unwrap();
        assert!(desc.contains("Encoding: UTF-8\n"));

        // The manifest survives a TOML round trip.
        let text = toml::to_string_pretty(&m).unwrap();
        assert_eq!(toml::from_str::<Rproj>(&text).unwrap(), m);
    }

    #[test]
    fn config_needs_entry_names_the_package_a_reference_implies() {
        // A `git`/GitHub reference, the same syntax `Remotes:` understands,
        // is parsed into a `git`-sourced `DepTable`, exactly like a `Remotes:`
        // entry.
        let git_cases = [
            (
                "tidyverse/tidytemplate",
                "tidytemplate",
                DepTable {
                    git: Some("https://github.com/tidyverse/tidytemplate.git".to_string()),
                    ref_: Some("tidyverse/tidytemplate".to_string()),
                    ..Default::default()
                },
            ),
            (
                "tidyverse/tidytemplate@main",
                "tidytemplate",
                DepTable {
                    git: Some("https://github.com/tidyverse/tidytemplate.git".to_string()),
                    rev: Some("main".to_string()),
                    ref_: Some("tidyverse/tidytemplate@main".to_string()),
                    ..Default::default()
                },
            ),
            (
                "r-lib/pak#123",
                "pak",
                DepTable {
                    git: Some("https://github.com/r-lib/pak.git".to_string()),
                    pr: Some(123),
                    ref_: Some("r-lib/pak#123".to_string()),
                    ..Default::default()
                },
            ),
            (
                "git::https://github.com/r-lib/cli.git",
                "cli",
                DepTable {
                    git: Some("https://github.com/r-lib/cli.git".to_string()),
                    ref_: Some("git::https://github.com/r-lib/cli.git".to_string()),
                    ..Default::default()
                },
            ),
            (
                "jsonlite=jeroen/jsonlite",
                "jsonlite",
                DepTable {
                    git: Some("https://github.com/jeroen/jsonlite.git".to_string()),
                    ref_: Some("jsonlite=jeroen/jsonlite".to_string()),
                    ..Default::default()
                },
            ),
            (
                "r-lib/usethis/subdir",
                "subdir",
                DepTable {
                    git: Some("https://github.com/r-lib/usethis.git".to_string()),
                    subdir: Some("subdir".to_string()),
                    ref_: Some("r-lib/usethis/subdir".to_string()),
                    ..Default::default()
                },
            ),
        ];
        for (entry, name, table) in git_cases {
            let (key, dep) = config_needs_entry(entry);
            assert_eq!(key, name, "{}", entry);
            assert_eq!(dep, Dependency::Detailed(Box::new(table)), "{}", entry);
        }

        // `bioc::S4Vectors` is not a `git`/GitHub reference, so it is kept
        // verbatim in `ref`.
        let (key, dep) = config_needs_entry("bioc::S4Vectors");
        assert_eq!(key, "S4Vectors");
        assert_eq!(
            dep,
            Dependency::Detailed(Box::new(DepTable {
                ref_: Some("bioc::S4Vectors".to_string()),
                ..Default::default()
            }))
        );

        // A reference with no package name in it is kept under the reference
        // itself, rather than being dropped.
        let (key, _) = config_needs_entry("url::https://example.org/x?a=1");
        assert_eq!(key, "url::https://example.org/x?a=1");
    }
}
