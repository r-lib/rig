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
use tabular::*;

#[cfg(target_os = "linux")]
use crate::linux::sc_get_default;
#[cfg(target_os = "macos")]
use crate::macos::sc_get_default;
#[cfg(target_os = "windows")]
use crate::windows::sc_get_default;

use crate::dcf::{Package, RDepType, RPackageVersion};
use crate::output::OUTPUT;
use crate::proj::BASE_PKGS;
use crate::repos::cranlike_metadata::{self, ArchivedPackage};
use crate::repos::feed::{BiocSetting, CranlikeRepo, MetadataFeed, PkgRepo, RepoId};
use crate::repos::interpret_repos_args::ReposSetupArgs;
use crate::repos::repo_metadata_urls;
use crate::repos::{
    configured_repos, interpret_pkg_repos_args, repos_with_setup, resolve_bioc_vars, PkgReposArgs,
};
use crate::textfmt::{reflow, wrap, write_field};

pub(crate) mod deps;
pub(crate) mod doctor;
pub(crate) mod install;
pub(crate) mod list;
mod manifest;
pub(crate) mod remove;
pub(crate) mod search;
#[cfg(test)]
mod stub;
pub(crate) mod tree;
mod views;

pub fn sc_pkg(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        Some(("available", s)) => sc_pkg_available(s, args, mainargs),
        Some(("deps", s)) => deps::sc_pkg_deps(s, args, mainargs),
        Some(("doctor", s)) => doctor::sc_pkg_doctor(s, args, mainargs),
        Some(("info", s)) => sc_pkg_info(s, args, mainargs),
        Some(("install", s)) => install::sc_pkg_install(s, args, mainargs),
        Some(("list", s)) => list::sc_pkg_list(s, args, mainargs),
        Some(("remove", s)) => remove::sc_pkg_remove(s, args, mainargs),
        Some(("search", s)) => search::sc_pkg_search(s, args, mainargs),
        Some(("tree", s)) => tree::sc_pkg_tree(s, args, mainargs),
        _ => Ok(()), // unreachable
    }
}

/// The repositories of a `rig pkg` command: the ones configured for its R
/// version, see [`pkg_repos`], changed by `--with-repos` and
/// `--without-repos`, if the command has them. The R version is
/// `--r-version`, if the command has it, or else the default R version.
/// Without any R version it is CRAN, and Bioconductor unless it is turned
/// off, see [`default_r_feeds`].
pub(crate) fn pkg_repos_for(args: &ArgMatches) -> Result<Vec<PkgRepo>, Box<dyn Error>> {
    let bioc = BiocSetting::default();
    let over = interpret_pkg_repos_args(args)?;
    let rver = match args.try_get_one::<String>("r-version").ok().flatten() {
        Some(_) => Some(crate::library::library_rver(args)?),
        None => sc_get_default().ok().flatten(),
    };
    match rver {
        Some(rver) => pkg_repos(&rver, &bioc, None, over.as_ref()),
        None => no_r_repos(&bioc, over.as_ref()),
    }
}

// The repositories without an installed R version: CRAN and Bioconductor,
// unless `--without-repos` turns them off, plus the ones in `--with-repos`
// given by URL. Repository names need an R version.
fn no_r_repos(
    bioc: &BiocSetting,
    over: Option<&PkgReposArgs>,
) -> Result<Vec<PkgRepo>, Box<dyn Error>> {
    let Some(over) = over else {
        return Ok(PkgRepo::from_feeds(default_r_feeds(bioc)));
    };
    let mut names: Vec<String> = over.enabled_names().to_vec();
    if let ReposSetupArgs::Default { blacklist, .. } = &over.setup {
        names.extend(blacklist.iter().cloned());
    }
    if !names.is_empty() {
        bail!(
            "Selecting repositories by name needs an installed R version: {}",
            names.join(", ")
        );
    }
    let mut repos: Vec<PkgRepo> = over
        .urls
        .iter()
        .map(|(name, url)| PkgRepo::Cranlike(CranlikeRepo::new(name, url)))
        .collect();
    if !over.is_empty_base() {
        repos.extend(PkgRepo::from_feeds(default_r_feeds(bioc)));
    }
    Ok(repos)
}

/// The repositories configured for R installation `rver`, in the order of
/// its `repositories` file, see [`repos_from_entries`].
///
/// `over` (from `--with-repos` and `--without-repos`) changes them for this
/// command, the same way `rig repos enable` and `rig repos disable` would,
/// but without changing the setup of the installation. Its repositories given
/// by URL come first.
///
/// If the `repositories` file cannot be read, e.g. for a very old R version,
/// it is CRAN, and Bioconductor unless it is turned off.
pub(crate) fn pkg_repos(
    rver: &str,
    bioc: &BiocSetting,
    cutoff: Option<&str>,
    over: Option<&PkgReposArgs>,
) -> Result<Vec<PkgRepo>, Box<dyn Error>> {
    let configured = match over {
        Some(over) => match repos_with_setup(rver, &over.setup)? {
            Some(mut entries) => {
                resolve_bioc_vars(rver, &mut entries)?;
                Some(entries)
            }
            None => None,
        },
        None => match configured_repos(Some(rver), false, true) {
            Ok(c) => Some(c.repos),
            Err(e) => {
                log::debug!("Cannot read the repositories of R {}: {}", rver, e);
                None
            }
        },
    };
    // Repositories given by URL are plain CRAN-like ones, even if their name
    // is the name of a repository with extended metadata.
    let url_repos = over.map_or(vec![], |o| {
        repos_from_entries(&o.urls, &Default::default(), true, None)
    });
    let entries: Vec<(String, String)> = match configured {
        Some(configured) => configured.into_iter().map(|r| (r.name, r.url)).collect(),
        None => {
            log::debug!(
                "No repositories file for R {}, using CRAN and Bioconductor",
                rver
            );
            if over.is_some_and(|o| !o.enabled_names().is_empty()) {
                OUTPUT.warn(&format!(
                    "R {} has no repositories file, ignoring repository names in --with-repos.",
                    rver
                ));
            }
            let mut repos = url_repos;
            if !over.is_some_and(|o| o.is_empty_base()) {
                repos.extend(PkgRepo::from_feeds(bioc.feeds(rver, cutoff)));
            }
            if repos.is_empty() {
                OUTPUT.warn(&format!(
                    "No package repositories are configured for R {}.",
                    rver
                ));
            }
            return Ok(repos);
        }
    };

    let metadata = repo_metadata_urls().unwrap_or_else(|e| {
        log::debug!("Cannot read the repository configuration: {}", e);
        Default::default()
    });

    let bioc_version = if entries.iter().any(|(n, u)| is_bioc_entry(n, u)) {
        bioc.bioc_version(rver, cutoff)
    } else {
        None
    };
    let mut repos = url_repos;
    for repo in repos_from_entries(&entries, &metadata, bioc.enabled, bioc_version.as_deref()) {
        if !repos.iter().any(|r| same_repo(r, &repo)) {
            repos.push(repo);
        }
    }
    if repos.is_empty() {
        OUTPUT.warn(&format!(
            "No package repositories are configured for R {}.",
            rver
        ));
    }
    Ok(repos)
}

/// Whether a `repositories` entry is a Bioconductor repository.
fn is_bioc_entry(name: &str, url: &str) -> bool {
    name.to_lowercase().starts_with("bioc") || url.contains("bioconductor.org/packages")
}

/// The repositories of the `(name, url)` entries of a `repositories` file,
/// in the same order.
///
/// An entry whose name has extended metadata in `metadata` (by lowercase
/// name, e.g. P3M and BioCsoft) is that feed. Every other entry is a plain
/// CRAN-like repository, including CRAN itself; R's `@CRAN@` placeholder is
/// the CRAN cloud mirror. Bioconductor entries are dropped if `bioc_enabled`
/// is false. A Bioconductor feed needs `bioc_version`, without it the entry
/// is a CRAN-like repository, too. Repeated feeds and URLs are dropped.
fn repos_from_entries(
    entries: &[(String, String)],
    metadata: &std::collections::HashMap<String, String>,
    bioc_enabled: bool,
    bioc_version: Option<&str>,
) -> Vec<PkgRepo> {
    let mut out: Vec<PkgRepo> = vec![];
    for (name, url) in entries {
        if !bioc_enabled && is_bioc_entry(name, url) {
            continue;
        }
        let feed = metadata
            .get(&name.to_lowercase())
            .and_then(|m| MetadataFeed::from_metadata_url(m, bioc_version));
        let repo = match feed {
            Some(feed) => PkgRepo::Extended(feed),
            None => {
                let url = if url == "@CRAN@" {
                    "https://cloud.r-project.org"
                } else {
                    url.as_str()
                };
                PkgRepo::Cranlike(CranlikeRepo::new(name, url))
            }
        };
        if !out.iter().any(|r| same_repo(r, &repo)) {
            out.push(repo);
        }
    }
    out
}

// Whether two repositories are the same feed, or have the same URL.
fn same_repo(a: &PkgRepo, b: &PkgRepo) -> bool {
    match (a, b) {
        (PkgRepo::Extended(a), PkgRepo::Extended(b)) => a == b,
        (PkgRepo::Cranlike(a), PkgRepo::Cranlike(b)) => a.url == b.url,
        _ => false,
    }
}

/// The metadata feeds for the default R version with `bioc`: CRAN, and the
/// Bioconductor release of the default R version, or the newest release if
/// there is no default R version.
pub(crate) fn default_r_feeds(bioc: &BiocSetting) -> Vec<MetadataFeed> {
    if !bioc.enabled {
        return MetadataFeed::for_target(None);
    }
    let default_r = sc_get_default().ok().flatten();
    match default_r {
        Some(rver) => bioc.feeds(&rver, None),
        None => MetadataFeed::for_target(
            bioc.version
                .clone()
                .or_else(crate::repos::latest_bioc_release)
                .as_deref(),
        ),
    }
}

fn sc_pkg_available(
    args: &ArgMatches,
    _pkgargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let include_archived = args.get_flag("include-archived");
    let repos = pkg_repos_for(args)?;
    let mut packages = cranlike_metadata::all_available_packages(
        &repos,
        include_archived,
        Some(crate::dcf::OsType::host()),
    )?;
    // Order the listing case-insensitively by package name, breaking ties by
    // version, so the output is stable regardless of how the metadata was
    // stored or downloaded.
    packages.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.version.cmp(&b.version))
    });

    if args.get_flag("json") || mainargs.get_flag("json") {
        print_package_list_json(&packages)?;
    } else {
        print_package_list(&packages);
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
/// A colored header line names the number of packages; the table then lists
/// each package with its version and hard-dependency count. The full
/// dependency lists are available via `--json`.
fn print_package_list(packages: &[Package]) {
    use owo_colors::OwoColorize;

    let color = std::io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none();

    // -- Header ------------------------------------------------------------
    let count = packages.len();
    let pkg_word = if count == 1 { "package" } else { "packages" };
    if color {
        println!("{} {}", count.cyan().bold(), pkg_word);
    } else {
        println!("{} {}", count, pkg_word);
    }
    if count == 0 {
        return;
    }
    println!();

    // -- Table -------------------------------------------------------------
    let mut tab: Table = Table::new("{:<}   {:<}   {:>}");
    tab.add_row(row!("Package", "Version", "Deps"));
    tab.add_heading(
        "--------------------------------------------------------------------------------",
    );
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

    // The version to show, from the configured repositories. For a CRAN
    // version P3M's manifests have the full DESCRIPTION, for a Bioconductor
    // version the repository's `VIEWS` file, for any other repository we only
    // know the fields of its index.
    let all = repo_versions(args, &package)?;
    let shown = if ver == "latest" {
        all.iter().max_by(|a, b| a.version.cmp(&b.version))
    } else {
        all.iter().find(|p| p.version.original == ver)
    };
    let mut info = match shown {
        None if all.is_empty() => bail!(
            "Could not find package '{}' in the package repositories.",
            package
        ),
        None => bail!(
            "Could not find version '{}' of package '{}' in the package repositories.",
            ver,
            package
        ),
        Some(pkg) if is_cran(pkg) => {
            manifest::get_package_description(&package, &pkg.version.original)?
        }
        Some(pkg) => manifest::PackageInfo {
            description: repo_description(pkg),
            readme: None,
            readme_type: None,
            archived: None,
        },
    };

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

/// The versions of `package` in the repositories of a `rig pkg info`
/// command, as the solver sees them. If several repositories have the same
/// version, the first one wins.
fn repo_versions(args: &ArgMatches, package: &str) -> Result<Vec<Package>, Box<dyn Error>> {
    let repos = pkg_repos_for(args)?;
    let loader = crate::repos::DbSourcePackageLoader::new_for_repos(&repos)?;
    crate::solver::PackageVersionLoader::load_versions(&loader, package)
}

/// Whether `pkg` comes from CRAN's metadata feed, so P3M's manifests have
/// its full DESCRIPTION.
fn is_cran(pkg: &Package) -> bool {
    pkg.repository == Some(RepoId::Cran)
}

/// The DESCRIPTION of a non-CRAN package version: the full one from the
/// `VIEWS` file of a Bioconductor repository, if it has this version, else
/// the fields of the repository's index. `Repository` and `DownloadURL` always
/// come from the index, so they are the same for every repository.
fn repo_description(pkg: &Package) -> serde_json::Value {
    let index = index_description(pkg);
    let Some(serde_json::Value::Object(mut desc)) = views::bioc_description(pkg) else {
        return index;
    };
    for key in ["Repository", "DownloadURL"] {
        if let Some(value) = index.get(key) {
            desc.insert(key.to_string(), value.clone());
        }
    }
    serde_json::Value::Object(desc)
}

/// The DESCRIPTION fields we know of a package version from the index of
/// its repository, i.e. a `PACKAGES` file or Bioconductor's metadata, as a
/// JSON object.
fn index_description(pkg: &Package) -> serde_json::Value {
    let mut desc = serde_json::Map::new();
    let mut set = |k: &str, v: String| {
        desc.insert(k.to_string(), serde_json::Value::String(v));
    };
    set("Package", pkg.name.clone());
    set("Version", pkg.version.original.clone());
    for dep_type in RDepType::all() {
        let deps: Vec<String> = pkg
            .dependencies
            .dependencies
            .iter()
            .filter(|d| d.types.contains(dep_type))
            .map(|d| {
                if d.constraints.is_empty() {
                    d.name.clone()
                } else {
                    let cons: Vec<String> = d
                        .constraints
                        .iter()
                        .map(|c| format!("{} {}", c.constraint_type, c.version))
                        .collect();
                    format!("{} ({})", d.name, cons.join(", "))
                }
            })
            .collect();
        if !deps.is_empty() {
            set(&dep_type.to_string(), deps.join(", "));
        }
    }
    if let Some(repo) = &pkg.repository {
        set("Repository", repo.to_string());
    }
    if let Some(url) = &pkg.download_url {
        set("DownloadURL", url.clone());
    }
    serde_json::Value::Object(desc)
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
    let all = repo_versions(args, package)?;
    // The CRAN history from P3M's manifests, if CRAN is configured and has
    // the package.
    let on_cran = all.iter().any(is_cran);
    let mut versions = if on_cran {
        manifest::get_package_versions(package)?
    } else {
        vec![]
    };
    // The versions of the other repositories, unless CRAN has them, too.
    for pkg in all.iter().filter(|p| !is_cran(p)) {
        if !versions.iter().any(|v| v.version == pkg.version) {
            versions.push(manifest::PackageVersion {
                version: pkg.version.clone(),
                description: index_description(pkg),
                dependencies: pkg.dependencies.clone(),
            });
        }
    }
    versions.sort_by(|a, b| a.version.cmp(&b.version));
    if versions.is_empty() {
        bail!(
            "Could not find package '{}' in the package repositories.",
            package
        );
    }

    let archived = if on_cran {
        cranlike_metadata::archived_package(package)?
    } else {
        None
    };

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

    fn entries(x: &[(&str, &str)]) -> Vec<(String, String)> {
        x.iter()
            .map(|(n, u)| (n.to_string(), u.to_string()))
            .collect()
    }

    fn metadata() -> std::collections::HashMap<String, String> {
        [
            ("p3m", "https://ppm.r-pkg.org"),
            ("biocsoft", "https://ppm-bioc.r-pkg.org/%v"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }

    #[test]
    fn configured_repos_map_to_their_kind() {
        let repos = repos_from_entries(
            &entries(&[
                ("BioCsoft", "https://bioconductor.org/packages/3.22/bioc"),
                (
                    "BioCann",
                    "https://bioconductor.org/packages/3.22/data/annotation",
                ),
                ("P3M", "https://packagemanager.posit.co/cran/latest"),
                ("CRAN", "@CRAN@"),
                ("acme", "https://cran.acme.com/"),
                ("acme2", "https://cran.acme.com"),
            ]),
            &metadata(),
            true,
            Some("3.22"),
        );
        assert_eq!(
            repos,
            vec![
                PkgRepo::Extended(MetadataFeed::bioc("3.22")),
                PkgRepo::Cranlike(CranlikeRepo::new(
                    "BioCann",
                    "https://bioconductor.org/packages/3.22/data/annotation"
                )),
                PkgRepo::Extended(MetadataFeed::cran()),
                PkgRepo::Cranlike(CranlikeRepo::new("CRAN", "https://cloud.r-project.org")),
                PkgRepo::Cranlike(CranlikeRepo::new("acme", "https://cran.acme.com")),
            ]
        );
    }

    #[test]
    fn bioc_entries_can_be_turned_off() {
        let es = entries(&[
            ("P3M", "https://packagemanager.posit.co/cran/latest"),
            ("BioCsoft", "https://bioconductor.org/packages/3.22/bioc"),
            (
                "BioCexp",
                "https://bioconductor.org/packages/3.22/data/experiment",
            ),
        ]);
        let repos = repos_from_entries(&es, &metadata(), false, None);
        assert_eq!(repos, vec![PkgRepo::Extended(MetadataFeed::cran())]);
        // No Bioconductor version: BioCsoft is a plain CRAN-like repository.
        let repos = repos_from_entries(&es, &metadata(), true, None);
        assert_eq!(repos.len(), 3);
        assert!(matches!(&repos[1], PkgRepo::Cranlike(r) if r.name == "BioCsoft"));
    }

    #[test]
    fn duplicate_feeds_are_dropped() {
        let repos = repos_from_entries(
            &entries(&[
                ("P3M", "https://packagemanager.posit.co/cran/latest"),
                (
                    "P3M",
                    "https://packagemanager.posit.co/cran/__linux__/noble/latest",
                ),
            ]),
            &metadata(),
            true,
            None,
        );
        assert_eq!(repos, vec![PkgRepo::Extended(MetadataFeed::cran())]);
    }
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
