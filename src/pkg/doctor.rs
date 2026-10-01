//! `rig pkg doctor`: find problems with the packages installed in a library.
//!
//! Like [`super::list`], this reads the library directory on disk and never
//! starts R. It checks that every package's `Depends` and `Imports` (and, with
//! `--dev`, `Suggests` and `Enhances`) are installed, in a version that satisfies the
//! package's requirements, and that packages compiled against a `LinkingTo`
//! dependency still match the version of that dependency that is installed
//! now.
//!
//! `LinkingTo` packages are only needed to compile a package, not to use it,
//! so a `LinkingTo` package that is not installed is not a problem. If it is
//! installed, though, and it is not the one the package was compiled against,
//! the package may not work. rig records what a package was compiled against
//! when it installs it (`RemoteLinkingToHashes`, see
//! [`crate::install::REMOTE_LINKINGTO_FIELD`]), so for those packages this is
//! an exact check. For packages installed by R, pak or renv, the best rig can
//! do is compare the `Built` timestamps, which is only a warning.
//!
//! Dependencies are looked up in the library first, then in the system
//! library of the R version (`.Library`), which holds the base and the
//! recommended packages.

use std::collections::{HashMap, HashSet};
use std::env;
use std::error::Error;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use clap::ArgMatches;
use log::debug;
use tabular::*;

#[cfg(target_os = "macos")]
use crate::macos::{get_r_binary, sc_get_default};

#[cfg(target_os = "windows")]
use crate::windows::{get_r_binary, sc_get_default};

#[cfg(target_os = "linux")]
use crate::linux::{get_r_binary, sc_get_default};

use crate::built::r_platform;
use crate::common::{check_installed, get_r_syslib_dir, get_r_version_data_version};
use crate::dcf::{DepVersionSpec, RDepType};
use crate::output::OUTPUT;
use crate::proj::BASE_PKGS;

use super::list::{
    print_table, read_installed, read_library, resolve_library, InstalledPackage, LibraryContents,
};

pub fn sc_pkg_doctor(
    args: &ArgMatches,
    pkgargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let json = args.get_flag("json") || pkgargs.get_flag("json") || mainargs.get_flag("json");
    let checks = Checks {
        dev: args.get_flag("dev"),
        stale: args.get_flag("stale"),
    };

    let lib = resolve_library(args)?;
    let contents = read_library(&lib.path)?;

    // A library given as a path does not belong to an R version, so check it
    // against the one `--r-version` names, or else the default one.
    let rver = match &lib.rversion {
        Some(rver) => Some(rver.clone()),
        None => match args.get_one::<String>("r-version") {
            Some(rver) => Some(check_installed(rver)?),
            None => {
                let rver = sc_get_default()?;
                match &rver {
                    Some(rver) => OUTPUT.info(&format!(
                        "Checking {} against the default R version, R {}. \
                        Use `--r-version` to select another one.",
                        lib.path.display(),
                        rver
                    )),
                    None => OUTPUT.info(
                        "No default R version, skipping the checks that need one. \
                        Use `--r-version` to select an R version.",
                    ),
                }
                rver
            }
        },
    };

    let (rinfo, syslib) = match &rver {
        Some(rver) => r_info(rver),
        None => (RInfo::default(), vec![]),
    };

    let mut problems = diagnose(&contents, &syslib, &rinfo, &checks);

    let mut checked = contents.pkgs.len();
    if let Some(names) = args.get_many::<String>("package") {
        let names: Vec<&str> = names.map(|x| x.as_str()).collect();
        for name in names.iter() {
            if !contents.pkgs.iter().any(|p| p.package == *name) {
                bail!("{} is not installed in {}", name, lib.path.display());
            }
        }
        problems.retain(|p| names.contains(&p.package.as_str()));
        checked = names.len();
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&problems)?);
    } else {
        print_problems(&lib.tag(), checked, &problems);
        print_hint(args, &problems);
    }

    let errors = problems
        .iter()
        .filter(|p| p.severity == Severity::Error)
        .count();
    if errors > 0 {
        if !json {
            println!();
        }
        bail!(
            "Found {} {} in {}",
            errors,
            if errors == 1 { "error" } else { "errors" },
            lib.path.display()
        );
    }

    Ok(())
}

// ------------------------------------------------------------------------
// The R version of the library

/// What the checks need to know about the R version a library belongs to.
/// Everything is unknown for a library given as a plain path.
#[derive(Debug, Default)]
pub(crate) struct RInfo {
    /// The R version number, e.g. `4.4.1`, from the base package.
    pub(crate) version: Option<String>,
    /// `R_PLATFORM`, e.g. `aarch64-apple-darwin20`.
    pub(crate) platform: Option<String>,
}

/// Look up the R version `rver`, and read its system library. Whatever cannot
/// be found is left unknown, and the checks that need it are skipped.
fn r_info(rver: &str) -> (RInfo, Vec<InstalledPackage>) {
    let version = get_r_version_data_version(rver)
        .map_err(|err| debug!("Cannot find version of R {}: {}", rver, err))
        .ok();
    let platform = get_r_binary(rver)
        .map_err(|err| debug!("Cannot find R {} binary: {}", rver, err))
        .ok()
        .and_then(|bin| r_platform(&bin.to_string_lossy()));

    let syslib: Vec<InstalledPackage> = get_r_syslib_dir(rver)
        .and_then(|dir: PathBuf| read_installed(&dir))
        .unwrap_or_else(|err| {
            debug!("Cannot read system library of R {}: {}", rver, err);
            vec![]
        });

    (RInfo { version, platform }, syslib)
}

// ------------------------------------------------------------------------
// Problems

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Severity {
    Error,
    Warning,
}

impl Severity {
    fn as_str(&self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Kind {
    /// A dependency is not installed.
    Missing,
    /// A dependency is installed, but not in a suitable version.
    Version,
    /// The package needs a newer (or older) R.
    RVersion,
    /// Compiled against another version of a `LinkingTo` dependency.
    Abi,
    /// A `LinkingTo` dependency was built after the package, so the package
    /// may have been compiled against an older version of it.
    Stale,
    /// Built for another R minor version.
    BuiltR,
    /// Built for another architecture.
    Platform,
    /// The `Package` field does not match the directory name.
    Name,
    /// A lock directory of an interrupted installation.
    Lock,
    /// A directory that cannot be read as a package, or a package whose
    /// dependencies cannot be parsed.
    Broken,
}

impl Kind {
    fn as_str(&self) -> &'static str {
        match self {
            Kind::Missing => "missing",
            Kind::Version => "version",
            Kind::RVersion => "r-version",
            Kind::Abi => "abi",
            Kind::Stale => "stale",
            Kind::BuiltR => "built-r",
            Kind::Platform => "platform",
            Kind::Name => "name",
            Kind::Lock => "lock",
            Kind::Broken => "broken",
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct Problem {
    pub(crate) package: String,
    pub(crate) version: String,
    pub(crate) severity: Severity,
    pub(crate) problem: Kind,
    /// The dependency the problem is about, if it is about one.
    pub(crate) dependency: Option<String>,
    pub(crate) details: String,
    /// The directory the problem is in, for the commands that fix it. Not
    /// part of the JSON output.
    #[serde(skip)]
    pub(crate) path: Option<PathBuf>,
}

// ------------------------------------------------------------------------
// Checks

/// The optional checks, off by default.
#[derive(Debug, Default)]
pub(crate) struct Checks {
    /// `--dev`: also check `Suggests` and `Enhances`.
    pub(crate) dev: bool,
    /// `--stale`: compare `Built` timestamps for packages without a record of
    /// what they were compiled against. Off by default because binary
    /// packages from CRAN are not rebuilt when a `LinkingTo` dependency
    /// changes, so this flags many packages that are fine.
    pub(crate) stale: bool,
}

/// Find the problems of the packages in `lib`. `syslib` is the system library
/// of the R version, where dependencies are looked up after `lib`.
pub(crate) fn diagnose(
    lib: &LibraryContents,
    syslib: &[InstalledPackage],
    rinfo: &RInfo,
    checks: &Checks,
) -> Vec<Problem> {
    // The library comes before the system library on `.libPaths()`, so a
    // package in both is found in the library.
    let mut lookup: HashMap<&str, &InstalledPackage> = HashMap::new();
    for pkg in syslib.iter().chain(lib.pkgs.iter()) {
        lookup.insert(pkg.package.as_str(), pkg);
    }

    let mut problems = vec![];
    for pkg in lib.pkgs.iter() {
        check_package(pkg, &lookup, rinfo, checks, &mut problems);
    }

    for lock in lib.locks.iter() {
        problems.push(Problem {
            package: lock.clone(),
            version: "-".to_string(),
            severity: Severity::Warning,
            problem: Kind::Lock,
            dependency: None,
            details: "lock directory left behind by an interrupted installation".to_string(),
            path: Some(lib.path.join(lock)),
        });
    }
    for (dir, reason) in lib.broken.iter() {
        problems.push(Problem {
            package: dir.clone(),
            version: "-".to_string(),
            severity: Severity::Warning,
            problem: Kind::Broken,
            dependency: None,
            details: reason.clone(),
            path: Some(lib.path.join(dir)),
        });
    }

    problems.sort_by(|a, b| {
        a.package
            .to_lowercase()
            .cmp(&b.package.to_lowercase())
            .then_with(|| a.package.cmp(&b.package))
            .then_with(|| a.severity.cmp(&b.severity))
            .then_with(|| a.dependency.cmp(&b.dependency))
    });
    problems
}

fn check_package(
    pkg: &InstalledPackage,
    lookup: &HashMap<&str, &InstalledPackage>,
    rinfo: &RInfo,
    checks: &Checks,
    problems: &mut Vec<Problem>,
) {
    let mut add = |severity: Severity, problem: Kind, dependency: Option<&str>, details: String| {
        problems.push(Problem {
            package: pkg.package.clone(),
            version: pkg.version.clone(),
            severity,
            problem,
            dependency: dependency.map(|x| x.to_string()),
            details,
            path: Some(pkg.path.clone()),
        })
    };

    // -- Housekeeping ------------------------------------------------------
    if let Some(dir) = pkg.path.file_name().and_then(|x| x.to_str()) {
        if dir != pkg.package {
            add(
                Severity::Warning,
                Kind::Name,
                None,
                format!("installed in directory {}", dir),
            );
        }
    }
    for err in pkg.deps_errors.iter() {
        add(
            Severity::Warning,
            Kind::Broken,
            None,
            format!("cannot parse {}", err),
        );
    }

    // -- Dependencies ------------------------------------------------------
    for dep in pkg.deps.iter() {
        let dep_type = match dep.types.first() {
            Some(x) => x,
            None => continue,
        };
        let severity = match dep_type {
            RDepType::Depends | RDepType::Imports => Severity::Error,
            RDepType::Suggests | RDepType::Enhances if checks.dev => Severity::Warning,
            // `LinkingTo` is only needed at install time, see the ABI checks
            // below.
            _ => continue,
        };

        if dep.name == "R" {
            if let Some(rver) = &rinfo.version {
                if let Ok(false) = dep.satisfies(rver) {
                    add(
                        severity,
                        Kind::RVersion,
                        Some("R"),
                        format!("{} {}, R {} is used", dep_type, spec_str(dep), rver),
                    );
                }
            }
            continue;
        }

        let inst = match lookup.get(dep.name.as_str()) {
            Some(x) => x,
            None if BASE_PKGS.contains(&dep.name.as_str()) => continue,
            None => {
                add(
                    severity,
                    Kind::Missing,
                    Some(&dep.name),
                    format!("{} {}, not installed", dep_type, spec_str(dep)),
                );
                continue;
            }
        };

        if dep.constraints.is_empty() {
            continue;
        }
        match dep.satisfies(&inst.version) {
            Ok(true) => {}
            Ok(false) => add(
                severity,
                Kind::Version,
                Some(&dep.name),
                format!(
                    "{} {}, {} is installed",
                    dep_type,
                    spec_str(dep),
                    inst.version
                ),
            ),
            Err(_) => add(
                Severity::Warning,
                Kind::Version,
                Some(&dep.name),
                format!(
                    "{} {}, cannot parse installed version {}",
                    dep_type,
                    spec_str(dep),
                    inst.version
                ),
            ),
        }
    }

    // -- ABI ---------------------------------------------------------------
    if !pkg.linkingto.is_empty() {
        // rig installed this package and recorded what it was compiled
        // against, so compare that to what is installed now.
        for (dep, ver, sha) in pkg.linkingto.iter() {
            let inst = match lookup.get(dep.as_str()) {
                Some(x) => x,
                None => continue,
            };
            let same = match &inst.hash {
                Some(hash) => hash == sha,
                None => inst.version == *ver,
            };
            if same {
                continue;
            }
            let details = if inst.version != *ver {
                format!(
                    "compiled against {} {}, {} is installed",
                    dep, ver, inst.version
                )
            } else {
                format!("compiled against another build of {} {}", dep, ver)
            };
            add(Severity::Error, Kind::Abi, Some(dep), details);
        }
    } else if let Some(built) = pkg
        .built_at
        .as_deref()
        .filter(|_| checks.stale)
        .and_then(parse_timestamp)
    {
        // No record, so all rig can tell is whether a `LinkingTo` dependency
        // was built after the package.
        for dep in pkg.deps.iter() {
            if !dep.types.contains(&RDepType::LinkingTo) {
                continue;
            }
            let inst = match lookup.get(dep.name.as_str()) {
                Some(x) => x,
                None => continue,
            };
            let dep_built = match inst.built_at.as_deref().and_then(parse_timestamp) {
                Some(x) => x,
                None => continue,
            };
            if dep_built > built {
                add(
                    Severity::Warning,
                    Kind::Stale,
                    Some(&dep.name),
                    format!(
                        "{} {} was built after this package, which may need a reinstall",
                        dep.name, inst.version
                    ),
                );
            }
        }
    }

    // -- Built for this R --------------------------------------------------
    if let (Some(rver), Some(built_r)) = (&rinfo.version, &pkg.built_r) {
        if minor_version(rver) != minor_version(built_r) {
            add(
                Severity::Warning,
                Kind::BuiltR,
                None,
                format!("built for R {}, R {} is used", built_r, rver),
            );
        }
    }
    if let (Some(rplat), Some(plat)) = (&rinfo.platform, &pkg.platform) {
        if arch(rplat) != arch(plat) {
            add(
                Severity::Warning,
                Kind::Platform,
                None,
                format!("built for {}, R is {}", plat, rplat),
            );
        }
    }
}

/// A dependency with its version requirements, as `DESCRIPTION` writes it,
/// e.g. `cli (>= 3.6.0)`.
fn spec_str(dep: &DepVersionSpec) -> String {
    if dep.constraints.is_empty() {
        return dep.name.clone();
    }
    let cons: Vec<String> = dep
        .constraints
        .iter()
        .map(|c| format!("{} {}", c.constraint_type, c.version))
        .collect();
    format!("{} ({})", dep.name, cons.join(", "))
}

/// The `x.y` part of an R version.
fn minor_version(ver: &str) -> String {
    ver.split(['.', '-']).take(2).collect::<Vec<_>>().join(".")
}

/// The architecture part of a platform string, e.g. `aarch64` of
/// `aarch64-apple-darwin20`.
fn arch(platform: &str) -> &str {
    platform.split('-').next().unwrap_or(platform)
}

/// A `Built` timestamp, as R writes it: `2024-06-21 20:16:33 UTC`. It is
/// always in UTC, so the numbers, in order, compare like the times do.
fn parse_timestamp(ts: &str) -> Option<Vec<u32>> {
    let ts = ts.trim().strip_suffix("UTC")?.trim();
    let (date, time) = ts.split_once(' ')?;
    let mut out = vec![];
    for part in date.split('-').chain(time.split(':')) {
        out.push(part.parse::<u32>().ok()?);
    }
    if out.len() == 6 {
        Some(out)
    } else {
        None
    }
}

// ------------------------------------------------------------------------
// Output

fn print_problems(tag: &str, checked: usize, problems: &[Problem]) {
    use owo_colors::OwoColorize;

    let color = std::io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none();
    let pkg_word = if checked == 1 { "package" } else { "packages" };
    let tag = if color {
        tag.dimmed().to_string()
    } else {
        tag.to_string()
    };

    if problems.is_empty() {
        let head = format!("No problems found in {} {}", checked, pkg_word);
        let head = if color {
            head.green().bold().to_string()
        } else {
            head
        };
        println!("{} {}", head, tag);
        return;
    }

    let nerr = problems
        .iter()
        .filter(|p| p.severity == Severity::Error)
        .count();
    let nwarn = problems.len() - nerr;
    let plural = |n: usize, word: &str| {
        if n == 1 {
            format!("{} {}", n, word)
        } else {
            format!("{} {}s", n, word)
        }
    };
    let head = format!(
        "{} ({}, {}) in {} {}",
        plural(problems.len(), "problem"),
        plural(nerr, "error"),
        plural(nwarn, "warning"),
        checked,
        pkg_word
    );
    let head = if !color {
        head
    } else if nerr > 0 {
        head.red().bold().to_string()
    } else {
        head.yellow().bold().to_string()
    };
    println!("{} {}", head, tag);
    println!();

    let mut tab: Table = Table::new("{:<}   {:<}   {:<}   {:<}   {:<}");
    tab.add_row(row!("Package", "Version", "Severity", "Problem", "Details"));
    for p in problems {
        tab.add_row(row!(
            &p.package,
            &p.version,
            p.severity.as_str(),
            p.problem.as_str(),
            &p.details
        ));
    }
    print_table(&tab);
}

/// Commands that fix the problems, as `(what they fix, commands)` pairs, in the
/// order they should be printed.
///
/// A package with a missing or too old dependency needs that dependency
/// installed, and a package that may have been compiled against another
/// version of a `LinkingTo` dependency, or was built for another R, needs to
/// be reinstalled. `rig pkg install` does both: it installs whatever is
/// missing or outdated, and replaces a package that it has no record of, or
/// whose record does not match. Leftover lock directories and broken packages
/// are removed. The rest (`r-version`, `name`) needs a human.
fn fix_commands(
    problems: &[Problem],
    library: Option<&str>,
    rversion: Option<&str>,
) -> Vec<(&'static str, Vec<String>)> {
    let install_target = |p: &Problem| match p.problem {
        Kind::Missing | Kind::Version => p.dependency.clone(),
        Kind::Abi | Kind::Stale | Kind::BuiltR | Kind::Platform => Some(p.package.clone()),
        _ => None,
    };
    let install_cmd = |targets: &[String]| {
        let mut cmd = format!("rig pkg install {}", targets.join(" "));
        if let Some(lib) = library {
            cmd.push_str(&format!(" --library {}", shell_quote(lib)));
        }
        if let Some(rver) = rversion {
            cmd.push_str(&format!(" --r-version {}", shell_quote(rver)));
        }
        cmd
    };

    let mut seen = HashSet::new();
    let mut targets = |severity: Severity| -> Vec<String> {
        problems
            .iter()
            .filter(|p| p.severity == severity)
            .filter_map(install_target)
            .filter(|t| seen.insert(t.clone()))
            .collect()
    };
    let errors = targets(Severity::Error);
    // Whatever the errors' command installs is not repeated here.
    let warnings = targets(Severity::Warning);

    let mut out = vec![];
    if !errors.is_empty() {
        out.push(("To fix the errors, run:", vec![install_cmd(&errors)]));
    }
    if !warnings.is_empty() {
        out.push(("To fix the warnings, run:", vec![install_cmd(&warnings)]));
    }

    // A package with several unparseable dependency fields is only removed
    // once.
    let mut seen_dirs = HashSet::new();
    let mut remove_cmds = |kind: Kind| -> Vec<String> {
        problems
            .iter()
            .filter(|p| p.problem == kind)
            .filter_map(|p| p.path.as_ref())
            .filter(|path| seen_dirs.insert(path.to_path_buf()))
            .map(|path| remove_dir_cmd(path))
            .collect()
    };
    let locks = remove_cmds(Kind::Lock);
    if !locks.is_empty() {
        out.push((
            "To remove the leftover lock directories, if no installation is running, run:",
            locks,
        ));
    }
    let broken = remove_cmds(Kind::Broken);
    if !broken.is_empty() {
        out.push(("To remove the broken packages, run:", broken));
    }

    out
}

#[cfg(not(target_os = "windows"))]
fn remove_dir_cmd(path: &Path) -> String {
    format!("rm -rf {}", shell_quote(&path.to_string_lossy()))
}

#[cfg(target_os = "windows")]
fn remove_dir_cmd(path: &Path) -> String {
    format!(
        "Remove-Item -Recurse -Force {}",
        shell_quote(&path.to_string_lossy())
    )
}

/// Quote `x` for the shell, if it needs it. Single quotes work the same way in
/// POSIX shells and in PowerShell, except for how a single quote itself is
/// escaped.
fn shell_quote(x: &str) -> String {
    let plain = !x.is_empty()
        && x.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-./:@=+,".contains(c));
    if plain {
        return x.to_string();
    }
    let escaped = if cfg!(target_os = "windows") {
        x.replace('\'', "''")
    } else {
        x.replace('\'', "'\\''")
    };
    format!("'{}'", escaped)
}

fn print_hint(args: &ArgMatches, problems: &[Problem]) {
    let fixes = fix_commands(
        problems,
        args.get_one::<String>("library").map(|x| x.as_str()),
        args.get_one::<String>("r-version").map(|x| x.as_str()),
    );
    for (what, cmds) in fixes {
        println!();
        println!("{}", what);
        for cmd in cmds {
            println!("  {}", cmd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a library with packages in it, from `(directory, DESCRIPTION)`
    /// pairs, and read it back.
    fn library(pkgs: &[(&str, &str)]) -> (tempfile::TempDir, LibraryContents) {
        let tmp = tempfile::tempdir().unwrap();
        for (dir, desc) in pkgs {
            add_package(tmp.path(), dir, desc);
        }
        let contents = read_library(tmp.path()).unwrap();
        (tmp, contents)
    }

    fn add_package(lib: &Path, dir: &str, desc: &str) {
        let dir = lib.join(dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("DESCRIPTION"), desc).unwrap();
    }

    const NONE: Checks = Checks {
        dev: false,
        stale: false,
    };
    const DEV: Checks = Checks {
        dev: true,
        stale: false,
    };
    const STALE: Checks = Checks {
        dev: false,
        stale: true,
    };

    fn r44() -> RInfo {
        RInfo {
            version: Some("4.4.1".to_string()),
            platform: Some("aarch64-apple-darwin20".to_string()),
        }
    }

    /// `(package, severity, problem, dependency)` of each problem, to compare
    /// against.
    fn summary(problems: &[Problem]) -> Vec<(String, Severity, Kind, Option<String>)> {
        problems
            .iter()
            .map(|p| {
                (
                    p.package.clone(),
                    p.severity,
                    p.problem,
                    p.dependency.clone(),
                )
            })
            .collect()
    }

    fn one(
        pkg: &str,
        severity: Severity,
        kind: Kind,
        dep: Option<&str>,
    ) -> (String, Severity, Kind, Option<String>) {
        (pkg.to_string(), severity, kind, dep.map(|x| x.to_string()))
    }

    #[test]
    fn a_healthy_library_has_no_problems() {
        let (_tmp, lib) = library(&[
            (
                "cli",
                "Package: cli\nVersion: 3.6.3\nDepends: R (>= 3.4)\nImports: utils\n",
            ),
            (
                "glue",
                "Package: glue\nVersion: 1.8.0\nImports: cli (>= 3.0.0), methods\n",
            ),
        ]);
        assert!(diagnose(&lib, &[], &r44(), &NONE).is_empty());
    }

    #[test]
    fn missing_dependencies_are_errors() {
        let (_tmp, lib) = library(&[(
            "glue",
            "Package: glue\nVersion: 1.8.0\nDepends: rlang\nImports: cli (>= 3.0.0)\n",
        )]);
        let problems = diagnose(&lib, &[], &r44(), &NONE);
        assert_eq!(
            summary(&problems),
            vec![
                one("glue", Severity::Error, Kind::Missing, Some("cli")),
                one("glue", Severity::Error, Kind::Missing, Some("rlang")),
            ]
        );
        assert_eq!(problems[0].details, "Imports cli (>= 3.0.0), not installed");
    }

    #[test]
    fn a_missing_linkingto_dependency_is_not_a_problem() {
        let (_tmp, lib) = library(&[("fs", "Package: fs\nVersion: 1.6.4\nLinkingTo: cpp11\n")]);
        assert!(diagnose(&lib, &[], &r44(), &NONE).is_empty());
    }

    #[test]
    fn a_linkingto_dependency_that_is_also_imported_must_be_installed() {
        let (_tmp, lib) = library(&[(
            "foo",
            "Package: foo\nVersion: 1.0.0\nImports: Rcpp\nLinkingTo: Rcpp\n",
        )]);
        assert_eq!(
            summary(&diagnose(&lib, &[], &r44(), &NONE)),
            vec![one("foo", Severity::Error, Kind::Missing, Some("Rcpp"))]
        );
    }

    #[test]
    fn an_old_dependency_is_an_error() {
        let (_tmp, lib) = library(&[
            ("cli", "Package: cli\nVersion: 3.4.1\n"),
            (
                "glue",
                "Package: glue\nVersion: 1.8.0\nImports: cli (>= 3.6.0)\n",
            ),
        ]);
        let problems = diagnose(&lib, &[], &r44(), &NONE);
        assert_eq!(
            summary(&problems),
            vec![one("glue", Severity::Error, Kind::Version, Some("cli"))]
        );
        assert_eq!(
            problems[0].details,
            "Imports cli (>= 3.6.0), 3.4.1 is installed"
        );
    }

    #[test]
    fn an_unparseable_installed_version_is_a_warning() {
        let (_tmp, lib) = library(&[
            ("cli", "Package: cli\nVersion: 3.6-alpha\n"),
            (
                "glue",
                "Package: glue\nVersion: 1.8.0\nImports: cli (>= 3.6.0)\n",
            ),
        ]);
        assert_eq!(
            summary(&diagnose(&lib, &[], &r44(), &NONE)),
            vec![one("glue", Severity::Warning, Kind::Version, Some("cli"))]
        );
    }

    #[test]
    fn the_r_version_requirement_is_checked() {
        let (_tmp, lib) = library(&[(
            "new",
            "Package: new\nVersion: 1.0.0\nDepends: R (>= 4.5.0)\n",
        )]);
        assert_eq!(
            summary(&diagnose(&lib, &[], &r44(), &NONE)),
            vec![one("new", Severity::Error, Kind::RVersion, Some("R"))]
        );
        // Unknown R version: nothing to check against.
        assert!(diagnose(&lib, &[], &RInfo::default(), &NONE).is_empty());
    }

    #[test]
    fn dependencies_are_found_in_the_system_library() {
        let (_tmp, lib) = library(&[(
            "foo",
            "Package: foo\nVersion: 1.0.0\nImports: Matrix (>= 1.6-0), stats (>= 4.0.0)\n",
        )]);
        let (_systmp, sys) = library(&[
            ("Matrix", "Package: Matrix\nVersion: 1.7-0\n"),
            ("stats", "Package: stats\nVersion: 4.4.1\n"),
        ]);
        assert!(diagnose(&lib, &sys.pkgs, &r44(), &NONE).is_empty());
        // Without the system library the recommended package is missing, but
        // a base package still counts as installed.
        assert_eq!(
            summary(&diagnose(&lib, &[], &r44(), &NONE)),
            vec![one("foo", Severity::Error, Kind::Missing, Some("Matrix"))]
        );
    }

    #[test]
    fn the_library_wins_over_the_system_library() {
        let (_tmp, lib) = library(&[
            ("Matrix", "Package: Matrix\nVersion: 1.5-0\n"),
            (
                "foo",
                "Package: foo\nVersion: 1.0.0\nImports: Matrix (>= 1.6-0)\n",
            ),
        ]);
        let (_systmp, sys) = library(&[("Matrix", "Package: Matrix\nVersion: 1.7-0\n")]);
        assert_eq!(
            summary(&diagnose(&lib, &sys.pkgs, &r44(), &NONE)),
            vec![one("foo", Severity::Error, Kind::Version, Some("Matrix"))]
        );
    }

    #[test]
    fn suggests_and_enhances_are_only_checked_with_dev() {
        let (_tmp, lib) = library(&[
            ("testthat", "Package: testthat\nVersion: 3.0.0\n"),
            ("zoo", "Package: zoo\nVersion: 1.8-0\n"),
            (
                "foo",
                "Package: foo\nVersion: 1.0.0\nSuggests: covr, testthat (>= 3.2.0)\n\
                 Enhances: chron, zoo (>= 1.8-12)\n",
            ),
        ]);
        assert!(diagnose(&lib, &[], &r44(), &NONE).is_empty());
        assert_eq!(
            summary(&diagnose(&lib, &[], &r44(), &DEV)),
            vec![
                one("foo", Severity::Warning, Kind::Missing, Some("chron")),
                one("foo", Severity::Warning, Kind::Missing, Some("covr")),
                one("foo", Severity::Warning, Kind::Version, Some("testthat")),
                one("foo", Severity::Warning, Kind::Version, Some("zoo")),
            ]
        );
    }

    #[test]
    fn a_recorded_linkingto_hash_mismatch_is_an_error() {
        let (_tmp, lib) = library(&[
            ("cpp11", "Package: cpp11\nVersion: 0.5.0\nRemoteHash: bbb\n"),
            (
                "fs",
                "Package: fs\nVersion: 1.6.4\nLinkingTo: cpp11\n\
                 RemoteLinkingToHashes: cpp11@0.5.0=aaa\n",
            ),
        ]);
        let problems = diagnose(&lib, &[], &r44(), &NONE);
        assert_eq!(
            summary(&problems),
            vec![one("fs", Severity::Error, Kind::Abi, Some("cpp11"))]
        );
        assert_eq!(
            problems[0].details,
            "compiled against another build of cpp11 0.5.0"
        );
    }

    #[test]
    fn a_recorded_linkingto_hash_match_is_fine() {
        let (_tmp, lib) = library(&[
            (
                "cpp11",
                "Package: cpp11\nVersion: 0.5.0\nRemoteHash: aaa\n\
                 Built: R 4.4.1; ; 2025-01-01 00:00:00 UTC; unix\n",
            ),
            (
                "fs",
                "Package: fs\nVersion: 1.6.4\nLinkingTo: cpp11\n\
                 RemoteLinkingToHashes: cpp11@0.5.0=aaa\n\
                 Built: R 4.4.1; ; 2024-01-01 00:00:00 UTC; unix\n",
            ),
        ]);
        // The record wins, the timestamps are not looked at.
        assert!(diagnose(&lib, &[], &r44(), &STALE).is_empty());
    }

    #[test]
    fn without_a_hash_the_recorded_version_is_compared() {
        let (_tmp, lib) = library(&[
            ("cpp11", "Package: cpp11\nVersion: 0.4.7\n"),
            (
                "fs",
                "Package: fs\nVersion: 1.6.4\nLinkingTo: cpp11\n\
                 RemoteLinkingToHashes: cpp11@0.5.0=aaa\n",
            ),
        ]);
        let problems = diagnose(&lib, &[], &r44(), &NONE);
        assert_eq!(
            summary(&problems),
            vec![one("fs", Severity::Error, Kind::Abi, Some("cpp11"))]
        );
        assert_eq!(
            problems[0].details,
            "compiled against cpp11 0.5.0, 0.4.7 is installed"
        );
    }

    #[test]
    fn a_recorded_linkingto_dependency_that_is_gone_is_fine() {
        let (_tmp, lib) = library(&[(
            "fs",
            "Package: fs\nVersion: 1.6.4\nLinkingTo: cpp11\n\
             RemoteLinkingToHashes: cpp11@0.5.0=aaa\n",
        )]);
        assert!(diagnose(&lib, &[], &r44(), &NONE).is_empty());
    }

    #[test]
    fn a_linkingto_dependency_built_later_is_a_warning() {
        let (_tmp, lib) = library(&[
            (
                "cpp11",
                "Package: cpp11\nVersion: 0.5.0\n\
                 Built: R 4.4.1; ; 2025-01-01 00:00:00 UTC; unix\n",
            ),
            (
                "fs",
                "Package: fs\nVersion: 1.6.4\nLinkingTo: cpp11\n\
                 Built: R 4.4.1; aarch64-apple-darwin20; 2024-01-01 00:00:00 UTC; unix\n",
            ),
        ]);
        assert_eq!(
            summary(&diagnose(&lib, &[], &r44(), &STALE)),
            vec![one("fs", Severity::Warning, Kind::Stale, Some("cpp11"))]
        );
        // Only with `--stale`.
        assert!(diagnose(&lib, &[], &r44(), &NONE).is_empty());
    }

    #[test]
    fn a_linkingto_dependency_built_earlier_is_fine() {
        let (_tmp, lib) = library(&[
            (
                "cpp11",
                "Package: cpp11\nVersion: 0.5.0\n\
                 Built: R 4.4.1; ; 2024-01-01 00:00:00 UTC; unix\n",
            ),
            (
                "fs",
                "Package: fs\nVersion: 1.6.4\nLinkingTo: cpp11\n\
                 Built: R 4.4.1; aarch64-apple-darwin20; 2024-06-01 00:00:00 UTC; unix\n",
            ),
        ]);
        assert!(diagnose(&lib, &[], &r44(), &STALE).is_empty());
    }

    #[test]
    fn built_for_another_r_or_arch_is_a_warning() {
        let (_tmp, lib) = library(&[
            (
                "old",
                "Package: old\nVersion: 1.0.0\n\
                 Built: R 4.3.3; aarch64-apple-darwin20; 2024-01-01 00:00:00 UTC; unix\n",
            ),
            (
                "patch",
                "Package: patch\nVersion: 1.0.0\n\
                 Built: R 4.4.0; aarch64-apple-darwin20; 2024-01-01 00:00:00 UTC; unix\n",
            ),
            (
                "intel",
                "Package: intel\nVersion: 1.0.0\n\
                 Built: R 4.4.1; x86_64-apple-darwin20; 2024-01-01 00:00:00 UTC; unix\n",
            ),
        ]);
        assert_eq!(
            summary(&diagnose(&lib, &[], &r44(), &NONE)),
            vec![
                one("intel", Severity::Warning, Kind::Platform, None),
                one("old", Severity::Warning, Kind::BuiltR, None),
            ]
        );
        assert!(diagnose(&lib, &[], &RInfo::default(), &NONE).is_empty());
    }

    #[test]
    fn housekeeping_problems_are_warnings() {
        let (tmp, _) = library(&[
            ("renamed", "Package: other\nVersion: 1.0.0\n"),
            ("bad", "Package: bad\nVersion: 1.0.0\nImports: cli (>= x)\n"),
        ]);
        std::fs::create_dir(tmp.path().join("00LOCK-cli")).unwrap();
        std::fs::create_dir(tmp.path().join("half")).unwrap();
        let lib = read_library(tmp.path()).unwrap();
        assert_eq!(
            summary(&diagnose(&lib, &[], &r44(), &NONE)),
            vec![
                one("00LOCK-cli", Severity::Warning, Kind::Lock, None),
                one("bad", Severity::Warning, Kind::Broken, None),
                one("half", Severity::Warning, Kind::Broken, None),
                one("other", Severity::Warning, Kind::Name, None),
            ]
        );
    }

    #[test]
    fn timestamps_are_parsed() {
        assert_eq!(
            parse_timestamp("2024-06-21 20:16:33 UTC"),
            Some(vec![2024, 6, 21, 20, 16, 33])
        );
        assert_eq!(parse_timestamp("2024-06-21 20:16:33"), None);
        assert_eq!(parse_timestamp("whatever UTC"), None);
    }

    fn problem(package: &str, severity: Severity, kind: Kind, dep: Option<&str>) -> Problem {
        Problem {
            package: package.to_string(),
            version: "1.0.0".to_string(),
            severity,
            problem: kind,
            dependency: dep.map(|x| x.to_string()),
            details: String::new(),
            path: None,
        }
    }

    #[test]
    fn fixes_install_dependencies_and_reinstall_packages() {
        let problems = vec![
            problem("foo", Severity::Error, Kind::Missing, Some("bar")),
            problem("glue", Severity::Error, Kind::Version, Some("cli")),
            problem("fs", Severity::Error, Kind::Abi, Some("cpp11")),
            problem("new", Severity::Error, Kind::RVersion, Some("R")),
            problem("qux", Severity::Warning, Kind::Stale, Some("Rcpp")),
            problem("old", Severity::Warning, Kind::BuiltR, None),
            // Already installed by the errors' command.
            problem("tidyr", Severity::Warning, Kind::Missing, Some("cli")),
            problem("other", Severity::Warning, Kind::Name, None),
        ];
        let fixes = fix_commands(&problems, None, None);
        assert_eq!(
            fixes,
            vec![
                (
                    "To fix the errors, run:",
                    vec!["rig pkg install bar cli fs".to_string()]
                ),
                (
                    "To fix the warnings, run:",
                    vec!["rig pkg install qux old".to_string()]
                ),
            ]
        );
    }

    #[test]
    fn fixes_keep_the_library_and_r_version() {
        let problems = vec![problem("foo", Severity::Error, Kind::Missing, Some("bar"))];
        let fixes = fix_commands(&problems, Some("my lib"), Some("4.4"));
        assert_eq!(
            fixes[0].1,
            vec!["rig pkg install bar --library 'my lib' --r-version 4.4".to_string()]
        );
    }

    #[test]
    fn fixes_remove_lock_directories_and_broken_packages() {
        let (tmp, _) = library(&[(
            "bad",
            "Package: bad\nVersion: 1.0.0\nImports: cli (>= x)\nSuggests: foo (>= y)\n",
        )]);
        std::fs::create_dir(tmp.path().join("00LOCK-cli")).unwrap();
        std::fs::create_dir(tmp.path().join("half")).unwrap();
        let lib = read_library(tmp.path()).unwrap();
        let problems = diagnose(&lib, &[], &r44(), &NONE);
        let rm = |dir: &str| remove_dir_cmd(&tmp.path().join(dir));
        assert_eq!(
            fix_commands(&problems, None, None),
            vec![
                (
                    "To remove the leftover lock directories, if no installation is running, run:",
                    vec![rm("00LOCK-cli")]
                ),
                (
                    "To remove the broken packages, run:",
                    // `bad` has two unparseable entries, but is removed once.
                    vec![rm("bad"), rm("half")]
                ),
            ]
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn paths_are_quoted() {
        assert_eq!(
            remove_dir_cmd(Path::new("/my lib/00LOCK-cli")),
            "rm -rf '/my lib/00LOCK-cli'"
        );
    }

    #[test]
    fn no_fixes_without_problems() {
        assert!(fix_commands(&[], None, None).is_empty());
    }
}
