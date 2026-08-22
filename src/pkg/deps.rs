//! `rig pkg deps`: the dependencies of a package.
//!
//! The dependency data comes from the local CRAN-wide metadata database (the
//! `ALLPACKAGES` feed, via [`DbSourcePackageLoader`]), so any version ever
//! published on CRAN can be queried, and no network round trip is needed once
//! the cache is warm.

use std::collections::{HashMap, VecDeque};
use std::env;
use std::error::Error;
use std::io::IsTerminal;

use clap::ArgMatches;
use log::debug;
use simple_error::*;
use tabular::*;

use crate::dcf::{DepVersionSpec, Package, RDepType, RPackageVersion, DEP_TYPES_SOFT};
use crate::repos::DbSourcePackageLoader;
use crate::solver::{is_base_package, PackageVersionLoader};

pub fn sc_pkg_deps(
    args: &ArgMatches,
    pkgargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let package: String = args.get_one::<String>("package").unwrap().to_string();
    let ver = if args.contains_id("version") {
        args.get_one::<String>("version").unwrap().to_string()
    } else {
        "latest".to_string()
    };
    let dev = args.get_flag("dev");
    let json = args.get_flag("json") || pkgargs.get_flag("json") || mainargs.get_flag("json");

    let loader = DbSourcePackageLoader::new()?;

    if args.get_flag("recursive") {
        let (version, rows, num_direct) = recursive_deps(&loader, &package, &ver, dev)?;
        if json {
            print_deps_json(&rows, true)?;
        } else {
            print_deps_recursive(&package, &version, num_direct, &rows);
        }
    } else {
        let (version, rows) = direct_deps(&loader, &package, &ver, dev)?;
        if json {
            print_deps_json(&rows, false)?;
        } else {
            print_deps(&package, &version, &rows);
        }
    }

    Ok(())
}

// ------------------------------------------------------------------------
// Collecting the dependencies

/// One row of the dependency table: a package another package needs, with the
/// version currently available, the dependency type(s) and the version
/// requirement(s).
#[derive(Debug, Clone, PartialEq, Eq)]
struct DepRow {
    name: String,
    /// Newest version in the database. `None` for R and the base packages,
    /// which ship with R, and for a package the database does not know about.
    version: Option<RPackageVersion>,
    /// Dependency type(s), e.g. `Imports`, or both `Imports` and `LinkingTo`.
    types: Vec<RDepType>,
    /// Version requirements, e.g. `>= 1.0.2`.
    requires: Vec<String>,
    /// Recursive mode only: shortest distance from the queried package.
    depth: usize,
    /// Recursive mode only: the packages that depend on this one.
    needed_by: Vec<String>,
}

/// The direct dependencies of one version of a package.
fn direct_deps(
    loader: &dyn PackageVersionLoader,
    package: &str,
    ver: &str,
    dev: bool,
) -> Result<(RPackageVersion, Vec<DepRow>), Box<dyn Error>> {
    let root = root_package(loader, package, ver)?;
    let mut newest = Newest::new(loader);

    let mut rows: Vec<DepRow> = vec![];
    for dep in root.dependencies.dependencies.iter() {
        if !wanted_dep(dep, dev) {
            continue;
        }
        rows.push(DepRow {
            name: dep.name.clone(),
            version: newest_version(&mut newest, &dep.name),
            types: dep.types.clone(),
            requires: requirements(dep),
            depth: 1,
            needed_by: vec![],
        });
    }

    // R first, then group by dependency type, in the order R lists the fields
    // in, and sort by name within a type.
    rows.sort_by(|a, b| {
        sort_key(a).cmp(&sort_key(b)).then_with(|| {
            type_rank(&a.types)
                .cmp(&type_rank(&b.types))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        })
    });

    Ok((root.version, rows))
}

/// The transitive dependency closure of one version of a package: every package
/// it needs, directly or indirectly, with the shortest distance from the
/// queried package and the packages that pull it in.
///
/// With `dev`, the soft dependencies (`Suggests`, `Enhances`) of the queried
/// package are part of the closure, but only hard dependencies are followed
/// below that, i.e. the walk does not visit the `Suggests` of a `Suggests`.
///
/// The closure is taken over the newest version of each package, so a version
/// requirement that would force an older version — with different dependencies
/// — is not honored. That is the same approximation
/// [`crate::solver::RPackageRegistry::prefetch_binaries`] makes; a full,
/// version-consistent resolution is what `rig proj solve` is for.
fn recursive_deps(
    loader: &dyn PackageVersionLoader,
    package: &str,
    ver: &str,
    dev: bool,
) -> Result<(RPackageVersion, Vec<DepRow>, usize), Box<dyn Error>> {
    let root = root_package(loader, package, ver)?;
    let mut newest = Newest::new(loader);

    let mut rows: Vec<DepRow> = vec![];
    // Row index of each package we have seen, which doubles as the "visited"
    // set of the walk.
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut queue: VecDeque<(String, usize)> = VecDeque::new();

    let mut num_direct = 0;
    for dep in root.dependencies.dependencies.iter() {
        if !wanted_dep(dep, dev) {
            continue;
        }
        num_direct += 1;
        if record_dep(&mut rows, &mut seen, dep, package, 1) && !is_base_package(&dep.name) {
            queue.push_back((dep.name.clone(), 1));
        }
    }

    while let Some((name, depth)) = queue.pop_front() {
        // A package that is not in the database, or that we cannot read, is not
        // fatal: we just cannot say what it needs.
        let deps = match newest.get(&name) {
            Some(pkg) => pkg.dependencies.dependencies.clone(),
            None => continue,
        };
        for dep in deps.iter() {
            if !wanted_dep(dep, false) {
                continue;
            }
            if record_dep(&mut rows, &mut seen, dep, &name, depth + 1)
                && !is_base_package(&dep.name)
            {
                queue.push_back((dep.name.clone(), depth + 1));
            }
        }
    }

    for row in rows.iter_mut() {
        row.version = newest_version(&mut newest, &row.name);
        row.needed_by.sort_by_key(|p| p.to_lowercase());
    }
    // R first, then by name.
    rows.sort_by_key(|r| (sort_key(r), r.name.to_lowercase()));

    Ok((root.version, rows, num_direct))
}

/// The packages `rig pkg deps --recursive` would list, by name. Lets
/// [`super::tree`] check that it walks the same closure this module does,
/// without exposing [`DepRow`] outside the module.
#[cfg(test)]
pub(super) fn recursive_dep_names(
    loader: &dyn PackageVersionLoader,
    package: &str,
    ver: &str,
    dev: bool,
) -> Result<Vec<String>, Box<dyn Error>> {
    let (_, rows, _) = recursive_deps(loader, package, ver, dev)?;
    Ok(rows.into_iter().map(|row| row.name).collect())
}

/// Record `dep` as a dependency of `parent`, `depth` steps from the queried
/// package, and return whether this is the first time we see it.
///
/// A package we have seen already keeps its original depth — the queue is
/// breadth first, so that is the shortest distance — and collects the extra
/// parent, dependency types and version requirements.
fn record_dep(
    rows: &mut Vec<DepRow>,
    seen: &mut HashMap<String, usize>,
    dep: &DepVersionSpec,
    parent: &str,
    depth: usize,
) -> bool {
    if let Some(&idx) = seen.get(&dep.name) {
        let row = &mut rows[idx];
        if !row.needed_by.iter().any(|p| p == parent) {
            row.needed_by.push(parent.to_string());
        }
        for t in dep.types.iter() {
            if !row.types.contains(t) {
                row.types.push(t.clone());
            }
        }
        for req in requirements(dep) {
            if !row.requires.contains(&req) {
                row.requires.push(req);
            }
        }
        return false;
    }

    seen.insert(dep.name.clone(), rows.len());
    rows.push(DepRow {
        name: dep.name.clone(),
        version: None,
        types: dep.types.clone(),
        requires: requirements(dep),
        depth,
        needed_by: vec![parent.to_string()],
    });
    true
}

/// The package version whose dependencies were asked about.
pub(super) fn root_package(
    loader: &dyn PackageVersionLoader,
    package: &str,
    ver: &str,
) -> Result<Package, Box<dyn Error>> {
    let versions = loader.load_versions(package)?;
    if versions.is_empty() {
        bail!("Could not find package '{}' on CRAN.", package);
    }
    match select_version(&versions, ver)? {
        Some(pkg) => Ok(pkg.clone()),
        None => bail!("Could not find version '{}' of package '{}'.", ver, package),
    }
}

/// Pick a version of a package, `"latest"` meaning the highest one.
fn select_version<'a>(
    versions: &'a [Package],
    want: &str,
) -> Result<Option<&'a Package>, Box<dyn Error>> {
    if want == "latest" {
        return Ok(versions.iter().max_by(|a, b| a.version.cmp(&b.version)));
    }
    let wanted = match RPackageVersion::from_str(want) {
        Ok(wanted) => wanted,
        Err(_) => bail!("Invalid package version: '{}'.", want),
    };
    Ok(versions.iter().find(|v| v.version == wanted))
}

/// Whether a dependency belongs in the listing: the soft dependencies
/// (`Suggests`, `Enhances`) only with `dev`. A package that is both a hard and
/// a soft dependency is always listed.
pub(super) fn wanted_dep(dep: &DepVersionSpec, dev: bool) -> bool {
    dev || !dep.types.iter().all(|t| DEP_TYPES_SOFT.contains(t))
}

/// The version requirements of a dependency, e.g. `>= 1.0.2`.
pub(super) fn requirements(dep: &DepVersionSpec) -> Vec<String> {
    dep.constraints
        .iter()
        .map(|c| format!("{} {}", c.constraint_type, c.version))
        .collect()
}

/// R is the dependency everything else is relative to, so it goes to the top of
/// the table, ahead of the packages.
fn sort_key(row: &DepRow) -> usize {
    if row.name == "R" {
        0
    } else {
        1
    }
}

/// Where a dependency sorts among the dependency types, by the first type it
/// has, in `Depends`, `Imports`, `LinkingTo`, `Suggests`, `Enhances` order.
pub(super) fn type_rank(types: &[RDepType]) -> usize {
    types
        .iter()
        .filter_map(|t| RDepType::all().iter().position(|a| a == t))
        .min()
        .unwrap_or(usize::MAX)
}

/// The newest version of a package, or nothing for R and the base packages,
/// which ship with R and so are not in the database.
pub(super) fn newest_version(newest: &mut Newest, package: &str) -> Option<RPackageVersion> {
    if is_base_package(package) {
        return None;
    }
    newest.get(package).map(|pkg| pkg.version.clone())
}

/// The newest version of each package, from the database, remembered so that a
/// package showing up many times in a closure is only queried once.
pub(super) struct Newest<'a> {
    loader: &'a dyn PackageVersionLoader,
    newest: HashMap<String, Option<Package>>,
}

impl<'a> Newest<'a> {
    pub(super) fn new(loader: &'a dyn PackageVersionLoader) -> Self {
        Newest {
            loader,
            newest: HashMap::new(),
        }
    }

    pub(super) fn get(&mut self, package: &str) -> Option<&Package> {
        if !self.newest.contains_key(package) {
            let newest = match self.loader.load_versions(package) {
                Ok(versions) => versions
                    .into_iter()
                    .max_by(|a, b| a.version.cmp(&b.version)),
                Err(err) => {
                    debug!("Failed to load versions of package '{}': {}", package, err);
                    None
                }
            };
            self.newest.insert(package.to_string(), newest);
        }
        self.newest.get(package).and_then(|pkg| pkg.as_ref())
    }
}

// ------------------------------------------------------------------------
// Output

/// Pretty-print the direct dependencies of a package.
///
/// A colored header line names the package version and how many dependencies
/// it has; the table then lists each dependency with the version currently
/// available, the dependency type(s) and the version requirement(s).
fn print_deps(name: &str, version: &RPackageVersion, rows: &[DepRow]) {
    use owo_colors::OwoColorize;

    let color = std::io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none();

    // -- Header ------------------------------------------------------------
    let count = rows.len();
    let dep_word = if count == 1 {
        "dependency"
    } else {
        "dependencies"
    };
    if color {
        println!(
            "{} {} — {} {}",
            name.cyan().bold(),
            version.bold(),
            count,
            dep_word
        );
    } else {
        println!("{} {} — {} {}", name, version, count, dep_word);
    }
    if count == 0 {
        return;
    }
    println!();

    // -- Table -------------------------------------------------------------
    let mut tab: Table = Table::new("{:<}   {:<}   {:<}   {:<}");
    tab.add_row(row!("Package", "Version", "Type", "Requires"));
    tab.add_heading("-------------------------------------------------------");
    for row in rows {
        tab.add_row(row!(
            &row.name,
            version_cell(row),
            type_list(&row.types),
            row.requires.join(", ")
        ));
    }

    print!("{}", tab);
}

/// Pretty-print the transitive dependency closure of a package.
///
/// A colored header line names the package version, how many direct
/// dependencies it has and how many packages the closure has altogether; the
/// table then lists every package in the closure, with the version currently
/// available, its distance from the queried package and the packages that pull
/// it in.
fn print_deps_recursive(name: &str, version: &RPackageVersion, num_direct: usize, rows: &[DepRow]) {
    use owo_colors::OwoColorize;

    let color = std::io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none();

    // -- Header ------------------------------------------------------------
    let tag = format!("{} direct, {} total", num_direct, rows.len());
    if color {
        println!(
            "{} {} — {}",
            name.cyan().bold(),
            version.bold(),
            tag.dimmed()
        );
    } else {
        println!("{} {} — {}", name, version, tag);
    }
    if rows.is_empty() {
        return;
    }
    println!();

    // -- Table -------------------------------------------------------------
    let mut tab: Table = Table::new("{:<}   {:<}   {:>}   {:<}");
    tab.add_row(row!("Package", "Version", "Depth", "Needed by"));
    tab.add_heading("-------------------------------------------------------");
    for row in rows {
        tab.add_row(row!(
            &row.name,
            version_cell(row),
            &row.depth,
            needed_by_cell(&row.needed_by)
        ));
    }

    print!("{}", tab);
}

/// The `Version` cell of a row: R and the base packages ship with R, so they
/// have no version of their own (`-`); `?` marks a package that is not in the
/// database at all.
fn version_cell(row: &DepRow) -> String {
    version_cell_for(&row.name, row.version.as_ref())
}

/// How a package's version is shown when the database has no version for it:
/// `-` for R and the base packages, which ship with R, `?` for a package that is
/// not in the database at all.
pub(super) fn version_cell_for(name: &str, version: Option<&RPackageVersion>) -> String {
    match version {
        Some(version) => version.to_string(),
        None if is_base_package(name) => "-".to_string(),
        None => "?".to_string(),
    }
}

/// The `Type` cell of a row, e.g. `Imports, LinkingTo`.
fn type_list(types: &[RDepType]) -> String {
    types
        .iter()
        .map(|t| t.to_string())
        .collect::<Vec<String>>()
        .join(", ")
}

/// The `Needed by` cell of a row. Popular packages are needed by most of the
/// closure, so we only name the first few.
fn needed_by_cell(needed_by: &[String]) -> String {
    const MAX: usize = 3;
    if needed_by.len() <= MAX {
        needed_by.join(", ")
    } else {
        format!("{}, …", needed_by[..MAX].join(", "))
    }
}

/// Print the dependencies as a JSON array, one object per package. `depth` and
/// `needed_by` only make sense for a closure, so they are omitted unless
/// `recursive`.
fn print_deps_json(rows: &[DepRow], recursive: bool) -> Result<(), Box<dyn Error>> {
    #[derive(serde::Serialize)]
    struct DepEntry<'a> {
        package: &'a str,
        version: Option<String>,
        types: Vec<String>,
        requires: &'a [String],
        #[serde(skip_serializing_if = "Option::is_none")]
        depth: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        needed_by: Option<&'a [String]>,
    }

    let entries: Vec<DepEntry> = rows
        .iter()
        .map(|row| DepEntry {
            package: &row.name,
            version: row.version.as_ref().map(|v| v.to_string()),
            types: row.types.iter().map(|t| t.to_string()).collect(),
            requires: &row.requires,
            depth: if recursive { Some(row.depth) } else { None },
            needed_by: if recursive {
                Some(&row.needed_by)
            } else {
                None
            },
        })
        .collect();

    println!("{}", serde_json::to_string_pretty(&entries)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pkg::stub::Stub;

    /// The names of the rows, in the order they are printed in.
    fn names(rows: &[DepRow]) -> Vec<&str> {
        rows.iter().map(|r| r.name.as_str()).collect()
    }

    fn row<'a>(rows: &'a [DepRow], name: &str) -> &'a DepRow {
        rows.iter().find(|r| r.name == name).unwrap()
    }

    // ---------------------------------------------------------------------
    // Direct dependencies

    #[test]
    fn direct_deps_are_hard_deps_grouped_by_type() {
        let stub = Stub {
            packages: vec![
                (
                    "a",
                    "1.0.0",
                    "Depends: R (>= 3.5.0); Imports: zoo, mid (>= 2.0.0); \
                     LinkingTo: cpp11; Suggests: testthat",
                ),
                ("zoo", "1.8.14", ""),
                ("mid", "2.1.0", ""),
                ("cpp11", "0.5.2", ""),
                ("testthat", "3.2.3", ""),
            ],
        };

        let (version, rows) = direct_deps(&stub, "a", "latest", false).unwrap();

        assert_eq!(version.original, "1.0.0");
        // Depends first, then Imports (by name), then LinkingTo; Suggests is
        // left out without `--dev`.
        assert_eq!(names(&rows), vec!["R", "mid", "zoo", "cpp11"]);
        assert_eq!(row(&rows, "mid").requires, vec![">= 2.0.0"]);
        assert_eq!(row(&rows, "zoo").requires, Vec::<String>::new());
        assert_eq!(
            row(&rows, "zoo").version.as_ref().unwrap().original,
            "1.8.14"
        );
    }

    #[test]
    fn dev_adds_the_soft_deps() {
        let stub = Stub {
            packages: vec![
                ("a", "1.0.0", "Imports: b; Suggests: t; Enhances: e"),
                ("b", "1.0.0", ""),
                ("t", "1.0.0", ""),
                ("e", "1.0.0", ""),
            ],
        };

        let (_, rows) = direct_deps(&stub, "a", "latest", true).unwrap();
        assert_eq!(names(&rows), vec!["b", "t", "e"]);
    }

    #[test]
    fn a_dep_of_several_types_is_one_row_and_counts_as_hard() {
        let stub = Stub {
            packages: vec![
                ("a", "1.0.0", "Imports: cpp11; LinkingTo: cpp11"),
                ("cpp11", "0.5.2", ""),
            ],
        };

        let (_, rows) = direct_deps(&stub, "a", "latest", false).unwrap();
        assert_eq!(names(&rows), vec!["cpp11"]);
        assert_eq!(rows[0].types, vec![RDepType::Imports, RDepType::LinkingTo]);
        assert_eq!(type_list(&rows[0].types), "Imports, LinkingTo");
    }

    #[test]
    fn r_and_base_packages_have_no_version() {
        let stub = Stub {
            packages: vec![("a", "1.0.0", "Depends: R (>= 3.5.0); Imports: utils, b")],
        };

        let (_, rows) = direct_deps(&stub, "a", "latest", false).unwrap();
        assert_eq!(row(&rows, "R").version, None);
        assert_eq!(version_cell(row(&rows, "R")), "-");
        assert_eq!(version_cell(row(&rows, "utils")), "-");
        // `b` is not a base package, it is simply not in the database.
        assert_eq!(version_cell(row(&rows, "b")), "?");
    }

    #[test]
    fn version_selects_the_deps_of_that_version() {
        let stub = Stub {
            packages: vec![
                ("a", "1.0.0", "Imports: old"),
                ("a", "2.0.0", "Imports: new"),
                ("old", "1.0.0", ""),
                ("new", "1.0.0", ""),
            ],
        };

        let (version, rows) = direct_deps(&stub, "a", "1.0.0", false).unwrap();
        assert_eq!(version.original, "1.0.0");
        assert_eq!(names(&rows), vec!["old"]);

        // The default is the latest version.
        let (version, rows) = direct_deps(&stub, "a", "latest", false).unwrap();
        assert_eq!(version.original, "2.0.0");
        assert_eq!(names(&rows), vec!["new"]);
    }

    #[test]
    fn unknown_package_and_version_are_errors() {
        let stub = Stub {
            packages: vec![("a", "1.0.0", "")],
        };

        let err = direct_deps(&stub, "nosuchpkg", "latest", false).unwrap_err();
        assert!(err.to_string().contains("Could not find package"));

        let err = direct_deps(&stub, "a", "9.9.9", false).unwrap_err();
        assert!(err.to_string().contains("Could not find version"));

        let err = direct_deps(&stub, "a", "not-a-version", false).unwrap_err();
        assert!(err.to_string().contains("Invalid package version"));
    }

    // ---------------------------------------------------------------------
    // Recursive dependencies

    #[test]
    fn recursive_deps_walk_the_whole_closure() {
        let stub = Stub {
            packages: vec![
                ("a", "1.0.0", "Imports: b, c"),
                ("b", "1.1.0", "Imports: d"),
                ("c", "1.2.0", "Imports: d"),
                ("d", "1.3.0", "Imports: e"),
                ("e", "1.4.0", ""),
            ],
        };

        let (_, rows, num_direct) = recursive_deps(&stub, "a", "latest", false).unwrap();

        assert_eq!(num_direct, 2);
        assert_eq!(names(&rows), vec!["b", "c", "d", "e"]);
        assert_eq!(row(&rows, "b").depth, 1);
        assert_eq!(row(&rows, "d").depth, 2);
        assert_eq!(row(&rows, "e").depth, 3);
        assert_eq!(row(&rows, "d").version.as_ref().unwrap().original, "1.3.0");
    }

    #[test]
    fn a_diamond_is_one_row_naming_both_parents() {
        let stub = Stub {
            packages: vec![
                ("a", "1.0.0", "Imports: b, c"),
                ("b", "1.0.0", "Imports: d (>= 2.0.0)"),
                ("c", "1.0.0", "Imports: d"),
                ("d", "2.1.0", ""),
            ],
        };

        let (_, rows, _) = recursive_deps(&stub, "a", "latest", false).unwrap();
        assert_eq!(names(&rows), vec!["b", "c", "d"]);
        assert_eq!(row(&rows, "d").needed_by, vec!["b", "c"]);
        assert_eq!(row(&rows, "d").requires, vec![">= 2.0.0"]);
    }

    #[test]
    fn depth_is_the_shortest_path() {
        let stub = Stub {
            packages: vec![
                // `d` is a direct dependency, and also three steps away.
                ("a", "1.0.0", "Imports: b, d"),
                ("b", "1.0.0", "Imports: c"),
                ("c", "1.0.0", "Imports: d"),
                ("d", "1.0.0", ""),
            ],
        };

        let (_, rows, _) = recursive_deps(&stub, "a", "latest", false).unwrap();
        assert_eq!(row(&rows, "d").depth, 1);
        assert_eq!(row(&rows, "d").needed_by, vec!["a", "c"]);
    }

    #[test]
    fn a_cycle_terminates() {
        let stub = Stub {
            packages: vec![
                ("a", "1.0.0", "Imports: b"),
                ("b", "1.0.0", "Imports: c"),
                ("c", "1.0.0", "Imports: b, a"),
            ],
        };

        let (_, rows, _) = recursive_deps(&stub, "a", "latest", false).unwrap();
        assert_eq!(names(&rows), vec!["a", "b", "c"]);
        assert_eq!(row(&rows, "a").needed_by, vec!["c"]);
    }

    #[test]
    fn base_packages_are_listed_but_not_walked() {
        let stub = Stub {
            packages: vec![
                ("a", "1.0.0", "Depends: R (>= 3.5.0); Imports: b, stats"),
                ("b", "1.0.0", "Imports: utils"),
                // Never consulted: `stats` is a base package.
                ("stats", "0.0.1", "Imports: neverseen"),
                ("neverseen", "1.0.0", ""),
            ],
        };

        let (_, rows, _) = recursive_deps(&stub, "a", "latest", false).unwrap();
        // R first, then by name, case insensitively.
        assert_eq!(names(&rows), vec!["R", "b", "stats", "utils"]);
        assert_eq!(row(&rows, "stats").version, None);
        assert_eq!(row(&rows, "utils").needed_by, vec!["b"]);
    }

    #[test]
    fn dev_only_applies_to_the_queried_package() {
        let stub = Stub {
            packages: vec![
                ("a", "1.0.0", "Imports: b; Suggests: t"),
                ("b", "1.0.0", "Imports: c"),
                ("c", "1.0.0", ""),
                // `t`'s own Suggests is not followed, its Imports is.
                ("t", "1.0.0", "Imports: ti; Suggests: tt"),
                ("ti", "1.0.0", ""),
                ("tt", "1.0.0", ""),
            ],
        };

        let (_, rows, num_direct) = recursive_deps(&stub, "a", "latest", true).unwrap();
        assert_eq!(num_direct, 2);
        assert_eq!(names(&rows), vec!["b", "c", "t", "ti"]);
    }

    #[test]
    fn a_dep_missing_from_the_database_does_not_fail_the_walk() {
        let stub = Stub {
            packages: vec![("a", "1.0.0", "Imports: gone, b"), ("b", "1.0.0", "")],
        };

        let (_, rows, _) = recursive_deps(&stub, "a", "latest", false).unwrap();
        assert_eq!(names(&rows), vec!["b", "gone"]);
        assert_eq!(version_cell(row(&rows, "gone")), "?");
    }

    // ---------------------------------------------------------------------
    // Formatting

    #[test]
    fn needed_by_cell_names_only_the_first_few() {
        let names: Vec<String> = ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
        assert_eq!(needed_by_cell(&names[..2]), "a, b");
        assert_eq!(needed_by_cell(&names[..3]), "a, b, c");
        assert_eq!(needed_by_cell(&names), "a, b, c, …");
    }
}
