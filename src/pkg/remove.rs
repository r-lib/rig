//! `rig pkg remove`: delete packages from a package library.
//!
//! The counterpart of [`super::list`], and it works the same way: it reads the
//! library directory on disk, and it selects that library with the same
//! `--library` / `--r-version` options. Removing a package is deleting its
//! directory, which is what `R CMD REMOVE` and `remove.packages()` do as well,
//! so rig does not need to start R for this either.
//!
//! Deleting files is not undoable, so rig is deliberately strict about what it
//! is asked to delete: a package that is not in the library, or a base package
//! that R itself needs, stops the command before anything is removed.

use std::error::Error;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use clap::ArgMatches;
use log::{debug, info};
use simple_error::*;

use crate::output::OUTPUT;
use crate::proj::BASE_PKGS;

use super::list::{read_installed, resolve_library, InstalledPackage, ResolvedLibrary};

pub fn sc_pkg_remove(
    args: &ArgMatches,
    pkgargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let json = args.get_flag("json") || pkgargs.get_flag("json") || mainargs.get_flag("json");
    let force = args.get_flag("force");

    let names: Vec<String> = args
        .get_many::<String>("package")
        .unwrap()
        .map(|x| x.to_string())
        .collect();

    let lib = resolve_library(args)?;
    let installed = read_installed(&lib.path)?;
    let targets = select_packages(&names, &installed, &lib, force)?;

    let mut removed: Vec<&Target> = vec![];
    let mut failed: Vec<String> = vec![];

    for target in &targets {
        if !json {
            OUTPUT.status(&format!(
                "Removing {} {} from {}...",
                target.package,
                target.version,
                target.path.display()
            ));
        }
        info!("Removing {} from {}", target.package, target.path.display());
        match remove_package(&target.path) {
            Ok(()) => removed.push(target),
            Err(err) => {
                OUTPUT.error(&err);
                failed.push(target.package.clone());
            }
        }
    }

    if json {
        print_removed_json(&removed)?;
    } else if !removed.is_empty() {
        let word = if removed.len() == 1 {
            "package"
        } else {
            "packages"
        };
        OUTPUT.success(&format!("Removed {} {} {}", removed.len(), word, lib.tag()));
    }

    if !failed.is_empty() {
        bail!("Failed to remove {}", failed.join(", "));
    }

    Ok(())
}

/// A package rig is about to delete.
///
/// The version is only kept to report what was removed; the path is the
/// package's own directory in the library, which is what actually gets deleted.
#[derive(Debug)]
struct Target {
    package: String,
    version: String,
    path: PathBuf,
}

/// The packages to delete, in the order they were named on the command line.
///
/// Every requested name must be installed in the library and must be safe to
/// delete, and a name that is not stops the whole command: rig removes either
/// all of the packages it was asked to remove, or, save for an I/O error along
/// the way, none of them. Naming the same package twice is not an error, it is
/// removed once.
fn select_packages(
    names: &[String],
    installed: &[InstalledPackage],
    lib: &ResolvedLibrary,
    force: bool,
) -> Result<Vec<Target>, Box<dyn Error>> {
    let mut targets: Vec<Target> = vec![];
    let mut missing: Vec<String> = vec![];
    let mut base: Vec<String> = vec![];

    for name in names {
        if targets.iter().any(|t| &t.package == name) {
            debug!("{} named more than once, removing it once", name);
            continue;
        }

        let pkg = match installed.iter().find(|p| &p.package == name) {
            Some(pkg) => pkg,
            None => {
                missing.push(missing_package_message(name, installed, lib));
                continue;
            }
        };

        // The base packages are part of R itself, and R does not start without
        // some of them, so deleting one from R's own library breaks that R
        // installation. That is hardly ever what the user meant, but it is
        // their call, hence `--force`.
        if !force && BASE_PKGS.contains(&name.as_str()) {
            base.push(name.clone());
            continue;
        }

        targets.push(Target {
            package: pkg.package.clone(),
            version: pkg.version.clone(),
            path: pkg.path.clone(),
        });
    }

    if !base.is_empty() {
        let msg = format!(
            "Refusing to remove the base {} {}, R needs {}. Use `--force` to \
            remove {} anyway.",
            if base.len() == 1 {
                "package"
            } else {
                "packages"
            },
            base.join(", "),
            if base.len() == 1 { "it" } else { "them" },
            if base.len() == 1 { "it" } else { "them" }
        );
        OUTPUT.error(&msg);
        bail!(msg);
    }

    if !missing.is_empty() {
        for msg in &missing {
            OUTPUT.error(msg);
        }
        bail!(missing.join(" "));
    }

    Ok(targets)
}

/// Why rig cannot remove a package it did not find, for a single package.
///
/// R package names are case sensitive, and so is the lookup, but a name that
/// differs from an installed one only in case is much more likely to be a typo
/// than a package that is not installed, so it is worth pointing at.
fn missing_package_message(
    name: &str,
    installed: &[InstalledPackage],
    lib: &ResolvedLibrary,
) -> String {
    let similar = installed
        .iter()
        .find(|p| p.package.to_lowercase() == name.to_lowercase());

    match similar {
        Some(pkg) => format!(
            "Package {} is not installed in {}, but {} is. \
            Package names are case sensitive.",
            name,
            lib.path.display(),
            pkg.package
        ),
        None => format!(
            "Package {} is not installed in {}.",
            name,
            lib.path.display()
        ),
    }
}

/// Delete the directory of an installed package.
///
/// A failure here is reported and the removal of the other packages goes on, so
/// the error message must name the package directory itself. A library the user
/// cannot write is the likely reason in admin mode, where the libraries of an R
/// installation belong to root, so that case says so.
fn remove_package(path: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let hint = if err.kind() == ErrorKind::PermissionDenied {
                " You may need to run rig as an administrator (`sudo`) to \
                write this library."
            } else {
                ""
            };
            Err(format!(
                "Cannot remove {}: {}.{}",
                path.display(),
                err,
                hint
            ))
        }
    }
}

/// Print the removed packages as a JSON array, one object per package.
fn print_removed_json(removed: &[&Target]) -> Result<(), Box<dyn Error>> {
    #[derive(serde::Serialize)]
    struct RemovedEntry<'a> {
        package: &'a str,
        version: &'a str,
        path: String,
    }

    let entries: Vec<RemovedEntry> = removed
        .iter()
        .map(|target| RemovedEntry {
            package: &target.package,
            version: &target.version,
            path: target.path.display().to_string(),
        })
        .collect();

    println!("{}", serde_json::to_string_pretty(&entries)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A library with the named packages installed in it, at version 1.0.0.
    fn library(pkgs: &[&str]) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        for name in pkgs {
            let dir = tmp.path().join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("DESCRIPTION"),
                format!("Package: {}\nVersion: 1.0.0\n", name),
            )
            .unwrap();
        }
        tmp
    }

    fn resolved(path: &Path) -> ResolvedLibrary {
        ResolvedLibrary {
            name: None,
            path: path.to_path_buf(),
            rversion: None,
        }
    }

    /// The packages `rig pkg remove <names>` would delete from `path`.
    fn select(
        path: &Path,
        names: &[&str],
        force: bool,
    ) -> Result<Vec<Target>, Box<dyn std::error::Error>> {
        let names: Vec<String> = names.iter().map(|x| x.to_string()).collect();
        let installed = read_installed(path).unwrap();
        select_packages(&names, &installed, &resolved(path), force)
    }

    #[test]
    fn selects_the_directories_of_the_named_packages() {
        let tmp = library(&["cli", "glue", "rlang"]);

        let targets = select(tmp.path(), &["glue", "cli"], false).unwrap();
        let names: Vec<&str> = targets.iter().map(|t| t.package.as_str()).collect();
        // Removal follows the order of the command line.
        assert_eq!(names, vec!["glue", "cli"]);
        assert_eq!(targets[0].version, "1.0.0");
        assert_eq!(targets[0].path, tmp.path().join("glue"));
    }

    #[test]
    fn a_package_named_twice_is_removed_once() {
        let tmp = library(&["cli"]);

        let targets = select(tmp.path(), &["cli", "cli"], false).unwrap();
        assert_eq!(targets.len(), 1);
    }

    /// Nothing is removed when one of the packages is not installed, not even
    /// the packages that are.
    #[test]
    fn a_missing_package_stops_the_whole_command() {
        let tmp = library(&["cli"]);

        let err = select(tmp.path(), &["cli", "nosuchpkg"], false).unwrap_err();
        assert!(
            err.to_string()
                .contains("Package nosuchpkg is not installed"),
            "{}",
            err
        );
        assert!(tmp.path().join("cli").exists());
    }

    /// Package names are case sensitive, and a name that is only off in case
    /// says so.
    #[test]
    fn a_case_mismatch_is_pointed_out() {
        let tmp = library(&["Matrix"]);

        let err = select(tmp.path(), &["matrix"], false).unwrap_err();
        assert!(err.to_string().contains("but Matrix is"), "{}", err);
    }

    #[test]
    fn base_packages_need_force() {
        let tmp = library(&["stats", "cli"]);

        let err = select(tmp.path(), &["stats"], false).unwrap_err();
        assert!(
            err.to_string()
                .contains("Refusing to remove the base package"),
            "{}",
            err
        );

        let targets = select(tmp.path(), &["stats"], true).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].package, "stats");
    }

    #[test]
    fn removing_a_package_deletes_its_directory() {
        let tmp = library(&["cli", "glue"]);

        remove_package(&tmp.path().join("cli")).unwrap();
        assert!(!tmp.path().join("cli").exists());
        assert!(tmp.path().join("glue").join("DESCRIPTION").exists());
    }

    #[test]
    fn removing_a_missing_directory_is_an_error() {
        let tmp = library(&[]);

        let err = remove_package(&tmp.path().join("cli")).unwrap_err();
        assert!(err.contains("Cannot remove"), "{}", err);
    }
}
