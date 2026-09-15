//! `rig pkg unlink`: remove a `rig pkg link` editable install.
//!
//! Unlike [`super::remove`], this only ever deletes a package's directory
//! when it is a link: a normal install named by mistake is left alone, with a
//! pointer at `rig pkg remove` instead. There is nothing to restore
//! afterwards -- a link never replaced a real install (`rig pkg link`
//! refuses to overwrite one), so removing it just leaves the name available
//! again.

use std::error::Error;
use std::path::PathBuf;

use clap::ArgMatches;
use log::info;
use simple_error::*;

use crate::output::OUTPUT;

use super::list::{read_installed, resolve_library, InstalledPackage};
use super::remove::remove_package;

pub fn sc_pkg_unlink(
    args: &ArgMatches,
    pkgargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let json = args.get_flag("json") || pkgargs.get_flag("json") || mainargs.get_flag("json");

    let names: Vec<String> = args
        .get_many::<String>("package")
        .unwrap()
        .map(|x| x.to_string())
        .collect();

    let lib = resolve_library(args)?;
    let installed = read_installed(&lib.path)?;
    let targets = select_links(&names, &installed)?;

    let mut removed: Vec<&Target> = vec![];
    let mut failed: Vec<String> = vec![];

    for target in &targets {
        if !json {
            OUTPUT.status(&format!(
                "Unlinking {} from {}...",
                target.package,
                target.path.display()
            ));
        }
        info!(
            "Unlinking {} from {}",
            target.package,
            target.path.display()
        );
        match remove_package(&target.path) {
            Ok(()) => removed.push(target),
            Err(err) => {
                OUTPUT.error(&err);
                failed.push(target.package.clone());
            }
        }
    }

    if json {
        print_unlinked_json(&removed)?;
    } else if !removed.is_empty() {
        let word = if removed.len() == 1 {
            "package"
        } else {
            "packages"
        };
        OUTPUT.success(&format!(
            "Unlinked {} {} {}",
            removed.len(),
            word,
            lib.tag()
        ));
    }

    if !failed.is_empty() {
        bail!("Failed to unlink {}", failed.join(", "));
    }

    Ok(())
}

#[derive(Debug)]
struct Target {
    package: String,
    source: String,
    path: PathBuf,
}

/// The linked packages to remove, in command-line order. As with `rig pkg
/// remove`, a name that is missing, or that names a package that is not a
/// link, stops the whole command -- rig unlinks either all of the requested
/// packages or none of them.
fn select_links(
    names: &[String],
    installed: &[InstalledPackage],
) -> Result<Vec<Target>, Box<dyn Error>> {
    let mut targets: Vec<Target> = vec![];
    let mut problems: Vec<String> = vec![];

    for name in names {
        if targets.iter().any(|t| &t.package == name) {
            continue;
        }

        match installed.iter().find(|p| &p.package == name) {
            None => problems.push(format!("Package {} is not installed.", name)),
            Some(pkg) => match &pkg.link_source {
                Some(source) => targets.push(Target {
                    package: pkg.package.clone(),
                    source: source.clone(),
                    path: pkg.path.clone(),
                }),
                None => problems.push(format!(
                    "{} is not a linked package. Use `rig pkg remove {}` instead.",
                    name, name
                )),
            },
        }
    }

    if !problems.is_empty() {
        for msg in &problems {
            OUTPUT.error(msg);
        }
        bail!(problems.join(" "));
    }

    Ok(targets)
}

fn print_unlinked_json(removed: &[&Target]) -> Result<(), Box<dyn Error>> {
    #[derive(serde::Serialize)]
    struct UnlinkedEntry<'a> {
        package: &'a str,
        source: &'a str,
        path: String,
    }

    let entries: Vec<UnlinkedEntry> = removed
        .iter()
        .map(|target| UnlinkedEntry {
            package: &target.package,
            source: &target.source,
            path: target.path.display().to_string(),
        })
        .collect();

    println!("{}", serde_json::to_string_pretty(&entries)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn library_with(pkgs: &[(&str, Option<&str>)]) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        for (name, link) in pkgs {
            let dir = tmp.path().join(name);
            std::fs::create_dir_all(&dir).unwrap();
            let mut desc = format!("Package: {}\nVersion: 1.0.0\n", name);
            if let Some(source) = link {
                desc.push_str(&format!("RigLink: {}\n", source));
            }
            std::fs::write(dir.join("DESCRIPTION"), desc).unwrap();
        }
        tmp
    }

    fn select(path: &Path, names: &[&str]) -> Result<Vec<Target>, Box<dyn std::error::Error>> {
        let names: Vec<String> = names.iter().map(|x| x.to_string()).collect();
        let installed = read_installed(path).unwrap();
        select_links(&names, &installed)
    }

    #[test]
    fn unlinks_only_linked_packages() {
        let tmp = library_with(&[("mypkg", Some("/src/mypkg"))]);
        let targets = select(tmp.path(), &["mypkg"]).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].source, "/src/mypkg");
    }

    #[test]
    fn refuses_a_normal_install() {
        let tmp = library_with(&[("cli", None)]);
        let err = select(tmp.path(), &["cli"]).unwrap_err();
        assert!(err.to_string().contains("not a linked package"), "{}", err);
        assert!(tmp.path().join("cli").exists());
    }

    #[test]
    fn refuses_a_missing_package() {
        let tmp = library_with(&[]);
        let err = select(tmp.path(), &["nope"]).unwrap_err();
        assert!(err.to_string().contains("is not installed"), "{}", err);
    }

    #[test]
    fn unlinking_deletes_the_directory() {
        let tmp = library_with(&[("mypkg", Some("/src/mypkg"))]);
        let targets = select(tmp.path(), &["mypkg"]).unwrap();
        remove_package(&targets[0].path).unwrap();
        assert!(!tmp.path().join("mypkg").exists());
    }
}
