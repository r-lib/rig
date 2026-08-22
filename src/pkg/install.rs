//! `rig pkg install`: install packages from the repositories into a library.
//!
//! The command is a solve followed by a download followed by an unpack. The
//! solve is the same one [`crate::proj`] runs for a project, with the packages
//! named on the command line standing in for a `DESCRIPTION`; the download and
//! the install are the same code `rig proj deploy` uses. What is specific to
//! this command is the third step in between: deciding which of the solved
//! packages actually have to be installed, because the library already holds
//! the rest.
//!
//! Answering that needs more than a version number. A repository can publish
//! several builds of one version, and a compiled package is only usable with the
//! `LinkingTo` dependency versions it was built against, so "cli 3.6.3 is
//! installed" does not mean the installed cli is the one the solve picked. So
//! rig records, in each installed package's `DESCRIPTION`, which artifact it
//! came from (`RemoteHash`) and what it was compiled against
//! (`RemoteLinkingToHashes`), and compares those. A package with no recorded
//! provenance — anything R, pak or renv installed — is reinstalled, because
//! there is no way to tell whether it matches.

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fs;
use std::io::ErrorKind;

use clap::ArgMatches;
use log::{debug, info};
use simple_error::*;
use tabular::*;

#[cfg(target_os = "macos")]
use crate::macos::get_r_binary;

#[cfg(target_os = "windows")]
use crate::windows::get_r_binary;

#[cfg(target_os = "linux")]
use crate::linux::get_r_binary;

use crate::cache::get_cache_dir;
use crate::dcf::{DepVersionSpec, PackageDependencies, RDepType};
use crate::install::{install_packages, PackageInfo, REMOTE_HASH_FIELD};
use crate::library::library_rver;
use crate::output::OUTPUT;
use crate::pak::{PakLockfile, PakLockfilePackage};
use crate::proj::{
    download_lockfile_packages, lockfile_package_info, proj_binary_target, sc_proj_solve_deps,
    BASE_PKGS,
};

use super::list::{read_installed, resolve_library, InstalledPackage, ResolvedLibrary};

/// How many packages are installed at once. The same default `rig proj deploy`
/// uses.
const MAX_CONCURRENT: usize = 8;

pub fn sc_pkg_install(
    args: &ArgMatches,
    pkgargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let json = args.get_flag("json") || pkgargs.get_flag("json") || mainargs.get_flag("json");
    let reinstall = args.get_flag("reinstall");
    let dry_run = args.get_flag("dry-run");

    let names: Vec<String> = args
        .get_many::<String>("package")
        .unwrap()
        .map(|x| x.to_string())
        .collect();
    let deps = requested_deps(&names)?;

    let lib = resolve_library(args)?;
    // Installing needs an R version even when `--library` is a plain path: it
    // selects which binary builds are usable, and provides the `R` that installs
    // a source package.
    let rver = match &lib.rversion {
        Some(rver) => rver.clone(),
        None => library_rver(args)?,
    };

    let target = proj_binary_target(args.get_one::<String>("platform"), &rver)?;
    let prefer_binary = args.get_one::<usize>("prefer-binary").copied();
    if prefer_binary.is_some() && target.is_none() {
        OUTPUT.warn("There are no binary packages to prefer, ignoring --prefer-binary");
        info!("Ignoring --prefer-binary: solving for source packages only");
    }

    let (registry, solution) = sc_proj_solve_deps(&rver, &deps, target, prefer_binary)?;
    OUTPUT.success("Solved dependencies");
    info!("Solved dependencies");

    // The lockfile is the bridge from the solution to the installer: it already
    // carries the download URLs, the cache-relative file names, the dependency
    // lists with R and the base packages filtered out, and the provenance
    // hashes. `rig pkg install` builds one in memory and never writes it.
    let lockfile = PakLockfile::from_solution(&registry, &solution);

    // A library that does not exist yet holds nothing; rig creates it below,
    // but only once it knows there is something to put in it, so that a
    // `--dry-run` leaves no trace.
    let installed = if lib.path.exists() {
        read_installed(&lib.path)?
    } else {
        debug!("Library {} does not exist yet", lib.path.display());
        vec![]
    };
    let plan = plan_installs(&lockfile.packages, &installed, reinstall);

    if json {
        print_plan_json(&plan)?;
    } else {
        print_plan(&lib, &plan);
    }

    if dry_run {
        info!("--dry-run, not installing anything");
        return Ok(());
    }

    let todo: Vec<&PakLockfilePackage> = plan
        .iter()
        .filter(|p| p.install)
        .map(|p| p.package)
        .collect();

    if todo.is_empty() {
        if !json {
            OUTPUT.success(&format!("Everything is up to date {}", lib.tag()));
        }
        info!("Nothing to install");
        return Ok(());
    }

    // The library may not exist yet, and `R CMD INSTALL` will not create it.
    if let Err(err) = fs::create_dir_all(&lib.path) {
        bail!("{}", library_error(&lib, err));
    }

    let to_download = PakLockfile {
        lockfile_version: lockfile.lockfile_version,
        os: lockfile.os.clone(),
        r_version: lockfile.r_version.clone(),
        platform: lockfile.platform.clone(),
        packages: todo.iter().map(|p| (*p).clone()).collect(),
    };
    download_lockfile_packages(&to_download)?;

    let cache_dir = get_cache_dir()?;
    let installing: HashSet<&str> = todo.iter().map(|p| p.package.as_str()).collect();
    let packages: Vec<PackageInfo> = todo
        .iter()
        .map(|p| {
            let mut info = lockfile_package_info(p, &cache_dir);
            // A package whose dependency is already installed must not wait for
            // it: the installer only starts a package once every name in its
            // `dependencies` has been installed *by this run*, and a name that
            // is not being installed at all would stall the whole batch.
            info.dependencies
                .retain(|d| installing.contains(d.as_str()));
            info
        })
        .collect();

    let r_binary = get_r_binary(&rver)?;
    let n = install_packages(
        packages,
        &lib.path,
        &r_binary.to_string_lossy(),
        MAX_CONCURRENT,
    )?;

    if !json {
        let word = if n == 1 { "package" } else { "packages" };
        OUTPUT.success(&format!("Installed {} {} {}", n, word, lib.tag()));
    }
    info!("Installed {} packages into {}", n, lib.path.display());

    Ok(())
}

// ------------------------------------------------------------------------
// What was asked for

/// The packages named on the command line, as a dependency set the solver takes.
///
/// They are `Depends` with no version constraint: the command asks for the
/// packages, and leaves it to the solve to say which versions that means.
fn requested_deps(names: &[String]) -> Result<PackageDependencies, Box<dyn Error>> {
    let mut deps = PackageDependencies::new();
    let mut base: Vec<&str> = vec![];

    for name in names {
        if deps.dependencies.iter().any(|d| &d.name == name) {
            debug!("{} named more than once, installing it once", name);
            continue;
        }
        // The base packages are part of R itself and are not published
        // separately, so there is nothing to install and nothing the solve could
        // find.
        if name == "R" || BASE_PKGS.contains(&name.as_str()) {
            base.push(name);
            continue;
        }
        deps.dependencies.push(DepVersionSpec {
            name: name.clone(),
            constraints: vec![],
            types: vec![RDepType::Depends],
        });
    }

    if !base.is_empty() {
        let msg = format!(
            "{} {} part of R itself and cannot be installed separately. \
            Use `rig add` to install another R version.",
            base.join(", "),
            if base.len() == 1 { "is" } else { "are" }
        );
        OUTPUT.error(&msg);
        bail!(msg);
    }

    if deps.dependencies.is_empty() {
        bail!("No packages to install");
    }

    Ok(deps)
}

// ------------------------------------------------------------------------
// What has to be installed

/// What rig decided to do about one package of the solution, and why.
#[derive(Debug)]
struct Planned<'a> {
    package: &'a PakLockfilePackage,
    install: bool,
    /// Why it is being installed, or why it is not. Reported, never acted on.
    reason: String,
}

/// Which of the solved packages have to be installed into the library.
///
/// A package is left alone only when the library already holds *the same
/// artifact*: the same version, installed from the same upstream tarball, and
/// compiled against the same `LinkingTo` dependency versions the solution has.
/// Anything else is installed, including a package whose provenance was never
/// recorded, since there is no way to tell whether that one matches.
///
/// The `LinkingTo` check is iterated to a fixpoint, because replacing a package
/// invalidates every compiled package built against it, which in turn
/// invalidates whatever was compiled against *those*. The coupling is
/// `LinkingTo` only: an `Imports` dependency being replaced changes nothing
/// about how its dependents were compiled.
fn plan_installs<'a>(
    solved: &'a [PakLockfilePackage],
    installed: &[InstalledPackage],
    reinstall: bool,
) -> Vec<Planned<'a>> {
    let by_name: HashMap<&str, &InstalledPackage> =
        installed.iter().map(|p| (p.package.as_str(), p)).collect();
    // The hash the solution says each package should have, for checking the
    // `LinkingTo` provenance of what is installed against it.
    let solved_hash: HashMap<&str, Option<&str>> = solved
        .iter()
        .map(|p| {
            (
                p.package.as_str(),
                p.metadata.get(REMOTE_HASH_FIELD).map(|s| s.as_str()),
            )
        })
        .collect();

    let mut plan: Vec<Planned> = solved
        .iter()
        .map(|package| {
            let (install, reason) = if reinstall {
                (true, "--reinstall".to_string())
            } else {
                match needs_install(
                    package,
                    by_name.get(package.package.as_str()).copied(),
                    &solved_hash,
                ) {
                    Some(reason) => (true, reason),
                    None => (false, "up to date".to_string()),
                }
            };
            Planned {
                package,
                install,
                reason,
            }
        })
        .collect();

    // Fixpoint: a package that is about to be replaced invalidates everything
    // compiled against it, and those in turn invalidate their own dependents.
    loop {
        let replacing: HashSet<&str> = plan
            .iter()
            .filter(|p| p.install)
            .map(|p| p.package.package.as_str())
            .collect();
        let mut changed = false;
        for entry in plan.iter_mut() {
            if entry.install {
                continue;
            }
            let inst = match by_name.get(entry.package.package.as_str()) {
                Some(inst) => *inst,
                None => continue,
            };
            if let Some(dep) = inst
                .linkingto
                .iter()
                .find(|(dep, _, _)| replacing.contains(dep.as_str()))
            {
                entry.install = true;
                entry.reason = format!("linked against {}, which is being replaced", dep.0);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    plan.sort_by(|a, b| {
        a.package
            .package
            .to_lowercase()
            .cmp(&b.package.package.to_lowercase())
            .then_with(|| a.package.package.cmp(&b.package.package))
    });
    plan
}

/// Why a solved package has to be installed, or `None` if the installed one
/// already is that package.
fn needs_install(
    solved: &PakLockfilePackage,
    installed: Option<&InstalledPackage>,
    solved_hash: &HashMap<&str, Option<&str>>,
) -> Option<String> {
    let installed = match installed {
        Some(x) => x,
        None => return Some("not installed".to_string()),
    };

    if installed.version != solved.version {
        return Some(format!("{} is installed", installed.version));
    }

    let want = solved.metadata.get(REMOTE_HASH_FIELD);
    match (&installed.hash, want) {
        // Nothing recorded on either side: the version is all we have to go on,
        // and it matches. This is what a source-only repository without hashes
        // looks like, and reinstalling on every run would be worse.
        (None, None) => {}
        (None, Some(_)) => return Some("no recorded hash".to_string()),
        (Some(_), None) => return Some("solved artifact has no hash".to_string()),
        (Some(have), Some(want)) if have != want => {
            return Some("built from a different tarball".to_string())
        }
        (Some(_), Some(_)) => {}
    }

    // What the installed package was compiled against has to be what the
    // solution says those packages are. A `LinkingTo` dependency that is not in
    // the solution at all is not checked: the solve did not need it, so nothing
    // is going to replace it.
    for (dep, _ver, sha) in installed.linkingto.iter() {
        if let Some(Some(want)) = solved_hash.get(dep.as_str()) {
            if want != sha {
                return Some(format!("compiled against another {}", dep));
            }
        }
    }

    None
}

// ------------------------------------------------------------------------
// Reporting

/// Print the plan as a table: what is being installed, what is not, and why.
fn print_plan(lib: &ResolvedLibrary, plan: &[Planned]) {
    let n = plan.iter().filter(|p| p.install).count();
    OUTPUT.println(&format!(
        "{} of {} packages to install {}",
        n,
        plan.len(),
        lib.tag()
    ));

    let mut tab = Table::new("{:<}  {:<}  {:<}  {:<}  {:<}");
    tab.add_row(row!("Package", "Version", "Type", "Action", "Reason"));
    for entry in plan {
        tab.add_row(row!(
            &entry.package.package,
            &entry.package.version,
            if entry.package.binary {
                "binary"
            } else {
                "source"
            },
            if entry.install { "install" } else { "skip" },
            &entry.reason
        ));
    }
    println!("{}", tab);
}

/// Print the plan as a JSON array, one object per package of the solution.
fn print_plan_json(plan: &[Planned]) -> Result<(), Box<dyn Error>> {
    #[derive(serde::Serialize)]
    struct PlanEntry<'a> {
        package: &'a str,
        version: &'a str,
        binary: bool,
        action: &'a str,
        reason: &'a str,
    }

    let entries: Vec<PlanEntry> = plan
        .iter()
        .map(|entry| PlanEntry {
            package: &entry.package.package,
            version: &entry.package.version,
            binary: entry.package.binary,
            action: if entry.install { "install" } else { "skip" },
            reason: &entry.reason,
        })
        .collect();

    println!("{}", serde_json::to_string_pretty(&entries)?);
    Ok(())
}

/// Why rig cannot write the library. In admin mode the site and system libraries
/// of an R installation belong to the administrator, which is the likely reason.
fn library_error(lib: &ResolvedLibrary, err: std::io::Error) -> String {
    let hint = if err.kind() == ErrorKind::PermissionDenied {
        " You may need to run rig as an administrator (`sudo`) to write this \
        library."
    } else {
        ""
    };
    format!("Cannot create {}: {}.{}", lib.path.display(), err, hint)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One package of a resolution: name, version, hash, and the `LinkingTo`
    /// provenance the artifact would be installed with.
    fn solved(name: &str, version: &str, hash: Option<&str>) -> PakLockfilePackage {
        let mut metadata = HashMap::new();
        if let Some(hash) = hash {
            metadata.insert(REMOTE_HASH_FIELD.to_string(), hash.to_string());
        }
        PakLockfilePackage {
            r#ref: name.to_string(),
            package: name.to_string(),
            version: version.to_string(),
            r#type: "standard".to_string(),
            direct: false,
            binary: true,
            dependencies: vec![],
            vignettes: false,
            metadata,
            sources: vec![],
            target: format!("bin/{}_{}.tgz", name, version),
            platform: "testos".to_string(),
            rversion: "4.5.1".to_string(),
            directpkg: false,
            license: "MIT".to_string(),
            dep_types: vec![],
            params: vec![],
            install_args: String::new(),
            sysreqs: String::new(),
        }
    }

    /// One installed package: name, version, hash, and what it was compiled
    /// against, as `(package, version, hash)`.
    fn inst(
        name: &str,
        version: &str,
        hash: Option<&str>,
        linkingto: &[(&str, &str, &str)],
    ) -> InstalledPackage {
        InstalledPackage::for_test(
            name,
            version,
            hash,
            linkingto
                .iter()
                .map(|(p, v, s)| (p.to_string(), v.to_string(), s.to_string()))
                .collect(),
        )
    }

    /// What the plan says about each package, as `name => (install, reason)`.
    fn plan(
        solved: &[PakLockfilePackage],
        installed: &[InstalledPackage],
        reinstall: bool,
    ) -> HashMap<String, (bool, String)> {
        plan_installs(solved, installed, reinstall)
            .into_iter()
            .map(|p| (p.package.package.clone(), (p.install, p.reason)))
            .collect()
    }

    #[test]
    fn a_package_that_is_not_installed_is_installed() {
        let out = plan(&[solved("cli", "3.6.3", Some("aa"))], &[], false);
        assert!(out["cli"].0);
        assert_eq!(out["cli"].1, "not installed");
    }

    #[test]
    fn the_same_artifact_is_left_alone() {
        let out = plan(
            &[solved("cli", "3.6.3", Some("aa"))],
            &[inst("cli", "3.6.3", Some("aa"), &[])],
            false,
        );
        assert!(!out["cli"].0);
        assert_eq!(out["cli"].1, "up to date");
    }

    #[test]
    fn another_version_is_installed() {
        let out = plan(
            &[solved("cli", "3.6.3", Some("aa"))],
            &[inst("cli", "3.6.2", Some("aa"), &[])],
            false,
        );
        assert!(out["cli"].0);
        assert_eq!(out["cli"].1, "3.6.2 is installed");
    }

    /// A package rig did not install has nothing recorded, so rig cannot tell
    /// whether it is the right artifact, and installs it.
    #[test]
    fn a_package_without_a_recorded_hash_is_installed() {
        let out = plan(
            &[solved("cli", "3.6.3", Some("aa"))],
            &[inst("cli", "3.6.3", None, &[])],
            false,
        );
        assert!(out["cli"].0);
        assert_eq!(out["cli"].1, "no recorded hash");
    }

    /// The same version can be published as more than one artifact, and a
    /// different one is a different package as far as installing goes.
    #[test]
    fn another_artifact_of_the_same_version_is_installed() {
        let out = plan(
            &[solved("cli", "3.6.3", Some("bb"))],
            &[inst("cli", "3.6.3", Some("aa"), &[])],
            false,
        );
        assert!(out["cli"].0);
        assert_eq!(out["cli"].1, "built from a different tarball");
    }

    /// Neither side knows a hash, so the version is all there is to go on, and
    /// it matches. Reinstalling on every run would be worse.
    #[test]
    fn no_hashes_anywhere_falls_back_to_the_version() {
        let out = plan(
            &[solved("cli", "3.6.3", None)],
            &[inst("cli", "3.6.3", None, &[])],
            false,
        );
        assert!(!out["cli"].0);
    }

    /// The installed package was compiled against a cpp11 the resolution does
    /// not have, so it has to be rebuilt even though its own hash is fine.
    #[test]
    fn a_stale_linkingto_hash_reinstalls_the_dependent() {
        let out = plan(
            &[
                solved("cpp11", "0.5.0", Some("new")),
                solved("tzdb", "0.4.0", Some("tz")),
            ],
            &[
                inst("cpp11", "0.5.0", Some("new"), &[]),
                inst("tzdb", "0.4.0", Some("tz"), &[("cpp11", "0.4.0", "old")]),
            ],
            false,
        );
        assert!(!out["cpp11"].0);
        assert!(out["tzdb"].0);
        assert_eq!(out["tzdb"].1, "compiled against another cpp11");
    }

    /// Replacing a package replaces everything compiled against it, and
    /// everything compiled against those, however deep the chain goes.
    #[test]
    fn replacing_a_package_cascades_through_the_linkingto_chain() {
        let out = plan(
            &[
                // cpp11's own hash changed, so it is replaced ...
                solved("cpp11", "0.5.0", Some("new")),
                // ... tzdb was compiled against it ...
                solved("tzdb", "0.4.0", Some("tz")),
                // ... and readr against tzdb.
                solved("readr", "2.1.5", Some("rd")),
            ],
            &[
                inst("cpp11", "0.5.0", Some("old"), &[]),
                inst("tzdb", "0.4.0", Some("tz"), &[("cpp11", "0.5.0", "new")]),
                inst("readr", "2.1.5", Some("rd"), &[("tzdb", "0.4.0", "tz")]),
            ],
            false,
        );
        assert!(out["cpp11"].0);
        assert!(out["tzdb"].0);
        assert!(out["readr"].0);
        assert_eq!(
            out["readr"].1,
            "linked against tzdb, which is being replaced"
        );
    }

    /// Only `LinkingTo` couples two packages' builds. A package that merely
    /// imports another is unaffected by that other one being replaced.
    #[test]
    fn an_imports_only_dependency_does_not_cascade() {
        let out = plan(
            &[
                solved("glue", "1.8.0", Some("new")),
                solved("cli", "3.6.3", Some("cl")),
            ],
            &[
                inst("glue", "1.8.0", Some("old"), &[]),
                // cli imports glue, but was not compiled against it.
                inst("cli", "3.6.3", Some("cl"), &[]),
            ],
            false,
        );
        assert!(out["glue"].0);
        assert!(!out["cli"].0);
    }

    /// A `LinkingTo` dependency that the resolution does not include cannot be
    /// replaced by this run, so it is not a reason to reinstall.
    #[test]
    fn a_linkingto_dependency_outside_the_resolution_is_ignored() {
        let out = plan(
            &[solved("tzdb", "0.4.0", Some("tz"))],
            &[inst(
                "tzdb",
                "0.4.0",
                Some("tz"),
                &[("cpp11", "0.4.0", "whatever")],
            )],
            false,
        );
        assert!(!out["tzdb"].0);
    }

    #[test]
    fn reinstall_installs_everything() {
        let out = plan(
            &[solved("cli", "3.6.3", Some("aa"))],
            &[inst("cli", "3.6.3", Some("aa"), &[])],
            true,
        );
        assert!(out["cli"].0);
        assert_eq!(out["cli"].1, "--reinstall");
    }

    #[test]
    fn base_packages_cannot_be_installed() {
        let err = requested_deps(&["stats".to_string()]).unwrap_err();
        assert!(err.to_string().contains("part of R itself"), "{}", err);
    }

    #[test]
    fn a_package_named_twice_is_installed_once() {
        let deps = requested_deps(&["cli".to_string(), "cli".to_string()]).unwrap();
        assert_eq!(deps.dependencies.len(), 1);
        assert_eq!(deps.dependencies[0].name, "cli");
        assert!(deps.dependencies[0].constraints.is_empty());
    }
}
