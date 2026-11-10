//! `rig audit`: check installed and locked packages against the policy of
//! their repositories: known vulnerabilities and blocked versions.
//!
//! In a project it checks the packages of `rproj.lock`, for every target,
//! and the project library. Outside of a project it checks a library of an
//! R installation, the default one, or the one of `--library` and
//! `--r-version`.
//!
//! The data comes from the extended metadata of the repositories, see
//! [`crate::repos::policy`]. Only a repository's own metadata has it, e.g.
//! that of a Posit Package Manager instance, so with the default P3M
//! metadata there is nothing to find.
//!
//! `--fix` upgrades every affected package to the nearest newer version that
//! is not blocked and has no known vulnerabilities: in a project with
//! `rig proj lock --upgrade-package` and `rig proj sync`, otherwise with
//! `rig pkg install`.
//!
//! `rig pkg audit` is the same, but it always checks a library, never a
//! project.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::error::Error;
use std::path::{Path, PathBuf};

use clap::ArgMatches;
use log::info;
use serde::Serialize;
use tabular::{Row, Table};

use crate::dcf::{PackageDependencies, RPackageVersion, VersionConstraintType};
use crate::output::OUTPUT;
use crate::pkg::list::{read_installed, resolve_library};
use crate::proj::{proj_lock_root, proj_read_solve_roots, read_existing_lock, BASE_PKGS};
use crate::repos::cranlike_metadata::{archived_package_in, repos_report_policy, AuditVersion};
use crate::repos::feed::{BiocSetting, PkgRepo, RepoId};
use crate::repos::{interpret_pkg_repos_args, DbSourcePackageLoader};
use crate::rproj::RPROJ_MANIFEST_FILE;
use crate::rvenv::{find_project_root, find_workspace_root, project_library};

/// A package version to check, and where it is.
#[derive(Debug, Clone)]
struct Subject {
    package: String,
    version: String,
    /// The `RemoteHash` of an installed package, or of a locked one.
    hash: Option<String>,
    /// E.g. `library` or `rproj.lock (R 4.6.1, source)`.
    place: String,
    /// The repositories to check it against, an index into the loaders.
    repos: usize,
}

/// What `rig audit` found about one package version.
#[derive(Debug, Clone, Serialize, PartialEq)]
struct Finding {
    package: String,
    version: String,
    /// `vulnerable`, `blocked` or `archived`.
    issue: &'static str,
    /// The vulnerabilities, the reason of the block, or the archival date.
    details: String,
    /// The nearest newer version without known problems, if any. Not for
    /// archived packages.
    #[serde(skip_serializing_if = "Option::is_none")]
    fixed_in: Option<String>,
    /// Where the version is.
    places: Vec<String>,
}

impl Finding {
    /// A vulnerable or blocked package, as opposed to an archived one.
    fn fails(&self) -> bool {
        self.issue != "archived"
    }
}

#[derive(Serialize)]
struct Report {
    findings: Vec<Finding>,
    /// The packages that are not in any repository, e.g. GitHub packages.
    not_checked: Vec<String>,
    /// Whether any of the repositories reports vulnerabilities and blocked
    /// versions at all.
    has_policy_data: bool,
}

/// What to check: the subjects, and the repositories of each.
struct Scope {
    subjects: Vec<Subject>,
    repos: Vec<Vec<PkgRepo>>,
    /// How to fix the problems found.
    fixer: Fixer,
}

enum Fixer {
    /// A project at this root, with the dependencies of its manifest, or
    /// of every member of a workspace.
    Project {
        root: PathBuf,
        deps: PackageDependencies,
    },
    /// A library, with the `--library` and `--r-version` values for
    /// `rig pkg install`.
    Library {
        library: PathBuf,
        rversion: Option<String>,
    },
}

pub fn sc_audit(args: &ArgMatches, mainargs: &ArgMatches) -> Result<i32, Box<dyn Error>> {
    audit_main(args, mainargs, false)
}

/// `rig pkg audit`: [`sc_audit`] for a library, also in a project.
pub fn sc_pkg_audit(args: &ArgMatches, mainargs: &ArgMatches) -> Result<i32, Box<dyn Error>> {
    audit_main(args, mainargs, true)
}

fn audit_main(
    args: &ArgMatches,
    mainargs: &ArgMatches,
    library_only: bool,
) -> Result<i32, Box<dyn Error>> {
    let json = args.get_flag("json") || mainargs.get_flag("json");
    let fix = args.get_flag("fix");
    let dry_run = args.get_flag("dry-run");

    let scope = audit_scope(args, library_only)?;
    let report = run_audit(&scope)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        let fix_cmd = if library_only {
            "rig pkg audit --fix"
        } else {
            "rig audit --fix"
        };
        print_report(&report, (!fix).then_some(fix_cmd));
    }

    let failing: Vec<&Finding> = report.findings.iter().filter(|f| f.fails()).collect();
    if !fix || failing.is_empty() {
        return Ok(if failing.is_empty() { 0 } else { 1 });
    }

    let mut fixes: BTreeMap<String, Fix> = BTreeMap::new();
    for f in &failing {
        match &f.fixed_in {
            Some(v) => {
                let fix = fixes.entry(f.package.clone()).or_insert_with(|| Fix {
                    from: vec![],
                    to: v.clone(),
                    why: vec![],
                });
                if !fix.from.contains(&f.version) {
                    fix.from.push(f.version.clone());
                }
                let why = if f.issue == "vulnerable" {
                    f.details.clone()
                } else {
                    f.issue.to_string()
                };
                if !fix.why.contains(&why) {
                    fix.why.push(why);
                }
            }
            None => OUTPUT.warn(&format!(
                "No version of {} without known problems, cannot fix it.",
                f.package
            )),
        }
    }
    // `rig proj lock --upgrade-package` cannot go against the manifest.
    if let Fixer::Project { deps, .. } = &scope.fixer {
        fixes.retain(|pkg, fix| match manifest_conflict(deps, pkg, &fix.to) {
            Some(cs) => {
                OUTPUT.warn(&format!(
                    "Cannot upgrade {} to {}, {} requires {} {}. \
                     Relax the constraint to fix it.",
                    pkg, fix.to, RPROJ_MANIFEST_FILE, pkg, cs
                ));
                false
            }
            None => true,
        });
    }
    if fixes.is_empty() {
        return Ok(1);
    }
    run_fix(&scope.fixer, &fixes, &repos_argv(args), dry_run, mainargs)?;
    if dry_run {
        return Ok(1);
    }

    // What is left after the fix.
    let scope = audit_scope(args, library_only)?;
    let report = run_audit(&scope)?;
    let left = report.findings.iter().filter(|f| f.fails()).count();
    if left == 0 {
        OUTPUT.success("No known vulnerabilities or blocked packages are left.");
        Ok(0)
    } else {
        if !json {
            print_report(&report, None);
        }
        Ok(1)
    }
}

/// The packages to check, from the project of the current directory, or
/// from a library if `--library` or `--r-version` is given, or if there is
/// no project. With `library_only` always from a library.
fn audit_scope(args: &ArgMatches, library_only: bool) -> Result<Scope, Box<dyn Error>> {
    if library_only {
        return library_scope(args);
    }
    let explicit_library = args.get_one::<String>("library").is_some()
        || args.get_one::<String>("r-version").is_some();
    let cwd = std::env::current_dir()?;
    let in_project = find_workspace_root(&cwd)?.is_some() || find_project_root(&cwd).is_some();
    if in_project && !explicit_library {
        project_scope(args)
    } else {
        library_scope(args)
    }
}

fn project_scope(args: &ArgMatches) -> Result<Scope, Box<dyn Error>> {
    let root = proj_lock_root()?;
    let mut solve = proj_read_solve_roots(&root)?;
    if let Some(over) = interpret_pkg_repos_args(args)? {
        solve.repos.apply_args(&over)?;
        solve.repos.check()?;
    }
    let lock = read_existing_lock(&root);
    if let Some(lock) = &lock {
        solve.repos.resolve_types(&lock.tool.rig.repositories);
    } else {
        solve.repos.resolve_types(&[]);
        OUTPUT.warn("The project has no lock file, only checking its library.");
    }

    // The repositories of each Bioconductor release of the lock.
    let mut repos: Vec<Vec<PkgRepo>> = vec![];
    let mut repo_index: HashMap<Option<String>, usize> = HashMap::new();
    let mut index_of = |bioc: Option<String>| -> usize {
        *repo_index.entry(bioc.clone()).or_insert_with(|| {
            repos.push(solve.repos.pkg_repos(bioc.as_deref()));
            repos.len() - 1
        })
    };

    let mut subjects: Vec<Subject> = vec![];
    let mut lib_repos: Option<usize> = None;
    for target in lock.iter().flat_map(|l| l.targets.iter()) {
        let idx = index_of(target.bioc_version.clone());
        lib_repos.get_or_insert(idx);
        let place = format!("rproj.lock (R {}, {})", target.r_version, target.platform);
        for pkg in &target.packages {
            // Packages from git, a URL or a local directory are not in any
            // repository.
            if pkg.metadata.contains_key("RemoteType") {
                continue;
            }
            subjects.push(Subject {
                package: pkg.package.clone(),
                version: pkg.version.clone(),
                hash: pkg.metadata.get("RemoteHash").cloned(),
                place: place.clone(),
                repos: idx,
            });
        }
    }

    let lib_repos = match lib_repos {
        Some(idx) => idx,
        None => index_of(None),
    };
    let library = project_library(&root)?;
    subjects.extend(library_subjects(&library, "project library", lib_repos)?);

    Ok(Scope {
        subjects,
        repos,
        fixer: Fixer::Project {
            root,
            deps: solve.merged,
        },
    })
}

fn library_scope(args: &ArgMatches) -> Result<Scope, Box<dyn Error>> {
    let lib = resolve_library(args)?;
    let rver = match &lib.rversion {
        Some(rver) => rver.clone(),
        None => crate::library::library_rver(args)?,
    };
    let over = interpret_pkg_repos_args(args)?;
    let repos = crate::pkg::pkg_repos(&rver, &BiocSetting::default(), None, over.as_ref())?;
    let place = match &lib.name {
        Some(name) => format!("library {} (R {})", name, rver),
        None => format!("library {}", lib.path.display()),
    };
    let subjects = library_subjects(&lib.path, &place, 0)?;
    Ok(Scope {
        subjects,
        repos: vec![repos],
        fixer: Fixer::Library {
            library: lib.path.clone(),
            rversion: args.get_one::<String>("r-version").cloned(),
        },
    })
}

/// The packages installed in `library` that came from a repository.
fn library_subjects(
    library: &Path,
    place: &str,
    repos: usize,
) -> Result<Vec<Subject>, Box<dyn Error>> {
    if !library.exists() {
        return Ok(vec![]);
    }
    Ok(read_installed(library)?
        .into_iter()
        .filter(|p| p.remote_sha.is_none())
        .map(|p| Subject {
            package: p.package.clone(),
            version: p.version.clone(),
            hash: p.hash.clone(),
            place: place.to_string(),
            repos,
        })
        .collect())
}

/// Check every subject of `scope`.
fn run_audit(scope: &Scope) -> Result<Report, Box<dyn Error>> {
    OUTPUT.status("Checking packages against the repositories");
    let loaders: Vec<DbSourcePackageLoader> = scope
        .repos
        .iter()
        .map(|r| DbSourcePackageLoader::new_for_repos(r))
        .collect::<Result<_, _>>()?;
    let has_policy_data = scope.repos.iter().any(|r| repos_report_policy(r));

    // One finding per package version and issue, with all its places.
    let mut findings: BTreeMap<(String, String, &'static str), Finding> = BTreeMap::new();
    let mut not_checked: BTreeSet<String> = BTreeSet::new();
    for subject in &scope.subjects {
        if subject.package == "R" || BASE_PKGS.contains(&subject.package.as_str()) {
            continue;
        }
        let Ok(version) = RPackageVersion::from_str(&subject.version) else {
            continue;
        };
        let rows = loaders[subject.repos].audit_versions(&subject.package)?;
        let archived = archived_date(&scope.repos[subject.repos], &subject.package)?;
        let found = check_version(
            &subject.package,
            &version,
            subject.hash.as_deref(),
            &rows,
            archived,
        );
        let Some(found) = found else {
            not_checked.insert(format!("{} {}", subject.package, subject.version));
            continue;
        };
        for finding in found {
            let key = (
                finding.package.clone(),
                finding.version.clone(),
                finding.issue,
            );
            let entry = findings.entry(key).or_insert(finding);
            if !entry.places.contains(&subject.place) {
                entry.places.push(subject.place.clone());
            }
        }
    }

    Ok(Report {
        findings: findings.into_values().collect(),
        not_checked: not_checked.into_iter().collect(),
        has_policy_data,
    })
}

/// The date `package` was archived, from the CRAN feed of `repos`.
fn archived_date(repos: &[PkgRepo], package: &str) -> Result<Option<String>, Box<dyn Error>> {
    for feed in PkgRepo::feeds(repos) {
        if feed.repo != RepoId::Cran {
            continue;
        }
        if let Some(a) = archived_package_in(&feed, package)? {
            return Ok(Some(a.archived));
        }
    }
    Ok(None)
}

/// The problems of `package` `version`, given all versions of the package
/// in the repositories, `rows`. `None` if no repository has the version.
/// A `hash`, the `RemoteHash` of the package, picks the matching records,
/// if there are several for the version.
fn check_version(
    package: &str,
    version: &RPackageVersion,
    hash: Option<&str>,
    rows: &[AuditVersion],
    archived: Option<String>,
) -> Option<Vec<Finding>> {
    let mut matching: Vec<&AuditVersion> = rows.iter().filter(|r| r.version == *version).collect();
    if matching.is_empty() {
        return None;
    }
    if let Some(hash) = hash {
        let same: Vec<&AuditVersion> = matching
            .iter()
            .copied()
            .filter(|r| r.sha256sum.as_deref() == Some(hash))
            .collect();
        if !same.is_empty() {
            matching = same;
        }
    }

    let finding = |issue: &'static str, details: String, fixed_in: Option<String>| Finding {
        package: package.to_string(),
        version: version.to_string(),
        issue,
        details,
        fixed_in,
        places: vec![],
    };
    let fixed_in = nearest_good_version(version, rows).map(|v| v.to_string());

    let mut out = vec![];
    let mut vulns: Vec<String> = vec![];
    for row in &matching {
        for v in row.policy.iter().flat_map(|p| p.vulnerabilities.iter()) {
            let v = v.to_string();
            if !vulns.contains(&v) {
                vulns.push(v);
            }
        }
    }
    if !vulns.is_empty() {
        out.push(finding("vulnerable", vulns.join(", "), fixed_in.clone()));
    }
    let blocked = matching
        .iter()
        .all(|r| r.policy.as_ref().is_some_and(|p| p.is_blocked()));
    if blocked {
        let reason = matching[0]
            .policy
            .as_ref()
            .map(|p| p.block_reason())
            .unwrap_or_default();
        out.push(finding("blocked", reason, fixed_in));
    }
    if let Some(date) = archived {
        out.push(finding("archived", format!("archived on {}", date), None));
    }
    Some(out)
}

/// The nearest version after `version` that a repository serves and that
/// has no known vulnerabilities.
fn nearest_good_version(
    version: &RPackageVersion,
    rows: &[AuditVersion],
) -> Option<RPackageVersion> {
    let mut by_version: BTreeMap<&RPackageVersion, Vec<&AuditVersion>> = BTreeMap::new();
    for row in rows.iter().filter(|r| r.version > *version) {
        by_version.entry(&row.version).or_default().push(row);
    }
    by_version
        .into_iter()
        .find(|(_, rows)| {
            let served = rows
                .iter()
                .any(|r| !r.policy.as_ref().is_some_and(|p| p.is_blocked()));
            let vulnerable = rows
                .iter()
                .any(|r| r.policy.as_ref().is_some_and(|p| p.is_vulnerable()));
            served && !vulnerable
        })
        .map(|(v, _)| v.clone())
}

/// `fix_cmd` adds a line with the number of problems found, and the hint
/// to run it, e.g. `rig audit --fix`.
fn print_report(report: &Report, fix_cmd: Option<&str>) {
    if !report.has_policy_data {
        OUTPUT.warn(
            "None of the repositories reports vulnerabilities or blocked versions, \
             the results are incomplete. A Posit Package Manager repository with \
             its own metadata does.",
        );
    }
    if !report.findings.is_empty() {
        let mut tab = Table::new("{:<}  {:<}  {:<}  {:<}  {:<}  {:<}");
        tab.add_row(
            Row::new()
                .with_cell("package")
                .with_cell("version")
                .with_cell("issue")
                .with_cell("details")
                .with_cell("fixed in")
                .with_cell("where"),
        );
        for f in &report.findings {
            tab.add_row(
                Row::new()
                    .with_cell(&f.package)
                    .with_cell(&f.version)
                    .with_cell(f.issue)
                    .with_cell(&f.details)
                    .with_cell(f.fixed_in.as_deref().unwrap_or(""))
                    .with_cell(short_places(&f.places)),
            );
        }
        print!("{}", tab);
    }
    if !report.not_checked.is_empty() {
        let msg = format!(
            "Not in any repository, not checked: {}.",
            report.not_checked.join(", ")
        );
        info!("{}", msg);
        OUTPUT.info(&msg);
    }

    let count = |issue: &str| report.findings.iter().filter(|f| f.issue == issue).count();
    let found: Vec<String> = [
        ("vulnerable", count("vulnerable")),
        ("blocked", count("blocked")),
    ]
    .into_iter()
    .filter(|(_, n)| *n > 0)
    .map(|(issue, n)| {
        format!(
            "{} {} package version{}",
            n,
            issue,
            if n == 1 { "" } else { "s" }
        )
    })
    .collect();
    if found.is_empty() {
        OUTPUT.success("No known vulnerabilities or blocked packages.");
    } else if let Some(fix_cmd) = fix_cmd {
        OUTPUT.warn(&format!(
            "Found {}. `{}` upgrades them.",
            found.join(" and "),
            fix_cmd
        ));
    }
}

/// The places of a finding, for the table: the targets of the lock file
/// are summarized, e.g. `rproj.lock (4 targets)`.
fn short_places(places: &[String]) -> String {
    let lock = places
        .iter()
        .filter(|p| p.starts_with("rproj.lock"))
        .count();
    let mut out: Vec<String> = vec![];
    match lock {
        0 => {}
        1 => out.extend(
            places
                .iter()
                .filter(|p| p.starts_with("rproj.lock"))
                .cloned(),
        ),
        n => out.push(format!("rproj.lock ({} targets)", n)),
    }
    out.extend(
        places
            .iter()
            .filter(|p| !p.starts_with("rproj.lock"))
            .cloned(),
    );
    out.join(", ")
}

/// The `--with-repos` and `--without-repos` arguments, for the commands
/// that fix the problems.
fn repos_argv(args: &ArgMatches) -> Vec<String> {
    let mut out = vec![];
    for name in ["with-repos", "without-repos"] {
        for value in args.get_many::<String>(name).into_iter().flatten() {
            if value == "ALL REPOSITORIES" {
                out.push(format!("--{}", name));
            } else {
                out.push(format!("--{}={}", name, value));
            }
        }
    }
    out
}

/// The constraints on `package` in `deps`, e.g. `==1.5`, if `version` does
/// not satisfy them.
fn manifest_conflict(deps: &PackageDependencies, package: &str, version: &str) -> Option<String> {
    let ver = RPackageVersion::from_str(version).ok()?;
    let mut conflict = false;
    let mut out: Vec<String> = vec![];
    for dep in deps.dependencies.iter().filter(|d| d.name == package) {
        for cs in &dep.constraints {
            let (op, ok) = match cs.constraint_type {
                VersionConstraintType::Less => ("<", ver < cs.version),
                VersionConstraintType::LessOrEqual => ("<=", ver <= cs.version),
                VersionConstraintType::Equal => ("==", ver == cs.version),
                VersionConstraintType::Greater => (">", ver > cs.version),
                VersionConstraintType::GreaterOrEqual => (">=", ver >= cs.version),
            };
            conflict |= !ok;
            out.push(format!("{}{}", op, cs.version));
        }
    }
    if conflict {
        Some(out.join(", "))
    } else {
        None
    }
}

/// An upgrade of a package by `--fix`.
struct Fix {
    /// The installed or locked versions, usually only one.
    from: Vec<String>,
    to: String,
    /// The vulnerability ids, or `blocked`.
    why: Vec<String>,
}

/// Upgrade the packages of `fixes` to their versions, see the module docs.
/// `repos` are the `--with-repos` and `--without-repos` arguments.
fn run_fix(
    fixer: &Fixer,
    fixes: &BTreeMap<String, Fix>,
    repos: &[String],
    dry_run: bool,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let specs: Vec<String> = fixes
        .iter()
        .map(|(pkg, fix)| format!("{}@=={}", pkg, fix.to))
        .collect();
    for (pkg, fix) in fixes {
        OUTPUT.info(&format!(
            "{} {} from {} to {} ({})",
            if dry_run {
                "Would upgrade"
            } else {
                "Upgrading"
            },
            pkg,
            fix.from.join(", "),
            fix.to,
            fix.why.join(", ")
        ));
    }
    match fixer {
        Fixer::Project { root, .. } => {
            if dry_run {
                OUTPUT.info(&format!(
                    "Dry run, not updating the project at {}",
                    root.display()
                ));
                return Ok(());
            }
            let mut argv = vec!["rig".to_string(), "proj".to_string(), "lock".to_string()];
            argv.extend(repos.iter().cloned());
            for spec in &specs {
                argv.push("--upgrade-package".to_string());
                argv.push(spec.clone());
            }
            run_rig(&argv, mainargs)?;
            let mut argv = vec!["rig".to_string(), "proj".to_string(), "sync".to_string()];
            argv.extend(repos.iter().cloned());
            run_rig(&argv, mainargs)
        }
        Fixer::Library { library, rversion } => {
            let mut argv = vec![
                "rig".to_string(),
                "pkg".to_string(),
                "install".to_string(),
                "--library".to_string(),
                library.display().to_string(),
            ];
            if let Some(rver) = rversion {
                argv.push("--r-version".to_string());
                argv.push(rver.clone());
            }
            if dry_run {
                argv.push("--dry-run".to_string());
            }
            argv.extend(repos.iter().cloned());
            argv.extend(specs);
            run_rig(&argv, mainargs)
        }
    }
}

/// Run the rig command `argv` in this process.
fn run_rig(argv: &[String], mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    info!("Running {}", argv.join(" "));
    let mut argv = argv.to_vec();
    if mainargs.get_flag("json") {
        argv.insert(1, "--json".to_string());
    }
    let matches = crate::args::rig_app().try_get_matches_from(&argv)?;
    match matches.subcommand() {
        Some(("proj", sub)) => crate::proj::sc_proj(sub, &matches),
        Some(("pkg", sub)) => crate::pkg::sc_pkg(sub, &matches),
        _ => bail!("Internal error, cannot run {}", argv.join(" ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repos::policy::{VersionPolicy, Vulnerability};

    fn row(version: &str, policy: Option<VersionPolicy>) -> AuditVersion {
        AuditVersion {
            version: RPackageVersion::from_str(version).unwrap(),
            sha256sum: None,
            policy,
        }
    }

    fn vulnerable(id: &str) -> Option<VersionPolicy> {
        Some(VersionPolicy {
            vulnerabilities: vec![Vulnerability {
                id: id.to_string(),
                score: None,
            }],
            ..Default::default()
        })
    }

    fn blocked() -> Option<VersionPolicy> {
        Some(VersionPolicy {
            blocked: Some("package".to_string()),
            blocked_rule: Some("9".to_string()),
            ..Default::default()
        })
    }

    fn v(s: &str) -> RPackageVersion {
        RPackageVersion::from_str(s).unwrap()
    }

    #[test]
    fn vulnerable_version_is_fixed_by_the_nearest_good_one() {
        let rows = vec![
            row("1.5", vulnerable("RSEC-2023-3")),
            row("1.6", vulnerable("RSEC-2023-3")),
            row("1.7", blocked()),
            row("1.8", None),
            row("1.9", None),
        ];
        let found = check_version("jsonlite", &v("1.5"), None, &rows, None).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].issue, "vulnerable");
        assert_eq!(found[0].details, "RSEC-2023-3");
        assert_eq!(found[0].fixed_in.as_deref(), Some("1.8"));

        let found = check_version("jsonlite", &v("1.8"), None, &rows, None).unwrap();
        assert!(found.is_empty());

        assert!(check_version("jsonlite", &v("0.1"), None, &rows, None).is_none());
    }

    #[test]
    fn blocked_and_archived_versions_are_reported() {
        let rows = vec![row("0.2.0", None), row("0.3.0", blocked())];
        let found = check_version(
            "otel",
            &v("0.3.0"),
            None,
            &rows,
            Some("2026-01-01".to_string()),
        )
        .unwrap();
        let issues: Vec<&str> = found.iter().map(|f| f.issue).collect();
        assert_eq!(issues, vec!["blocked", "archived"]);
        assert_eq!(found[0].details, "package rule 9");
        assert_eq!(found[0].fixed_in, None);
        assert!(found[0].fails());
        assert!(!found[1].fails());
    }

    #[test]
    fn a_version_that_another_repository_serves_is_not_blocked() {
        let rows = vec![row("0.3.0", blocked()), row("0.3.0", None)];
        let found = check_version("otel", &v("0.3.0"), None, &rows, None).unwrap();
        assert!(found.is_empty());
    }

    #[test]
    fn manifest_constraints_can_block_the_fix() {
        let mut deps = PackageDependencies::from_str("jsonlite (== 1.5), otel", "Imports").unwrap();
        deps.append(&mut PackageDependencies::from_str("cli (>= 3.0)", "Imports").unwrap());
        assert_eq!(
            manifest_conflict(&deps, "jsonlite", "1.8").as_deref(),
            Some("==1.5")
        );
        assert_eq!(manifest_conflict(&deps, "jsonlite", "1.5"), None);
        assert_eq!(manifest_conflict(&deps, "otel", "0.3.0"), None);
        assert_eq!(manifest_conflict(&deps, "cli", "3.6.0"), None);
        assert_eq!(manifest_conflict(&deps, "other", "1.0"), None);
    }

    #[test]
    fn the_hash_picks_the_record() {
        let mut a = row("1.0", vulnerable("X-1"));
        a.sha256sum = Some("aaa".to_string());
        let mut b = row("1.0", None);
        b.sha256sum = Some("bbb".to_string());
        let rows = vec![a, b];
        let found = check_version("p", &v("1.0"), Some("bbb"), &rows, None).unwrap();
        assert!(found.is_empty());
        let found = check_version("p", &v("1.0"), Some("aaa"), &rows, None).unwrap();
        assert_eq!(found[0].issue, "vulnerable");
        let found = check_version("p", &v("1.0"), None, &rows, None).unwrap();
        assert_eq!(found[0].issue, "vulnerable");
    }
}
