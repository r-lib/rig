use std::env;
use std::error::Error;
use std::io::IsTerminal;

use clap::ArgMatches;
use simple_error::*;
use tabular::*;

use crate::common::*;
use crate::dcf::{Package, RDepType};
use crate::hardcoded::*;
use crate::proj::BASE_PKGS;
use crate::repositories::*;

#[cfg(target_os = "macos")]
use crate::macos::*;

#[cfg(target_os = "windows")]
use crate::windows::*;

#[cfg(target_os = "linux")]
use crate::linux::*;

mod config;
pub use config::{get_repos_config, RepoEntry, Repository};
mod interpret_repos_args;
pub use interpret_repos_args::interpret_repos_args;
mod repos_available;
use repos_available::sc_repos_available;
mod repos_list;
use repos_list::sc_repos_list;
pub mod cranlike_metadata;
pub use cranlike_metadata::repos_get_packages;
mod setup;
pub use setup::repos_setup;
mod crandb;
pub use crandb::get_all_cran_package_versions;
use crandb::CranVersionRow;

pub fn sc_repos(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        // Some(("add", s)) => sc_repos_add(s, args, mainargs),
        Some(("available", s)) => sc_repos_available(s, args, mainargs),
        // Some(("disable", s)) => sc_repos_disable(s, args, mainargs),
        // Some(("enable", s)) => sc_repos_enable(s, args, mainargs),
        Some(("list", s)) => sc_repos_list(s, args, mainargs),
        Some(("package-list", s)) => sc_repos_package_list(s, args, mainargs),
        Some(("package-info", s)) => sc_repos_package_info(s, args, mainargs),
        Some(("package-versions", s)) => sc_repos_package_versions(s, args, mainargs),
        // Some(("reset", s)) => sc_repos_reset(s, args, mainargs),
        // Some(("rm", s)) => sc_repos_rm(s, args, mainargs),
        Some(("setup", s)) => sc_repos_setup(s, args, mainargs),
        _ => Ok(()), // unreachable
    }
}

pub fn r_version_to_bioc_version(rver: &str) -> Result<String, Box<dyn Error>> {
    match env::var("R_BIOC_VERSION") {
        Ok(biocver) => Ok(biocver),
        Err(_) => {
            let minor = rver.split('.').take(2).collect::<Vec<&str>>().join(".");
            match HC_R_VERSION_TO_BIOC_VERSION.get(&minor) {
                Some(biocver) => Ok(biocver.to_string()),
                None => {
                    bail!(
                        "Cannot determine Bioconductor version for R version {}, \n\
                        set R_BIOC_VERSION environment variable to override.",
                        rver
                    );
                }
            }
        }
    }
}

// pub fn sc_repos_add(
//     args: &ArgMatches,
//     _libargs: &ArgMatches,
//     _mainargs: &ArgMatches,
// ) -> Result<(), Box<dyn Error>> {
//     panic!("TODO: implement sc_repos_add");
//     Ok(())
// }

// pub fn sc_repos_disable(
//     args: &ArgMatches,
//     _libargs: &ArgMatches,
//     _mainargs: &ArgMatches,
// ) -> Result<(), Box<dyn Error>> {
//     panic!("TODO: implement sc_repos_disable");
//     Ok(())
// }

// pub fn sc_repos_enable(
//     args: &ArgMatches,
//     _libargs: &ArgMatches,
//     _mainargs: &ArgMatches,
// ) -> Result<(), Box<dyn Error>> {
//     panic!("TODO: implement sc_repos_enable");
//     Ok(())
// }

// pub fn sc_repos_reset(
//     args: &ArgMatches,
//     _libargs: &ArgMatches,
//     _mainargs: &ArgMatches,
// ) -> Result<(), Box<dyn Error>> {
//     panic!("TODO: implement sc_repos_reset");
//     Ok(())
// }

// pub fn sc_repos_rm(
//     args: &ArgMatches,
//     _libargs: &ArgMatches,
//     _mainargs: &ArgMatches,
// ) -> Result<(), Box<dyn Error>> {
//     panic!("TODO: implement sc_repos_rm");
//     Ok(())
// }

fn sc_repos_setup(
    args: &ArgMatches,
    _libargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let vers: Vec<String> = if args.contains_id("r-version") {
        vec![args.get_one::<String>("r-version").unwrap().to_string()]
    } else {
        sc_get_list()?
    };

    let setup = interpret_repos_args(args, false);
    repos_setup(Some(vers), setup)
}

fn sc_repos_package_list(
    args: &ArgMatches,
    _libargs: &ArgMatches,
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
/// `Deps` column of `rig repos package-versions`.
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

/// Pretty-print the package listing for `rig repos package-list`.
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

fn sc_repos_package_info(
    args: &ArgMatches,
    _libargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let package: String = args.get_one::<String>("package").unwrap().to_string();
    let ver = if args.contains_id("version") {
        args.get_one::<String>("version").unwrap().to_string()
    } else {
        "latest".to_string()
    };

    let info = crandb::get_cran_package_version(&package, &ver)?;

    // crandb replies with an `error` object (e.g. `not_found`) instead of the
    // package metadata when the package or version does not exist.
    if info.get("Package").is_none() || info.get("error").is_some() {
        let which = if ver == "latest" {
            format!("package '{}'", package)
        } else {
            format!("package '{}' version '{}'", package, ver)
        };
        bail!("Could not find {} in the CRAN metadata database.", which);
    }

    if args.get_flag("json") {
        let json = serde_json::to_string_pretty(&info)?;
        println!("{}", json);
    } else {
        print_package_info(&info);
    }

    Ok(())
}

/// Pretty-print package metadata (as returned by crandb) to stdout.
///
/// The most useful fields are grouped into a header (name, version, title,
/// description), a metadata block and a dependency block; noisy internal
/// fields (checksums, timestamps, `Config/*` entries, ...) are omitted. The
/// full record is still available via `--json`.
fn print_package_info(info: &serde_json::Value) {
    use owo_colors::OwoColorize;

    let color = std::io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none();
    let str_field = |k: &str| -> Option<String> {
        info.get(k)
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
    println!("{}", header);

    if let Some(title) = str_field("Title") {
        println!(
            "{}",
            if color {
                title.italic().to_string()
            } else {
                title
            }
        );
    }

    if let Some(desc) = str_field("Description") {
        println!();
        for line in wrap(&desc, 78) {
            println!("{}", line);
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
    }
    if !meta.is_empty() {
        println!();
        for (label, value) in meta {
            print_field(label, &value, label_width, color);
        }
    }

    // -- Dependencies ------------------------------------------------------
    let dep_fields: Vec<(&str, String)> =
        ["Depends", "Imports", "LinkingTo", "Suggests", "Enhances"]
            .iter()
            .filter_map(|k| info.get(*k).and_then(format_deps).map(|v| (*k, v)))
            .collect();
    if !dep_fields.is_empty() {
        println!();
        for (label, value) in dep_fields {
            print_field(label, &value, label_width, color);
        }
    }
}

/// Print a single `label   value` line, wrapping long values under the label.
fn print_field(label: &str, value: &str, width: usize, color: bool) {
    use owo_colors::OwoColorize;
    let padded = format!("{:width$}", label);
    let shown_label = if color {
        padded.dimmed().to_string()
    } else {
        padded
    };
    let indent = " ".repeat(width);
    let lines = wrap(value, 78usize.saturating_sub(width));
    for (i, line) in lines.iter().enumerate() {
        if i == 0 {
            println!("{}{}", shown_label, line);
        } else {
            println!("{}{}", indent, line);
        }
    }
}

/// Format a crandb dependency object (`{"pkg": "*" | ">= x.y"}`) as a
/// comma-separated list, showing version constraints where present.
fn format_deps(value: &serde_json::Value) -> Option<String> {
    let obj = value.as_object()?;
    if obj.is_empty() {
        return None;
    }
    let parts: Vec<String> = obj
        .iter()
        .map(|(name, spec)| match spec.as_str() {
            Some("*") | None => name.to_string(),
            Some(s) => format!("{} ({})", name, s),
        })
        .collect();
    Some(parts.join(", "))
}

/// Collapse runs of whitespace (including the newlines DCF fields carry) into
/// single spaces.
fn reflow(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Word-wrap `text` to at most `width` columns, keeping words intact.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines: Vec<String> = vec![];
    let mut line = String::new();
    for word in text.split_whitespace() {
        if line.is_empty() {
            line.push_str(word);
        } else if line.len() + 1 + word.len() <= width {
            line.push(' ');
            line.push_str(word);
        } else {
            lines.push(std::mem::take(&mut line));
            line.push_str(word);
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn sc_repos_package_versions(
    args: &ArgMatches,
    _libargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let package: String = args.get_one::<String>("package").unwrap().to_string();

    // `--json` dumps the full crandb record, mirroring `package-info --json`.
    if args.get_flag("json") {
        let json = crandb::fetch_crandb_all(&package, None)?;
        println!("{}", serde_json::to_string_pretty(&json)?);
        return Ok(());
    }

    let info = crandb::get_cran_package_versions_info(&package, None)?;
    if info.rows.is_empty() {
        bail!(
            "Could not find package '{}' in the CRAN metadata database.",
            package
        );
    }

    let mut rows = info.rows;
    rows.sort_by(|a, b| a.version.cmp(&b.version));

    print_package_versions(&info.name, info.latest.as_deref(), info.archived, &rows);

    Ok(())
}

/// Pretty-print the version table for `rig repos package-versions`.
///
/// A colored header line names the package, the number of versions and the
/// latest one; the table then lists each version with its publication date, R
/// requirement and hard-dependency count, marking the latest version. The full
/// per-version metadata is available via `--json`.
fn print_package_versions(
    name: &str,
    latest: Option<&str>,
    archived: bool,
    rows: &[CranVersionRow],
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
        tags.push(format!("latest {}", latest));
    }
    if archived {
        tags.push("archived".to_string());
    }
    if !tags.is_empty() {
        let tag = format!("({})", tags.join(", "));
        header.push(' ');
        header.push_str(&if color { tag.dimmed().to_string() } else { tag });
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

    #[test]
    fn reflow_collapses_newlines_and_spaces() {
        assert_eq!(reflow("a\nb  c\n  d"), "a b c d");
        assert_eq!(reflow("  spaced  out  "), "spaced out");
        assert_eq!(reflow(""), "");
    }

    #[test]
    fn wrap_keeps_words_intact_within_width() {
        let lines = wrap("the quick brown fox", 10);
        assert_eq!(lines, vec!["the quick", "brown fox"]);
        for line in &lines {
            assert!(line.len() <= 10);
        }
    }

    #[test]
    fn wrap_does_not_split_overlong_words() {
        let lines = wrap("supercalifragilistic word", 8);
        assert_eq!(lines, vec!["supercalifragilistic", "word"]);
    }

    #[test]
    fn wrap_empty_yields_single_empty_line() {
        assert_eq!(wrap("", 10), vec![String::new()]);
    }

    #[test]
    fn format_deps_shows_constraints_and_skips_wildcards() {
        let deps = serde_json::json!({ "R": ">= 3.5.0", "utils": "*" });
        // serde_json orders object keys, so output is deterministic.
        assert_eq!(format_deps(&deps), Some("R (>= 3.5.0), utils".to_string()));
    }

    #[test]
    fn format_deps_empty_object_is_none() {
        assert_eq!(format_deps(&serde_json::json!({})), None);
        assert_eq!(format_deps(&serde_json::json!("not an object")), None);
    }
}
