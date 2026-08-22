//! `rig pkg`: information about R packages in the configured repositories.
//!
//! The package metadata itself comes from the local CRAN-like metadata database
//! (`crate::repos::cranlike_metadata`) and, for full DESCRIPTION files of
//! arbitrary versions, from P3M's sync manifests ([`manifest`]).

use std::env;
use std::error::Error;
use std::io::IsTerminal;

use clap::ArgMatches;
use lazy_static::lazy_static;
use simple_error::*;
use tabular::*;

use crate::common::get_default_r_version;
use crate::dcf::{Package, RDepType, RPackageVersion};
use crate::proj::BASE_PKGS;
use crate::repos::cranlike_metadata::{self, repos_get_packages, ArchivedPackage};
use crate::textfmt::{reflow, wrap, write_field};

pub(crate) mod deps;
mod manifest;
#[cfg(test)]
mod stub;
pub(crate) mod tree;

pub fn sc_pkg(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        Some(("available", s)) => sc_pkg_available(s, args, mainargs),
        Some(("deps", s)) => deps::sc_pkg_deps(s, args, mainargs),
        Some(("info", s)) => sc_pkg_info(s, args, mainargs),
        Some(("tree", s)) => tree::sc_pkg_tree(s, args, mainargs),
        _ => Ok(()), // unreachable
    }
}

fn sc_pkg_available(
    args: &ArgMatches,
    _pkgargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let platform = if args.contains_id("platform") {
        crate::platform::parse_platform_string(
            &args.get_one::<String>("platform").unwrap().to_string(),
        )?
    } else {
        crate::platform::detect_platform()?
    };
    let r_version = if args.contains_id("r-version") {
        args.get_one::<String>("r-version").unwrap().to_string()
    } else {
        get_default_r_version()?.ok_or("Cannot determine default R version")?
    };
    let pkg_type = if args.contains_id("pkg-type") {
        match crate::platform::resolve_package_type_synonyms(
            &platform,
            &r_version,
            &args.get_one::<String>("pkg-type").unwrap().to_string(),
        ) {
            Some(pt) => pt,
            None => "source".to_string(),
        }
    } else {
        "source".to_string()
    };
    let mut packages = repos_get_packages("https://cloud.r-project.org", &pkg_type, &r_version)?;
    // Order the listing case-insensitively by package name, breaking ties by
    // version, so the output is stable regardless of how the metadata was
    // stored or downloaded.
    packages.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.version.cmp(&b.version))
    });

    // Echo the platform in the header only when the user asked for a specific
    // one; otherwise the package type already conveys the relevant flavor.
    let platform_label = if args.contains_id("platform") {
        Some(
            platform
                .rig_platform
                .clone()
                .unwrap_or_else(|| platform.arch.clone()),
        )
    } else {
        None
    };

    if args.get_flag("json") || mainargs.get_flag("json") {
        print_package_list_json(&packages)?;
    } else {
        print_package_list(&packages, &r_version, &pkg_type, platform_label.as_deref());
    }

    Ok(())
}

/// Count the hard dependencies of a package: `Depends`, `Imports` and
/// `LinkingTo`, excluding R itself and the base packages. This matches the
/// `Deps` column of `rig pkg info --versions`.
fn num_hard_deps(pkg: &Package) -> usize {
    pkg.dependencies
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
        .count()
}

/// Pretty-print the package listing for `rig pkg available`.
///
/// A colored header line names the number of packages and the context they
/// were resolved for (R version, package type, platform); the table then lists
/// each package with its version and hard-dependency count. The full
/// dependency lists are available via `--json`.
fn print_package_list(
    packages: &[Package],
    r_version: &str,
    pkg_type: &str,
    platform: Option<&str>,
) {
    use owo_colors::OwoColorize;

    let color = std::io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none();

    // -- Header ------------------------------------------------------------
    let count = packages.len();
    let pkg_word = if count == 1 { "package" } else { "packages" };
    let head = if color {
        format!("{} {}", count.cyan().bold(), pkg_word)
    } else {
        format!("{} {}", count, pkg_word)
    };
    let tag = match platform {
        Some(platform) => format!("(R {}, {}, {})", r_version, pkg_type, platform),
        None => format!("(R {}, {})", r_version, pkg_type),
    };
    println!(
        "{} {}",
        head,
        if color { tag.dimmed().to_string() } else { tag }
    );
    if count == 0 {
        return;
    }
    println!();

    // -- Table -------------------------------------------------------------
    let mut tab: Table = Table::new("{:<}   {:<}   {:>}");
    tab.add_row(row!("Package", "Version", "Deps"));
    tab.add_heading("------------------------------------------------------------");
    for pkg in packages {
        tab.add_row(row!(&pkg.name, &pkg.version, num_hard_deps(pkg)));
    }

    print!("{}", tab);
}

/// Print the package listing as a JSON array, one object per package, with the
/// full dependency information (name, types and version constraints).
fn print_package_list_json(packages: &[Package]) -> Result<(), Box<dyn Error>> {
    #[derive(serde::Serialize)]
    struct PackageListEntry<'a> {
        package: &'a str,
        version: String,
        dependencies: &'a [crate::dcf::DepVersionSpec],
    }

    let entries: Vec<PackageListEntry> = packages
        .iter()
        .map(|pkg| PackageListEntry {
            package: &pkg.name,
            version: pkg.version.to_string(),
            dependencies: &pkg.dependencies.dependencies,
        })
        .collect();

    println!("{}", serde_json::to_string_pretty(&entries)?);
    Ok(())
}

fn sc_pkg_info(
    args: &ArgMatches,
    _pkgargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let package: String = args.get_one::<String>("package").unwrap().to_string();

    // `--versions` switches from one version in detail to the version table.
    if args.get_flag("versions") {
        return pkg_info_versions(args, &package);
    }

    let ver = if args.contains_id("version") {
        args.get_one::<String>("version").unwrap().to_string()
    } else {
        "latest".to_string()
    };

    let mut info = manifest::get_package_description(&package, &ver)?;

    if args.get_flag("readme") {
        return pkg_info_readme(&info, args.get_flag("json"));
    }

    if args.get_flag("json") {
        add_archived_field(&mut info.description, info.archived.as_ref());
        let json = serde_json::to_string_pretty(&info.description)?;
        println!("{}", json);
    } else {
        let color = std::io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none();
        print!("{}", format_package_info(&info, color));
    }

    Ok(())
}

/// `--readme`: the README of the package, as the repository stores it, i.e.
/// not rendered for the terminal. `--json` adds the format it is written in,
/// which the repository reports and we pass through unchanged, so it can be
/// `rst` or `html` as well as `md` or `txt`.
///
/// A package without a README is not an error, it prints nothing (or an
/// object with null fields for `--json`).
fn pkg_info_readme(info: &manifest::PackageInfo, json: bool) -> Result<(), Box<dyn Error>> {
    let readme = info.readme.as_deref().filter(|s| !s.is_empty());

    if json {
        println!("{}", serde_json::to_string_pretty(&readme_json(info))?);
    } else if let Some(readme) = readme {
        // As-is, except that we make sure it ends with a newline.
        print!("{}", readme);
        if !readme.ends_with('\n') {
            println!();
        }
    }

    Ok(())
}

#[derive(serde::Serialize)]
struct ReadmeJson<'a> {
    package: Option<&'a str>,
    version: Option<&'a str>,
    format: Option<&'a str>,
    readme: Option<&'a str>,
}

fn readme_json(info: &manifest::PackageInfo) -> ReadmeJson<'_> {
    let readme = info.readme.as_deref().filter(|s| !s.is_empty());
    // The name and version of the resolved package, so that the default
    // (`latest`) reports the actual version number.
    let field = |k: &str| info.description.get(k).and_then(|v| v.as_str());
    ReadmeJson {
        package: field("Package"),
        version: field("Version"),
        // The repository can have a README without a type, or the other way
        // around; a format without a README would be meaningless.
        format: readme.and(info.readme_type.as_deref()),
        readme,
    }
}

fn add_archived_field(desc: &mut serde_json::Value, archived: Option<&ArchivedPackage>) {
    if let (Some(archived), Some(obj)) = (archived, desc.as_object_mut()) {
        obj.insert(
            "Archived".to_string(),
            serde_json::Value::String(archived.archived.clone()),
        );
    }
}

/// Format package metadata (the fields of a DESCRIPTION file) for the
/// terminal.
///
/// The most useful fields are grouped into a header (name, version, title,
/// description), a metadata block and a dependency block; noisy internal
/// fields (checksums, timestamps, `Config/*` entries, ...) are omitted. The
/// full record is still available via `--json`, and the README via
/// `--readme`.
fn format_package_info(info: &manifest::PackageInfo, color: bool) -> String {
    use owo_colors::OwoColorize;
    use std::fmt::Write;

    let mut out = String::new();
    let desc = &info.description;
    let str_field = |k: &str| -> Option<String> {
        desc.get(k)
            .and_then(|v| v.as_str())
            .map(reflow)
            .filter(|s| !s.is_empty())
    };

    // -- Header ------------------------------------------------------------
    let name = str_field("Package").unwrap_or_default();
    let version = str_field("Version").unwrap_or_default();
    let repo = str_field("Repository");

    let mut header = if color {
        format!("{} {}", name.cyan().bold(), version.bold())
    } else {
        format!("{} {}", name, version)
    };
    if let Some(repo) = &repo {
        let tag = format!("({})", repo);
        header.push(' ');
        header.push_str(&if color { tag.dimmed().to_string() } else { tag });
    }
    let _ = writeln!(out, "{}", header);

    if let Some(title) = str_field("Title") {
        let _ = writeln!(
            out,
            "{}",
            if color {
                title.italic().to_string()
            } else {
                title
            }
        );
    }

    if let Some(description) = str_field("Description") {
        let _ = writeln!(out);
        for line in wrap(&description, 78) {
            let _ = writeln!(out, "{}", line);
        }
    }

    // -- Metadata ----------------------------------------------------------
    let label_width = 14;
    let mut meta: Vec<(&str, String)> = vec![];
    for (label, key) in [
        ("Maintainer", "Maintainer"),
        ("License", "License"),
        ("Published", "Date/Publication"),
        ("URL", "URL"),
        ("BugReports", "BugReports"),
        ("Compilation", "NeedsCompilation"),
    ] {
        if let Some(v) = str_field(key) {
            meta.push((label, v));
        }
        if key == "Date/Publication" {
            if let Some(archived) = &info.archived {
                let note = format!("{} (removed from CRAN)", archived.archived);
                meta.push((
                    "Archived",
                    if color {
                        note.yellow().to_string()
                    } else {
                        note
                    },
                ));
            }
        }
    }
    if !meta.is_empty() {
        let _ = writeln!(out);
        for (label, value) in meta {
            write_field(&mut out, label, &value, label_width, color);
        }
    }

    // -- Dependencies ------------------------------------------------------
    let dep_fields: Vec<(&str, String)> =
        ["Depends", "Imports", "LinkingTo", "Suggests", "Enhances"]
            .iter()
            .filter_map(|k| desc.get(*k).and_then(format_deps).map(|v| (*k, v)))
            .collect();
    if !dep_fields.is_empty() {
        let _ = writeln!(out);
        for (label, value) in dep_fields {
            write_field(&mut out, label, &value, label_width, color);
        }
    }

    out
}

/// Format a DESCRIPTION dependency field (`cli (>= 3.2.0), glue`), which DCF
/// wraps over several lines, as a single comma-separated list.
fn format_deps(value: &serde_json::Value) -> Option<String> {
    let deps = reflow(value.as_str()?);
    if deps.is_empty() {
        return None;
    }
    Some(deps)
}

/// `rig pkg info --versions`: every version of a package ever published.
fn pkg_info_versions(args: &ArgMatches, package: &str) -> Result<(), Box<dyn Error>> {
    let mut versions = manifest::get_package_versions(package)?;
    if versions.is_empty() {
        bail!("Could not find package '{}' on CRAN.", package);
    }

    let archived = cranlike_metadata::archived_package(package)?;

    // `--json` dumps the full DESCRIPTION of every version, mirroring
    // `rig pkg info --json`.
    if args.get_flag("json") {
        for version in versions.iter_mut() {
            add_archived_field(&mut version.description, archived.as_ref());
        }
        let descs: Vec<&serde_json::Value> = versions.iter().map(|v| &v.description).collect();
        println!("{}", serde_json::to_string_pretty(&descs)?);
        return Ok(());
    }

    let latest = versions.last().map(|v| v.version.original.clone());
    let rows: Vec<PackageVersionRow> = versions.iter().map(package_version_row).collect();

    print_package_versions(package, latest.as_deref(), archived.as_ref(), &rows);

    Ok(())
}

/// A single row of `rig pkg info --versions` output: a version, when it was
/// published, its R version requirement and how many hard dependencies it has.
struct PackageVersionRow {
    version: RPackageVersion,
    /// Publication date as `YYYY-MM-DD`, if the DESCRIPTION carries one.
    date: Option<String>,
    /// R version requirement (e.g. `>= 3.5.0`), or `None` when unconstrained.
    r_requirement: Option<String>,
    /// Number of hard dependencies (Depends / Imports / LinkingTo), excluding R
    /// and the base packages.
    num_deps: usize,
}

/// When a version was published, as `YYYY-MM-DD`.
///
/// `Date/Publication` is authoritative but only exists from about 2009 on, so
/// older versions fall back to `Packaged` and `Date`. Neither of those is a
/// formatted date field: `Packaged` is `date()` output in R versions of that
/// era (`Tue Feb 28 14:17:08 2006; csardi`) and `Date` is free-form prose
/// (`Januar 25, 2005`). Values we cannot read confidently are dropped.
fn publication_date(desc: &serde_json::Value) -> Option<String> {
    ["Date/Publication", "Packaged", "Date"]
        .iter()
        .filter_map(|k| desc.get(*k).and_then(|v| v.as_str()))
        .find_map(parse_date)
}

/// Read a `YYYY-MM-DD` date from the start of a DESCRIPTION date field, either
/// already ISO formatted or in R's `date()` format.
fn parse_date(value: &str) -> Option<String> {
    lazy_static! {
        static ref ISO: regex::Regex = regex::Regex::new(r"^\s*(\d{4}-\d{2}-\d{2})").unwrap();
        static ref CTIME: regex::Regex = regex::Regex::new(
            r"^\s*[[:alpha:]]{3}\s+([[:alpha:]]{3})\s+(\d{1,2})\s+[\d:]+\s+(\d{4})"
        )
        .unwrap();
    }

    if let Some(caps) = ISO.captures(value) {
        return Some(caps[1].to_string());
    }

    let caps = CTIME.captures(value)?;
    let month = match &caps[1].to_lowercase()[..] {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    };
    let day: u32 = caps[2].parse().ok()?;
    Some(format!("{}-{:02}-{:02}", &caps[3], month, day))
}

/// Summarize one version's DESCRIPTION into a table row.
fn package_version_row(version: &manifest::PackageVersion) -> PackageVersionRow {
    let date = publication_date(&version.description);

    let r_requirement = version
        .dependencies
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

    let num_deps = version
        .dependencies
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

    PackageVersionRow {
        version: version.version.clone(),
        date,
        r_requirement,
        num_deps,
    }
}

/// Pretty-print the version table for `rig pkg info --versions`.
///
/// A colored header line names the package, the number of versions, the latest
/// one and, for a package CRAN has archived, the date it was archived; the table
/// then lists each version with its publication date, R requirement and
/// hard-dependency count, marking the latest version. The full per-version
/// metadata is available via `--json`.
fn print_package_versions(
    name: &str,
    latest: Option<&str>,
    archived: Option<&ArchivedPackage>,
    rows: &[PackageVersionRow],
) {
    use owo_colors::OwoColorize;

    let color = std::io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none();

    // -- Header ------------------------------------------------------------
    let count = rows.len();
    let ver_word = if count == 1 { "version" } else { "versions" };
    let mut header = if color {
        format!("{} — {} {}", name.cyan().bold(), count, ver_word)
    } else {
        format!("{} — {} {}", name, count, ver_word)
    };
    let mut tags: Vec<String> = vec![];
    if let Some(latest) = latest {
        let tag = format!("latest {}", latest);
        tags.push(if color { tag.dimmed().to_string() } else { tag });
    }
    if let Some(archived) = archived {
        let tag = format!("archived {}", archived.archived);
        tags.push(if color { tag.yellow().to_string() } else { tag });
    }
    if !tags.is_empty() {
        let (open, close) = if color {
            ("(".dimmed().to_string(), ")".dimmed().to_string())
        } else {
            ("(".to_string(), ")".to_string())
        };
        header.push(' ');
        header.push_str(&format!("{}{}{}", open, tags.join(", "), close));
    }
    println!("{}", header);
    println!();

    // -- Table -------------------------------------------------------------
    let mut tab: Table = Table::new("{:<}   {:<}   {:<}   {:>}   {:<}");
    tab.add_row(row!("Version", "Published", "R", "Deps", ""));
    tab.add_heading("-------------------------------------------------------");
    for row in rows {
        let marker = if latest == Some(row.version.original.as_str()) {
            "← latest"
        } else {
            ""
        };
        tab.add_row(row!(
            &row.version,
            row.date.as_deref().unwrap_or("?"),
            row.r_requirement.as_deref().unwrap_or(""),
            &row.num_deps,
            marker
        ));
    }

    print!("{}", tab);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dcf::{DepVersionSpec, RPackageVersion};

    fn dep(name: &str, ty: RDepType) -> DepVersionSpec {
        DepVersionSpec {
            name: name.to_string(),
            types: vec![ty],
            constraints: vec![],
        }
    }

    fn pkg_with_deps(deps: Vec<DepVersionSpec>) -> Package {
        Package::from_crandb(
            "test".to_string(),
            RPackageVersion::from_str("1.0").unwrap(),
            deps,
        )
    }

    #[test]
    fn num_hard_deps_counts_hard_deps_only() {
        // R, the base package `utils` and the Suggests dependency do not count;
        // cli (Imports), Rcpp (LinkingTo) and MASS (Depends) do.
        let pkg = pkg_with_deps(vec![
            dep("R", RDepType::Depends),
            dep("utils", RDepType::Imports),
            dep("MASS", RDepType::Depends),
            dep("cli", RDepType::Imports),
            dep("Rcpp", RDepType::LinkingTo),
            dep("testthat", RDepType::Suggests),
        ]);
        assert_eq!(num_hard_deps(&pkg), 3);
    }

    #[test]
    fn num_hard_deps_zero_when_no_hard_deps() {
        let pkg = pkg_with_deps(vec![
            dep("R", RDepType::Depends),
            dep("knitr", RDepType::Suggests),
        ]);
        assert_eq!(num_hard_deps(&pkg), 0);
    }

    fn info_with_readme(readme: Option<&str>, readme_type: Option<&str>) -> manifest::PackageInfo {
        manifest::PackageInfo {
            description: serde_json::json!({ "Package": "pkg", "Version": "1.0.0" }),
            readme: readme.map(|s| s.to_string()),
            readme_type: readme_type.map(|s| s.to_string()),
            archived: None,
        }
    }

    #[test]
    fn package_info_has_no_readme() {
        // The README is only shown by `--readme`, never as part of the
        // metadata page.
        let mut info = info_with_readme(Some("Hello, README.\n"), Some("txt"));
        info.description = serde_json::json!({
            "Package": "pkg",
            "Version": "1.0.0",
            "Title": "A package",
            "Imports": "cli",
        });
        let out = format_package_info(&info, false);
        assert!(out.starts_with("pkg 1.0.0\nA package\n"));
        assert!(out.ends_with("Imports       cli\n"), "{:?}", out);
        assert!(!out.contains("README"));
    }

    #[test]
    fn readme_json_reports_the_readme_and_its_format() {
        let info = info_with_readme(Some("# pkg\n"), Some("md"));
        assert_eq!(
            serde_json::to_value(readme_json(&info)).unwrap(),
            serde_json::json!({
                "package": "pkg",
                "version": "1.0.0",
                "format": "md",
                "readme": "# pkg\n",
            })
        );

        // Formats we do not know anything about are passed through as they
        // are.
        let info = info_with_readme(Some("pkg\n===\n"), Some("rst"));
        assert_eq!(
            serde_json::to_value(readme_json(&info)).unwrap()["format"],
            serde_json::json!("rst")
        );
    }

    #[test]
    fn readme_json_is_null_without_a_readme() {
        // A missing README, and an empty or type-less one, are all "no
        // README".
        for info in [
            info_with_readme(None, None),
            info_with_readme(Some(""), Some("md")),
            info_with_readme(None, Some("md")),
        ] {
            let json = serde_json::to_value(readme_json(&info)).unwrap();
            assert_eq!(json["readme"], serde_json::Value::Null);
            assert_eq!(json["format"], serde_json::Value::Null);
            assert_eq!(json["package"], serde_json::json!("pkg"));
        }
    }

    #[test]
    fn parse_date_reads_iso_and_r_date_output() {
        assert_eq!(
            parse_date("2026-07-22 15:50:07 UTC").as_deref(),
            Some("2026-07-22")
        );
        assert_eq!(
            parse_date("2009-05-07 11:20:43 UTC; ripley").as_deref(),
            Some("2009-05-07")
        );
        // R's `date()` output, as old `Packaged` fields carry it.
        assert_eq!(
            parse_date("Tue Feb 28 14:17:08 2006; csardi").as_deref(),
            Some("2006-02-28")
        );
        assert_eq!(
            parse_date("Wed Aug  9 23:13:10 2006; csardi").as_deref(),
            Some("2006-08-09")
        );
        // Free-form prose is not a date we can trust.
        assert_eq!(parse_date("Januar 25, 2005"), None);
        assert_eq!(parse_date("Feb 14, 2008"), None);
        assert_eq!(parse_date(""), None);
    }

    #[test]
    fn publication_date_prefers_the_publication_field() {
        let desc = serde_json::json!({
            "Date/Publication": "2009-10-28 07:15:48",
            "Packaged": "Thu Oct 15 09:24:40 2009; ripley",
            "Date": "2009-10-15",
        });
        assert_eq!(publication_date(&desc).as_deref(), Some("2009-10-28"));

        // Before `Date/Publication` existed, `Packaged` is the best we have.
        let desc = serde_json::json!({
            "Packaged": "Tue Feb 28 14:17:08 2006; csardi",
            "Date": "Januar 25, 2005",
        });
        assert_eq!(publication_date(&desc).as_deref(), Some("2006-02-28"));

        // An unreadable `Date` alone leaves the date unknown.
        let desc = serde_json::json!({ "Date": "Januar 25, 2005" });
        assert_eq!(publication_date(&desc), None);
    }

    #[test]
    fn format_deps_reflows_a_dcf_field() {
        // DCF wraps long dependency fields over several indented lines.
        let deps = serde_json::json!("R (>= 3.5.0), utils,\n        cli (>= 3.2.0)");
        assert_eq!(
            format_deps(&deps),
            Some("R (>= 3.5.0), utils, cli (>= 3.2.0)".to_string())
        );
    }

    #[test]
    fn format_deps_empty_field_is_none() {
        assert_eq!(format_deps(&serde_json::json!("")), None);
        assert_eq!(format_deps(&serde_json::json!("   ")), None);
        assert_eq!(format_deps(&serde_json::json!({})), None);
    }
}
