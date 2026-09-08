use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::error::Error;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::ArgMatches;
use deb822_fast::Deb822;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use log::{error, info};
use pubgrub::{resolve, SelectedDependencies};
use rayon::prelude::*;
use simple_error::*;
use tabular::*;

use crate::args::rig_app;
use crate::built::BuiltCache;
use crate::cache::get_cache_dir;
use crate::common::{get_arch, get_default_r_version, get_platform, sc_get_list_details};
use crate::dcf::*;
use crate::download::download_multiple_first_available_with_progress;
use crate::install::{
    install_packages, parse_linkingto, PackageInfo, REMOTE_HASH_FIELD, REMOTE_LINKINGTO_FIELD,
};
use crate::output::OUTPUT;
use crate::pkg::deps::{
    dep_count, print_deps_json, print_deps_recursive, print_header, type_list, walk_deps,
};
use crate::pkg::install::{plan_installs, print_plan};
use crate::pkg::list::read_installed;
use crate::pkg::tree::proj_tree;
use crate::platform::{detect_platform, parse_platform_string};
use crate::repos::binaries::loader::{BinaryTarget, P3mBinaryLoader};
use crate::repos::cranlike_metadata::{ensure_allpackages_fresh, minor_r_version};
use crate::repos::*;
use crate::resolve::resolve_versions;
use crate::rproj::{
    parse_add_spec, Author, Rproj, RprojLock, RprojLockPackage, RprojLockTarget,
    RPROJ_LOCK_VERSION, RPROJ_MANIFEST_FILE,
};
use crate::rvenv::{
    existing_targets, find_project_root, project_library, read_rvenv_cfg, rvenv_init, rvenv_sync,
    write_sync_stamp, RvenvCfg, RPROJ_LOCK_FILE,
};
use crate::solver::*;
use crate::textfmt::reflow;
use crate::utils::create_parent_dir_if_needed;

#[cfg(target_os = "macos")]
use crate::macos::{get_r_binary, sc_add};

#[cfg(target_os = "windows")]
use crate::windows::{get_r_binary, sc_add};

#[cfg(target_os = "linux")]
use crate::linux::{get_r_binary, sc_add};

pub const BASE_PKGS: &[&str] = &[
    "base",
    "compiler",
    "datasets",
    "graphics",
    "grDevices",
    "grid",
    "methods",
    "parallel",
    "splines",
    "stats",
    "stats4",
    "tcltk",
    "tools",
    "utils",
];

pub fn sc_proj(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        Some(("init", s)) => sc_proj_init(s, args, mainargs),
        Some(("import", s)) => sc_proj_import(s, args, mainargs),
        Some(("export", s)) => sc_proj_export(s, args, mainargs),
        Some(("add", s)) => sc_proj_add(s, args, mainargs),
        Some(("remove", s)) => sc_proj_remove(s, args, mainargs),
        Some(("deps", s)) => sc_proj_deps(s, args, mainargs),
        Some(("tree", s)) => sc_proj_tree(s, args, mainargs),
        Some(("lock", s)) => sc_proj_lock(s, args, mainargs),
        Some(("sync", s)) => sc_proj_sync(s, args, mainargs),
        Some(("renv", s)) => crate::renv::sc_renv(s, mainargs),
        _ => Ok(()), // unreachable
    }
}

/// Create a new project in the current directory: the `rproj.toml` manifest
/// plus the tracked part of the `.rvenv` layout (see `src/rvenv.rs`).
fn sc_proj_init(
    args: &ArgMatches,
    _projargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let root = std::env::current_dir()?;
    let force = args.get_flag("force");

    // Check every file we are about to write before writing any of them, so
    // that a conflict does not leave a half-created project behind, and so
    // that the error can name all of them at once.
    if !force {
        let existing = existing_targets(&root)?;
        if !existing.is_empty() {
            let names: Vec<String> = existing
                .iter()
                .map(|p| {
                    p.strip_prefix(&root)
                        .unwrap_or(p)
                        .to_string_lossy()
                        .into_owned()
                })
                .collect();
            let msg = format!(
                "{} already exist{}, use --force to overwrite",
                names.join(", "),
                if names.len() == 1 { "s" } else { "" }
            );
            OUTPUT.error(&msg);
            error!("{}", msg);
            bail!("{}", msg);
        }
    }

    // The R version decides both the manifest's R requirement and which
    // pre-built shim package the project gets. It does not have to be
    // installed, nothing we write here refers to an R installation.
    let rver = match args.get_one::<String>("r-version") {
        Some(rv) => rv.to_string(),
        None => match get_default_r_version()? {
            Some(rv) => rv,
            // No R installed (or no default set), so fall back to the current
            // release, which needs the network.
            None => match resolve_release_r_version(args) {
                Some(rv) => {
                    OUTPUT.info(&format!(
                        "No default R version, using the current release (R {}).",
                        rv
                    ));
                    info!("No default R version, using the current release (R {})", rv);
                    rv
                }
                None => {
                    let msg = "Cannot determine R version. Install R with `rig add`, \
                               or set the version with --r-version.";
                    OUTPUT.error(msg);
                    error!("{}", msg);
                    bail!("{}", msg)
                }
            },
        },
    };

    // Project name defaults to the current directory's name.
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "myproject".to_string());

    let manifest = Rproj::minimal_for_r(&name, &rver)?;
    let manifest_path = root.join(RPROJ_MANIFEST_FILE);
    fs::write(&manifest_path, toml::to_string_pretty(&manifest)?)?;

    let mut created = vec![manifest_path];
    created.extend(rvenv_init(&root, &rver)?);

    for path in &created {
        let name = path.strip_prefix(&root).unwrap_or(path).to_string_lossy();
        OUTPUT.success(&format!("Created {}", name));
        info!("Created {}", name);
    }
    OUTPUT.info(&format!(
        "Project set up for R {}. Next: add dependencies to {}, \
         then run `rig proj lock` and `rig proj sync`.",
        rver, RPROJ_MANIFEST_FILE
    ));

    Ok(())
}

/// Current release version or None on error.
fn resolve_release_r_version(args: &ArgMatches) -> Option<String> {
    let platform = match get_platform(args) {
        Ok(p) => p,
        Err(err) => {
            info!("Cannot detect platform to resolve R release: {}", err);
            return None;
        }
    };
    let arch = get_arch(&platform, args);
    match resolve_versions(vec!["release".to_string()], &platform, &arch) {
        Ok(vers) => vers.first().and_then(|v| v.version.clone()),
        Err(err) => {
            info!("Cannot resolve the current R release: {}", err);
            None
        }
    }
}

/// DESCRIPTION fields that already have a structured home in the manifest
/// (project metadata, `Maintainer` superseded by `Authors@R`, and each
/// dependency type field), so `rig proj import` does not also copy them into
/// the `[description]` escape hatch. `Config/*` fields are excluded
/// separately, since they always go to `[config.*]` / dependency groups
/// instead.
const KNOWN_DESCRIPTION_FIELDS: [&str; 15] = [
    "Package",
    "Version",
    "Type",
    "Title",
    "Description",
    "License",
    "Authors@R",
    "Maintainer",
    "URL",
    "BugReports",
    "Depends",
    "Imports",
    "LinkingTo",
    "Suggests",
    "Enhances",
];

/// Import a `DESCRIPTION` file into `rproj.toml`.
///
/// By default this is a full import: name, version, title, description,
/// license, authors and urls, plus dependencies, into a *new* `rproj.toml`
/// (it refuses to run if one already exists, since populating the full
/// `[project]` metadata block is not a well-defined merge onto an existing,
/// possibly hand-edited, manifest). `--dependencies` keeps the old
/// behavior: only merge dependencies, creating a minimal manifest if
/// missing or merging into an existing one.
fn sc_proj_import(
    args: &ArgMatches,
    _projargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let default_input = "DESCRIPTION".to_string();
    let input: &String = args.get_one::<String>("input").unwrap_or(&default_input);
    let dependencies_only = args.get_flag("dependencies");
    let path = Path::new(RPROJ_MANIFEST_FILE);

    if !dependencies_only && path.exists() {
        let msg = format!(
            "{} already exists; import would only overwrite dependencies, not \
             merge full metadata. Use --dependencies to merge into it, or \
             remove it first.",
            RPROJ_MANIFEST_FILE
        );
        OUTPUT.error(&msg);
        error!("{}", msg);
        bail!("{}", msg);
    }

    let paragraph = read_description_paragraph(input)?;
    let pkg = Package::from_dcf_paragraph(&paragraph)?;
    let dep_count = pkg.dependencies.dependencies.len();

    let mut manifest = if dependencies_only {
        if path.exists() {
            toml::from_str::<Rproj>(&fs::read_to_string(path)?)?
        } else {
            OUTPUT.status(&format!(
                "{} does not exist, creating a new one",
                RPROJ_MANIFEST_FILE
            ));
            info!("{} does not exist, creating a new one", RPROJ_MANIFEST_FILE);
            Rproj::minimal(&pkg.name)
        }
    } else {
        Rproj::minimal(&pkg.name)
    };

    if !dependencies_only {
        manifest.project.version = pkg.version.to_string();
        manifest.project.type_ = Some(
            paragraph
                .get("Type")
                .map(|t| t.to_lowercase())
                .unwrap_or_else(|| "package".to_string()),
        );
        manifest.project.title = paragraph.get("Title").map(reflow);
        manifest.project.description = paragraph.get("Description").map(reflow);
        manifest.project.license = paragraph.get("License").map(|l| reflow(l));
        manifest.project.authors = match paragraph.get("Authors@R") {
            Some(raw) => Author::from_authors_r(raw),
            None => paragraph
                .get("Maintainer")
                .and_then(Author::from_maintainer)
                .into_iter()
                .collect(),
        };
        if let Some(url) = paragraph.get("URL") {
            let reflowed = reflow(url);
            let mut urls = reflowed
                .split([',', ' '])
                .map(str::trim)
                .filter(|u| !u.is_empty());
            if let Some(homepage) = urls.next() {
                manifest
                    .project
                    .urls
                    .insert("homepage".to_string(), homepage.to_string());
            }
            if let Some(source) = urls.next() {
                manifest
                    .project
                    .urls
                    .insert("source".to_string(), source.to_string());
            }
        }
        if let Some(bugreports) = paragraph.get("BugReports") {
            manifest
                .project
                .urls
                .insert("bugreports".to_string(), reflow(bugreports));
        }
        // Any other field, e.g. `Encoding`, has no structured home in the
        // manifest, so it round-trips verbatim through the `[description]`
        // escape hatch: `Encoding: UTF-8` becomes `Encoding = "UTF-8"`.
        for (key, value) in paragraph.iter() {
            if KNOWN_DESCRIPTION_FIELDS.contains(&key) || key.starts_with("Config/") {
                continue;
            }
            manifest
                .description
                .insert(key.to_string(), toml::Value::String(reflow(value)));
        }
    }

    manifest.merge_description(&pkg);
    // `Config/Needs/*` fields are dependencies as well, so they are imported
    // in `--dependencies` mode, too.
    let needs: Vec<(String, String)> = paragraph
        .iter()
        .filter_map(|(key, value)| {
            key.strip_prefix("Config/Needs/")
                .map(|group| (group.to_string(), reflow(value)))
        })
        .collect();
    manifest.merge_config_needs(&needs);
    // `Config/<group>/<key>` fields, other than `Config/Needs/*` above, become
    // `[config.<group>]` entries, e.g. `Config/testthat/edition: 3` becomes
    // `edition = 3` under `[config.testthat]`.
    let config: Vec<(String, String, String)> = paragraph
        .iter()
        .filter_map(|(key, value)| {
            let rest = key.strip_prefix("Config/")?;
            if rest.starts_with("Needs/") {
                return None;
            }
            let (group, field) = rest.split_once('/')?;
            Some((group.to_string(), field.to_string(), reflow(value)))
        })
        .collect();
    manifest.merge_config(&config);
    fs::write(path, toml::to_string_pretty(&manifest)?)?;

    let groups = match needs.len() {
        0 => "".to_string(),
        1 => " and 1 dependency group".to_string(),
        n => format!(" and {} dependency groups", n),
    };
    let msg = if dependencies_only {
        format!(
            "Imported {} dependencies{} from {} into {}",
            dep_count, groups, input, RPROJ_MANIFEST_FILE
        )
    } else {
        format!(
            "Imported {} {}, {} dependencies{} from {} into {}",
            pkg.name, pkg.version, dep_count, groups, input, RPROJ_MANIFEST_FILE
        )
    };
    OUTPUT.success(&msg);
    info!("{}", msg);
    Ok(())
}

/// Create a `DESCRIPTION` file from `rproj.toml`: `rig proj export`.
fn sc_proj_export(
    args: &ArgMatches,
    _projargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let default_output = "DESCRIPTION".to_string();
    let output: &String = args.get_one::<String>("output").unwrap_or(&default_output);
    let force = args.get_flag("force");
    let path = Path::new(output);

    if path.exists() && !force {
        let msg = format!("{} already exists, use --force to overwrite", output);
        OUTPUT.error(&msg);
        error!("{}", msg);
        bail!("{}", msg);
    }

    let cwd = std::env::current_dir()?;
    let root = find_project_root(&cwd).unwrap_or(cwd);
    let manifest = proj_read_manifest(&root)?;

    let (description, dropped) = manifest.to_description()?;
    fs::write(path, description)?;

    if !dropped.is_empty() {
        OUTPUT.warn(&format!(
            "Dropped the upper version bound for {}, DESCRIPTION only supports \
             a single version comparison per dependency",
            dropped.join(", ")
        ));
    }

    let msg = format!("Exported {} to {}", RPROJ_MANIFEST_FILE, output);
    OUTPUT.success(&msg);
    info!("{}", msg);
    Ok(())
}

/// Add dependencies to `rproj.toml`, then update the lockfile and install
/// them: `rig proj add`.
fn sc_proj_add(
    args: &ArgMatches,
    _projargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    // The project is the nearest one at or above the current directory, like
    // `rig proj sync`, so that `rig proj add` works from a subdirectory.
    let cwd = std::env::current_dir()?;
    let root = find_project_root(&cwd).unwrap_or(cwd);
    let path = root.join(RPROJ_MANIFEST_FILE);
    let dev = args.get_flag("dev");

    // The manifest is restored from this if the added packages turn out not to
    // be installable, so that a failed `rig proj add` leaves no trace.
    let original = fs::read_to_string(&path).ok();
    let mut manifest = proj_read_manifest(&root)?;

    // Parse every specification before changing anything, so that a typo in
    // the last one does not leave the earlier ones added.
    let specs: Vec<(String, String)> = args
        .get_many::<String>("package")
        .unwrap_or_default()
        .map(|spec| parse_add_spec(spec))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| {
            OUTPUT.error(&err.to_string());
            error!("{}", err);
            err
        })?;

    let mut messages: Vec<String> = Vec::new();
    for (name, version) in specs.iter() {
        // A dev dependency of a package the project already depends on
        // directly is installed either way, so `--dev` does not do what it
        // looks like it does there.
        if dev && manifest.dependencies.contains_key(name) {
            OUTPUT.warn(&format!(
                "{} is already a dependency in [dependencies], \
                 it stays a hard dependency",
                name
            ));
        }

        let previous = manifest.add_dependency(name, version, dev);
        messages.push(match previous {
            Some(previous) if &previous == version => {
                format!("Kept {} ({}) in {}", name, version, RPROJ_MANIFEST_FILE)
            }
            Some(previous) => format!(
                "Updated {} in {}, {} -> {}",
                name, RPROJ_MANIFEST_FILE, previous, version
            ),
            None => format!("Added {} ({}) to {}", name, version, RPROJ_MANIFEST_FILE),
        });
    }

    fs::write(&path, toml::to_string_pretty(&manifest)?)?;
    for msg in messages.iter() {
        OUTPUT.success(msg);
        info!("{}", msg);
    }

    if args.get_flag("no-lock") {
        OUTPUT.info(&format!(
            "Next: run `rig proj lock` to update {}.",
            RPROJ_LOCK_FILE
        ));
        return Ok(());
    }

    // A package no repository has cannot be locked, and a manifest that
    // cannot be locked is of no use to anyone, so undo the edit.
    if let Err(err) = proj_lock(&root, &ProjLockOptions::default(), args) {
        if let Some(original) = original {
            fs::write(&path, original)?;
            let msg = format!(
                "Could not resolve the dependencies, {} unchanged",
                RPROJ_MANIFEST_FILE
            );
            OUTPUT.error(&msg);
            error!("{}", msg);
        }
        return Err(err);
    }

    if args.get_flag("no-sync") {
        return Ok(());
    }

    proj_sync(&root, &ProjSyncOptions::default(), args)
}

/// Remove dependencies from `rproj.toml`, then update the lockfile and the
/// project library: `rig proj remove`.
fn sc_proj_remove(
    args: &ArgMatches,
    _projargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let cwd = std::env::current_dir()?;
    let root = find_project_root(&cwd).unwrap_or(cwd);
    let path = root.join(RPROJ_MANIFEST_FILE);

    // The manifest is restored from this if re-locking after the removal
    // fails, so that a failed `rig proj remove` leaves no trace.
    let original = fs::read_to_string(&path).ok();
    let mut manifest = proj_read_manifest(&root)?;

    // A name named more than once is removed once; a name that is not a
    // dependency anywhere stops the whole command before anything is
    // removed, the same all-or-none behavior as `rig pkg remove`.
    let mut names: Vec<String> = Vec::new();
    for name in args.get_many::<String>("package").unwrap_or_default() {
        if !names.contains(name) {
            names.push(name.clone());
        }
    }

    let missing: Vec<&String> = names
        .iter()
        .filter(|name| !manifest.has_dependency(name))
        .collect();
    if !missing.is_empty() {
        let word = if missing.len() == 1 {
            "dependency"
        } else {
            "dependencies"
        };
        let msg = format!(
            "Not a {} in {}: {}",
            word,
            RPROJ_MANIFEST_FILE,
            missing
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        OUTPUT.error(&msg);
        error!("{}", msg);
        bail!("{}", msg);
    }

    let mut messages: Vec<String> = Vec::new();
    for name in names.iter() {
        manifest.remove_dependency(name);
        messages.push(format!("Removed {} from {}", name, RPROJ_MANIFEST_FILE));
    }

    fs::write(&path, toml::to_string_pretty(&manifest)?)?;
    for msg in messages.iter() {
        OUTPUT.success(msg);
        info!("{}", msg);
    }

    if args.get_flag("no-lock") {
        OUTPUT.info(&format!(
            "Next: run `rig proj lock` to update {}.",
            RPROJ_LOCK_FILE
        ));
        return Ok(());
    }

    if let Err(err) = proj_lock(&root, &ProjLockOptions::default(), args) {
        if let Some(original) = original {
            fs::write(&path, original)?;
            let msg = format!(
                "Could not resolve the dependencies, {} unchanged",
                RPROJ_MANIFEST_FILE
            );
            OUTPUT.error(&msg);
            error!("{}", msg);
        }
        return Err(err);
    }

    if args.get_flag("no-sync") {
        return Ok(());
    }

    proj_sync(&root, &ProjSyncOptions::default(), args)
}

/// Read a `DESCRIPTION` file (or any single-paragraph DCF file) and return
/// its one paragraph.
fn read_description_paragraph(input: &str) -> Result<deb822_fast::Paragraph, Box<dyn Error>> {
    OUTPUT.status(&format!("Reading dependencies from {}", input));
    info!("Reading dependencies from {}", input);
    let df: File = File::open(input).map_err(|e| {
        OUTPUT.error(&format!("Cannot read {}: {}", input, e));
        error!("Cannot read {}: {}", input, e);
        e
    })?;
    let desc = Deb822::from_reader(df)?;

    if desc.is_empty() {
        OUTPUT.error("Empty DESCRIPTION file");
        error!("Empty DESCRIPTION file");
        bail!("Empty DESCRIPTION file");
    }

    if desc.len() > 1 {
        OUTPUT.error("Invalid DESCRIPTION file, empty lines are not allowed");
        error!("Invalid DESCRIPTION file, empty lines are not allowed");
        bail!("Invalid DESCRIPTION file, empty lines are not allowed");
    }

    let paragraph = desc.iter().next().unwrap().clone();
    Ok(paragraph)
}

/// Read the project's `rproj.toml` manifest from `root`, the project
/// directory.
fn proj_read_manifest(root: &Path) -> Result<Rproj, Box<dyn Error>> {
    let path = root.join(RPROJ_MANIFEST_FILE);
    if !path.exists() {
        OUTPUT.error(&format!(
            "{} not found, run `rig proj init` first",
            RPROJ_MANIFEST_FILE
        ));
        error!("{} not found", RPROJ_MANIFEST_FILE);
        bail!("{} not found", RPROJ_MANIFEST_FILE);
    }

    let manifest: Rproj = toml::from_str(&fs::read_to_string(path)?).map_err(|e| {
        OUTPUT.error(&format!("Cannot parse {}: {}", RPROJ_MANIFEST_FILE, e));
        error!("Cannot parse {}: {}", RPROJ_MANIFEST_FILE, e);
        e
    })?;
    Ok(manifest)
}

/// Read the project's `rproj.toml` manifest from `root`, or return `None` if
/// there is no manifest. A project can be detected from an `rproj.lock` or an
/// `.rvenv` directory alone, so a missing manifest is not always an error, and
/// this is the variant for the callers that can do without one. A manifest that
/// exists but does not parse still fails.
pub(crate) fn proj_read_manifest_opt(root: &Path) -> Result<Option<Rproj>, Box<dyn Error>> {
    if !root.join(RPROJ_MANIFEST_FILE).exists() {
        return Ok(None);
    }
    Ok(Some(proj_read_manifest(root)?))
}

/// Read the project's `rproj.toml` manifest and return its name, version and
/// dependencies, with the soft dependencies dropped unless `dev`. The manifest
/// is read from `root`, the project directory.
pub(crate) fn proj_read_manifest_deps(
    root: &Path,
    dev: bool,
) -> Result<(String, RPackageVersion, PackageDependencies), Box<dyn Error>> {
    OUTPUT.status(&format!(
        "Reading dependencies from {}",
        RPROJ_MANIFEST_FILE
    ));
    info!("Reading dependencies from {}", RPROJ_MANIFEST_FILE);
    let manifest = proj_read_manifest(root)?;

    let deps = manifest.to_dep_version_specs(dev)?;
    let version = RPackageVersion::from_str(&manifest.project.version)?;
    Ok((manifest.project.name, version, deps))
}

/// Parse dependencies from the project manifest and print them out
fn sc_proj_deps(
    args: &ArgMatches,
    projargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let dev = args.get_flag("dev");
    let json = args.get_flag("json") || projargs.get_flag("json") || mainargs.get_flag("json");
    let (name, version, pkg_deps) = proj_read_manifest_deps(Path::new("."), dev)?;

    if args.get_flag("recursive") {
        return proj_deps_recursive(&name, &version, &pkg_deps, json);
    }

    let mut deps = pkg_deps.dependencies.clone();

    // Sort by dependency type first, then by package name
    deps.sort_by(|a, b| {
        // Put "R" first, always
        if a.name == "R" && b.name != "R" {
            return std::cmp::Ordering::Less;
        }
        if a.name != "R" && b.name == "R" {
            return std::cmp::Ordering::Greater;
        }
        // Original sort: by type first, then by package name
        let a_types = a
            .types
            .iter()
            .map(|t| t.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let b_types = b
            .types
            .iter()
            .map(|t| t.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        a_types.cmp(&b_types).then_with(|| a.name.cmp(&b.name))
    });

    if json {
        println!("[");
        let num = deps.len();
        for (i, pkg) in deps.iter().enumerate() {
            let mut cst: String = "".to_string();
            for (i, cs) in pkg.constraints.iter().enumerate() {
                if i > 0 {
                    cst += ", ";
                }
                cst += &format!("{} {}", cs.constraint_type, cs.version);
            }
            println!(" {{");
            let comma = if cst.is_empty() { "" } else { ", " };
            // TODO: should this be an array? Probably
            let types_str = pkg
                .types
                .iter()
                .map(|t| t.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            println!("     \"types\": \"{}\",", types_str);
            println!("     \"package\": \"{}\"{}", pkg.name, comma);
            if !cst.is_empty() {
                println!("     \"version\": \"{}\"", cst)
            }
            println!("  }}{}", if i == num - 1 { "" } else { "," });
        }
        println!("]");
    } else {
        print_header(&name, &version, &dep_count(deps.len()), false);
        if deps.is_empty() {
            return Ok(());
        }
        println!();

        let mut tab: Table = Table::new("{:<}   {:<}   {:<}");
        tab.add_row(row!("Package", "Type", "Requires"));
        tab.add_heading("-------------------------------------------------------");
        for dep in deps {
            let mut cst: String = "".to_string();
            for (i, cs) in dep.constraints.iter().enumerate() {
                if i > 0 {
                    cst += ", ";
                }
                cst += &format!("{} {}", cs.constraint_type, cs.version);
            }
            tab.add_row(row!(dep.name, type_list(&dep.types), cst));
        }

        print!("{}", tab);
    }

    Ok(())
}

/// The transitive dependency closure of a project, in the same table
/// `rig pkg deps --recursive` prints.
///
/// The soft dependencies were already dropped by [`proj_read_manifest_deps`]
/// unless `--dev` was given, so the walk takes the manifest's dependencies as
/// they are; below the project itself it only ever follows hard dependencies.
fn proj_deps_recursive(
    name: &str,
    version: &RPackageVersion,
    deps: &PackageDependencies,
    json: bool,
) -> Result<(), Box<dyn Error>> {
    let loader = DbSourcePackageLoader::new()?;
    let (rows, num_direct) = walk_deps(&loader, name, &deps.dependencies, true);

    if json {
        print_deps_json(&rows, true)?;
    } else {
        print_deps_recursive(name, version, num_direct, &rows);
    }

    Ok(())
}

/// The transitive dependency closure of a project, as the tree
/// `rig pkg tree` prints for a package.
///
/// The same closure [`proj_deps_recursive`] lists in a flat table, so the two
/// read the manifest the same way and follow the same edges; only the layout
/// differs.
///
/// `--why` prints that closure inverted, rooted at the named package, so it
/// covers the same edges the other way around.
fn sc_proj_tree(
    args: &ArgMatches,
    projargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let dev = args.get_flag("dev");
    let no_base = args.get_flag("no-base");
    let why = args.get_one::<String>("why").map(|s| s.as_str());
    let json = args.get_flag("json") || projargs.get_flag("json") || mainargs.get_flag("json");
    let (name, version, pkg_deps) = proj_read_manifest_deps(Path::new("."), dev)?;

    proj_tree(
        &name,
        &version,
        &pkg_deps.dependencies,
        dev,
        no_base,
        why,
        json,
    )
}

/// The P3M build target to resolve binary packages for.
///
/// `--platform source` means "source only", and so does a platform P3M has no
/// binaries for. Not being able to look up P3M's targets at all is an error:
/// falling back to source packages silently would produce a lockfile that does
/// not say what the caller asked for. Use `--platform source` to ask for that.
pub(crate) fn proj_binary_target(
    platform: Option<&String>,
    r_version: &str,
) -> Result<Option<BinaryTarget>, Box<dyn Error>> {
    let (target, no_binaries) = proj_binary_target_quiet(platform, r_version)?;
    if let Some(name) = no_binaries {
        OUTPUT.warn(&format!(
            "No binary packages for {}, using source packages",
            name
        ));
    }
    Ok(target)
}

/// [`proj_binary_target`] without the "no binary packages" warning.
///
/// Instead of warning, it returns the platform name that has no binaries, for
/// callers that resolve several targets at once ([`proj_lock`]): every R
/// version they resolve the platform for would repeat the same warning, so
/// they collect the names and warn once per platform.
pub(crate) fn proj_binary_target_quiet(
    platform: Option<&String>,
    r_version: &str,
) -> Result<(Option<BinaryTarget>, Option<String>), Box<dyn Error>> {
    let platform = match platform {
        Some(p) if p == "source" => {
            info!("Solving for source packages only");
            return Ok((None, None));
        }
        Some(p) => parse_platform_string(p)?,
        None => detect_platform()?,
    };

    let target = match BinaryTarget::detect(&platform, r_version) {
        Ok(target) => target,
        Err(err) => {
            // The error itself is reported by the download layer and again by
            // main, so this only adds the way out.
            OUTPUT.error(
                "Cannot look up binary package targets. \
                Use --platform source to solve for source packages only.",
            );
            error!("Cannot look up binary package targets: {}", err);
            return Err(err);
        }
    };

    let no_binaries = match &target {
        Some(target) => {
            info!("Solving for binary target {}", target.name());
            None
        }
        None => Some(
            platform
                .rig_platform
                .as_deref()
                .unwrap_or(&platform.os)
                .to_string(),
        ),
    };
    Ok((target, no_binaries))
}

/// Solve the dependencies of `deps` for one R version and one binary target.
///
/// `report_status` is for callers that solve several targets in parallel
/// ([`proj_lock`]): they print the status lines once for the whole batch, so
/// the per-solve lines here would only be N interleaved, unlabelled copies of
/// them. Single-target callers pass `true` and get the progress reported as
/// each phase starts. The log file gets the messages either way.
pub(crate) fn sc_proj_solve_deps(
    r_version: &str,
    deps: &PackageDependencies,
    target: Option<BinaryTarget>,
    prefer_binary: Option<usize>,
    report_status: bool,
) -> Result<(RPackageRegistry, SelectedDependencies<RPackageRegistry>), Box<dyn Error>> {
    info!("Solving dependencies");

    // The registry lazily loads each package's versions from the local database
    // (the full ALLPACKAGES history) as the solver visits them, instead of
    // preloading the entire CRAN version history.
    let loader = DbSourcePackageLoader::new()?;
    // Binary builds are candidates alongside the source tarball, so that the
    // `LinkingTo` versions a build was compiled against become constraints the
    // solver can backtrack over. Their indices are fetched lazily too, one
    // request per package the solve visits.
    let binaries: Option<Box<dyn BinaryIndexLoader>> =
        target.map(|t| Box::new(P3mBinaryLoader::new(t)) as Box<dyn BinaryIndexLoader>);
    let reg: RPackageRegistry =
        RPackageRegistry::with_loaders(Box::new(loader), binaries).prefer_binary(prefer_binary);

    reg.add_package_version(
        "_project".to_string(),
        RegistryPackageVersion::new("_project", "1.0.0")?,
        rpackage_version_ranges_from_constraints(deps, true),
    );

    // add R itself, for now a hardcoded version
    reg.add_package_version(
        "R".to_string(),
        RegistryPackageVersion::new("R", r_version)?,
        HashMap::with_hasher(rustc_hash::FxBuildHasher),
    );

    // add base packages, these are always available
    for bp in BASE_PKGS.iter() {
        reg.add_package_version(
            bp.to_string(),
            RegistryPackageVersion::new(bp, r_version)?,
            HashMap::with_hasher(rustc_hash::FxBuildHasher),
        );
    }

    // The binary indices are one HTTP request per package, and the solver would
    // otherwise make them one at a time, as it discovers each package. Fetch
    // them for the whole dependency closure up front instead, in parallel.
    if report_status {
        OUTPUT.status("Downloading binary package metadata");
    }
    info!("Downloading binary package metadata");
    let roots: Vec<String> = deps.dependencies.iter().map(|d| d.name.clone()).collect();
    reg.prefetch_binaries(&roots);

    if report_status {
        OUTPUT.status("Solving dependencies");
    }
    let solution = resolve(
        &reg,
        "_project".to_string(),
        RegistryPackageVersion::new("_project", "1.0.0")?,
    );

    match solution {
        Ok(sol) => Ok((reg, sol)),
        Err(e) => {
            // Parallel callers report the one failure that aborts the command
            // themselves, with the target it belongs to; N unlabelled copies
            // of the same message would say less, not more.
            if report_status {
                OUTPUT.error(&format!("Solver failed: {}", e));
            }
            error!("Solver failed: {}", e);
            bail!("Solver failed: {}", e)
        }
    }
}

fn solution_to_sorted_vec(
    solution: &SelectedDependencies<RPackageRegistry>,
) -> Vec<(String, RegistryPackageVersion)> {
    let mut vec: Vec<(String, RegistryPackageVersion)> = solution
        .iter()
        .filter(|(pkg, _ver)| *pkg != "_project")
        .map(|(pkg, ver)| (pkg.clone(), ver.clone()))
        .collect();
    vec.sort_by(|a, b| {
        // Put "R" first, always
        if a.0 == "R" && b.0 != "R" {
            return std::cmp::Ordering::Less;
        }
        if a.0 != "R" && b.0 == "R" {
            return std::cmp::Ordering::Greater;
        }
        // Original sort: by package name
        a.0.cmp(&b.0)
    });
    vec
}

/// Everything `rig proj lock` takes from the command line. `rig proj sync`
/// builds the default set of these when it has to create the lockfile itself.
struct ProjLockOptions {
    /// R versions to solve for, from `--r-version`'s comma-separated list.
    /// Empty means "the default logic in `proj_lock_r_version` picks one".
    /// More than one solves each in turn, combined with `platforms` as a
    /// cross product.
    r_versions: Vec<String>,
    /// Platforms to solve binary packages for, from `--platform`'s
    /// comma-separated list. Empty means the default platform set (see
    /// `proj_lock`). More than one solves each in turn, combined with
    /// `r_versions` as a cross product.
    platforms: Vec<String>,
    prefer_binary: Option<usize>,
    dev: bool,
}

impl Default for ProjLockOptions {
    fn default() -> Self {
        ProjLockOptions {
            r_versions: vec![],
            platforms: vec![],
            prefer_binary: None,
            // dev dependencies are included unless --no-dev is given
            dev: true,
        }
    }
}

fn sc_proj_lock(
    args: &ArgMatches,
    _libargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let opts = ProjLockOptions {
        r_versions: args
            .get_many::<String>("r-version")
            .map(|vs| vs.cloned().collect())
            .unwrap_or_default(),
        platforms: args
            .get_many::<String>("platform")
            .map(|vs| vs.cloned().collect())
            .unwrap_or_default(),
        prefer_binary: args.get_one::<usize>("prefer-binary").copied(),
        dev: !args.get_flag("no-dev"),
    };
    proj_lock(Path::new("."), &opts, args)
}

/// The R version to solve the project for, when the caller did not name one:
/// the default R version if the manifest's `R` requirement allows it, else the
/// newest installed R that does, else the current R release.
///
/// The version does not have to be installed. `rig proj lock` never runs R,
/// and `rig proj sync` installs the R version the lock file names.
pub(crate) fn proj_lock_r_version(
    deps: &PackageDependencies,
    args: &ArgMatches,
) -> Result<String, Box<dyn Error>> {
    let req = deps.dependencies.iter().find(|d| d.name == "R");
    let allowed = |version: &str| match req {
        Some(req) => req.satisfies(version).unwrap_or(false),
        None => true,
    };

    if let Some(rv) = get_default_r_version()? {
        if allowed(&rv) {
            return Ok(rv);
        }
        info!(
            "The default R ({}) does not satisfy the project's R {}",
            rv,
            r_requirement(req)
        );
    }

    // The newest installed R the manifest allows, so that a project needing
    // an R other than the default one does not have to download one.
    let mut installed: Vec<RPackageVersion> = sc_get_list_details()?
        .iter()
        .filter_map(|v| v.version.as_deref())
        .filter(|v| allowed(v))
        .filter_map(|v| RPackageVersion::from_str(v).ok())
        .collect();
    installed.sort();
    if let Some(rv) = installed.pop() {
        let msg = format!(
            "Solving for R {}, the project needs R {}.",
            rv,
            r_requirement(req)
        );
        OUTPUT.info(&msg);
        info!("{}", msg);
        return Ok(rv.original);
    }

    // Nothing installed will do, so the project needs an R it does not have
    // yet. The current release is the only version to pick without asking,
    // and `rig proj sync` installs it.
    match resolve_release_r_version(args) {
        Some(rv) if allowed(&rv) => {
            let msg = format!(
                "Solving for the current R release ({}), the project needs R {}.",
                rv,
                r_requirement(req)
            );
            OUTPUT.info(&msg);
            info!("{}", msg);
            Ok(rv)
        }
        _ => {
            let msg = format!(
                "No R version satisfies the project's R {}, specify one with --r-version.",
                r_requirement(req)
            );
            OUTPUT.error(&msg);
            error!("{}", msg);
            bail!("{}", msg)
        }
    }
}

/// A manifest's `R` requirement as it reads in `rproj.toml`, e.g. `>= 4.5`,
/// for the messages of [`proj_lock_r_version`].
fn r_requirement(req: Option<&DepVersionSpec>) -> String {
    let constraints = match req {
        Some(req) => &req.constraints,
        None => return "*".to_string(),
    };
    if constraints.is_empty() {
        return "*".to_string();
    }
    constraints
        .iter()
        .map(|c| format!("{} {}", c.constraint_type, c.version))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Solve the dependencies of the project in `root` for every `(R version,
/// platform)` combination `opts` asks for (a cross product of
/// `opts.r_versions` and `opts.platforms`), and write them all into
/// `rproj.lock`. An empty `opts.r_versions` picks one R version the usual
/// way (`proj_lock_r_version`); an empty `opts.platforms` solves for this
/// machine plus three other platforms a project typically has to run on
/// (Windows, generic glibc Linux, macOS arm64) -- see the platform_specs
/// comment below.
fn proj_lock(root: &Path, opts: &ProjLockOptions, args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    // Do this first, to report local errors early
    let dev = opts.dev;
    let (_name, _version, pkg_deps) = proj_read_manifest_deps(root, dev)?;

    // Each R version has to satisfy the manifest's own `R` requirement,
    // otherwise the solve either fails or produces a lock file for an R the
    // project rules out. An `--r-version` is taken as given, the solver
    // reports the conflict if there is one. With none given, the default
    // logic in `proj_lock_r_version` picks the one version to solve for.
    let rvers: Vec<String> = if opts.r_versions.is_empty() {
        vec![proj_lock_r_version(&pkg_deps, args)?]
    } else {
        opts.r_versions.clone()
    };
    // With no --platform, solve for this machine plus the three other
    // platforms a project typically needs to run on: Windows, a generic
    // glibc Linux build (P3M's "manylinux" distro-independent build,
    // covering any glibc-based x86_64 distro P3M has no specific build
    // for), and macOS on arm64. Each platform string is fully explicit
    // (arch-vendor-os), so it resolves the same regardless of which OS
    // `rig proj lock` itself runs on; only "this machine" (`None`) depends
    // on the host. Duplicates (e.g. "this machine" already being macOS
    // arm64) are dropped before solving, by the resolved-target dedup below,
    // so a redundant solve is never dispatched in the first place.
    let platform_specs: Vec<Option<String>> = if !opts.platforms.is_empty() {
        opts.platforms.iter().cloned().map(Some).collect()
    } else {
        vec![
            None,
            Some("x86_64-w64-mingw32".to_string()),
            Some("x86_64-unknown-linux-gnu".to_string()),
            Some("aarch64-apple-darwin".to_string()),
        ]
    };

    // Resolve and dedup every `(rver, platform)` pair up front, sequentially,
    // before any solving starts. This does two things: it decides the dedup
    // winner the same way as before (first pair in CLI order wins a given
    // resolved key), and it means the parallel solve below never wastes a
    // thread solving a target that would just be thrown away afterwards
    // (which the old post-solve dedup did routinely -- e.g. "this machine"
    // resolving to the same target as one of the three fixed platforms).
    // Dedup is keyed on the resolved `(r_version, platform)` pair, not the
    // raw CLI strings: two `--platform` spellings (e.g. "macos" and a full
    // platform triple for the same machine) can resolve to the same target.
    struct SolveTarget {
        rver: String,
        target: Option<BinaryTarget>,
        platform_key: String,
    }
    let mut solve_targets: Vec<SolveTarget> = vec![];
    let mut seen: HashSet<(String, String)> = HashSet::new();
    // Warned about once each below, not once per (R version, platform) pair.
    let mut no_binaries: BTreeSet<String> = BTreeSet::new();
    let mut source_only = false;

    for rver in &rvers {
        for platform in &platform_specs {
            let (target, missing) = proj_binary_target_quiet(platform.as_ref(), rver)?;
            source_only = source_only || target.is_none();
            if let Some(name) = missing {
                no_binaries.insert(name);
            }

            // Mirrors how `RprojLockTarget::from_solution` derives the
            // target's `platform` field (src/rproj.rs), so this pre-solve key
            // matches the key the old post-solve dedup used.
            let platform_key = target
                .as_ref()
                .map(|t| t.name())
                .unwrap_or_else(|| std::env::consts::ARCH.to_string());
            let key = (rver.clone(), platform_key.clone());
            if !seen.insert(key.clone()) {
                // Not worth a warning: with the default platform set, "this
                // machine" routinely resolves to the same target as one of
                // the three fixed platforms.
                info!("Skipping duplicate target R {} / {}", key.0, key.1);
                continue;
            }

            solve_targets.push(SolveTarget {
                rver: rver.clone(),
                target,
                platform_key,
            });
        }
    }

    // Refresh the shared package metadata cache once, sequentially, before
    // fanning the solves out to threads below. Each solve's
    // `DbSourcePackageLoader::new()` would otherwise do this too, but
    // finding it already fresh, it becomes a cheap read instead of every
    // thread racing to update the same on-disk cache at once.
    ensure_allpackages_fresh()?;

    for name in &no_binaries {
        OUTPUT.warn(&format!(
            "No binary packages for {}, using source packages",
            name
        ));
    }

    if opts.prefer_binary.is_some() && source_only {
        OUTPUT.warn("There are no binary packages to prefer, ignoring --prefer-binary");
        info!("Ignoring --prefer-binary: solving for source packages only");
    }

    // The solves below run in parallel, so each one printing its own status
    // lines would give N interleaved copies of them. Report the phases once,
    // for the whole batch, instead (`report_status: false` below).
    let multi = solve_targets.len() > 1;
    OUTPUT.status("Downloading binary package metadata");
    if multi {
        OUTPUT.status(&format!(
            "Solving dependencies for {} targets",
            solve_targets.len()
        ));
    } else {
        OUTPUT.status("Solving dependencies");
    }

    // A single solver over the full CRAN version history: it picks the
    // latest in-range version of each package first and only falls back to
    // older versions when a constraint forces it, so the common case still
    // resolves to the latest versions. With `--prefer-binary` it also falls
    // back to an older version to get a binary package instead of a source
    // one. Independent targets (different R versions and/or platforms) don't
    // share any solver state, so they solve in parallel, one thread per
    // target.
    type SolveResult = (RPackageRegistry, SelectedDependencies<RPackageRegistry>);
    let prefer_binary = opts.prefer_binary;
    let solved: Vec<(String, String, Result<SolveResult, String>)> = solve_targets
        .par_iter()
        .map(|st| {
            let result =
                sc_proj_solve_deps(&st.rver, &pkg_deps, st.target.clone(), prefer_binary, false)
                    .map_err(|e| e.to_string());
            (st.rver.clone(), st.platform_key.clone(), result)
        })
        .collect();

    let mut targets: Vec<RprojLockTarget> = vec![];
    let mut summaries: Vec<TargetSolution> = vec![];
    for (rver, platform_key, result) in solved {
        let (registry, solution) = match result {
            Ok(v) => v,
            // The failing solve logged itself inside `sc_proj_solve_deps` but
            // left the reporting to here, so that a batch of parallel solves
            // reports one failure, with the target it belongs to, instead of
            // one unlabelled message per failing thread. This aborts the whole
            // `proj lock` command, same as the old sequential `?` did, and
            // main prints the message.
            Err(msg) => {
                if multi {
                    bail!("{} for R {} / {}", msg, rver, platform_key);
                } else {
                    bail!("{}", msg);
                }
            }
        };

        let target = RprojLockTarget::from_solution(&registry, &solution);
        info!("Solved dependencies for R {} / {}", rver, platform_key);

        summaries.push(TargetSolution {
            r_version: rver.clone(),
            platform: platform_key.clone(),
            rows: solution_rows(&registry, &solution),
        });

        targets.push(target);
    }

    if multi {
        OUTPUT.success(&format!(
            "Solved dependencies for {} targets",
            summaries.len()
        ));
    } else {
        OUTPUT.success("Solved dependencies");
    }

    // The targets mostly resolve to the same packages at the same versions, so
    // one merged table with the differences called out is both shorter and
    // easier to compare than one full table per target. The targets are the
    // cross product of the R versions and the platforms, so listing the two
    // separately says the same thing in fewer, shorter lines.
    let header = if multi {
        let rvers = dedup_in_order(solve_targets.iter().map(|st| st.rver.as_str()));
        let platforms = dedup_in_order(solve_targets.iter().map(|st| st.platform_key.as_str()));
        Some(format!(
            "R {}: {}\n{}: {}",
            if rvers.len() > 1 {
                "versions"
            } else {
                "version"
            },
            rvers.join(", "),
            if platforms.len() > 1 {
                "Platforms"
            } else {
                "Platform"
            },
            platforms.join(", ")
        ))
    } else {
        None
    };
    print_solution_table(&summaries, header.as_deref());

    // Deterministic diffs: always the same order regardless of the order
    // --r-version/--platform were given in.
    targets.sort_by(|a, b| (&a.r_version, &a.platform).cmp(&(&b.r_version, &b.platform)));

    let rproj_lock = RprojLock {
        version: RPROJ_LOCK_VERSION,
        targets,
    };
    fs::write(root.join(RPROJ_LOCK_FILE), rproj_lock.to_toml()?)?;
    OUTPUT.success("Written project lockfile to rproj.lock");
    info!("Written project lockfile to rproj.lock");

    Ok(())
}

/// What one solved target says about one package, for [`proj_lock`]'s summary.
#[derive(PartialEq)]
struct SolvedRow {
    version: String,
    /// `binary`, `source`, or empty for R and the base packages.
    kind: &'static str,
    /// With `--prefer-binary`, the newer version this one was traded for.
    held_back_from: Option<String>,
}

/// One solved target's contribution to [`proj_lock`]'s summary table.
struct TargetSolution {
    r_version: String,
    /// e.g. `macos-arm64`.
    platform: String,
    rows: BTreeMap<String, SolvedRow>,
}

/// One solved target's packages, keyed by package name.
fn solution_rows(
    registry: &RPackageRegistry,
    solution: &SelectedDependencies<RPackageRegistry>,
) -> BTreeMap<String, SolvedRow> {
    let sorted_solution = solution_to_sorted_vec(solution);
    let mut rows: BTreeMap<String, SolvedRow> = BTreeMap::new();
    for (pkg, ver) in sorted_solution.iter() {
        let kind = if pkg == "R" || BASE_PKGS.contains(&pkg.as_str()) {
            ""
        } else if ver.artifact.is_binary() {
            "binary"
        } else {
            "source"
        };
        // Only set when `--prefer-binary` traded this version for a binary, so
        // that a version an ordinary constraint pushed back is not reported as
        // if the flag had done it.
        let held_back_from = registry.held_back_from(pkg, ver).map(|latest| {
            info!(
                "Held {} back to {} for a binary package, latest is {}",
                pkg, ver.version, latest
            );
            latest.to_string()
        });
        rows.insert(
            pkg.clone(),
            SolvedRow {
                version: ver.version.to_string(),
                kind,
                held_back_from,
            },
        );
    }
    rows
}

/// The value most targets agree on, with the first target breaking a tie.
fn majority<'a, T: Eq + std::hash::Hash>(values: impl Iterator<Item = &'a T> + Clone) -> &'a T {
    let mut counts: HashMap<&T, usize> = HashMap::new();
    for v in values.clone() {
        *counts.entry(v).or_insert(0) += 1;
    }
    let mut best: Option<(&T, usize)> = None;
    for v in values {
        let count = counts.get(v).copied().unwrap_or(0);
        // Strictly greater, so that a tie keeps the earlier target's value.
        if best.is_none_or(|(_, b)| count > b) {
            best = Some((v, count));
        }
    }
    best.expect("at least one target").0
}

/// The distinct values, in the order they first appear.
fn dedup_in_order<'a>(values: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let mut seen: HashSet<&str> = HashSet::new();
    values.filter(|v| seen.insert(v)).collect()
}

/// `a, b and c`, for the target lists in the notes column.
fn join_labels(labels: &[&str]) -> String {
    match labels {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {}", rest.join(", "), last),
    }
}

/// Name the targets in `subset` (indices into `targets`) as briefly as the
/// matrix allows, for the notes column of [`solution_table`].
///
/// The targets are an R version × platform matrix, so a subset that is itself
/// a full block of that matrix is named by the axes that single it out: every
/// platform of one R version is "R 4.1", every R version of one platform is
/// "macos-arm64". Only a subset that does not line up with the matrix has to
/// name its targets one by one. The empty string means "all of them", which
/// the caller leaves unsaid.
fn describe_targets(subset: &[usize], targets: &[TargetSolution]) -> String {
    let all_rvers = dedup_in_order(targets.iter().map(|t| t.r_version.as_str()));
    let all_platforms = dedup_in_order(targets.iter().map(|t| t.platform.as_str()));
    let rvers = dedup_in_order(subset.iter().map(|&i| targets[i].r_version.as_str()));
    let platforms = dedup_in_order(subset.iter().map(|&i| targets[i].platform.as_str()));

    // Every target in the `rvers` × `platforms` block is in the subset, so the
    // two axes describe it exactly.
    let block = targets
        .iter()
        .filter(|t| {
            rvers.contains(&t.r_version.as_str()) && platforms.contains(&t.platform.as_str())
        })
        .count();
    if block == subset.len() {
        return match (
            rvers.len() == all_rvers.len(),
            platforms.len() == all_platforms.len(),
        ) {
            (true, true) => String::new(),
            (false, true) => format!("R {}", join_labels(&rvers)),
            (true, false) => join_labels(&platforms),
            (false, false) => format!("R {} / {}", join_labels(&rvers), join_labels(&platforms)),
        };
    }

    let one_rver = all_rvers.len() == 1;
    let labels: Vec<String> = subset
        .iter()
        .map(|&i| {
            if one_rver {
                targets[i].platform.clone()
            } else {
                format!("R {} / {}", targets[i].r_version, targets[i].platform)
            }
        })
        .collect();
    join_labels(&labels.iter().map(|l| l.as_str()).collect::<Vec<_>>())
}

/// Print [`proj_lock`]'s summary of everything it solved, as one table.
///
/// The targets nearly always resolve to the same packages at the same
/// versions, differing only in whether a package is available as a binary, so
/// this is one row per package, and a further row only for a package that some
/// targets solved differently -- those rows leave the package column empty and
/// name the targets they hold for in the platform column. With a single target
/// nothing differs and the table is simply that target's packages.
fn print_solution_table(targets: &[TargetSolution], header: Option<&str>) {
    if let Some(header) = header {
        println!("{}", header);
    }
    println!("{}", solution_table(targets));
}

/// The table [`print_solution_table`] prints, see there.
///
/// Built twice: the rule under the header is as wide as the table, and how
/// wide that is only shows once the rows are laid out.
fn solution_table(targets: &[TargetSolution]) -> Table {
    let rows = solution_table_rows(targets);
    let measured = table_of(&rows, 0);
    let width = measured
        .to_string()
        .lines()
        .map(|l| l.trim_end().chars().count())
        .max()
        .unwrap_or(0);
    table_of(&rows, width)
}

/// One `(package, version, type, platform)` row of [`solution_table`].
type SolutionTableRow = (String, String, &'static str, String);

/// [`solution_table`]'s rows, under a header and a `rule` characters wide rule.
fn table_of(rows: &[SolutionTableRow], rule: usize) -> Table {
    let mut tab: Table = Table::new("{:<}   {:<}   {:<}   {:<}");
    tab.add_row(row!["package", "version", "type", "platform"]);
    tab.add_heading("-".repeat(rule));
    for (pkg, version, kind, platform) in rows {
        tab.add_row(row!(pkg, version, kind, platform));
    }
    tab
}

/// The rows of [`solution_table`], see there for what they say.
fn solution_table_rows(targets: &[TargetSolution]) -> Vec<SolutionTableRow> {
    let mut rows: Vec<SolutionTableRow> = vec![];

    let sorted: BTreeSet<&str> = targets
        .iter()
        .flat_map(|t| t.rows.keys().map(|k| k.as_str()))
        .collect();
    // "R" first, then the packages by name, as before.
    let mut packages: Vec<&str> = sorted.iter().copied().filter(|p| *p != "R").collect();
    if sorted.contains("R") {
        packages.insert(0, "R");
    }

    for pkg in packages {
        // Targets that have the package at all, in target order.
        let have: Vec<(usize, &SolvedRow)> = targets
            .iter()
            .enumerate()
            .filter_map(|(i, t)| t.rows.get(pkg).map(|r| (i, r)))
            .collect();

        // The targets a package is missing from are said on its first row.
        let missing: Vec<usize> = targets
            .iter()
            .enumerate()
            .filter(|(_, t)| !t.rows.contains_key(pkg))
            .map(|(i, _)| i)
            .collect();
        let missing = if missing.is_empty() {
            None
        } else {
            Some(format!("not on {}", describe_targets(&missing, targets)))
        };

        // R and the base packages are versioned with R itself, so with more
        // than one R version locked they differ on every target by
        // definition. One row per R version would say the least and take the
        // most room, so they get a placeholder version instead.
        if have.iter().all(|(_, r)| r.kind.is_empty()) {
            let version = majority(have.iter().map(|(_, r)| &r.version));
            let differs = have.iter().any(|(_, r)| &r.version != version);
            let version = if differs { "*" } else { version.as_str() };
            rows.push((
                pkg.to_string(),
                version.to_string(),
                "",
                missing.unwrap_or_default(),
            ));
            continue;
        }

        // Group the targets by everything the row says. Each group is a row of
        // its own -- a package solved to two versions, or to a binary on some
        // targets and a source build on others, is easier to read as two rows
        // than as one row with the differences squeezed into a note.
        let mut groups: Vec<(&SolvedRow, Vec<usize>)> = vec![];
        for (i, row) in &have {
            match groups.iter_mut().find(|(r, _)| *r == *row) {
                Some((_, idx)) => idx.push(*i),
                None => groups.push((row, vec![*i])),
            }
        }
        // Most targets first, so that the package's name is on the row that
        // holds for most of them, and the exceptions read as exceptions. A
        // held-back version loses a tie: it is the exception by nature.
        groups.sort_by_key(|(r, idx)| (std::cmp::Reverse(idx.len()), r.held_back_from.is_some()));
        let versions: HashSet<&str> = groups.iter().map(|(r, _)| r.version.as_str()).collect();

        for (n, (row, idx)) in groups.iter().enumerate() {
            let mut what: Vec<String> = vec![];
            // The first row is the one that holds for most targets, so it is
            // the default and the rows below it are the exceptions to it.
            // Naming its targets would be the longest and least useful cell in
            // the table -- every target the exceptions do not claim.
            if n > 0 {
                what.push(describe_targets(idx, targets));
            }
            if let Some(latest) = &row.held_back_from {
                // The version it was held back from is already in the table
                // when some other target solved to it.
                if versions.contains(latest.as_str()) {
                    what.push("held back for a binary package".to_string());
                } else {
                    what.push(format!(
                        "held back for a binary package, latest is {}",
                        latest
                    ));
                }
            }
            if n == 0 {
                if let Some(missing) = &missing {
                    what.push(missing.clone());
                }
            }
            // Only the first row names the package, the rest belong to it.
            let name = if n == 0 { pkg } else { "" };
            rows.push((
                name.to_string(),
                row.version.clone(),
                row.kind,
                what.join(", "),
            ));
        }
    }

    rows
}

/// The lockfile packages that are needed without the dev dependencies.
fn nondev_packages(
    root: &Path,
    packages: &[RprojLockPackage],
) -> Result<HashSet<String>, Box<dyn Error>> {
    let (_name, _version, deps) = proj_read_manifest_deps(root, false)?;
    let by_name: HashMap<&str, &RprojLockPackage> =
        packages.iter().map(|p| (p.package.as_str(), p)).collect();

    let mut keep: HashSet<String> = HashSet::new();
    let mut todo: Vec<String> = deps.dependencies.iter().map(|d| d.name.clone()).collect();
    while let Some(name) = todo.pop() {
        if !keep.insert(name.clone()) {
            continue;
        }
        // R and the base packages are dependencies in the manifest, but never
        // lockfile entries, so they simply do not match anything here.
        if let Some(pkg) = by_name.get(name.as_str()) {
            todo.extend(pkg.dependencies.iter().cloned());
        }
    }
    Ok(keep)
}

/// The R installation an environment for `r_version` on `arch` uses: its
/// name, as `rig list` shows it, and the absolute path of its R binary.
///
/// The lock file records the R version and the platform its solve is valid
/// for, so neither is a preference, they are what the environment *is*. When
/// that R is missing and `install` is set, install it first -- the thing renv
/// cannot do.
fn rvenv_r_installation(
    r_version: &str,
    arch: &str,
    install: bool,
) -> Result<(String, PathBuf), Box<dyn Error>> {
    if let Some(name) = find_r_installation(r_version, arch)? {
        let binary = get_r_binary(&name)?;
        return Ok((name, binary));
    }

    let add_args = r_add_args(r_version, arch);
    if !install {
        let msg = format!(
            "R {} ({}) is not installed, install it with `rig {}` \
             (or drop --no-install-r)",
            r_version,
            arch,
            add_args[1..].join(" ")
        );
        OUTPUT.error(&msg);
        error!("{}", msg);
        bail!("{}", msg);
    }

    OUTPUT.status(&format!(
        "R {} ({}) is not installed, installing it now",
        r_version, arch
    ));
    info!(
        "R {} ({}) is not installed, installing it now",
        r_version, arch
    );
    // `rig add` is a subcommand, not a function that takes a version, so go
    // through clap. It escalates on its own in admin mode.
    let matches = rig_app().try_get_matches_from(add_args)?;
    let (_name, addargs) = match matches.subcommand() {
        Some(x) => x,
        None => bail!("Internal error: `rig add` did not parse"),
    };
    sc_add(addargs)?;

    match find_r_installation(r_version, arch)? {
        Some(name) => {
            let binary = get_r_binary(&name)?;
            Ok((name, binary))
        }
        None => {
            let msg = format!(
                "Installed R {} ({}), but cannot find it now",
                r_version, arch
            );
            OUTPUT.error(&msg);
            error!("{}", msg);
            bail!("{}", msg)
        }
    }
}

/// The `rig add` command line that installs `r_version` for `arch`. Only
/// macOS has R builds for more than one architecture, and only there does
/// `rig add` take `--arch`.
fn r_add_args(r_version: &str, arch: &str) -> Vec<String> {
    let mut args = vec!["rig".to_string(), "add".to_string()];
    if cfg!(target_os = "macos") {
        args.push("--arch".to_string());
        args.push(arch.to_string());
    }
    args.push(r_version.to_string());
    args
}

/// The installed R that matches `r_version` on `arch`: the installation of
/// that name, or, failing that, one with the very same version -- the lock
/// file records a version like `4.6.1`, while an installation of it can be
/// called `4.6.1` or `4.6.1-arm64`.
///
/// The architecture has to match too: an R of another architecture cannot use
/// the packages the lock file resolved, whatever its version.
fn find_r_installation(r_version: &str, arch: &str) -> Result<Option<String>, Box<dyn Error>> {
    let installed = sc_get_list_details()?;
    let matching = installed.iter().find(|candidate| {
        if rvenv_r_arch(&candidate.name) != arch {
            return false;
        }
        if candidate.name == r_version {
            return true;
        }
        // An installation with no version, or a version that is not a number
        // (`devel`, `next`), is never a match for a lock file's R version.
        match &candidate.version {
            Some(version) => r_version_matches(r_version, version),
            None => false,
        }
    });
    Ok(matching.map(|v| v.name.clone()))
}

/// The architecture the lock file's target platform needs, in the form
/// [`rvenv_r_arch`] reports it, or the machine's own if the platform does not
/// name one.
fn target_r_arch(platform: &str) -> String {
    match platform.rsplit_once('-') {
        Some((_, "arm64")) | Some((_, "aarch64")) => native_arch_name("aarch64"),
        Some((_, "x86_64")) => "x86_64".to_string(),
        _ => native_arch_name(std::env::consts::ARCH),
    }
}

/// The OS family a lock target's platform string implies, or `None` if it
/// names none -- a `--platform source` solve's `platform` field is just the
/// bare CPU arch (e.g. `"aarch64"`, see `RprojLockTarget::from_solution`), which
/// carries no OS marker and so matches any machine with the right arch.
fn target_os_family(platform: &str) -> Option<&'static str> {
    match platform.rsplit_once('-') {
        Some(("macos", _)) => Some("macos"),
        Some(("windows", _)) => Some("windows"),
        Some((prefix, _)) if !prefix.is_empty() => Some("linux"),
        _ => None,
    }
}

/// This machine's OS family, in [`target_os_family`]'s terms.
fn this_os_family() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        _ => "linux",
    }
}

/// The lock file target `rig proj sync` installs: the one whose platform's OS
/// matches this machine (or names none), further narrowed by `--r-version`/
/// `--platform` if the caller gave them. Several matches are not an error --
/// the highest R version among them wins, so locking for several R versions
/// just works without extra flags; only zero matches is a hard error.
fn select_sync_target<'a>(
    targets: &'a [RprojLockTarget],
    r_version: Option<&str>,
    platform: Option<&str>,
) -> Result<&'a RprojLockTarget, Box<dyn Error>> {
    let this_os = this_os_family();
    let mut candidates: Vec<&RprojLockTarget> = targets
        .iter()
        .filter(|t| match target_os_family(&t.platform) {
            Some(os) => os == this_os,
            None => true,
        })
        .filter(|t| platform.is_none_or(|p| t.platform == p))
        .filter(|t| r_version.is_none_or(|rv| r_version_matches(rv, &t.r_version)))
        .collect();

    if candidates.is_empty() {
        let available: Vec<String> = targets
            .iter()
            .map(|t| format!("R {} / {}", t.r_version, t.platform))
            .collect();
        let msg = format!(
            "No target in rproj.lock matches this machine ({}{}{}). Available: {}. \
             Run `rig proj lock` for this machine, or check --r-version/--platform.",
            this_os,
            r_version.map(|v| format!(", R {}", v)).unwrap_or_default(),
            platform
                .map(|p| format!(", platform {}", p))
                .unwrap_or_default(),
            if available.is_empty() {
                "none".to_string()
            } else {
                available.join(", ")
            }
        );
        OUTPUT.error(&msg);
        error!("{}", msg);
        bail!("{}", msg);
    }

    candidates.sort_by_key(|t| r_components(&t.r_version).unwrap_or_default());
    Ok(candidates.pop().unwrap())
}

/// Whether an installed R version is the one the lock file asks for: the same
/// version, or, if the lock file names a minor version only (`4.6`), any patch
/// release of it (`4.6.1`).
///
/// Another patch release of the same minor version is not a match. R packages
/// are compatible across patch releases, so using one would work, but the lock
/// file says which R the project is for, and `rig proj sync` installs that one
/// instead of silently building the environment for a different R.
fn r_version_matches(want: &str, have: &str) -> bool {
    if want == have {
        return true;
    }
    let (want, have) = match (r_components(want), r_components(have)) {
        (Some(w), Some(h)) => (w, h),
        _ => return false,
    };
    !want.is_empty() && want.len() < 3 && have.len() >= want.len() && have[..want.len()] == want[..]
}

/// The numeric components of an R version, or `None` if it is not a version
/// number at all (`devel`, `next`). Quiet, unlike `minor_r_version`, because
/// this runs over every installed R version, most of which do not match anyway.
fn r_components(version: &str) -> Option<Vec<u32>> {
    RPackageVersion::from_str(version)
        .ok()
        .map(|v| v.components)
}

/// The architecture of an R installation, from its name (`4.6-arm64`), or the
/// machine's own if the name does not say.
fn rvenv_r_arch(name: &str) -> String {
    match name.rsplit_once('-') {
        Some((_, arch)) if arch == "arm64" || arch == "x86_64" => arch.to_string(),
        _ => native_arch_name(std::env::consts::ARCH),
    }
}

/// An architecture the way rig names it: `arm64` on macOS and `aarch64`
/// everywhere else.
fn native_arch_name(arch: &str) -> String {
    match arch {
        "aarch64" | "arm64" if cfg!(target_os = "macos") => "arm64".to_string(),
        "arm64" => "aarch64".to_string(),
        other => other.to_string(),
    }
}

/// What [`proj_sync`] does beyond the defaults, i.e. the options of
/// `rig proj sync`. `rig run` syncs with the defaults.
pub(crate) struct ProjSyncOptions {
    /// Install the manifest's dev dependencies as well (`--no-dev` turns this
    /// off).
    pub dev: bool,
    /// Install into this library instead of the project's own `.rvenv/lib`
    /// (`--library`). Only the project library gets the wrappers, the
    /// repositories file and the sync stamp.
    pub library: Option<PathBuf>,
    /// Install the R version the lock file names, if it is missing
    /// (`--no-install-r` turns this off).
    pub install_r: bool,
    /// How many packages to install at the same time (`--max-concurrent`).
    pub max_concurrent: usize,
    /// Which target to sync when more than one matches this machine
    /// (`--r-version`). Selects among `rproj.lock`'s existing targets, does
    /// not trigger a new solve.
    pub r_version: Option<String>,
    /// Which target to sync when more than one matches this machine
    /// (`--platform`). Selects among `rproj.lock`'s existing targets, does
    /// not trigger a new solve.
    pub platform: Option<String>,
}

impl Default for ProjSyncOptions {
    fn default() -> Self {
        ProjSyncOptions {
            dev: true,
            library: None,
            install_r: true,
            max_concurrent: 8,
            r_version: None,
            platform: None,
        }
    }
}

fn sc_proj_sync(
    args: &ArgMatches,
    _libargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    // The project is the nearest one at or above the current directory, so
    // that `rig proj sync` works from a subdirectory, like `git` does.
    let cwd = std::env::current_dir()?;
    let root = find_project_root(&cwd).unwrap_or(cwd);

    let opts = ProjSyncOptions {
        dev: !args.get_flag("no-dev"),
        library: args.get_one::<String>("library").map(PathBuf::from),
        install_r: !args.get_flag("no-install-r"),
        max_concurrent: args
            .get_one::<usize>("max-concurrent")
            .copied()
            .unwrap_or(8),
        r_version: args.get_one::<String>("r-version").cloned(),
        platform: args.get_one::<String>("platform").cloned(),
    };

    proj_sync(&root, &opts, args)
}

/// Install the project's locked dependencies into its environment, creating
/// the machine-specific part of `.rvenv` on the way: `rig proj sync`, and the
/// automatic sync of `rig run`.
///
/// `args` is only used for the platform and architecture of an R version that
/// has to be resolved from scratch, so any subcommand's matches will do.
pub(crate) fn proj_sync(
    root: &Path,
    opts: &ProjSyncOptions,
    args: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    // Read the lockfile to get package information, then pick the target
    // this machine's OS matches (see `select_sync_target`), the highest R
    // version among them if there is more than one.
    // No lockfile yet, so create one first, with the default options, instead
    // of erroring out. `rig proj lock` reads the project's `rproj.toml`, and
    // errors out itself if there is none.
    let lock_path = root.join(RPROJ_LOCK_FILE);
    if !lock_path.exists() {
        OUTPUT.info(&format!(
            "No {}, running `rig proj lock` first",
            RPROJ_LOCK_FILE
        ));
        info!("No {}, running `rig proj lock` first", RPROJ_LOCK_FILE);
        let lock_opts = ProjLockOptions {
            dev: opts.dev,
            ..ProjLockOptions::default()
        };
        proj_lock(root, &lock_opts, args)?;
    }

    let lock_content = fs::read_to_string(&lock_path)?;
    RprojLock::check_version(&lock_content)?;
    let lock: RprojLock = toml::from_str(&lock_content)?;
    let target = select_sync_target(
        &lock.targets,
        opts.r_version.as_deref(),
        opts.platform.as_deref(),
    )?;

    let nondev;
    let wanted: &[RprojLockPackage] = if !opts.dev {
        let keep = nondev_packages(root, &target.packages)?;
        nondev = target
            .packages
            .iter()
            .filter(|p| keep.contains(&p.package))
            .cloned()
            .collect::<Vec<_>>();
        &nondev
    } else {
        &target.packages
    };

    // Library path: --library, or the project library by default. The
    // project library is created by `rig proj init`, together with the
    // `.gitignore` files that keep it in version control, so do not create
    // it here.
    let library_path = match &opts.library {
        Some(lib) => lib.clone(),
        None => {
            let lib = project_library(root);
            if !lib.exists() {
                let msg = format!(
                    "No project library in {}, run `rig proj init` first \
                     (or pass --library)",
                    lib.display()
                );
                OUTPUT.error(&msg);
                error!("{}", msg);
                bail!("{}", msg);
            }
            lib
        }
    };

    // Everything below installs against the R version the lock file was
    // solved for, so resolve (and, unless --no-install-r, install) it before
    // touching the library: installed R packages are tied to the R minor
    // version, so the R on `PATH` is not good enough.
    // The architecture comes from the lock file's platform, not from the
    // machine: a lock file solved for macos-x86_64 needs an x86_64 R even on
    // an arm64 Mac.
    let r_arch = target_r_arch(&target.platform);
    let (r_name, r_binary) = rvenv_r_installation(&target.r_version, &r_arch, opts.install_r)?;

    // The project environment, as opposed to an arbitrary `--library`, also
    // owns the wrappers, the activation scripts and the sync stamp.
    let in_project_library = library_path == project_library(root);
    if in_project_library {
        let cfg = RvenvCfg {
            r_version: r_name.clone(),
            r_minor: minor_r_version(&target.r_version)?,
            r_binary: r_binary.clone(),
            platform: target.platform.clone(),
            r_arch: rvenv_r_arch(&r_name),
            rig_version: env!("CARGO_PKG_VERSION").to_string(),
        };
        // An environment that was built against a different R is not stale,
        // it is broken: R packages are tied to the R minor version. Say so,
        // because the packages already in the library are about to be used
        // with a different R than they were installed for.
        if let Some(old) = read_rvenv_cfg(root)? {
            if old.r_minor != cfg.r_minor || old.r_arch != cfg.r_arch {
                let msg = format!(
                    "This environment was built for R {} ({}), rebuilding it for R {} ({}). \
                     Remove {} and sync again if a package misbehaves.",
                    old.r_minor,
                    old.r_arch,
                    cfg.r_minor,
                    cfg.r_arch,
                    project_library(root).display()
                );
                OUTPUT.warn(&msg);
                info!("{}", msg);
            }
        }

        let manifest = proj_read_manifest(root)?;
        let written = rvenv_sync(root, &cfg, &manifest.repository)?;
        for path in &written {
            let path = path.strip_prefix(root).unwrap_or(path);
            info!("Updated {}", path.display());
        }
        OUTPUT.success(&format!(
            "Updated the project environment for R {} ({})",
            r_name,
            r_binary.display()
        ));
    }

    // A package already in the library, at the version and provenance the
    // lockfile asks for, does not need to be downloaded or reinstalled.
    let already_installed = if library_path.exists() {
        read_installed(&library_path)?
    } else {
        vec![]
    };
    let plan = plan_installs(wanted, &already_installed, false);
    print_plan(&format!("({})", library_path.display()), &plan);
    let todo: Vec<&RprojLockPackage> = plan
        .iter()
        .filter(|p| p.install)
        .map(|p| p.package)
        .collect();

    // The shim package in the project library compares this stamp to
    // `rproj.lock` and warns in every R session while they differ, so it has
    // to be updated even when there was nothing to install.
    if todo.is_empty() {
        if in_project_library {
            write_sync_stamp(&library_path, &lock_path)?;
        }
        OUTPUT.success(&format!(
            "Everything is up to date in {}",
            library_path.display()
        ));
        info!("Nothing to install in {}", library_path.display());
        return Ok(());
    }

    // Download only the packages that are actually going to be installed
    OUTPUT.status("Downloading packages");
    info!("Downloading packages");
    let to_download: Vec<RprojLockPackage> = todo.iter().map(|p| (*p).clone()).collect();
    download_lockfile_packages(&to_download)?;

    // Get cache directory where packages were downloaded
    let cache_dir = get_cache_dir()?;

    // Ensure library directory exists
    fs::create_dir_all(&library_path)?;

    // Install with the R version the lock file was solved for, not whatever
    // is on `PATH`: an installed R package is tied to the R minor version.
    let r_binary = r_binary
        .to_str()
        .ok_or("The R installation path is not valid Unicode")?;

    // Build Vec<PackageInfo> for the packages that need installing
    let built = BuiltCache::new(&target.r_version, r_binary);
    let installing: HashSet<&str> = todo.iter().map(|p| p.package.as_str()).collect();
    let packages: Vec<PackageInfo> = todo
        .iter()
        .map(|pkg| {
            let mut info = lockfile_package_info(pkg, &cache_dir, built.as_ref());
            info.dependencies
                .retain(|d| installing.contains(d.as_str()));
            info
        })
        .collect();

    let max_concurrent = opts.max_concurrent;

    let total_packages = packages.len();
    OUTPUT.status(&format!(
        "Installing {} of {} packages to {}",
        total_packages,
        wanted.len(),
        library_path.display()
    ));
    info!(
        "Installing {} of {} packages to {}",
        total_packages,
        wanted.len(),
        library_path.display()
    );

    let installed = install_packages(packages, &library_path, r_binary, max_concurrent)?;

    if in_project_library {
        write_sync_stamp(&library_path, &lock_path)?;
    }

    OUTPUT.success(&format!(
        "Deployment complete, installed {} packages",
        installed
    ));
    info!("Deployment complete, installed {} packages", installed);
    Ok(())
}

/// What to install for one lockfile entry, including the provenance
/// `RprojLockTarget::from_solution` recorded in its `metadata`.
///
/// `built` is the cache of packages rig compiled itself, and only a source
/// package has anything to do with it: a binary is already built, and rig has
/// nothing to add to it.
pub(crate) fn lockfile_package_info(
    pkg: &RprojLockPackage,
    cache_dir: &Path,
    built: Option<&BuiltCache>,
) -> PackageInfo {
    let mut info = PackageInfo {
        name: pkg.package.clone(),
        version: pkg.version.clone(),
        binary: pkg.binary,
        file_path: cache_dir.join("packages").join(&pkg.target),
        dependencies: pkg.dependencies.clone(),
        hash: pkg.metadata.get(REMOTE_HASH_FIELD).cloned(),
        linkingto: pkg
            .metadata
            .get(REMOTE_LINKINGTO_FIELD)
            .map(|s| parse_linkingto(s))
            .unwrap_or_default(),
        built: None,
    };
    if !info.binary {
        info.built = built.and_then(|cache| cache.path(&info));
    }
    info
}

/// Cache package files forever. They are immutable on PPM.
/// This will be different for CRAN and CRAN-like repositories.
pub(crate) const PACKAGE_FILE_TTL: Duration = Duration::MAX;

/// Download every package a lockfile names into the package cache.
///
/// Takes a plain package slice, not a whole lockfile, so any caller with a
/// `Vec<RprojLockPackage>` can use it directly — `rig pkg install`, which
/// solves in memory and never writes a lockfile, and `rig proj sync`, which
/// reads one target's packages out of `rproj.lock`.
pub(crate) fn download_lockfile_packages(
    packages: &[RprojLockPackage],
) -> Result<(), Box<dyn Error>> {
    // Get cache directory
    let cache_dir = get_cache_dir()?;

    // Build download list: (sources, target_path) for each package
    let mut downloads: Vec<(Vec<String>, PathBuf)> = Vec::new();
    for pkg in packages {
        let target_path = cache_dir.join("packages").join(&pkg.target);
        create_parent_dir_if_needed(&target_path)?;
        downloads.push((pkg.sources.clone(), target_path));
    }

    let total = downloads.len();

    // Create progress bars
    let multi_progress = MultiProgress::new();
    let overall_pb = multi_progress.add(ProgressBar::new(total as u64));
    overall_pb.set_style(
        ProgressStyle::default_bar()
            .template("{msg} [{bar:40.green/blue}] {pos}/{len} packages")
            .unwrap()
            .progress_chars("=>-"),
    );
    overall_pb.set_message("Downloading");

    // Track results using Cell for interior mutability
    let success_count = Cell::new(0);
    let cached_count = Cell::new(0);
    let error: Cell<Option<(usize, String)>> = Cell::new(None);

    // Download all packages concurrently with progress updates
    OUTPUT.status(&format!("Downloading {} packages", total));
    info!("Downloading {} packages", total);
    download_multiple_first_available_with_progress(
        downloads,
        Some(PACKAGE_FILE_TTL),
        None,
        |idx, result| match result {
            Ok((downloaded, _etag)) => {
                if *downloaded {
                    success_count.set(success_count.get() + 1);
                    overall_pb.println(format!("✓ Downloaded: {}", packages[idx].package));
                } else {
                    cached_count.set(cached_count.get() + 1);
                    overall_pb.println(format!("✓ Cached: {}", packages[idx].package));
                }
                overall_pb.inc(1);
            }
            Err(e) => {
                error.set(Some((idx, e.to_string())));
                overall_pb.finish_and_clear();
            }
        },
    );

    // Check if there was an error
    if let Some((idx, err)) = error.into_inner() {
        OUTPUT.error(&format!(
            "Failed to download {}: {}",
            packages[idx].package, err
        ));
        error!("Failed to download {}: {}", packages[idx].package, err);
        bail!("Failed to download {}: {}", packages[idx].package, err);
    }

    overall_pb.finish_with_message(format!(
        "Complete: {} downloaded, {} cached",
        success_count.get(),
        cached_count.get()
    ));

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rproj::{Dependency, Group};
    use std::collections::BTreeMap;

    /// One target's rows for [`solution_table`], from `(package, version,
    /// kind, held back from)` tuples.
    fn solved_target(
        r_version: &str,
        platform: &str,
        rows: &[(&str, &str, &'static str, Option<&str>)],
    ) -> TargetSolution {
        TargetSolution {
            r_version: r_version.to_string(),
            platform: platform.to_string(),
            rows: rows
                .iter()
                .map(|(pkg, version, kind, held_back_from)| {
                    (
                        pkg.to_string(),
                        SolvedRow {
                            version: version.to_string(),
                            kind,
                            held_back_from: held_back_from.map(|v| v.to_string()),
                        },
                    )
                })
                .collect(),
        }
    }

    /// The table rows, without the header and with the column padding
    /// squeezed to a single space, so that the tests read as rows and not as
    /// whatever width the columns happen to have.
    fn table_rows(targets: &[TargetSolution]) -> Vec<String> {
        solution_table(targets)
            .to_string()
            .lines()
            .skip(2)
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect()
    }

    #[test]
    fn one_target_is_that_target() {
        let rows = table_rows(&[solved_target(
            "4.5.1",
            "macos-arm64",
            &[
                ("R", "4.5.1", "", None),
                ("cli", "3.6.5", "binary", None),
                ("glue", "1.8.0", "source", Some("1.8.1")),
            ],
        )]);
        assert_eq!(
            rows,
            vec![
                "R 4.5.1",
                "cli 3.6.5 binary",
                "glue 1.8.0 source held back for a binary package, latest is 1.8.1",
            ]
        );
    }

    #[test]
    fn agreeing_targets_collapse_to_one_row_each() {
        let rows = table_rows(&[
            solved_target("4.5.1", "macos-arm64", &[("cli", "3.6.5", "binary", None)]),
            solved_target(
                "4.5.1",
                "windows-x86_64",
                &[("cli", "3.6.5", "binary", None)],
            ),
        ]);
        assert_eq!(rows, vec!["cli 3.6.5 binary"]);
    }

    #[test]
    fn a_target_that_solved_differently_is_a_row_of_its_own() {
        let rows = table_rows(&[
            solved_target(
                "4.5.1",
                "macos-arm64",
                &[
                    ("cli", "3.6.5", "binary", None),
                    ("glue", "1.8.0", "binary", None),
                ],
            ),
            solved_target(
                "4.5.1",
                "windows-x86_64",
                &[
                    ("cli", "3.6.5", "source", None),
                    ("glue", "1.7.0", "source", None),
                ],
            ),
            solved_target(
                "4.5.1",
                "manylinux-x86_64",
                &[
                    ("cli", "3.6.5", "binary", None),
                    ("glue", "1.8.0", "binary", None),
                ],
            ),
        ]);
        // The first row of a package is the one most targets got, and only
        // the rows below it, the exceptions, name their targets.
        assert_eq!(
            rows,
            vec![
                "cli 3.6.5 binary",
                "3.6.5 source windows-x86_64",
                "glue 1.8.0 binary",
                "1.7.0 source windows-x86_64",
            ]
        );
    }

    #[test]
    fn a_whole_row_or_column_of_the_matrix_is_named_by_its_axis() {
        let held = |version: &'static str, from: Option<&'static str>| {
            move |rver: &str, platform: &str| {
                solved_target(rver, platform, &[("bslib", version, "binary", from)])
            }
        };
        let old = held("0.10.0", Some("0.12.0"));
        let new = held("0.12.0", None);
        let rows = table_rows(&[
            old("4.1", "macos-arm64"),
            old("4.1", "windows-x86_64"),
            new("4.4", "macos-arm64"),
            new("4.4", "windows-x86_64"),
            new("4.5", "macos-arm64"),
            new("4.5", "windows-x86_64"),
        ]);
        // Every platform of R 4.1, so the platforms need no naming, and the
        // version it was held back from is a row of the table already.
        assert_eq!(
            rows,
            vec![
                "bslib 0.12.0 binary",
                "0.10.0 binary R 4.1, held back for a binary package",
            ]
        );

        // The same the other way round: every R version of one platform.
        let rows = table_rows(&[
            solved_target("4.4", "macos-arm64", &[("cli", "3.6.5", "binary", None)]),
            solved_target("4.4", "windows-x86_64", &[("cli", "3.6.5", "source", None)]),
            solved_target("4.5", "macos-arm64", &[("cli", "3.6.5", "binary", None)]),
            solved_target("4.5", "windows-x86_64", &[("cli", "3.6.5", "source", None)]),
        ]);
        assert_eq!(
            rows,
            vec!["cli 3.6.5 binary", "3.6.5 source windows-x86_64"]
        );
    }

    #[test]
    fn r_and_the_base_packages_do_not_list_every_r_version() {
        let rows = table_rows(&[
            solved_target(
                "4.4",
                "macos-arm64",
                &[("R", "4.4", "", None), ("methods", "4.4", "", None)],
            ),
            solved_target(
                "4.6",
                "macos-arm64",
                &[("R", "4.6", "", None), ("methods", "4.6", "", None)],
            ),
        ]);
        assert_eq!(rows, vec!["R *", "methods *"]);
    }

    #[test]
    fn a_package_only_some_targets_have_says_which() {
        let rows = table_rows(&[
            solved_target("4.5.1", "macos-arm64", &[("cli", "3.6.5", "binary", None)]),
            solved_target(
                "4.5.1",
                "windows-x86_64",
                &[
                    ("cli", "3.6.5", "binary", None),
                    ("curl", "6.0.1", "binary", None),
                ],
            ),
        ]);
        assert_eq!(
            rows,
            vec!["cli 3.6.5 binary", "curl 6.0.1 binary not on macos-arm64"]
        );
    }

    #[test]
    fn a_held_back_version_no_other_target_has_names_the_version_it_lost() {
        let rows = table_rows(&[
            solved_target(
                "4.5.1",
                "macos-arm64",
                &[("glue", "1.8.0", "binary", Some("1.8.1"))],
            ),
            solved_target(
                "4.5.1",
                "windows-x86_64",
                &[("glue", "1.8.0", "binary", Some("1.8.1"))],
            ),
        ]);
        assert_eq!(
            rows,
            vec!["glue 1.8.0 binary held back for a binary package, latest is 1.8.1"]
        );

        let rows = table_rows(&[
            solved_target(
                "4.5.1",
                "macos-arm64",
                &[("glue", "1.8.0", "binary", Some("1.8.1"))],
            ),
            solved_target(
                "4.5.1",
                "windows-x86_64",
                &[("glue", "1.8.0", "binary", None)],
            ),
        ]);
        assert_eq!(
            rows,
            vec![
                "glue 1.8.0 binary",
                "1.8.0 binary macos-arm64, held back for a binary package, latest is 1.8.1",
            ]
        );
    }

    /// One lockfile entry: its name and the packages it depends on.
    fn locked(name: &str, deps: &[&str]) -> RprojLockPackage {
        RprojLockPackage {
            package: name.to_string(),
            version: "1.0.0".to_string(),
            binary: true,
            platform: "testos".to_string(),
            dependencies: deps.iter().map(|d| d.to_string()).collect(),
            metadata: HashMap::new(),
            sources: vec![],
            target: format!("bin/{}_1.0.0.tgz", name),
        }
    }

    fn dep(version: &str) -> Dependency {
        Dependency::Version(version.to_string())
    }

    #[test]
    fn a_lock_files_r_version_matches_that_version_only() {
        assert!(r_version_matches("4.6.1", "4.6.1"));
        // A different patch release is a different R
        assert!(!r_version_matches("4.6.1", "4.6.0"));
        assert!(!r_version_matches("4.6.1", "4.6.2"));
        assert!(!r_version_matches("4.6.1", "4.5.1"));
        // A minor version matches all of its patch releases
        assert!(r_version_matches("4.6", "4.6.1"));
        assert!(r_version_matches("4.6", "4.6"));
        assert!(!r_version_matches("4.6", "4.5.1"));
        // `devel` and `next` are not version numbers
        assert!(!r_version_matches("4.6.1", "devel"));
        assert!(!r_version_matches("devel", "4.6.1"));
        assert!(r_version_matches("devel", "devel"));
    }

    #[test]
    fn the_target_platform_decides_the_architecture() {
        let native = native_arch_name(std::env::consts::ARCH);
        assert_eq!(target_r_arch("macos-x86_64"), "x86_64");
        assert_eq!(target_r_arch("linux-ubuntu-24.04-x86_64"), "x86_64");
        assert_eq!(target_r_arch("windows"), native);
        assert_eq!(target_r_arch("source"), native);
        if cfg!(target_os = "macos") {
            assert_eq!(target_r_arch("macos-arm64"), "arm64");
            assert_eq!(target_r_arch("macos-aarch64"), "arm64");
        } else {
            assert_eq!(target_r_arch("linux-ubuntu-24.04-aarch64"), "aarch64");
        }
    }

    #[test]
    fn r_is_installed_for_the_architecture_the_lock_file_needs() {
        assert_eq!(
            r_add_args("4.6.1", "x86_64"),
            if cfg!(target_os = "macos") {
                vec!["rig", "add", "--arch", "x86_64", "4.6.1"]
            } else {
                vec!["rig", "add", "4.6.1"]
            }
        );
    }

    #[test]
    fn default_lock_platforms_parse_to_the_expected_targets() {
        // These literals are `proj_lock`'s default `--platform` set (used
        // when the user gives none): host-independent so they resolve the
        // same regardless of which OS `rig proj lock` runs on.
        let windows = parse_platform_string("x86_64-w64-mingw32").unwrap();
        assert_eq!(windows.arch, "x86_64");
        assert_eq!(windows.os, "mingw32");

        let manylinux = parse_platform_string("x86_64-unknown-linux-gnu").unwrap();
        assert_eq!(manylinux.arch, "x86_64");
        assert!(manylinux.os.starts_with("linux"));
        assert_eq!(manylinux.distro, None);

        let macos_arm64 = parse_platform_string("aarch64-apple-darwin").unwrap();
        assert_eq!(macos_arm64.arch, "aarch64");
        assert!(macos_arm64.os.starts_with("darwin"));
    }

    #[test]
    fn target_os_family_reads_the_platform_string() {
        assert_eq!(target_os_family("macos-arm64"), Some("macos"));
        assert_eq!(target_os_family("macos-x86_64"), Some("macos"));
        assert_eq!(target_os_family("windows-x86_64"), Some("windows"));
        assert_eq!(target_os_family("jammy-x86_64"), Some("linux"));
        assert_eq!(target_os_family("linux-ubuntu-24.04-x86_64"), Some("linux"));
        // A `--platform source` solve's platform is a bare CPU arch: no OS.
        assert_eq!(target_os_family("aarch64"), None);
        assert_eq!(target_os_family("x86_64"), None);
    }

    fn target(r_version: &str, platform: &str) -> RprojLockTarget {
        RprojLockTarget {
            r_version: r_version.to_string(),
            platform: platform.to_string(),
            packages: vec![],
        }
    }

    #[test]
    fn select_sync_target_is_a_noop_with_a_single_target() {
        let this_os = this_os_family();
        let platform = format!("{}-x86_64", this_os);
        let targets = vec![target("4.6.1", &platform)];
        let picked = select_sync_target(&targets, None, None).unwrap();
        assert_eq!(picked.r_version, "4.6.1");
    }

    #[test]
    fn select_sync_target_ignores_foreign_os_targets() {
        let this_os = this_os_family();
        let other_os = if this_os == "linux" { "macos" } else { "linux" };
        let targets = vec![
            target("4.6.1", &format!("{}-x86_64", other_os)),
            target("4.5.0", &format!("{}-x86_64", this_os)),
        ];
        let picked = select_sync_target(&targets, None, None).unwrap();
        assert_eq!(picked.r_version, "4.5.0");
    }

    #[test]
    fn select_sync_target_matches_a_source_only_target_on_any_os() {
        // A `--platform source` solve's platform is a bare CPU arch, with no
        // OS marker, so it matches this machine regardless of OS.
        let targets = vec![target("4.6.1", std::env::consts::ARCH)];
        let picked = select_sync_target(&targets, None, None).unwrap();
        assert_eq!(picked.r_version, "4.6.1");
    }

    #[test]
    fn select_sync_target_picks_the_highest_r_version_among_matches() {
        let this_os = this_os_family();
        let platform = format!("{}-x86_64", this_os);
        let targets = vec![
            target("4.5.0", &platform),
            target("4.6.1", &platform),
            target("4.4.2", &platform),
        ];
        let picked = select_sync_target(&targets, None, None).unwrap();
        assert_eq!(picked.r_version, "4.6.1");
    }

    #[test]
    fn select_sync_target_honors_an_explicit_r_version() {
        let this_os = this_os_family();
        let platform = format!("{}-x86_64", this_os);
        let targets = vec![target("4.5.0", &platform), target("4.6.1", &platform)];
        let picked = select_sync_target(&targets, Some("4.5.0"), None).unwrap();
        assert_eq!(picked.r_version, "4.5.0");
    }

    #[test]
    fn select_sync_target_errors_when_nothing_matches() {
        let other_os = if this_os_family() == "linux" {
            "macos"
        } else {
            "linux"
        };
        let targets = vec![target("4.6.1", &format!("{}-x86_64", other_os))];
        assert!(select_sync_target(&targets, None, None).is_err());
    }

    #[test]
    fn the_r_requirement_reads_as_it_does_in_the_manifest() {
        let deps = Rproj::minimal("mypkg").to_dep_version_specs(false).unwrap();
        let req = deps.dependencies.iter().find(|d| d.name == "R");
        assert_eq!(r_requirement(req), ">= 4.1");
        assert_eq!(r_requirement(None), "*");
    }

    #[test]
    fn nondev_packages_keeps_the_non_dev_closure_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Rproj::minimal("mypkg");
        manifest.dependencies.insert("cli".to_string(), dep("*"));
        manifest.dependency_groups.insert(
            "test".to_string(),
            Group {
                include_groups: vec![],
                dependencies: BTreeMap::from([("testthat".to_string(), dep("*"))]),
            },
        );
        fs::write(
            dir.path().join(RPROJ_MANIFEST_FILE),
            toml::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();

        let packages = vec![
            locked("cli", &["glue"]),
            locked("glue", &[]),
            locked("testthat", &["waldo", "glue"]),
            locked("waldo", &[]),
        ];

        let keep = nondev_packages(dir.path(), &packages).unwrap();
        // `glue` is a dev dependency too, but a non-dev one pulls it in
        assert!(keep.contains("cli"));
        assert!(keep.contains("glue"));
        assert!(!keep.contains("testthat"));
        assert!(!keep.contains("waldo"));
    }
}
