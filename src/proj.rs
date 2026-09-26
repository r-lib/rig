use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::error::Error;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::ArgMatches;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use log::{debug, error, info};
use pubgrub::{resolve, SelectedDependencies};
use rayon::prelude::*;
use simple_error::*;
use tabular::*;

use crate::args::rig_app;
use crate::built::BuiltCache;
use crate::cache::get_cache_dir;
use crate::common::{
    find_installed, get_arch, get_default_r_version, get_platform, get_r_version_data_version,
    sc_get_list_details,
};
use crate::dcf::*;
use crate::download::download_multiple_first_available_with_progress;
use crate::install::{
    install_packages, parse_linkingto, PackageInfo, REMOTE_GIT_FIELDS, REMOTE_HASH_FIELD,
    REMOTE_LINKINGTO_FIELD, REMOTE_SHA_FIELD, REMOTE_SUBDIR_FIELD, REMOTE_TYPE_FIELD,
};
use crate::library::get_library_path;
use crate::output::OUTPUT;
use crate::pkg::deps::{
    dep_count, print_deps_json, print_deps_recursive, print_header, type_list, walk_deps,
};
use crate::pkg::install::{plan_installs, print_plan};
use crate::pkg::list::{read_installed, InstalledPackage};
use crate::pkg::remove::remove_package;
use crate::pkg::tree::proj_tree;
use crate::platform::{detect_platform, parse_platform_string};
use crate::repos::binaries::loader::{BinaryTarget, P3mBinaryLoader};
use crate::repos::cranlike_metadata::{ensure_allpackages_fresh, minor_r_version};
use crate::repos::*;
use crate::resolve::resolve_versions;
use crate::rproj::{
    format_constraints, parse_add_spec, Author, DepTable, LockDirectDependency, Repository, Rproj,
    RprojLock, RprojLockPackage, RprojLockTarget, DESCRIPTION_RIG_NOTE_FIELD, RPROJ_LOCK_VERSION,
    RPROJ_MANIFEST_FILE,
};
use crate::rvenv::{
    ensure_rvenv_files, existing_targets, find_project_root, find_workspace_root,
    link_library_compat_symlink, project_library, project_library_in_tree, read_rvenv_cfg,
    rvenv_init, rvenv_sync, rvenv_sync_needed, workspace_members, write_sync_stamp, RvenvCfg,
    RPROJ_LOCK_FILE, RVENV_CFG_FILE, RVENV_DIR,
};
use crate::solver::*;
use crate::textfmt::{dcf_field_to_text, reflow};
use crate::utils::create_parent_dir_if_needed;
use toml_edit::DocumentMut;

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
        Some(("status", s)) => sc_proj_status(s, args, mainargs),
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
        check_project_conflicts(&root)?;
    }

    // The R version the manifest's R requirement is derived from. It does not
    // have to be installed, nothing we write here refers to an R installation.
    let rver = resolve_project_r_version(args)?;

    // Project name defaults to the current directory's name.
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "myproject".to_string());

    let manifest = Rproj::minimal_for_r(&name, &rver)?;
    let manifest_path = root.join(RPROJ_MANIFEST_FILE);
    fs::write(&manifest_path, manifest.to_toml()?)?;

    let mut created = vec![manifest_path];
    created.extend(rvenv_init(&root)?);

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

/// Fail if any of the project files we are about to write is already there,
/// naming all of them at once, so that a conflict does not leave a
/// half-created project behind.
pub fn check_project_conflicts(root: &Path) -> Result<(), Box<dyn Error>> {
    let existing = existing_targets(root)?;
    if existing.is_empty() {
        return Ok(());
    }
    let names: Vec<String> = existing
        .iter()
        .map(|p| {
            p.strip_prefix(root)
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

/// Write the tracked part of the `.rvenv` layout for a freshly written
/// manifest, and report every file the project now has.
pub fn init_rvenv_for_manifest(
    args: &ArgMatches,
    root: &Path,
    manifest_path: &Path,
) -> Result<(), Box<dyn Error>> {
    let rver = resolve_project_r_version(args)?;
    let mut created = vec![manifest_path.to_path_buf()];
    created.extend(rvenv_init(root)?);
    for path in &created {
        let name = path.strip_prefix(root).unwrap_or(path).to_string_lossy();
        OUTPUT.success(&format!("Created {}", name));
        info!("Created {}", name);
    }
    OUTPUT.info(&format!(
        "Project set up for R {}. Next: run `rig proj lock` and `rig proj sync`.",
        rver
    ));
    Ok(())
}

/// The R version a new project is set up for: an explicit `--r-version`, else
/// the default R version, else the current R release.
///
/// Deliberately *not* the R version the project itself declares a minimum of:
/// what the project is set up for should be the R it will be locked and synced
/// against, see [`proj_lock_r_version`]. Nothing in the committed `.rvenv`
/// layout is tied to an R version.
fn resolve_project_r_version(args: &ArgMatches) -> Result<String, Box<dyn Error>> {
    if let Some(rv) = args.get_one::<String>("r-version") {
        return Ok(rv.to_string());
    }
    if let Some(rv) = get_default_r_version()? {
        return Ok(rv);
    }
    // No R installed (or no default set), so fall back to the current
    // release, which needs the network.
    match resolve_release_r_version(args) {
        Some(rv) => {
            OUTPUT.info(&format!(
                "No default R version, using the current release (R {}).",
                rv
            ));
            info!("No default R version, using the current release (R {})", rv);
            Ok(rv)
        }
        None => {
            let msg = "Cannot determine R version. Install R with `rig add`, \
                       or set the version with --r-version.";
            OUTPUT.error(msg);
            error!("{}", msg);
            bail!("{}", msg)
        }
    }
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
const KNOWN_DESCRIPTION_FIELDS: [&str; 16] = [
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
    "Remotes",
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
    let root = std::env::current_dir()?;
    let path = root.join(RPROJ_MANIFEST_FILE);
    let path = path.as_path();

    if !dependencies_only && !args.get_flag("force") && path.exists() {
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

    // A full import also creates the `.rvenv` layout, so check for conflicts
    // before writing anything. `--dependencies` only touches the manifest.
    if !dependencies_only && !args.get_flag("force") {
        check_project_conflicts(&root)?;
    }

    let paragraph = read_description_paragraph(input, false)?;
    let pkg = Package::from_dcf_paragraph(&paragraph)?;
    let dep_count = pkg.dependencies.dependencies.len();

    let existing_text = if dependencies_only && path.exists() {
        Some(fs::read_to_string(path)?)
    } else {
        None
    };

    let mut manifest = match &existing_text {
        Some(text) => toml::from_str::<Rproj>(text)?,
        None if dependencies_only => {
            OUTPUT.status(&format!(
                "{} does not exist, creating a new one",
                RPROJ_MANIFEST_FILE
            ));
            info!("{} does not exist, creating a new one", RPROJ_MANIFEST_FILE);
            Rproj::minimal(&pkg.name)
        }
        None => Rproj::minimal(&pkg.name),
    };

    // `--dependencies` merges into an EXISTING manifest, so every
    // dependency/config edit below is also mirrored onto the ORIGINAL
    // document (parsed from `existing_text`, not regenerated from
    // `manifest`), so that comments, blank-line grouping, and unmodeled
    // tables/keys survive the merge. A full import has no prior file to
    // preserve. The "before" snapshots let the diff after merging apply only
    // what actually changed, without having to intercept every individual
    // `merge_description`/`merge_config_needs`/`merge_config`/`Remotes` call.
    let mut original_doc: Option<toml_edit::DocumentMut> = None;
    let mut before_dependencies = BTreeMap::new();
    let mut before_linking = BTreeMap::new();
    let mut before_groups = BTreeMap::new();
    let mut before_optional = BTreeMap::new();
    let mut before_config = BTreeMap::new();
    if let Some(text) = existing_text.as_deref() {
        original_doc = Some(text.parse()?);
        before_dependencies = manifest.dependencies.clone();
        before_linking = manifest.linking_dependencies.clone();
        before_groups = manifest.dependency_groups.clone();
        before_optional = manifest.optional_dependencies.clone();
        before_config = manifest.config.clone();
    }

    if !dependencies_only {
        manifest.project.version = pkg.version.to_string();
        manifest.project.type_ = Some(
            paragraph
                .get("Type")
                .map(|t| t.to_lowercase())
                .unwrap_or_else(|| "package".to_string()),
        );
        manifest.project.title = paragraph.get("Title").map(reflow);
        manifest.project.description = paragraph.get("Description").map(dcf_field_to_text);
        manifest.project.license = paragraph.get("License").map(reflow);
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
    // `Remotes:` names the git/GitHub/GitLab/url source for packages that are
    // also listed in `Depends`/`Imports`/`Suggests` above; only `git`/
    // `github`/`gitlab`/`url` remotes are understood, other remote types
    // (`bioc::`, `bitbucket::`, `local::`, `svn::`, ...) are warned about and
    // skipped rather than failing the whole import.
    if let Some(remotes) = paragraph.get("Remotes") {
        for entry in reflow(remotes).split(',') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            match crate::pkgsource::parse_pkg_source(entry) {
                Ok(crate::pkgsource::PkgSource::Remote(r)) => {
                    match crate::rproj::pak_ref_name(entry) {
                        Some(name) => {
                            let dev = manifest
                                .dependency_groups
                                .get("dev")
                                .is_some_and(|g| g.dependencies.contains_key(&name))
                                && !manifest.dependencies.contains_key(&name);
                            let table = dep_table_from_remote(&r, entry);
                            manifest.add_remote_dependency(&name, table, dev);
                        }
                        None => {
                            let msg = format!(
                                "Cannot determine the package name for Remotes entry \
                                 `{}`, skipping it",
                                entry
                            );
                            OUTPUT.warn(&msg);
                            info!("{}", msg);
                        }
                    }
                }
                // A `url::` reference's own `pak_ref_name` heuristic misreads
                // a versioned archive file name, so unlike a git reference,
                // only an explicit `<name>=` override is usable here.
                Ok(crate::pkgsource::PkgSource::Url(u)) => match &u.name_override {
                    Some(name) => {
                        let dev = manifest
                            .dependency_groups
                            .get("dev")
                            .is_some_and(|g| g.dependencies.contains_key(name))
                            && !manifest.dependencies.contains_key(name);
                        let table = dep_table_from_url(&u);
                        manifest.add_remote_dependency(name, table, dev);
                    }
                    None => {
                        let msg = format!(
                            "Remotes entry `{}` has no `<name>=` override, cannot tell \
                             which package it names, skipping it",
                            entry
                        );
                        OUTPUT.warn(&msg);
                        info!("{}", msg);
                    }
                },
                Ok(crate::pkgsource::PkgSource::Cran)
                | Ok(crate::pkgsource::PkgSource::Local(_))
                | Err(_) => {
                    let msg = format!(
                        "Remotes entry `{}` is not a supported git/GitHub/url reference, \
                         skipping it",
                        entry
                    );
                    OUTPUT.warn(&msg);
                    info!("{}", msg);
                }
            }
        }
    }
    // `Config/Needs/*` fields are dependencies as well, so they are imported
    // in `--dependencies` mode, too. `Config/Needs/Optional/<name>` is its own
    // sub-namespace -- it maps to `[optional-dependencies.<name>]`, not
    // `[dependency-groups.<name>]` -- so it's excluded here and collected
    // separately below.
    let needs: Vec<(String, String)> = paragraph
        .iter()
        .filter_map(|(key, value)| {
            let group = key.strip_prefix("Config/Needs/")?;
            if group.starts_with("Optional/") {
                return None;
            }
            Some((group.to_string(), reflow(value)))
        })
        .collect();
    manifest.merge_config_needs(&needs);
    let optional_needs: Vec<(String, String)> = paragraph
        .iter()
        .filter_map(|(key, value)| {
            key.strip_prefix("Config/Needs/Optional/")
                .map(|group| (group.to_string(), reflow(value)))
        })
        .collect();
    manifest.merge_optional_dependencies(&optional_needs);
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

    if let Some(doc) = original_doc.as_mut() {
        for (name, dep) in manifest.dependencies.iter() {
            if before_dependencies.get(name) != Some(dep) {
                Rproj::doc_set_dependency(doc, &["dependencies"], name, dep)?;
            }
        }
        for (name, dep) in manifest.linking_dependencies.iter() {
            if before_linking.get(name) != Some(dep) {
                Rproj::doc_set_dependency(doc, &["linking-dependencies"], name, dep)?;
            }
        }
        for (group_name, group) in manifest.dependency_groups.iter() {
            let before_group = before_groups.get(group_name);
            for (name, dep) in group.dependencies.iter() {
                if before_group.and_then(|g| g.dependencies.get(name)) != Some(dep) {
                    Rproj::doc_set_dependency(doc, &["dependency-groups", group_name], name, dep)?;
                }
            }
        }
        for (group_name, extra) in manifest.optional_dependencies.iter() {
            let before_extra = before_optional.get(group_name);
            for (name, dep) in extra.iter() {
                if before_extra.and_then(|g| g.get(name)) != Some(dep) {
                    Rproj::doc_set_dependency(
                        doc,
                        &["optional-dependencies", group_name],
                        name,
                        dep,
                    )?;
                }
            }
        }
        for (group_name, table) in manifest.config.iter() {
            let before_table = before_config.get(group_name);
            for (key, value) in table.iter() {
                if before_table.and_then(|t| t.get(key)) != Some(value) {
                    Rproj::doc_set_config(doc, group_name, key, value);
                }
            }
        }
    }

    match original_doc {
        Some(doc) => fs::write(path, doc.to_string())?,
        None => fs::write(path, manifest.to_toml()?)?,
    }

    let groups_count = needs.len() + optional_needs.len();
    let groups = match groups_count {
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

    // A full import sets up a whole project, not just its manifest, so it
    // creates the same `.rvenv` layout as `rig proj init`.
    if !dependencies_only {
        init_rvenv_for_manifest(args, &root, path)?;
    }

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

    if path.exists() && !force && !description_is_rig_generated(path) {
        let msg = format!("{} already exists, use --force to overwrite", output);
        OUTPUT.error(&msg);
        error!("{}", msg);
        bail!("{}", msg);
    }

    let cwd = std::env::current_dir()?;
    let root = find_project_root(&cwd).unwrap_or(cwd);
    let manifest = proj_read_manifest(&root)?;

    write_description_to(path, &manifest)?;

    let msg = format!("Exported {} to {}", RPROJ_MANIFEST_FILE, output);
    OUTPUT.success(&msg);
    info!("{}", msg);
    Ok(())
}

/// Whether `path` looks like a `DESCRIPTION` rig itself generated, i.e. it
/// carries the [`DESCRIPTION_RIG_NOTE_FIELD`] rig writes in
/// [`Rproj::to_description`]. `rig proj export` uses this to overwrite such
/// a file without requiring `--force`. Any read/parse failure is treated as
/// "not rig-generated" rather than an error, since the caller falls back to
/// the normal existing-file check.
fn description_is_rig_generated(path: &Path) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    let Ok(paragraph) = parse_description_paragraph(file) else {
        return false;
    };
    paragraph.get(DESCRIPTION_RIG_NOTE_FIELD).is_some()
}

/// Render `manifest.to_description()` and write it to `path`, warning about
/// any dropped upper version bound the same way either caller needs it: the
/// explicit `rig proj export`, and `rig proj sync`'s automatic (re)write of
/// the project's own `DESCRIPTION` before installing it, see
/// [`ProjSyncOptions::install_project`].
fn write_description_to(path: &Path, manifest: &Rproj) -> Result<(), Box<dyn Error>> {
    let (description, dropped) = manifest.to_description()?;
    fs::write(path, description)?;

    if !dropped.is_empty() {
        OUTPUT.warn(&format!(
            "Dropped the upper version bound for {}, DESCRIPTION only supports \
             a single version comparison per dependency",
            dropped.join(", ")
        ));
    }
    Ok(())
}

/// One parsed `rig proj add` argument: an ordinary CRAN-style `<package>`/
/// `<package>@<version>`, or a git/GitHub reference, already fetched to learn
/// its real package name (from `DESCRIPTION`'s `Package:` field, which may
/// differ from the repository name) and pinned commit.
enum AddSpec {
    Cran(String, String),
    Remote(String, Box<DepTable>),
}

impl AddSpec {
    fn name(&self) -> &str {
        match self {
            AddSpec::Cran(name, _) => name,
            AddSpec::Remote(name, _) => name,
        }
    }
}

/// Parse one `rig proj add` argument. A git/GitHub reference is fetched here,
/// so that its real package name and pinned commit are known before anything
/// is written to `rproj.toml` -- see [`fetch_and_read_git_package`]. `root`
/// is the project's own directory, against which a local path is made
/// relative -- see [`relativize_to_root`].
fn parse_add_arg(spec: &str, root: &Path) -> Result<AddSpec, Box<dyn Error>> {
    match crate::pkgsource::parse_pkg_source(spec)? {
        crate::pkgsource::PkgSource::Cran => {
            let (name, version) = parse_add_spec(spec)?;
            Ok(AddSpec::Cran(name, version))
        }
        crate::pkgsource::PkgSource::Remote(r) => {
            let table = dep_table_from_remote(&r, spec);
            let git_url = table.git.clone().unwrap_or_default();
            OUTPUT.status(&format!("Fetching {}", git_url));
            let (pkg, _source, _remotes) =
                fetch_and_read_git_package(&git_url, &table, &HashMap::new(), &HashMap::new())?;
            let name = r.name_override.unwrap_or(pkg.name);
            Ok(AddSpec::Remote(name, Box::new(table)))
        }
        crate::pkgsource::PkgSource::Url(u) => {
            let table = dep_table_from_url(&u);
            OUTPUT.status(&format!("Fetching {}", u.url));
            // The table written to `rproj.toml` keeps `subdir` as the user
            // wrote it (usually unset) -- an archive's auto-detected
            // top-level wrapper directory is resolved provenance, not
            // manifest input, so it only ever goes into the lockfile's
            // `RemoteSubdir`, via `GitSourceInfo` (see
            // `fetch_and_read_url_package`).
            let (pkg, _url_source, _remotes) = fetch_and_read_url_package(&u.url, &table)?;
            let name = u.name_override.unwrap_or(pkg.name);
            Ok(AddSpec::Remote(name, Box::new(table)))
        }
        // A local path is resolved against `root` and stored relative to it
        // (see `relativize_to_root`), so the dependency still means the same
        // thing after the project is moved or checked out elsewhere, as long
        // as the local package stays at the same relative location.
        crate::pkgsource::PkgSource::Local(l) => {
            let resolved = crate::pkgsource::local::resolve_local_path(&l.path)?;
            let (pkg, _source, _remotes) = read_local_package(&resolved)?;
            let name = l.name_override.unwrap_or(pkg.name);
            let relative = relativize_to_root(root, &resolved)?;
            let table = DepTable {
                path: Some(relative),
                ..Default::default()
            };
            Ok(AddSpec::Remote(name, Box::new(table)))
        }
    }
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

    // Every edit below is also mirrored onto the ORIGINAL document (parsed
    // from `original`, not regenerated from `manifest`), so that a write
    // through this path preserves comments, blank-line grouping, and any
    // table/key the `Rproj` schema doesn't model. There is no original text
    // to preserve when the manifest file doesn't exist yet, but
    // `proj_read_manifest` above already requires it to, so this is
    // defensive, not expected to be `None` in practice.
    let mut original_doc: Option<DocumentMut> = original.as_deref().map(str::parse).transpose()?;

    // Parse (and, for a git/GitHub reference, fetch) every specification
    // before changing anything, so that a typo -- or an unreachable repo --
    // in the last one does not leave the earlier ones added.
    let mut specs: Vec<AddSpec> = Vec::new();
    for spec in args.get_many::<String>("package").unwrap_or_default() {
        specs.push(parse_add_arg(spec, &root).map_err(|err| {
            OUTPUT.error(&err.to_string());
            error!("{}", err);
            err
        })?);
    }

    let mut messages: Vec<String> = Vec::new();
    for spec in specs.iter() {
        // A dev dependency of a package the project already depends on
        // directly is installed either way, so `--dev` does not do what it
        // looks like it does there.
        let name = spec.name();
        if dev && manifest.dependencies.contains_key(name) {
            OUTPUT.warn(&format!(
                "{} is already a dependency in [dependencies], \
                 it stays a hard dependency",
                name
            ));
        }

        messages.push(match spec {
            AddSpec::Cran(name, version) => {
                let previous = manifest.add_dependency(name, version, dev);
                match previous {
                    Some(previous) if previous == *version => {
                        format!("Kept {} ({}) in {}", name, version, RPROJ_MANIFEST_FILE)
                    }
                    Some(previous) => format!(
                        "Updated {} in {}, {} -> {}",
                        name, RPROJ_MANIFEST_FILE, previous, version
                    ),
                    None => format!("Added {} ({}) to {}", name, version, RPROJ_MANIFEST_FILE),
                }
            }
            AddSpec::Remote(name, table) => {
                manifest.add_remote_dependency(name, (**table).clone(), dev);
                let source = table
                    .git
                    .as_deref()
                    .or(table.url.as_deref())
                    .or(table.path.as_deref())
                    .unwrap_or_default();
                format!("Added {} ({}) to {}", name, source, RPROJ_MANIFEST_FILE)
            }
        });

        if let Some(doc) = original_doc.as_mut() {
            let path: &[&str] = if dev {
                &["dependency-groups", "dev"]
            } else {
                &["dependencies"]
            };
            let value = if dev {
                manifest
                    .dependency_groups
                    .get("dev")
                    .and_then(|group| group.dependencies.get(name))
            } else {
                manifest.dependencies.get(name)
            }
            .expect("just inserted by add_dependency/add_remote_dependency above");
            Rproj::doc_set_dependency(doc, path, name, value)?;
        }
    }

    match original_doc {
        Some(doc) => fs::write(&path, doc.to_string())?,
        None => fs::write(&path, manifest.to_toml()?)?,
    }
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

    // Mirrored onto the ORIGINAL document below, same reasoning as
    // `sc_proj_add`.
    let mut original_doc: Option<DocumentMut> = original.as_deref().map(str::parse).transpose()?;

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
        if let Some(doc) = original_doc.as_mut() {
            Rproj::doc_remove_dependency(doc, name);
        }
        messages.push(format!("Removed {} from {}", name, RPROJ_MANIFEST_FILE));
    }

    match original_doc {
        Some(doc) => fs::write(&path, doc.to_string())?,
        None => fs::write(&path, manifest.to_toml()?)?,
    }
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
fn read_description_paragraph(
    input: &str,
    quiet: bool,
) -> Result<deb822_fast::Paragraph, Box<dyn Error>> {
    if !quiet {
        OUTPUT.status(&format!("Reading dependencies from {}", input));
    }
    info!("Reading dependencies from {}", input);
    let df: File = File::open(input).map_err(|e| {
        OUTPUT.error(&format!("Cannot read {}: {}", input, e));
        error!("Cannot read {}: {}", input, e);
        e
    })?;
    parse_description_paragraph(df)
}

/// Parse a single-paragraph DCF document (e.g. a `DESCRIPTION` file's
/// content) from any [`std::io::Read`], the shared body behind
/// [`read_description_paragraph`] (path-based) and a git/GitHub dependency's
/// fetched content (in-memory, via [`fetch_and_read_git_package`]).
pub(crate) fn parse_description_paragraph<R: std::io::Read>(
    reader: R,
) -> Result<deb822_fast::Paragraph, Box<dyn Error>> {
    let desc = parse_dcf_reader(reader)?;

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

/// [`proj_read_manifest_deps`], plus the manifest's git/GitHub/GitLab-sourced
/// dependencies (see [`crate::rproj::Rproj::git_dependencies`]), for
/// `rig proj tree`, which needs them to resolve a remote package met while
/// walking the tree the same way [`register_git_sources`] does for a solve.
#[allow(clippy::type_complexity)]
fn proj_read_manifest_deps_with_remotes(
    root: &Path,
    dev: bool,
) -> Result<
    (
        String,
        RPackageVersion,
        PackageDependencies,
        Vec<(String, DepTable)>,
    ),
    Box<dyn Error>,
> {
    OUTPUT.status(&format!(
        "Reading dependencies from {}",
        RPROJ_MANIFEST_FILE
    ));
    info!("Reading dependencies from {}", RPROJ_MANIFEST_FILE);
    let manifest = proj_read_manifest(root)?;

    let deps = manifest.to_dep_version_specs(dev)?;
    let version = RPackageVersion::from_str(&manifest.project.version)?;
    let git_deps = manifest.git_dependencies(root);
    Ok((manifest.project.name, version, deps, git_deps))
}

/// What one solve is rooted at: a plain project, or every member of a
/// workspace.
#[derive(Debug)]
pub(crate) struct ProjectSolve {
    /// The member directories, the workspace root first. A plain project has
    /// exactly one entry, its own directory.
    pub members: Vec<PathBuf>,
    /// The solver roots, in the same order as `members`.
    pub roots: Vec<SolveRoot>,
    /// For a plain (non-workspace) project whose own type is "package": a
    /// root, under the project's own real name and version, so a dependency
    /// on that name resolves to the project itself instead of CRAN/PPM. See
    /// [`register_roots`]. Workspace members already get this via `roots`,
    /// under their own real names, so this is always `None` for a workspace.
    pub self_alias: Option<SolveRoot>,
    /// Every root's dependencies in one set, for the decisions taken once for
    /// the whole solve: which R version to solve for, and which packages the
    /// non-dev subset of the lockfile needs.
    pub merged: PackageDependencies,
    /// Every member's dependency groups, merged by name: `"main"` for the
    /// hard dependencies, plus every `[dependency-groups.*]` name
    /// (`include-groups` resolved), each mapped to its effective dependency
    /// names. Used after the solve to tag each locked package with the
    /// group(s) that need it. Kept separate from [`Self::extra_roots`] so
    /// `rig proj sync`'s `--group`/`--all-groups` and `--extra`/`--all-extras`
    /// mean different things.
    pub group_roots: HashMap<String, Vec<String>>,
    /// Every member's `[optional-dependencies.*]` extras, merged by name,
    /// each mapped to its direct dependency names. See [`Self::group_roots`].
    pub extra_roots: HashMap<String, Vec<String>>,
    /// Every member's git/GitHub-sourced dependencies, merged, see
    /// [`Rproj::git_dependencies`]. Fetched and registered with the solver by
    /// [`register_git_sources`] before it runs.
    pub git_deps: Vec<(String, crate::rproj::DepTable)>,
}

/// Read the project or workspace rooted at `root` and turn it into the roots
/// of one solve.
///
/// `root` is a workspace root if its manifest has a `[workspace]` with
/// members, in which case every member is read, its `{ workspace = true }`
/// dependencies are resolved against the root's `[workspace.dependencies]`
/// (see [`Rproj::inherit_workspace_deps`]), and each becomes a root of the
/// solve under its own name and version. Otherwise this is one plain project,
/// and the single root is the synthetic one the solver has always used.
pub(crate) fn proj_read_solve_roots(root: &Path) -> Result<ProjectSolve, Box<dyn Error>> {
    let manifest = proj_read_manifest(root)?;
    let ws = match &manifest.workspace {
        Some(ws) if !ws.members.is_empty() => ws,
        _ => {
            OUTPUT.status(&format!(
                "Reading dependencies from {}",
                RPROJ_MANIFEST_FILE
            ));
            info!("Reading dependencies from {}", RPROJ_MANIFEST_FILE);
            let deps = manifest.to_dep_version_specs(true)?;
            let group_roots = manifest.main_and_group_roots()?;
            let extra_roots = manifest.optional_dependency_roots();
            let git_deps = manifest.git_dependencies(root);
            // Same name/base-package restriction as a workspace member (see
            // below): it would be nonsense for the project's own name to
            // shadow R or a base package in the registry.
            let name = &manifest.project.name;
            let self_alias = if manifest.project.is_package()
                && name != "R"
                && !BASE_PKGS.contains(&name.as_str())
            {
                Some(SolveRoot {
                    name: name.clone(),
                    version: RPackageVersion::from_str(&manifest.project.version)?,
                    deps: deps.clone(),
                })
            } else {
                None
            };
            return Ok(ProjectSolve {
                members: vec![root.to_path_buf()],
                roots: vec![SolveRoot::project(deps.clone())?],
                self_alias,
                merged: deps,
                group_roots,
                extra_roots,
                git_deps,
            });
        }
    };

    let dirs = workspace_members(root, ws)?;
    OUTPUT.status(&format!(
        "Reading dependencies from {} workspace {}",
        dirs.len(),
        if dirs.len() == 1 { "member" } else { "members" }
    ));
    info!("Reading dependencies from {} workspace members", dirs.len());

    let mut roots: Vec<SolveRoot> = vec![];
    let mut merged = PackageDependencies {
        dependencies: vec![],
    };
    let mut group_roots: HashMap<String, Vec<String>> = HashMap::new();
    let mut extra_roots: HashMap<String, Vec<String>> = HashMap::new();
    let mut seen: HashMap<String, PathBuf> = HashMap::new();
    let mut git_deps: Vec<(String, crate::rproj::DepTable)> = vec![];

    for dir in &dirs {
        let mut member = proj_read_manifest(dir)?;
        member.inherit_workspace_deps(ws, &dir.join(RPROJ_MANIFEST_FILE))?;
        let name = member.project.name.clone();
        git_deps.extend(member.git_dependencies(dir));

        // The solver equates R and the base packages with the R version
        // itself, so a member of one of those names would be resolved against
        // R's version rather than its own.
        if name == "R" || BASE_PKGS.contains(&name.as_str()) {
            bail!(
                "Workspace member {} is called `{}`, which is R itself or one \
                 of the packages that come with it",
                dir.display(),
                name
            );
        }
        if let Some(previous) = seen.insert(name.clone(), dir.to_path_buf()) {
            bail!(
                "Two workspace members are called `{}`: {} and {}",
                name,
                previous.display(),
                dir.display()
            );
        }

        let deps = member.to_dep_version_specs(true)?;
        merged.append(&mut deps.clone());
        for (group_name, names) in member.main_and_group_roots()? {
            group_roots.entry(group_name).or_default().extend(names);
        }
        for (extra_name, names) in member.optional_dependency_roots() {
            extra_roots.entry(extra_name).or_default().extend(names);
        }
        roots.push(SolveRoot {
            name,
            version: RPackageVersion::from_str(&member.project.version)?,
            deps,
        });
    }

    // Every member's requirement on the same package, in one entry: this is
    // what makes the merged `R` requirement the intersection of the members'.
    merged.simplify();

    Ok(ProjectSolve {
        members: dirs,
        roots,
        self_alias: None,
        merged,
        group_roots,
        extra_roots,
        git_deps,
    })
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
    let (name, version, pkg_deps, git_deps) =
        proj_read_manifest_deps_with_remotes(Path::new("."), dev)?;

    proj_tree(
        &name,
        &version,
        &pkg_deps.dependencies,
        git_deps.into_iter().collect(),
        dev,
        no_base,
        why,
        json,
    )
}

/// Turns a symbolic, installed R name (e.g. `"devel"`, `"next"`) into the
/// numeric R version P3M's binary repo paths need. Names that already look
/// like a version are passed through unchanged.
fn resolve_binary_target_r_version(r_version: &str) -> Result<String, Box<dyn Error>> {
    if r_version.starts_with(|c: char| c.is_ascii_digit()) {
        return Ok(r_version.to_string());
    }
    match find_installed(r_version)? {
        Some(name) => get_r_version_data_version(&name),
        None => Ok(r_version.to_string()),
    }
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

    let r_version = &resolve_binary_target_r_version(r_version)?;

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

/// Solve the dependencies of one project for one R version and one binary
/// target, i.e. [`sc_proj_solve_deps`] for the callers that have a single
/// manifest and no workspace.
pub(crate) fn sc_proj_solve_project_deps(
    r_version: &str,
    deps: &PackageDependencies,
    target: Option<BinaryTarget>,
    prefer_binary: Option<usize>,
    report_status: bool,
) -> Result<(RPackageRegistry, SelectedDependencies<RPackageRegistry>), Box<dyn Error>> {
    let roots = [SolveRoot::project(deps.clone())?];
    sc_proj_solve_deps(
        r_version,
        &roots,
        None,
        &[],
        target,
        prefer_binary,
        report_status,
    )
}

/// Solve the dependencies of every root in `roots` for one R version and one
/// binary target, in one solve, so that all of them end up satisfied by one
/// version of each package. A plain project has a single root; a workspace has
/// one per member (see [`register_roots`]).
///
/// `report_status` is for callers that solve several targets in parallel
/// ([`proj_lock`]): they print the status lines once for the whole batch, so
/// the per-solve lines here would only be N interleaved, unlabelled copies of
/// them. Single-target callers pass `true` and get the progress reported as
/// each phase starts. The log file gets the messages either way.
pub(crate) fn sc_proj_solve_deps(
    r_version: &str,
    roots: &[SolveRoot],
    self_alias: Option<&SolveRoot>,
    git_sources: &[ResolvedGitSource],
    target: Option<BinaryTarget>,
    prefer_binary: Option<usize>,
    report_status: bool,
) -> Result<(RPackageRegistry, SelectedDependencies<RPackageRegistry>), Box<dyn Error>> {
    info!("Solving dependencies");

    // `r_version` may be a symbolic, installed R name (e.g. "devel", "next")
    // rather than a numeric version: resolve it before it reaches package
    // version comparisons, which expect numbers.
    let r_version = &resolve_binary_target_r_version(r_version)?;

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

    let (root_pkg, root_version) = register_roots(&reg, roots, self_alias)?;

    if !git_sources.is_empty() {
        if report_status {
            OUTPUT.status("Registering git/GitHub/URL package sources");
        }
        info!("Registering git/GitHub/URL package sources");
        register_git_sources(&reg, git_sources);
    }

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
    // A root is a local directory, or synthetic, so there is no binary index
    // to fetch for it, however many members depend on it. Same for a
    // git/URL/path-sourced package (`git_sources`, despite the name, covers
    // all three -- see `resolve_git_sources`): its version is already
    // resolved, so there is no repo binary index to fetch.
    let git_names: HashSet<&str> = git_sources.iter().map(|s| s.name.as_str()).collect();
    let mut direct: Vec<String> = roots
        .iter()
        .flat_map(|root| root.deps.dependencies.iter().map(|d| d.name.clone()))
        .filter(|name| !reg.is_local(name) && !git_names.contains(name.as_str()))
        .collect();
    direct.sort();
    direct.dedup();
    reg.prefetch_binaries(&direct);

    if report_status {
        OUTPUT.status("Solving dependencies");
    }
    let solution = resolve(&reg, root_pkg, root_version);

    match solution {
        Ok(sol) => Ok((reg, sol)),
        Err(e) => {
            // The target goes into the message here, where the platform is
            // known, so that a batch of parallel solves can report the failure
            // that aborts the command without having to append the target to
            // the end of a multi-line report.
            let msg = format!(
                "Cannot resolve dependencies for R {} / {}:\n{}",
                r_version,
                solve_platform_key(reg.binary_target()),
                format_solver_error(e)
            );
            // Parallel callers print the one failure that aborts the command
            // themselves; N copies of the same report would say less, not more.
            if report_status {
                report_solve_failure(&msg);
            }
            error!("{}", msg);
            bail!("{}", msg)
        }
    }
}

/// The identity of a git/GitHub dependency's *request* -- its URL, resolved
/// refspec (`None` meaning "just take the default branch tip"), and
/// subdirectory -- but not the commit it resolves to. Two `rig proj lock`
/// runs producing the same key for the same dependency are asking git the
/// same question, so [`existing_git_shas`] lets the second one skip asking
/// again. Deliberately excludes a `release = true` dependency, whose
/// `refspec` (the release tag) is only known after asking GitHub which
/// release is latest -- exactly the remote round trip being skipped, so it
/// can't be part of the key up front. [`existing_release_refs`] gives that
/// case its own sticky mechanism, keyed on URL and subdirectory alone.
type GitSourceKey = (String, Option<String>, Option<String>);

/// Read and parse the `rproj.lock` at `root`, if there is one and it's the
/// current lock format -- `None` for a first `rig proj lock`, or one
/// recovering from a corrupt or outdated lockfile, in which case every
/// target always resolves fresh, same as before any sticky-lock behavior
/// existed. Shared by [`existing_git_shas`] and [`existing_lock_satisfies`]
/// so a `proj_lock` run parses the file once instead of twice.
fn read_existing_lock(root: &Path) -> Option<RprojLock> {
    let text = fs::read_to_string(root.join(RPROJ_LOCK_FILE)).ok()?;
    RprojLock::check_version(&text).ok()?;
    toml::from_str::<RprojLock>(&text).ok()
}

/// Every git/GitHub dependency's previously resolved commit, read from an
/// already-parsed `rproj.lock`, keyed by [`GitSourceKey`].
///
/// This is what lets an ordinary `rig proj lock` run reuse a pinned commit
/// without contacting the dependency's remote at all when its request is
/// unchanged, instead of re-fetching a possibly-moved branch/tag/PR every
/// time -- the same "a lockfile is sticky until you ask to upgrade" behavior
/// `Cargo.lock`/`uv.lock` have (see the `--upgrade` flag, which skips calling
/// this instead, forcing every git dependency to resolve fresh).
fn existing_git_shas(lock: &RprojLock) -> HashMap<GitSourceKey, String> {
    let mut map = HashMap::new();
    for target in &lock.targets {
        for pkg in &target.packages {
            if !pkg.metadata.contains_key(REMOTE_TYPE_FIELD) {
                continue;
            }
            let url = pkg.metadata.get(crate::install::REMOTE_URL_FIELD);
            let sha = pkg.metadata.get(crate::install::REMOTE_SHA_FIELD);
            let (Some(url), Some(sha)) = (url, sha) else {
                continue;
            };
            let refspec = pkg.metadata.get(crate::install::REMOTE_REF_FIELD).cloned();
            let subdir = pkg.metadata.get(REMOTE_SUBDIR_FIELD).cloned();
            map.insert((url.clone(), refspec, subdir), sha.clone());
        }
    }
    map
}

/// Every `release = true` dependency's previously resolved tag and commit,
/// read from an already-parsed `rproj.lock`, keyed by (URL, subdirectory) --
/// unlike [`existing_git_shas`]'s [`GitSourceKey`], not by refspec, since a
/// release dependency's refspec (the release tag) is only known after
/// resolving "whatever is latest" against the remote, which is exactly what
/// this lets an ordinary `rig proj lock` run skip: reuse the previously
/// resolved tag as-is instead of asking GitHub which release is latest every
/// time. `--upgrade` skips calling this, same as [`existing_git_shas`], so a
/// release dependency always re-resolves to whatever is actually latest.
fn existing_release_refs(lock: &RprojLock) -> HashMap<(String, Option<String>), (String, String)> {
    let mut map = HashMap::new();
    for target in &lock.targets {
        for pkg in &target.packages {
            if !pkg.metadata.contains_key(REMOTE_TYPE_FIELD) {
                continue;
            }
            let url = pkg.metadata.get(crate::install::REMOTE_URL_FIELD);
            let sha = pkg.metadata.get(crate::install::REMOTE_SHA_FIELD);
            let refspec = pkg.metadata.get(crate::install::REMOTE_REF_FIELD);
            let (Some(url), Some(sha), Some(refspec)) = (url, sha, refspec) else {
                continue;
            };
            let subdir = pkg.metadata.get(REMOTE_SUBDIR_FIELD).cloned();
            map.insert((url.clone(), subdir), (refspec.clone(), sha.clone()));
        }
    }
    map
}

/// Whether an existing lock `target` still satisfies the manifest's current
/// direct dependencies, without solving anything: every name in
/// `direct_deps` must be pinned in `target`, at a version satisfying its
/// requirement (skipped for a git/GitHub-sourced entry -- those aren't
/// versioned by the manifest, just named), and `target.direct_dependencies`
/// -- the fingerprint recorded the last time this target was actually
/// solved -- must name exactly the same set of packages, so an added or
/// removed manifest dependency always forces a real solve.
fn lock_target_satisfies(target: &RprojLockTarget, direct_deps: &[DepVersionSpec]) -> bool {
    let fingerprint: HashSet<&str> = target
        .direct_dependencies
        .iter()
        .map(|d| d.name.as_str())
        .collect();
    let wanted: HashSet<&str> = direct_deps.iter().map(|d| d.name.as_str()).collect();
    if fingerprint != wanted {
        return false;
    }

    let packages: HashMap<&str, &RprojLockPackage> = target
        .packages
        .iter()
        .map(|p| (p.package.as_str(), p))
        .collect();

    for dep in direct_deps {
        let Some(pkg) = packages.get(dep.name.as_str()) else {
            return false;
        };
        if pkg.metadata.contains_key(REMOTE_TYPE_FIELD) {
            continue;
        }
        match dep.satisfies(&pkg.version) {
            Ok(true) => {}
            _ => return false,
        }
    }
    true
}

/// Whether every git/GitHub-sourced dependency `resolve_git_sources` just
/// resolved for this target matches what the candidate lock `target` already
/// has pinned -- same URL, ref and commit. A mismatch (a moved branch/PR, a
/// changed `git =`/`ref =`/subdir, or a brand-new git dependency) means the
/// target's git portion is stale and it needs a real solve, even if
/// [`lock_target_satisfies`] passed for its CRAN/PPM dependencies.
fn lock_target_git_sources_fresh(
    target: &RprojLockTarget,
    git_sources: &[ResolvedGitSource],
) -> bool {
    let packages: HashMap<&str, &RprojLockPackage> = target
        .packages
        .iter()
        .map(|p| (p.package.as_str(), p))
        .collect();

    for source in git_sources {
        let Some(pkg) = packages.get(source.name.as_str()) else {
            // Not every resolved git source is necessarily a direct
            // dependency of this project (a `Remotes:` dependency reached
            // transitively might not have made it into a given target at
            // all), so a resolved source absent from this target's packages
            // isn't by itself a mismatch.
            continue;
        };
        let url = pkg.metadata.get(crate::install::REMOTE_URL_FIELD);
        let sha = pkg.metadata.get(crate::install::REMOTE_SHA_FIELD);
        let refspec = pkg.metadata.get(crate::install::REMOTE_REF_FIELD);
        let subdir = pkg.metadata.get(REMOTE_SUBDIR_FIELD);
        if url != Some(&source.git_source.url)
            || sha != Some(&source.git_source.sha)
            || refspec != source.git_source.ref_.as_ref()
            || subdir != source.git_source.subdir.as_ref()
        {
            return false;
        }
    }
    true
}

/// Whether `target`'s own recorded project-package entry (if any) still
/// matches the project's current name, version and content digest -- the
/// project-specific half of "is this lock target still fresh", alongside
/// [`lock_target_satisfies`]/[`lock_target_git_sources_fresh`]'s CRAN/git
/// checks. `self_alias: None` (not a package project, or a workspace, see
/// `ProjectSolve::self_alias`) trivially always passes: there is no project
/// entry to go stale. `self_sha` is the project's current content digest
/// (`compute_dir_stat_digest`), recomputed once per `rig proj lock` run the
/// same way `git_sources` resolves a real `path` dependency's digest fresh
/// every time.
fn project_entry_fresh(
    target: &RprojLockTarget,
    self_alias: Option<&SolveRoot>,
    self_sha: Option<&String>,
) -> bool {
    let Some(alias) = self_alias else {
        return true;
    };
    let Some(pkg) = target.packages.iter().find(|p| p.is_project) else {
        return false;
    };
    pkg.package == alias.name
        && pkg.version == alias.version.to_string()
        && pkg.metadata.get(REMOTE_SHA_FIELD) == self_sha
}

/// The [`RprojLockPackage`] entry for the project's own package -- `alias` is
/// `solve.self_alias`, `root_abs` the project's canonicalized root, `self_sha`
/// its content digest (`compute_dir_stat_digest`, empty/`None` if it could not
/// be computed). Mirrors the "local" `RemoteType` case
/// [`RprojLockTarget::from_solution`] already writes for a real `path`
/// dependency, so [`lockfile_package_info`]/[`fetch_git_lockfile_packages`]
/// handle it with no changes: `sources`/`target` are empty, since the
/// installer reads `RemoteUrl` directly instead of downloading or caching
/// anything.
fn project_lock_package(
    alias: &SolveRoot,
    root_abs: &Path,
    self_sha: Option<&str>,
) -> RprojLockPackage {
    let mut metadata: HashMap<String, String> = HashMap::new();
    metadata.insert(REMOTE_TYPE_FIELD.to_string(), "local".to_string());
    metadata.insert(
        crate::install::REMOTE_URL_FIELD.to_string(),
        root_abs.display().to_string(),
    );
    if let Some(sha) = self_sha.filter(|s| !s.is_empty()) {
        metadata.insert(REMOTE_SHA_FIELD.to_string(), sha.to_string());
    }
    let dependencies: Vec<String> = alias
        .deps
        .dependencies
        .iter()
        // Only a hard dependency (`Depends`/`Imports`/`LinkingTo`) gates
        // install order; a dev-group/`Suggests`-only entry (e.g. testthat,
        // rmarkdown) is installed too, but never has to come before the
        // project's own package, and some of them (e.g. pillar, tibble)
        // depend on the project itself, which would deadlock as a circular
        // dependency if it were treated as a hard dependency here.
        .filter(|d| !d.types.iter().all(|t| DEP_TYPES_SOFT.contains(t)))
        .map(|d| d.name.clone())
        .filter(|n| n != "R" && !BASE_PKGS.contains(&n.as_str()))
        .collect();
    RprojLockPackage {
        package: alias.name.clone(),
        version: alias.version.to_string(),
        binary: false,
        platform: "source".to_string(),
        dependencies,
        metadata,
        sources: vec![],
        target: String::new(),
        groups: vec!["main".to_string()],
        extra_groups: vec![],
        is_project: true,
    }
}

/// Whether an already-parsed existing `lock` has a target for `(rver,
/// platform_key)` that still satisfies the manifest's current direct
/// dependencies (`direct_deps`) and git sources (`git_sources`), without
/// solving anything -- see [`lock_target_satisfies`] and
/// [`lock_target_git_sources_fresh`]. Returns a clone of that target, ready
/// to reuse as-is, or `None` if there's no matching target or it no longer
/// satisfies the manifest, in which case the target needs a real solve.
fn existing_lock_satisfies(
    lock: &RprojLock,
    rver: &str,
    platform_key: &str,
    direct_deps: &[DepVersionSpec],
    git_sources: &[ResolvedGitSource],
) -> Option<RprojLockTarget> {
    let target = lock
        .targets
        .iter()
        .find(|t| t.r_version == rver && t.platform == platform_key)?;
    if !lock_target_satisfies(target, direct_deps) {
        return None;
    }
    if !lock_target_git_sources_fresh(target, git_sources) {
        return None;
    }
    Some(target.clone())
}

/// One git/GitHub-sourced dependency, already fetched and turned into
/// everything [`register_git_sources`] needs to hand it to a solver's
/// registry: no I/O left to do, just three lookups/inserts.
pub(crate) struct ResolvedGitSource {
    name: String,
    version: RegistryPackageVersion,
    ranges: HashMap<String, RPackageVersionRanges, rustc_hash::FxBuildHasher>,
    git_source: GitSourceInfo,
}

/// Fetch every git/GitHub-sourced dependency in `git_deps`, and every
/// dependency reachable from their `Remotes:` fields, exactly once -- not
/// once per solve target. `sc_proj_solve_deps` used to call
/// `fetch_and_read_git_package` itself, so a lockfile solved for several
/// R versions/platforms (`proj_lock`'s `solve_targets.par_iter()`) fetched
/// the same git ref once per target; callers now resolve everything up
/// front with this function and pass the result to every target's
/// [`register_git_sources`] instead.
///
/// A fetched package's own `Remotes:` field, if it has one, is resolved the
/// same way, recursively -- this is the only way a git/GitHub source can
/// appear below the project's own direct dependencies: an ordinary CRAN/PPM
/// package's index metadata has no `Remotes:` field to check. Since a
/// package's `Remotes:` is only known once it's fetched, this walks the
/// dependency graph breadth-first, one `git fetch` round trip per level, but
/// fetches every dependency *within* a level in parallel: the common case
/// (no `Remotes:` at all) is one round trip fetching every `git_deps` entry
/// at once.
pub(crate) fn resolve_git_sources(
    git_deps: &[(String, DepTable)],
    known_shas: &HashMap<GitSourceKey, String>,
    known_releases: &HashMap<(String, Option<String>), (String, String)>,
) -> Result<Vec<ResolvedGitSource>, Box<dyn Error>> {
    // A git/GitHub/URL-sourced package's own soft dependencies (`Suggests:`,
    // `Enhances:`) are dropped here, the same as a CRAN/PPM package's --
    // see the `dev = false` call in `ensure_loaded`. This applies whether the
    // package is named directly in `rproj.toml` or reached transitively
    // through another package's `Remotes:`.
    let mut seen: HashSet<String> = HashSet::new();
    let mut resolved: Vec<ResolvedGitSource> = vec![];
    let mut frontier: Vec<(String, DepTable)> = git_deps.to_vec();

    while !frontier.is_empty() {
        let batch: Vec<(String, DepTable)> = frontier
            .into_iter()
            .filter(|(name, _)| seen.insert(name.clone()))
            .collect();

        let fetched: Vec<Result<(String, Package, GitSourceInfo, String), String>> = batch
            .par_iter()
            .map(|(name, table)| {
                let (pkg, git_source, remotes, source_desc) = if let Some(git_url) = &table.git {
                    let (pkg, git_source, remotes) =
                        fetch_and_read_git_package(git_url, table, known_shas, known_releases)
                            .map_err(|err| err.to_string())?;
                    (pkg, git_source, remotes, git_url.clone())
                } else if let Some(url) = &table.url {
                    let (pkg, url_source, remotes) =
                        fetch_and_read_url_package(url, table).map_err(|err| err.to_string())?;
                    (pkg, url_source, remotes, url.clone())
                } else if let Some(path) = &table.path {
                    let (pkg, local_source, remotes) =
                        read_local_package(Path::new(path)).map_err(|err| err.to_string())?;
                    (pkg, local_source, remotes, path.clone())
                } else {
                    return Err(format!(
                        "{} has a dependency source with no `git`, `url` or `path`",
                        name
                    ));
                };
                if pkg.name != *name {
                    return Err(format!(
                        "`{}` in rproj.toml points at {}, but its DESCRIPTION says `Package: {}`",
                        name, source_desc, pkg.name
                    ));
                }
                Ok((name.clone(), pkg, git_source, remotes))
            })
            .collect();

        let mut next_frontier: Vec<(String, DepTable)> = vec![];
        for item in fetched {
            let (name, pkg, git_source, remotes) = item.map_err(SimpleError::new)?;

            let version = RegistryPackageVersion {
                name: name.clone(),
                version: pkg.version.clone(),
                artifact: Artifact::Source,
            };
            let ranges = rpackage_version_ranges_from_constraints(&pkg.dependencies, false);
            resolved.push(ResolvedGitSource {
                name,
                version,
                ranges,
                git_source,
            });

            for entry in remotes.split(',') {
                let entry = entry.trim();
                if entry.is_empty() {
                    continue;
                }
                let Some(dep_name) = crate::rproj::pak_ref_name(entry) else {
                    continue;
                };
                if seen.contains(&dep_name) {
                    continue;
                }
                match crate::pkgsource::parse_pkg_source(entry) {
                    Ok(crate::pkgsource::PkgSource::Remote(r)) => {
                        next_frontier.push((dep_name, dep_table_from_remote(&r, entry)));
                    }
                    Ok(crate::pkgsource::PkgSource::Url(u)) => {
                        next_frontier.push((dep_name, dep_table_from_url(&u)));
                    }
                    // A `Remotes:` entry that is a path on whoever's
                    // machine wrote it means nothing here.
                    Ok(crate::pkgsource::PkgSource::Cran)
                    | Ok(crate::pkgsource::PkgSource::Local(_))
                    | Err(_) => {}
                }
            }
        }
        frontier = next_frontier;
    }
    Ok(resolved)
}

/// Register every already-resolved git/GitHub-sourced dependency in
/// `git_sources` with `reg` as a pre-resolved package version, the same
/// mechanism `register_roots` uses for workspace members: the version and
/// its dependencies are already known (see [`resolve_git_sources`]), so the
/// solver never looks it up in a repository index (`RPackageRegistry::
/// add_package_version` marks it loaded). Pure in-memory bookkeeping, no I/O,
/// so it's cheap to call once per solve target.
fn register_git_sources(reg: &RPackageRegistry, git_sources: &[ResolvedGitSource]) {
    for source in git_sources {
        reg.add_package_version(
            source.name.clone(),
            source.version.clone(),
            source.ranges.clone(),
        );
        reg.set_git_source(
            source.name.clone(),
            source.version.clone(),
            source.git_source.clone(),
        );
    }
}

/// The manifest `DepTable` a parsed `git`/`github::`/`gitlab::` reference
/// implies, the same shape `rig proj add` writes -- used to feed a fetched
/// package's own `Remotes:` entries back into [`register_git_sources`]'s
/// worklist. `entry` is the original reference text (e.g. `gitlab::group/
/// project/-/pkg@main`), kept verbatim in `ref_` so `rig proj export` can
/// write it back unchanged instead of reconstructing it -- see
/// [`crate::rproj::dep_table_to_pak_ref`].
pub(crate) fn dep_table_from_remote(r: &crate::pkgsource::RemoteSource, entry: &str) -> DepTable {
    DepTable {
        git: Some(r.git.clone()),
        branch: r.branch.clone(),
        tag: r.tag.clone(),
        rev: r.rev.clone(),
        pr: r.pr,
        release: if r.release { Some(true) } else { None },
        subdir: r.subdir.clone(),
        ref_: Some(entry.trim().to_string()),
        ..Default::default()
    }
}

/// The manifest `DepTable` a parsed `url::` reference implies -- the `url`
/// counterpart of [`dep_table_from_remote`], used the same way: to feed a
/// fetched package's own `Remotes:` entries back into
/// [`register_git_sources`]'s worklist.
pub(crate) fn dep_table_from_url(u: &crate::pkgsource::UrlSource) -> DepTable {
    DepTable {
        url: Some(u.url.clone()),
        ..Default::default()
    }
}

/// The `DepTable` a local path implies: the `path` field, always absolute, so
/// that the rest of the pipeline never has to know what rig's working
/// directory was. `path` is resolved by the caller
/// ([`crate::pkgsource::local::resolve_local_path`]), which is also what
/// checks that it exists.
pub(crate) fn dep_table_from_local(path: &Path) -> DepTable {
    DepTable {
        path: Some(path.display().to_string()),
        ..Default::default()
    }
}

/// `target`, as a path relative to `root`: for `rig proj add <path>`, so the
/// `path` written to `rproj.toml` stays correct if the project is moved or
/// checked out elsewhere with the local package at the same relative
/// location -- unlike [`dep_table_from_local`]'s always-absolute path, which
/// is fine for `rig pkg install`'s one-shot, never-persisted use but wrong
/// for a manifest meant to be committed. Both `root` and `target` are
/// canonicalized first, so the result is exact regardless of `..`/`.` or
/// symlinks in either. [`crate::rproj::Rproj::git_dependencies`] is the
/// inverse: it resolves this relative path back to absolute against the
/// project root before anything reads it.
pub(crate) fn relativize_to_root(root: &Path, target: &Path) -> Result<String, Box<dyn Error>> {
    let root = root.canonicalize()?;
    let target = target.canonicalize()?;

    let root_components: Vec<_> = root.components().collect();
    let target_components: Vec<_> = target.components().collect();
    let common = root_components
        .iter()
        .zip(target_components.iter())
        .take_while(|(a, b)| a == b)
        .count();

    let mut result = PathBuf::new();
    for _ in common..root_components.len() {
        result.push("..");
    }
    for component in &target_components[common..] {
        result.push(component);
    }
    if result.as_os_str().is_empty() {
        result.push(".");
    }
    Ok(result.display().to_string())
}

/// Read a local package's `DESCRIPTION`, the local counterpart of
/// [`fetch_and_read_url_package`]: same return shape, no I/O beyond reading
/// the path (and extracting a package file to a tempdir, see
/// [`crate::pkgsource::local::read_local_package_files`]).
///
/// A local *file* (a source tarball or `.zip`, not a directory, and not one
/// already built) is pinned to its own content sha256. A local directory is
/// pinned to a stat digest instead (see [`compute_dir_stat_digest`]): hashing
/// its full content on every install scales with the package's payload size,
/// which gets expensive for a package that vendors a large or compiled tree,
/// while a digest of every file's path/size/mtime is a constant-cost stat
/// call per file. Either way `sha` lets the directory or file be cached
/// exactly like any other non-CRAN source (see `needs_install` in
/// `crate::pkg::install`), rebuilding only when that digest changes.
pub(crate) fn read_local_package(
    path: &Path,
) -> Result<(Package, GitSourceInfo, String), Box<dyn Error>> {
    let files = crate::pkgsource::local::read_local_package_files(path)?;

    let sha = if !files.binary && path.is_file() {
        crate::utils::calculate_file_hash(path).unwrap_or_else(|err| {
            debug!("Not caching {}, cannot hash it: {}", path.display(), err);
            String::new()
        })
    } else if !files.binary && path.is_dir() {
        compute_dir_stat_digest(path, false).unwrap_or_else(|err| {
            debug!("Not caching {}, cannot stat it: {}", path.display(), err);
            String::new()
        })
    } else {
        String::new()
    };

    let local_source = GitSourceInfo {
        remote_type: "local",
        url: path.display().to_string(),
        host: None,
        repo: None,
        username: None,
        subdir: None,
        ref_: None,
        sha,
        binary: files.binary,
    };

    let paragraph = parse_description_paragraph(files.description.as_bytes())?;
    let pkg = Package::from_dcf_paragraph(&paragraph)?;
    let remotes = paragraph
        .get("Remotes")
        .map(|s| s.to_string())
        .unwrap_or_default();

    Ok((pkg, local_source, remotes))
}

/// A directory's `RemoteSha`: a hash of every file's path, size, and
/// modification time (never its content), skipping `.git` directories and
/// anything `.Rbuildignore` excludes. Two calls on an unchanged directory
/// return the same digest; touching a file's mtime, or adding/removing one,
/// changes it. This mirrors `uv`'s local-path caching, which also keys on
/// file stat rather than content for a directory source, for the same
/// reason: a stat is a fixed-cost syscall per file, while hashing content
/// scales with the package's total payload size.
///
/// `skip_description` also leaves out `DESCRIPTION` at the directory's root:
/// pass `true` only when hashing a package project's own root for its
/// `is_project` lock entry (see `project_lock_package`), never for an
/// ordinary `path` dependency. There, `DESCRIPTION` is the dependency's real,
/// user-maintained metadata and has to be hashed like any other file -- an
/// added `Imports:` entry must invalidate the digest. The project's own
/// `DESCRIPTION`, in contrast, is rig's own generated mirror of `rproj.toml`
/// (see `rig proj sync`'s `write_description_to` call), rewritten on every
/// sync -- hashing it would make `rig proj lock` see a "changed" project
/// after every sync that touched nothing else, defeating the sticky-lock
/// behavior the same way a hashed `.rvenv`/`rproj.lock` would.
fn compute_dir_stat_digest(path: &Path, skip_description: bool) -> std::io::Result<String> {
    let ignore = read_rbuildignore(path);

    let mut entries = Vec::new();
    collect_dir_stats(path, path, &ignore, skip_description, &mut entries)?;
    entries.sort();

    let mut buf = String::new();
    for (rel, len, mtime) in &entries {
        buf.push_str(rel);
        buf.push('\n');
        buf.push_str(&len.to_string());
        buf.push('\n');
        buf.push_str(mtime);
        buf.push('\n');
    }
    Ok(crate::utils::calculate_hash(&buf))
}

/// `<dir>/.Rbuildignore`'s patterns, as compiled regexes, exactly as R
/// reads them: one Perl-style regex per line, blank lines skipped, each
/// tested unanchored against a file's path relative to `dir`. A missing
/// file, or a line that isn't a valid regex, is treated as no pattern.
fn read_rbuildignore(dir: &Path) -> Vec<regex::Regex> {
    let Ok(content) = fs::read_to_string(dir.join(".Rbuildignore")) else {
        return Vec::new();
    };
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(|line| regex::Regex::new(line).ok())
        .collect()
}

/// Recursively collects `(relative_path, len, mtime)` for every file under
/// `dir`, skipping `.git` directories and anything matching `ignore`.
/// `mtime` is the file's modification time as whole nanoseconds since the
/// Unix epoch, textually, so it sorts and compares like any other field
/// here without depending on a particular `SystemTime` debug format.
///
/// Also always skips `.rvenv` (rig's own machine-specific environment) and
/// `rproj.lock` at the directory's root, regardless of `.Rbuildignore`:
/// hashing either would make a `rig proj lock`/`sync` run on a package
/// project change its own digest just by having run, since both are rewritten
/// by rig itself and neither is part of the package's installable content.
/// This matters for a `path` dependency in general, but is guaranteed to bite
/// when `dir` is the project's own root, since `.rvenv`/`rproj.lock` always
/// live right there (see `ProjectSolve::self_alias`). `skip_description`
/// additionally leaves out a root-level `DESCRIPTION` -- see
/// [`compute_dir_stat_digest`].
fn collect_dir_stats(
    root: &Path,
    dir: &Path,
    ignore: &[regex::Regex],
    skip_description: bool,
    out: &mut Vec<(String, u64, String)>,
) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            if path.file_name().is_some_and(|n| n == ".git")
                || (dir == root && path.file_name().is_some_and(|n| n == RVENV_DIR))
            {
                continue;
            }
            collect_dir_stats(root, &path, ignore, skip_description, out)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }

        if dir == root
            && ((path.file_name().is_some_and(|n| n == RPROJ_LOCK_FILE))
                || (skip_description && path.file_name().is_some_and(|n| n == "DESCRIPTION")))
        {
            continue;
        }

        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if ignore.iter().any(|re| re.is_match(&rel)) {
            continue;
        }

        let metadata = entry.metadata()?;
        let mtime = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos().to_string())
            .unwrap_or_default();
        out.push((rel, metadata.len(), mtime));
    }
    Ok(())
}

/// A github.com URL's `owner`/`repo`, if `git_url` is one.
pub(crate) fn github_owner_repo(git_url: &str) -> Option<(&str, &str)> {
    let path_part = git_url.strip_prefix("https://github.com/")?;
    let path_part = path_part.trim_end_matches(".git");
    path_part.split_once('/')
}

/// Fetch a git/GitHub dependency's `DESCRIPTION`, and return the parsed
/// package, its provenance (for the solver's git-source side table), and its
/// raw `Remotes:` field (empty if it has none).
///
/// Only `DESCRIPTION` is ever downloaded (a sparse, partial-clone fetch, see
/// [`crate::pkgsource::git::fetch_git_description`]) -- resolving a
/// dependency needs nothing else from the repository. `table`'s `pr`/
/// `release`/`rev`/`branch`/`tag` fields (in that priority order) are
/// resolved to a single refspec to fetch; `pr`/`release` only make sense for
/// a `github.com` URL (mirroring `RemoteSource`'s doc comments).
///
/// `known_shas` is [`existing_git_shas`]'s map of this dependency's
/// previously resolved commit, if any -- looked up by [`GitSourceKey`].
/// `known_releases` is [`existing_release_refs`]'s equivalent for a
/// `release = true` dependency, looked up by URL and subdirectory instead,
/// since its refspec (the release tag) isn't known ahead of resolving it.
/// Both are empty on `--upgrade`, forcing a fresh resolve either way.
pub(crate) fn fetch_and_read_git_package(
    git_url: &str,
    table: &DepTable,
    known_shas: &HashMap<GitSourceKey, String>,
    known_releases: &HashMap<(String, Option<String>), (String, String)>,
) -> Result<(Package, GitSourceInfo, String), Box<dyn Error>> {
    let owner_repo = github_owner_repo(git_url);

    if (table.pr.is_some() || table.release == Some(true)) && owner_repo.is_none() {
        bail!(
            "`{}` is not a github.com URL, `pr`/`release` are only supported for GitHub sources",
            git_url
        );
    }

    let (refspec, known_sha) = if table.release == Some(true) {
        let (owner, repo) = owner_repo.expect("checked above");
        let release_key = (git_url.to_string(), table.subdir.clone());
        match known_releases.get(&release_key) {
            Some((tag, sha)) => (Some(tag.clone()), Some(sha.clone())),
            None => (
                Some(crate::pkgsource::git::resolve_release_tag(owner, repo)?),
                None,
            ),
        }
    } else {
        let refspec = if let Some(pr) = table.pr {
            Some(format!("refs/pull/{}/head", pr))
        } else {
            table
                .rev
                .clone()
                .or_else(|| table.branch.clone())
                .or_else(|| table.tag.clone())
        };
        let key = (git_url.to_string(), refspec.clone(), table.subdir.clone());
        let known_sha = known_shas.get(&key).cloned();
        (refspec, known_sha)
    };

    let (description, sha) = crate::pkgsource::git::fetch_git_description(
        git_url,
        refspec.as_deref(),
        table.subdir.as_deref(),
        known_sha.as_deref(),
    )?;

    let git_source = match owner_repo {
        Some((owner, repo)) => GitSourceInfo {
            remote_type: "github",
            url: git_url.to_string(),
            host: Some("github.com".to_string()),
            repo: Some(format!("{}/{}", owner, repo)),
            username: Some(owner.to_string()),
            subdir: table.subdir.clone(),
            ref_: refspec.clone(),
            sha,
            binary: false,
        },
        None => GitSourceInfo {
            remote_type: "git",
            url: git_url.to_string(),
            host: None,
            repo: None,
            username: None,
            subdir: table.subdir.clone(),
            ref_: refspec.clone(),
            sha,
            binary: false,
        },
    };

    let paragraph = parse_description_paragraph(description.as_bytes())?;
    let pkg = Package::from_dcf_paragraph(&paragraph)?;
    let remotes = paragraph
        .get("Remotes")
        .map(|s| s.to_string())
        .unwrap_or_default();

    Ok((pkg, git_source, remotes))
}

/// Fetch a `url`-sourced dependency's `DESCRIPTION`, the `url` counterpart of
/// [`fetch_and_read_git_package`]. There is no cheap partial fetch for an
/// arbitrary HTTP resource, so this downloads (and caches) the whole
/// archive -- see [`crate::pkgsource::url::fetch_url_description`] -- and
/// extracts it to read `DESCRIPTION` back out. `table.hash`, if set, pins
/// the archive's expected sha256; otherwise whatever the URL currently
/// serves is trusted, and its sha256 is recorded for the lockfile.
pub(crate) fn fetch_and_read_url_package(
    url: &str,
    table: &DepTable,
) -> Result<(Package, GitSourceInfo, String), Box<dyn Error>> {
    let (description, sha256, effective_subdir) = crate::pkgsource::url::fetch_url_description(
        url,
        table.subdir.as_deref(),
        table.hash.as_deref(),
    )?;

    let url_source = GitSourceInfo {
        remote_type: "url",
        url: url.to_string(),
        host: None,
        repo: None,
        username: None,
        subdir: effective_subdir,
        ref_: None,
        sha: sha256,
        binary: false,
    };

    let paragraph = parse_description_paragraph(description.as_bytes())?;
    let pkg = Package::from_dcf_paragraph(&paragraph)?;
    let remotes = paragraph
        .get("Remotes")
        .map(|s| s.to_string())
        .unwrap_or_default();

    Ok((pkg, url_source, remotes))
}

/// Show a solve failure: the headline as an error, the pubgrub report under it.
///
/// The report body is deliberately not colored — [`OUTPUT.error`] bolds and
/// reddens everything it is given, and a dozen lines of that is harder to read,
/// not easier.
fn report_solve_failure(msg: &str) {
    match msg.split_once('\n') {
        Some((headline, report)) => {
            OUTPUT.error(headline);
            OUTPUT.println(report);
        }
        None => OUTPUT.error(msg),
    }
}

/// How a solve target's platform is named in messages and in the lock file.
///
/// Mirrors how `RprojLockTarget::from_solution` derives the target's `platform`
/// field (src/rproj.rs), so the two never disagree.
fn solve_platform_key(target_name: Option<String>) -> String {
    target_name.unwrap_or_else(|| std::env::consts::ARCH.to_string())
}

fn solution_to_sorted_vec(
    registry: &RPackageRegistry,
    solution: &SelectedDependencies<RPackageRegistry>,
) -> Vec<(String, RegistryPackageVersion)> {
    let mut vec: Vec<(String, RegistryPackageVersion)> = solution
        .iter()
        .filter(|(pkg, _ver)| !registry.is_local(pkg))
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
#[derive(Default)]
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
    /// Platforms to add to `platforms` (or the default set), from
    /// `--add-platform`'s comma-separated, repeatable list.
    add_platforms: Vec<String>,
    prefer_binary: Option<usize>,
    /// `--upgrade`: re-resolve every dependency instead of reusing an
    /// existing `rproj.lock`: re-check every git/GitHub dependency's ref
    /// against its remote instead of reusing the commit already pinned (see
    /// [`existing_git_shas`]), and re-run the solver for CRAN/PPM
    /// dependencies instead of keeping a pin that already satisfies
    /// `rproj.toml` (see [`existing_lock_satisfies`]).
    upgrade: bool,
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
        add_platforms: args
            .get_many::<String>("add-platform")
            .map(|vs| vs.cloned().collect())
            .unwrap_or_default(),
        prefer_binary: args.get_one::<usize>("prefer-binary").copied(),
        upgrade: args.get_flag("upgrade"),
    };
    proj_lock(&proj_lock_root()?, &opts, args)
}

/// The directory `rig proj lock` and `rig proj sync` work on: the workspace a
/// project belongs to, else the project itself, else the current directory --
/// which is where `proj_read_solve_roots` reports the missing manifest.
///
/// A workspace has one lock file and one library, both at its root, so a
/// command run in a member directory has to act on the whole workspace.
fn proj_lock_root() -> Result<PathBuf, Box<dyn Error>> {
    let cwd = std::env::current_dir()?;
    if let Some(workspace) = find_workspace_root(&cwd)? {
        if workspace != cwd {
            let msg = format!("Using the workspace at {}", workspace.display());
            OUTPUT.info(&msg);
            info!("{}", msg);
        }
        return Ok(workspace);
    }
    Ok(find_project_root(&cwd).unwrap_or(cwd))
}

/// Report what `rig proj` knows about the project at or above the current
/// directory: the manifest, the lock file, and whether the local `.rvenv`
/// environment is in sync with it. Read-only -- never solves or syncs.
fn sc_proj_status(
    args: &ArgMatches,
    projargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let json = args.get_flag("json") || projargs.get_flag("json") || mainargs.get_flag("json");
    let root = proj_lock_root()?;

    let mut warnings: Vec<String> = vec![];

    // Read every piece by hand rather than through the usual helpers: those
    // bail (and some log their own error) on a problem that here should only
    // drop that one section and warn, not take down the rest of the report.
    let manifest_path = root.join(RPROJ_MANIFEST_FILE);
    let manifest: Option<Rproj> = if manifest_path.exists() {
        match fs::read_to_string(&manifest_path)
            .map_err(|e| e.to_string())
            .and_then(|text| toml::from_str::<Rproj>(&text).map_err(|e| e.to_string()))
        {
            Ok(m) => Some(m),
            Err(e) => {
                warnings.push(format!("Could not read {}: {}", RPROJ_MANIFEST_FILE, e));
                None
            }
        }
    } else {
        None
    };

    let lock_path = root.join(RPROJ_LOCK_FILE);
    let lock: Option<RprojLock> = if lock_path.exists() {
        match fs::read_to_string(&lock_path)
            .map_err(|e| e.to_string())
            .and_then(|text| {
                RprojLock::check_version(&text).map_err(|e| e.to_string())?;
                toml::from_str::<RprojLock>(&text).map_err(|e| e.to_string())
            }) {
            Ok(lock) => Some(lock),
            Err(e) => {
                warnings.push(format!("Could not read {}: {}", RPROJ_LOCK_FILE, e));
                None
            }
        }
    } else {
        None
    };

    let cfg = match read_rvenv_cfg(&root) {
        Ok(cfg) => cfg,
        Err(e) => {
            warnings.push(format!("Could not read {}: {}", RVENV_CFG_FILE, e));
            None
        }
    };
    let sync_reason = if cfg.is_some() {
        match rvenv_sync_needed(&root) {
            Ok(reason) => reason,
            Err(e) => {
                warnings.push(format!(
                    "Could not check whether the environment is in sync: {}",
                    e
                ));
                None
            }
        }
    } else {
        None
    };

    if json {
        print_proj_status_json(&root, &manifest, &lock, &cfg, &sync_reason, &warnings)
    } else {
        print_proj_status(&root, &manifest, &lock, &cfg, &sync_reason, &warnings)
    }
}

/// The plain-text report of [`sc_proj_status`].
fn print_proj_status(
    root: &Path,
    manifest: &Option<Rproj>,
    lock: &Option<RprojLock>,
    cfg: &Option<RvenvCfg>,
    sync_reason: &Option<String>,
    warnings: &[String],
) -> Result<(), Box<dyn Error>> {
    println!("Project: {}", root.display());
    println!();

    match manifest {
        Some(m) => {
            println!("Manifest: {} {}", m.project.name, m.project.version);
            if let Some(ws) = &m.workspace {
                if !ws.members.is_empty() {
                    match workspace_members(root, ws) {
                        Ok(dirs) => {
                            println!("Workspace members ({}):", dirs.len());
                            for dir in &dirs {
                                match proj_read_manifest(dir) {
                                    Ok(member) => println!(
                                        "  {} {}",
                                        member.project.name, member.project.version
                                    ),
                                    Err(e) => {
                                        println!("  {}: could not read: {}", dir.display(), e)
                                    }
                                }
                            }
                        }
                        Err(e) => println!("Workspace members: could not list: {}", e),
                    }
                }
            }
            let groups = m.dependency_group_roots()?;
            if !groups.is_empty() {
                let mut names: Vec<&String> = groups.keys().collect();
                names.sort();
                println!("Dependencies:");
                for name in names {
                    println!("  {}: {}", name, groups[name].len());
                }
            }
        }
        None => println!("Manifest: none, run `rig proj init` first"),
    }
    println!();

    match lock {
        Some(lock) => {
            println!("Lock file: {} (version {})", RPROJ_LOCK_FILE, lock.version);
            let mut tab: Table = Table::new("{:<}   {:<}   {:<}");
            tab.add_row(row!("R version", "Platform", "Packages"));
            tab.add_heading("-------------------------------------");
            for target in &lock.targets {
                tab.add_row(row!(
                    &target.r_version,
                    &target.platform,
                    target.packages.len().to_string()
                ));
            }
            print!("{}", tab);
        }
        None => println!("Lock file: none, run `rig proj lock` first"),
    }
    println!();

    match cfg {
        Some(cfg) => {
            println!(
                "Environment: R {} ({}, {})",
                cfg.r_version, cfg.platform, cfg.r_arch
            );
            match sync_reason {
                None => println!("  up to date"),
                Some(reason) => println!("  needs sync: {}", reason),
            }
        }
        None => println!("Environment: none, run `rig proj sync` first"),
    }

    if !warnings.is_empty() {
        println!();
        for warning in warnings {
            OUTPUT.warn(warning);
        }
    }

    Ok(())
}

/// The `--json` report of [`sc_proj_status`].
fn print_proj_status_json(
    root: &Path,
    manifest: &Option<Rproj>,
    lock: &Option<RprojLock>,
    cfg: &Option<RvenvCfg>,
    sync_reason: &Option<String>,
    warnings: &[String],
) -> Result<(), Box<dyn Error>> {
    let mut warnings = warnings.to_vec();

    let manifest_json = match manifest {
        Some(m) => {
            let workspace_members_json: Vec<serde_json::Value> = match &m.workspace {
                Some(ws) if !ws.members.is_empty() => match workspace_members(root, ws) {
                    Ok(dirs) => dirs
                        .iter()
                        .filter_map(|dir| match proj_read_manifest(dir) {
                            Ok(member) => Some(serde_json::json!({
                                "name": member.project.name,
                                "version": member.project.version,
                            })),
                            Err(e) => {
                                warnings.push(format!("Could not read {}: {}", dir.display(), e));
                                None
                            }
                        })
                        .collect(),
                    Err(e) => {
                        warnings.push(format!("Could not list workspace members: {}", e));
                        vec![]
                    }
                },
                _ => vec![],
            };
            let mut groups: Vec<(String, usize)> = m
                .dependency_group_roots()?
                .into_iter()
                .map(|(name, deps)| (name, deps.len()))
                .collect();
            groups.sort();
            serde_json::json!({
                "name": m.project.name,
                "version": m.project.version,
                "workspace_members": workspace_members_json,
                "dependency_groups": groups.into_iter().collect::<BTreeMap<_, _>>(),
            })
        }
        None => serde_json::Value::Null,
    };

    let lock_json = match lock {
        Some(lock) => {
            let targets: Vec<serde_json::Value> = lock
                .targets
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "r_version": t.r_version,
                        "platform": t.platform,
                        "package_count": t.packages.len(),
                    })
                })
                .collect();
            serde_json::json!({
                "version": lock.version,
                "targets": targets,
            })
        }
        None => serde_json::Value::Null,
    };

    let sync_json = match cfg {
        Some(cfg) => serde_json::json!({
            "r_version": cfg.r_version,
            "r_arch": cfg.r_arch,
            "platform": cfg.platform,
            "up_to_date": sync_reason.is_none(),
            "reason": sync_reason,
        }),
        None => serde_json::Value::Null,
    };

    let out = serde_json::json!({
        "root": root.display().to_string(),
        "manifest": manifest_json,
        "lock": lock_json,
        "sync": sync_json,
        "warnings": warnings,
    });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
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

/// The platforms `proj_lock` solves for, given `--platform` and
/// `--add-platform`. With no `--platform`, solve for this machine plus the
/// three other platforms a project typically needs to run on: Windows, a
/// generic glibc Linux build (P3M's "manylinux" distro-independent build,
/// covering any glibc-based x86_64 distro P3M has no specific build for),
/// and macOS on arm64. Each platform string is fully explicit
/// (arch-vendor-os), so it resolves the same regardless of which OS `rig
/// proj lock` itself runs on; only "this machine" (`None`) depends on the
/// host. `--add-platform` extends that set (or an explicit `--platform`
/// list) instead of replacing it. Duplicates (e.g. "this machine" already
/// being macOS arm64, or a redundant `--add-platform`) are dropped before
/// solving, by the resolved-target dedup in `proj_lock`, so a redundant
/// solve is never dispatched in the first place.
fn lock_platform_specs(opts: &ProjLockOptions) -> Vec<Option<String>> {
    let mut specs: Vec<Option<String>> = if !opts.platforms.is_empty() {
        opts.platforms.iter().cloned().map(Some).collect()
    } else {
        vec![
            None,
            Some("x86_64-w64-mingw32".to_string()),
            Some("x86_64-unknown-linux-gnu".to_string()),
            Some("aarch64-apple-darwin".to_string()),
        ]
    };
    specs.extend(opts.add_platforms.iter().cloned().map(Some));
    specs
}

/// Solve the dependencies of the project in `root` for every `(R version,
/// platform)` combination `opts` asks for (a cross product of
/// `opts.r_versions` and the platforms from [`lock_platform_specs`]), and
/// write them all into `rproj.lock`. An empty `opts.r_versions` picks one R
/// version the usual way (`proj_lock_r_version`).
fn proj_lock(root: &Path, opts: &ProjLockOptions, args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    // Do this first, to report local errors early
    let solve = proj_read_solve_roots(root)?;
    let pkg_deps = &solve.merged;

    // Lock itself never reads `.Renviron`/`.rvenvlib` -- they only matter for
    // R started directly -- but fill them in if missing, same as sync/run.
    ensure_rvenv_files(root)?;

    if solve.members.len() > 1 {
        let names = solve
            .roots
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let msg = format!(
            "Locking {} workspace members together: {}",
            solve.members.len(),
            names
        );
        OUTPUT.info(&msg);
        info!("{}", msg);
    }

    // Each R version has to satisfy the manifest's own `R` requirement,
    // otherwise the solve either fails or produces a lock file for an R the
    // project rules out. An `--r-version` is taken as given, the solver
    // reports the conflict if there is one. With none given, the default
    // logic in `proj_lock_r_version` picks the one version to solve for.
    let rvers: Vec<String> = if opts.r_versions.is_empty() {
        vec![proj_lock_r_version(pkg_deps, args)?]
    } else {
        opts.r_versions.clone()
    };
    let platform_specs = lock_platform_specs(opts);

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
            let platform_key = target.as_ref().map(|t| t.name()).unwrap_or_else(|| {
                // "This machine" (no `--platform` spec) still keys on the
                // host arch, so it can dedup against a fixed default
                // platform that resolves to the same target. Two distinct
                // named `--platform` specs that both fail to resolve must
                // not collapse onto that same key.
                platform
                    .clone()
                    .unwrap_or_else(|| std::env::consts::ARCH.to_string())
            });
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

    // The manifest's own direct dependencies, i.e. what a target's lock has
    // to still satisfy to be reused untouched -- same name/BASE_PKGS scope
    // `RprojLockTarget::from_solution` uses for `packages`, since that's the
    // universe `lock_target_satisfies` looks names up in.
    let direct_deps: Vec<DepVersionSpec> = solve
        .merged
        .dependencies
        .iter()
        .filter(|d| d.name != "R" && !BASE_PKGS.contains(&d.name.as_str()))
        .cloned()
        .collect();

    // `--upgrade` and `--no-cache` both skip reading the existing lock at
    // all, forcing every target to solve fresh -- same as a first `rig proj
    // lock`, or one recovering from a corrupt/outdated lockfile.
    let existing_lock = if opts.upgrade || crate::cache::no_cache() {
        None
    } else {
        read_existing_lock(root)
    };

    // Resolve every git/GitHub dependency once, up front, instead of letting
    // each solve target fetch it on its own -- see `resolve_git_sources`.
    // `--upgrade` (folded into `existing_lock` above) skips `existing_git_shas`
    // and `existing_release_refs` (empty maps) so every git dependency
    // re-resolves against its remote instead of reusing whatever commit (or,
    // for a `release = true` dependency, whatever tag) the existing lock file
    // already pinned it to.
    let (known_shas, known_releases) = match &existing_lock {
        Some(lock) => (existing_git_shas(lock), existing_release_refs(lock)),
        None => (HashMap::new(), HashMap::new()),
    };
    let git_sources = resolve_git_sources(&solve.git_deps, &known_shas, &known_releases)?;

    // For a package project (see `solve.self_alias`): the project's own
    // content digest, resolved once up front the same way `git_sources` is
    // above, and reused both to decide whether an existing target's own
    // package entry is still fresh (`project_fresh` below) and to build that
    // entry when a target does need solving (further down). Recomputed on
    // every `rig proj lock`, unconditionally, the same as a real `path`
    // dependency's digest -- so editing the project's own source, or
    // renaming/re-versioning it, forces a fresh lock the same way a changed
    // dependency does.
    let root_abs = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let self_sha = solve
        .self_alias
        .as_ref()
        .map(|_| compute_dir_stat_digest(&root_abs, true).unwrap_or_default());

    let project_fresh = |target: &RprojLockTarget| {
        project_entry_fresh(target, solve.self_alias.as_ref(), self_sha.as_ref())
    };

    // Every target the existing lock already satisfies, reused byte-for-byte
    // instead of solved again -- the "a lockfile is sticky until you ask to
    // upgrade" behavior `Cargo.lock`/`uv.lock` have, now covering CRAN/PPM
    // dependencies too (git/GitHub already got it via `known_shas` above).
    let mut reused: Vec<RprojLockTarget> = vec![];
    let mut to_solve: Vec<&SolveTarget> = vec![];
    for st in &solve_targets {
        let existing = existing_lock.as_ref().and_then(|lock| {
            existing_lock_satisfies(lock, &st.rver, &st.platform_key, &direct_deps, &git_sources)
                .filter(project_fresh)
        });
        match existing {
            Some(target) => reused.push(target),
            None => to_solve.push(st),
        }
    }

    // Every requested target was already satisfied: no metadata to refresh,
    // no solving to do. Still only a no-op if the existing lock doesn't also
    // carry stray targets outside this request (e.g. a previous `--r-version
    // 4.3,4.4` narrowed to `--r-version 4.3`) -- those have to be dropped, so
    // the file gets rewritten even though nothing needed solving.
    let existing_count = existing_lock.as_ref().map_or(0, |l| l.targets.len());
    if to_solve.is_empty() && reused.len() == existing_count {
        OUTPUT.success("rproj.lock is already up to date");
        info!("rproj.lock is already up to date, nothing to solve");
        return Ok(());
    }

    if !reused.is_empty() {
        OUTPUT.info(&format!(
            "{} of {} targets already up to date",
            reused.len(),
            solve_targets.len()
        ));
    }
    if existing_count > reused.len() + to_solve.len() {
        OUTPUT.info(&format!(
            "Dropping {} target(s) no longer requested",
            existing_count - reused.len() - to_solve.len()
        ));
    }

    let multi = to_solve.len() > 1;
    if !to_solve.is_empty() {
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
        OUTPUT.status("Downloading binary package metadata");
        if multi {
            OUTPUT.status(&format!(
                "Solving dependencies for {} targets",
                to_solve.len()
            ));
        } else {
            OUTPUT.status("Solving dependencies");
        }
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
    let solved: Vec<(String, String, Result<SolveResult, String>)> = to_solve
        .par_iter()
        .map(|st| {
            let result = sc_proj_solve_deps(
                &st.rver,
                &solve.roots,
                solve.self_alias.as_ref(),
                &git_sources,
                st.target.clone(),
                prefer_binary,
                false,
            )
            .map_err(|e| e.to_string());
            (st.rver.clone(), st.platform_key.clone(), result)
        })
        .collect();

    let mut targets: Vec<RprojLockTarget> = reused;
    let mut summaries: Vec<TargetSolution> = vec![];
    for (rver, platform_key, result) in solved {
        let (registry, solution) = match result {
            Ok(v) => v,
            // The failing solve logged itself inside `sc_proj_solve_deps` but
            // left the printing to here, so that a batch of parallel solves
            // shows one failure instead of one message per failing thread. The
            // message already names the target it belongs to. This aborts the
            // whole `proj lock` command, same as the old sequential `?` did;
            // printing it here is what makes it visible, since `error!` only
            // reaches the log file in interactive mode.
            Err(msg) => {
                report_solve_failure(&msg);
                bail!("{}", msg);
            }
        };

        let mut target = RprojLockTarget::from_solution(&registry, &solution);
        let groups = compute_package_groups(&solve.group_roots, &target.packages);
        let extra_groups = compute_package_groups(&solve.extra_roots, &target.packages);
        for pkg in target.packages.iter_mut() {
            if let Some(names) = groups.get(&pkg.package) {
                pkg.groups = names.clone();
            }
            if let Some(names) = extra_groups.get(&pkg.package) {
                pkg.extra_groups = names.clone();
            }
        }

        // The project's own package, for a package project (`solve.self_alias`):
        // built directly rather than read off `solution`, since nothing may
        // actually depend on the project by name -- `self_alias` only makes it
        // *available* to the solver, it doesn't force it into the graph.
        if let Some(alias) = solve.self_alias.as_ref() {
            target
                .packages
                .push(project_lock_package(alias, &root_abs, self_sha.as_deref()));
        }

        target.direct_dependencies = direct_deps
            .iter()
            .map(|d| LockDirectDependency {
                name: d.name.clone(),
                constraint: format_constraints(&d.constraints),
            })
            .collect();
        info!("Solved dependencies for R {} / {}", rver, platform_key);

        summaries.push(TargetSolution {
            r_version: rver.clone(),
            platform: platform_key.clone(),
            rows: solution_rows(&registry, &solution),
        });

        targets.push(target);
    }

    if !to_solve.is_empty() {
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
            let rvers = dedup_in_order(to_solve.iter().map(|st| st.rver.as_str()));
            let platforms = dedup_in_order(to_solve.iter().map(|st| st.platform_key.as_str()));
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
    }

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
    let sorted_solution = solution_to_sorted_vec(registry, solution);
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

/// Which dependency group(s) (see [`Rproj::dependency_group_roots`]) need
/// each of `packages`, by walking each group's direct dependencies out
/// through the lockfile's own dependency graph. In a workspace, `group_roots`
/// is every member's roots already merged by group name (see
/// `proj_read_solve_roots`), so a walk from one group's roots covers every
/// member's subtree for that group, not just the first member reached.
fn compute_package_groups(
    group_roots: &HashMap<String, Vec<String>>,
    packages: &[RprojLockPackage],
) -> HashMap<String, Vec<String>> {
    let by_name: HashMap<&str, &RprojLockPackage> =
        packages.iter().map(|p| (p.package.as_str(), p)).collect();

    let mut package_groups: HashMap<String, Vec<String>> = HashMap::new();
    for (group_name, roots) in group_roots {
        let mut seen: HashSet<String> = HashSet::new();
        let mut todo: Vec<String> = roots.clone();
        while let Some(name) = todo.pop() {
            if !seen.insert(name.clone()) {
                continue;
            }
            // R and the base packages are dependencies in the manifest, but
            // never lockfile entries, so they simply do not match anything
            // here.
            if let Some(pkg) = by_name.get(name.as_str()) {
                package_groups
                    .entry(pkg.package.clone())
                    .or_default()
                    .push(group_name.clone());
                todo.extend(pkg.dependencies.iter().cloned());
            }
        }
    }
    for names in package_groups.values_mut() {
        names.sort();
        names.dedup();
    }
    package_groups
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
    dry_run: bool,
) -> Result<(String, PathBuf), Box<dyn Error>> {
    if let Some(name) = find_r_installation(r_version, arch)? {
        let binary = get_r_binary(&name)?;
        return Ok((name, binary));
    }

    let add_args = r_add_args(r_version, arch);
    if dry_run {
        let msg = format!(
            "R {} ({}) is not installed, `rig proj sync` would install it with \
             `rig {}`",
            r_version,
            arch,
            add_args[1..].join(" ")
        );
        OUTPUT.info(&msg);
        info!("{}", msg);
        bail!("{}", msg);
    }
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
    if let Some(exact) = installed
        .iter()
        .find(|candidate| rvenv_r_arch(&candidate.name) == arch && candidate.name == r_version)
    {
        return Ok(Some(exact.name.clone()));
    }
    // Several installs can match a minor version like `4.6` (`4.6.1`,
    // `4.6.2`, ...); the newest one wins, same as `select_sync_target` and
    // `proj_lock_r_version` pick the newest among several candidates.
    let mut matching: Vec<_> = installed
        .iter()
        .filter(|candidate| {
            if rvenv_r_arch(&candidate.name) != arch {
                return false;
            }
            // An installation with no version, or a version that is not a
            // number (`devel`, `next`), is never a match for a lock file's R
            // version.
            match &candidate.version {
                Some(version) => r_version_matches(r_version, version),
                None => false,
            }
        })
        .collect();
    matching.sort_by_key(|c| {
        c.version
            .as_deref()
            .and_then(r_components)
            .unwrap_or_default()
    });
    Ok(matching.pop().map(|v| v.name.clone()))
}

/// The raw CPU arch a lock target's platform string names (`arm64`,
/// `aarch64`, `x86_64`), whether it is the whole string (a `--platform
/// source` solve's bare-arch platform) or its last `-`-separated component
/// (`manylinux_2_28-arm64`, `macos-x86_64`). `None` if the platform names no
/// arch rig recognizes.
fn platform_arch(platform: &str) -> Option<&str> {
    let candidate = platform
        .rsplit_once('-')
        .map_or(platform, |(_, suffix)| suffix);
    match candidate {
        "arm64" | "aarch64" | "x86_64" => Some(candidate),
        _ => None,
    }
}

/// The architecture the lock file's target platform needs, in the form
/// [`rvenv_r_arch`] reports it, or the machine's own if the platform does not
/// name one.
fn target_r_arch(platform: &str) -> String {
    match platform_arch(platform) {
        Some(arch) => native_arch_name(arch),
        None => native_arch_name(std::env::consts::ARCH),
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
/// and CPU arch match this machine (or name none), further narrowed by
/// `--r-version`/`--platform` if the caller gave them. Several matches are
/// not an error --
/// the highest R version among them wins, so locking for several R versions
/// just works without extra flags; only zero matches is a hard error.
fn select_sync_target<'a>(
    targets: &'a [RprojLockTarget],
    r_version: Option<&str>,
    platform: Option<&str>,
) -> Result<&'a RprojLockTarget, Box<dyn Error>> {
    let this_os = this_os_family();
    let this_arch = native_arch_name(std::env::consts::ARCH);
    let mut candidates: Vec<&RprojLockTarget> = targets
        .iter()
        .filter(|t| match target_os_family(&t.platform) {
            Some(os) => os == this_os,
            None => true,
        })
        .filter(|t| match platform_arch(&t.platform) {
            Some(arch) => native_arch_name(arch) == this_arch,
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
    /// Install the `dev` dependency group, in addition to `main` (default:
    /// on). `--no-dev` turns it off; an explicit `--group dev` or
    /// `--all-groups` installs it regardless, since explicit selection wins
    /// over the default-suppressing flag.
    pub dev: bool,
    /// `--group`, repeatable/comma-separated: install these dependency
    /// groups, in addition to the default set (`main`, plus `dev` unless
    /// `--no-dev`).
    pub groups: Vec<String>,
    /// `--all-groups`: install every dependency group the manifest
    /// declares, regardless of `dev`/`groups`.
    pub all_groups: bool,
    /// `--extra`, repeatable/comma-separated: install these
    /// optional-dependency extras. None are installed by default.
    pub extras: Vec<String>,
    /// `--all-extras`: install every optional-dependency extra.
    pub all_extras: bool,
    /// Install the R version the lock file names, if it is missing
    /// (`--no-install-r` turns this off).
    pub install_r: bool,
    /// Install the project's own package (`type = "package"` in
    /// `rproj.toml`), i.e. the `rproj.lock` entry with
    /// [`RprojLockPackage::is_project`] set (`--no-install-project` turns
    /// this off). Even then, the entry stays in the wanted set -- and so
    /// still gets installed -- if some other wanted package actually depends
    /// on it by name; see [`sync_wanted_packages`].
    pub install_project: bool,
    /// How many packages to install at the same time (`--max-concurrent`).
    /// `None` means fall back to `get_concurrent_installs()` (the
    /// `concurrent-installs` config entry / `RIG_CONCURRENT_INSTALLS`, or the
    /// number of CPU cores).
    pub max_concurrent: Option<usize>,
    /// Which target to sync when more than one matches this machine
    /// (`--r-version`). Selects among `rproj.lock`'s existing targets, does
    /// not trigger a new solve.
    pub r_version: Option<String>,
    /// Which target to sync when more than one matches this machine
    /// (`--platform`). Selects among `rproj.lock`'s existing targets, does
    /// not trigger a new solve.
    pub platform: Option<String>,
    /// Leave packages that are installed but not in `rproj.lock` alone,
    /// instead of removing them (`--inexact`).
    pub inexact: bool,
    /// Install only from `rproj.lock`, failing instead of running
    /// `rig proj lock` when it is missing (`--frozen`). Also skips the
    /// repositories file when there is no `rproj.toml` to build it from.
    pub frozen: bool,
    /// Report what sync would install, remove or write, without actually
    /// doing any of it (`--dry-run`).
    pub dry_run: bool,
}

impl Default for ProjSyncOptions {
    fn default() -> Self {
        ProjSyncOptions {
            dev: true,
            groups: vec![],
            all_groups: false,
            extras: vec![],
            all_extras: false,
            install_r: true,
            install_project: true,
            max_concurrent: None,
            r_version: None,
            platform: None,
            inexact: false,
            frozen: false,
            dry_run: false,
        }
    }
}

fn sc_proj_sync(
    args: &ArgMatches,
    _libargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    // The project is the nearest one at or above the current directory, so
    // that `rig proj sync` works from a subdirectory, like `git` does. In a
    // workspace it is the workspace root, which owns the one library every
    // member shares.
    let root = proj_lock_root()?;

    let opts = ProjSyncOptions {
        dev: !args.get_flag("no-dev"),
        groups: args
            .get_many::<String>("group")
            .map(|vs| vs.cloned().collect())
            .unwrap_or_default(),
        all_groups: args.get_flag("all-groups"),
        extras: args
            .get_many::<String>("extra")
            .map(|vs| vs.cloned().collect())
            .unwrap_or_default(),
        all_extras: args.get_flag("all-extras"),
        install_r: !args.get_flag("no-install-r"),
        install_project: !args.get_flag("no-install-project"),
        max_concurrent: args.get_one::<usize>("max-concurrent").copied(),
        r_version: args.get_one::<String>("r-version").cloned(),
        platform: args.get_one::<String>("platform").cloned(),
        inexact: args.get_flag("inexact"),
        frozen: args.get_flag("frozen"),
        dry_run: args.get_flag("dry-run"),
    };

    proj_sync(&root, &opts, args)
}

/// Which of a lock target's packages `rig proj sync` should install: `main`
/// always, plus `dev` unless `opts.dev` is off, plus whatever `opts.groups`
/// names, plus every group if `opts.all_groups`; extras work the same way
/// through `opts.extras`/`opts.all_extras`, but none are installed unless
/// asked for. Explicit selection (`--group dev`, `--all-groups`) wins over
/// `--no-dev`, since that flag only skips the automatic `dev` insert below,
/// it never removes a name that got in some other way.
fn sync_wanted_packages(
    packages: &[RprojLockPackage],
    opts: &ProjSyncOptions,
) -> Vec<RprojLockPackage> {
    let mut wanted_groups: HashSet<String> = HashSet::from(["main".to_string()]);
    if opts.all_groups {
        wanted_groups.extend(packages.iter().flat_map(|p| p.groups.iter().cloned()));
    } else {
        if opts.dev {
            wanted_groups.insert("dev".to_string());
        }
        wanted_groups.extend(opts.groups.iter().cloned());
    }
    let wanted_extras: HashSet<String> = if opts.all_extras {
        packages
            .iter()
            .flat_map(|p| p.extra_groups.iter().cloned())
            .collect()
    } else {
        opts.extras.iter().cloned().collect()
    };
    let wanted: Vec<RprojLockPackage> = packages
        .iter()
        .filter(|p| {
            p.groups.iter().any(|g| wanted_groups.contains(g))
                || p.extra_groups.iter().any(|g| wanted_extras.contains(g))
        })
        .cloned()
        .collect();

    // `--no-install-project` (`opts.install_project == false`) drops the
    // project's own package from the wanted set -- but only if nothing else
    // still wanted actually needs it. `self_alias` exists precisely so
    // another package (or the project's own optional dependency graph) can
    // depend on the project by name, and installing it *as a dependency* is
    // not the same thing the flag opts out of.
    if opts.install_project {
        return wanted;
    }
    let still_needed: HashSet<String> = wanted
        .iter()
        .filter(|p| !p.is_project)
        .flat_map(|p| p.dependencies.iter().cloned())
        .collect();
    wanted
        .into_iter()
        .filter(|p| !p.is_project || still_needed.contains(&p.package))
        .collect()
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
        if opts.frozen {
            let msg = format!(
                "No {} found, run `rig proj lock` first (without --frozen)",
                RPROJ_LOCK_FILE
            );
            OUTPUT.error(&msg);
            error!("{}", msg);
            bail!("{}", msg);
        }
        if opts.dry_run {
            OUTPUT.info(&format!(
                "No {}, `rig proj sync` would run `rig proj lock` first",
                RPROJ_LOCK_FILE
            ));
            return Ok(());
        }
        OUTPUT.info(&format!(
            "No {}, running `rig proj lock` first",
            RPROJ_LOCK_FILE
        ));
        info!("No {}, running `rig proj lock` first", RPROJ_LOCK_FILE);
        proj_lock(root, &ProjLockOptions::default(), args)?;
    }

    let lock_content = fs::read_to_string(&lock_path)?;
    RprojLock::check_version(&lock_content)?;
    let lock: RprojLock = toml::from_str(&lock_content)?;
    let target = select_sync_target(
        &lock.targets,
        opts.r_version.as_deref(),
        opts.platform.as_deref(),
    )?;

    let wanted: Vec<RprojLockPackage> = sync_wanted_packages(&target.packages, opts);
    let wanted: &[RprojLockPackage] = &wanted;

    // The project's own package needs a real `DESCRIPTION` on disk: rig's
    // local-path installer always points `R CMD INSTALL` at a directory
    // containing one (see `src/pkgsource/local.rs`), but `rproj.toml` is
    // the only thing rig otherwise keeps in sync automatically. So, right
    // before installing it, (re)write `DESCRIPTION` from the current
    // manifest -- as if a plain `rig proj export` had just run -- so it never
    // drifts out of sync with `rproj.toml` between syncs. Not `--force`: a
    // `DESCRIPTION` that isn't rig's own (no `Config/rig/note`, see
    // `description_is_rig_generated`) is left alone, same as `rig proj
    // export` without `--force` would refuse to touch it.
    if wanted.iter().any(|p| p.is_project) {
        let description_path = root.join("DESCRIPTION");
        if opts.dry_run {
            OUTPUT.info(&format!("Would write {}", description_path.display()));
        } else if description_path.exists() && !description_is_rig_generated(&description_path) {
            OUTPUT.warn(&format!(
                "{} already exists and was not generated by rig, leaving it as is \
                 -- run `rig proj export --force` to replace it",
                description_path.display()
            ));
        } else {
            let manifest = proj_read_manifest(root)?;
            write_description_to(&description_path, &manifest)?;
        }
    }

    // The project library itself is created below, for a project `rig proj
    // init` has already set up (there is an `rproj.toml`) -- but nothing
    // else here reads `.Renviron`/`.rvenvlib`, they only matter for R
    // started directly, so fill them in if missing rather than failing.
    ensure_rvenv_files(root)?;
    let library_path = project_library(root)?;

    // `rig proj init` does not create the project library, this is where it
    // comes from. Create it now rather than just before the installs: an
    // up-to-date project with nothing to install still gets a sync stamp
    // written into it. Skipped under `--dry-run`, which touches nothing.
    if !opts.dry_run {
        fs::create_dir_all(&library_path)?;
    }

    // Everything below installs against the R version the lock file was
    // solved for, so resolve (and, unless --no-install-r or --dry-run,
    // install) it before touching the library: installed R packages are tied
    // to the R minor version, so the R on `PATH` is not good enough.
    // The architecture comes from the lock file's platform, not from the
    // machine: a lock file solved for macos-x86_64 needs an x86_64 R even on
    // an arm64 Mac.
    let r_arch = target_r_arch(&target.platform);
    let (r_name, r_binary) = rvenv_r_installation(
        &target.r_version,
        &r_arch,
        opts.install_r && !opts.dry_run,
        opts.dry_run,
    )?;

    {
        // When the library is centralized, leave a compatibility symlink at
        // its default in-project location, `.rvenv/lib`, pointing at the real
        // (centralized) library -- mirroring uv's `.venv` junction for its
        // own `centralized-project-envs` feature. Anything that still
        // expects a real `.rvenv/lib` (manual inspection, other tools) keeps
        // working; recreated on every sync like the rest of `.rvenv`.
        if !opts.dry_run && crate::utils::get_proj_library_root()?.is_some() {
            link_library_compat_symlink(&project_library_in_tree(root), &library_path)?;
        }
        // The base of the shared tools library, `__tools` alongside it (see
        // `RvenvCfg::tools_lib`). Resolved here, once, rather than by the
        // shim at R startup: `rig run`'s wrapper sets `R_LIBS_USER` to the
        // project library before R starts, so by the time R gets to read
        // its own default user library it is already gone.
        let (tools_main, _) = get_library_path(&r_name, true)?;
        let cfg = RvenvCfg {
            r_version: r_name.clone(),
            r_minor: minor_r_version(&target.r_version)?,
            r_binary: r_binary.clone(),
            platform: target.platform.clone(),
            r_arch: rvenv_r_arch(&r_name),
            rig_version: env!("CARGO_PKG_VERSION").to_string(),
            tools_lib: tools_main.join("__tools"),
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
                    library_path.display()
                );
                OUTPUT.warn(&msg);
                info!("{}", msg);
            }
        }

        // `--frozen` installs from the lockfile alone, whose packages already
        // carry full download URLs, so a missing manifest here is not fatal:
        // just skip the repositories file instead of failing.
        let manifest = if opts.frozen {
            proj_read_manifest_opt(root)?
        } else {
            Some(proj_read_manifest(root)?)
        };
        // One library, one set of repositories to fill it from, so the
        // workspace root's `[[repository]]` is the workspace's. A member that
        // declares its own -- `rig proj import` writes them from a
        // DESCRIPTION -- is warned about rather than rejected, so that
        // importing a package into a workspace still works.
        if let Some(manifest) = &manifest {
            if let Some(ws) = &manifest.workspace {
                for member in workspace_members(root, ws)? {
                    if member == root {
                        continue;
                    }
                    let has_own = proj_read_manifest_opt(&member)?
                        .map(|m| !m.repository.is_empty())
                        .unwrap_or(false);
                    if has_own {
                        let msg = format!(
                            "Ignoring the repositories of workspace member {}, a \
                             workspace uses the ones in its root {}",
                            member.display(),
                            RPROJ_MANIFEST_FILE
                        );
                        OUTPUT.warn(&msg);
                        info!("{}", msg);
                    }
                }
            }
        } else {
            let msg = format!(
                "No {} found, skipping repository setup",
                RPROJ_MANIFEST_FILE
            );
            OUTPUT.warn(&msg);
            info!("{}", msg);
        }
        let repos: &[Repository] = manifest
            .as_ref()
            .map(|m| m.repository.as_slice())
            .unwrap_or(&[]);
        if opts.dry_run {
            OUTPUT.info(&format!(
                "Would refresh the project environment for R {} ({})",
                r_name,
                r_binary.display()
            ));
        } else {
            let written = rvenv_sync(root, &cfg, repos)?;
            if written.is_empty() {
                info!(
                    "Project environment for R {} ({}) is already up to date",
                    r_name,
                    r_binary.display()
                );
            } else {
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
        }
    }

    // A package already in the library, at the version and provenance the
    // lockfile asks for, does not need to be downloaded or reinstalled.
    // Under `--dry-run` the library may not exist yet (its creation above was
    // skipped too), which just means nothing is installed.
    let already_installed = if library_path.exists() {
        read_installed(&library_path)?
    } else {
        vec![]
    };

    // A package in the library that is not wanted any more (dropped from
    // `rproj.toml`, or left over from before `--no-dev`) is removed by
    // default, the same as `uv sync`; `--inexact` leaves it alone. The base
    // packages are never touched, even under prune, since they are not
    // something a lockfile ever lists in the first place.
    if !opts.inexact {
        let wanted_names: HashSet<&str> = wanted.iter().map(|p| p.package.as_str()).collect();
        let extras: Vec<&InstalledPackage> = already_installed
            .iter()
            .filter(|p| {
                !wanted_names.contains(p.package.as_str())
                    && !BASE_PKGS.contains(&p.package.as_str())
            })
            .collect();
        if !extras.is_empty() {
            let names = extras
                .iter()
                .map(|p| format!("{} ({})", p.package, p.version))
                .collect::<Vec<_>>()
                .join(", ");
            let word = if extras.len() == 1 {
                "package"
            } else {
                "packages"
            };
            if opts.dry_run {
                OUTPUT.info(&format!(
                    "Would remove {} {} no longer in {}: {}",
                    extras.len(),
                    word,
                    RPROJ_LOCK_FILE,
                    names
                ));
            } else {
                let mut removed: Vec<&InstalledPackage> = vec![];
                let mut failed: Vec<String> = vec![];
                for extra in &extras {
                    match remove_package(&extra.path) {
                        Ok(()) => removed.push(extra),
                        Err(err) => {
                            OUTPUT.error(&err);
                            failed.push(extra.package.clone());
                        }
                    }
                }
                if !removed.is_empty() {
                    let names = removed
                        .iter()
                        .map(|p| format!("{} ({})", p.package, p.version))
                        .collect::<Vec<_>>()
                        .join(", ");
                    OUTPUT.success(&format!(
                        "Removed {} {} no longer in {}: {}",
                        removed.len(),
                        word,
                        RPROJ_LOCK_FILE,
                        names
                    ));
                    info!("Removed {} from {}", names, library_path.display());
                }
                if !failed.is_empty() {
                    bail!("Failed to remove {}", failed.join(", "));
                }
            }
        }
    }

    let already_installed: Vec<InstalledPackage> = if library_path.exists() {
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

    // The `rvenv` package in `.rvenvlib` compares this stamp to
    // `rproj.lock` and warns in every R session while they differ, so it has
    // to be updated even when there was nothing to install.
    if todo.is_empty() {
        if !opts.dry_run {
            write_sync_stamp(&library_path, &lock_path)?;
        }
        OUTPUT.success(&format!(
            "Everything is up to date in {}",
            library_path.display()
        ));
        info!("Nothing to install in {}", library_path.display());
        return Ok(());
    }

    if opts.dry_run {
        OUTPUT.info(&format!(
            "Would install {} of {} packages to {}",
            todo.len(),
            wanted.len(),
            library_path.display()
        ));
        return Ok(());
    }

    // Download only the packages that are actually going to be installed
    OUTPUT.status("Downloading packages");
    info!("Downloading packages");
    let to_download: Vec<RprojLockPackage> = todo.iter().map(|p| (*p).clone()).collect();
    download_lockfile_packages(&to_download)?;

    // Get cache directory where packages were downloaded
    let cache_dir = get_cache_dir()?;

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

    let max_concurrent = match opts.max_concurrent {
        Some(n) => n,
        None => crate::utils::get_concurrent_installs()?,
    };

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

    write_sync_stamp(&library_path, &lock_path)?;

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
    let mut remote: HashMap<String, String> = HashMap::new();
    for field in REMOTE_GIT_FIELDS {
        if let Some(value) = pkg.metadata.get(*field) {
            remote.insert(field.to_string(), value.clone());
        }
    }
    // A local package is never fetched and never cached: it is installed from
    // where it already is, which `RemoteUrl` holds as an absolute path.
    let local = pkg.metadata.get(REMOTE_TYPE_FIELD).map(|t| t.as_str()) == Some("local");
    // A git/GitHub package's `target` is the fetched directory (a tarball
    // unpacked, or a git checkout); a subdirectory source lives at
    // `<target>/<subdir>` within it. An ordinary CRAN/PPM package's `target`
    // is the downloaded file itself.
    let file_path = if local {
        PathBuf::from(
            pkg.metadata
                .get(crate::install::REMOTE_URL_FIELD)
                .cloned()
                .unwrap_or_default(),
        )
    } else {
        let base = cache_dir.join("packages").join(&pkg.target);
        match pkg.metadata.get(REMOTE_SUBDIR_FIELD) {
            Some(subdir) => base.join(subdir),
            None => base,
        }
    };
    let mut info = PackageInfo {
        name: pkg.package.clone(),
        version: pkg.version.clone(),
        binary: pkg.binary,
        file_path,
        dependencies: pkg.dependencies.clone(),
        hash: pkg.metadata.get(REMOTE_HASH_FIELD).cloned(),
        linkingto: pkg
            .metadata
            .get(REMOTE_LINKINGTO_FIELD)
            .map(|s| parse_linkingto(s))
            .unwrap_or_default(),
        built: None,
        remote,
    };
    // The build cache is keyed on the source's content hash -- a stat digest
    // (path/size/mtime, not content) for a local directory, a real content
    // sha256 for a local file (see `read_local_package`) -- recorded as
    // `RemoteSha` in `info.remote` just like a git/GitHub/url source's.
    if !info.binary && (!local || info.remote.contains_key(REMOTE_SHA_FIELD)) {
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

    // A git/GitHub/url package's `target` is a directory, fetched by
    // checking out a git worktree or downloading and extracting an archive,
    // not a plain HTTP download to a file -- handled separately, see
    // `fetch_git_lockfile_packages`.
    let (git_packages, http_packages): (Vec<&RprojLockPackage>, Vec<&RprojLockPackage>) = packages
        .iter()
        .partition(|pkg| pkg.metadata.contains_key(REMOTE_TYPE_FIELD));

    fetch_git_lockfile_packages(&git_packages, &cache_dir)?;
    download_http_lockfile_packages(&http_packages, &cache_dir)
}

/// Fetch every git/GitHub/url package in `packages` into its cache
/// directory: a shallow (`--depth 1`), sparse-checkout-scoped `git` fetch
/// (see [`crate::pkgsource::git::fetch_git_checkout`]) for a git/GitHub
/// source, or a cached archive download and extraction (see
/// [`crate::pkgsource::url::fetch_url_checkout`]) for a `url` source, both
/// reusing the `RemoteUrl`/`RemoteRef`/`RemoteSubdir`/`RemoteSha` recorded at
/// lock time. Skipped entirely when the target directory already exists --
/// the target is keyed by the resolved commit sha (or archive sha256), so an
/// existing one is always the right content.
fn fetch_git_lockfile_packages(
    packages: &[&RprojLockPackage],
    cache_dir: &Path,
) -> Result<(), Box<dyn Error>> {
    for pkg in packages {
        // A local source is already on disk, where the user pointed rig at
        // it, so there is nothing to fetch and nothing to cache.
        if pkg.metadata.get(REMOTE_TYPE_FIELD).map(|s| s.as_str()) == Some("local") {
            continue;
        }
        let target_dir = cache_dir.join("packages").join(&pkg.target);
        if target_dir.exists() {
            OUTPUT.success(&format!(
                "{} is cached at {}",
                pkg.package,
                target_dir.display()
            ));
            continue;
        }
        create_parent_dir_if_needed(&target_dir)?;

        match pkg.metadata.get(REMOTE_TYPE_FIELD).map(|s| s.as_str()) {
            Some("github") | Some("git") => {
                let url = pkg
                    .metadata
                    .get(crate::install::REMOTE_URL_FIELD)
                    .ok_or_else(|| SimpleError::new(format!("{} has no RemoteUrl", pkg.package)))?;
                let refspec = pkg.metadata.get(crate::install::REMOTE_REF_FIELD).cloned();
                let subdir = pkg
                    .metadata
                    .get(crate::install::REMOTE_SUBDIR_FIELD)
                    .cloned();
                OUTPUT.status(&format!("Fetching {} from {}", pkg.package, url));
                crate::pkgsource::git::fetch_git_checkout(
                    url,
                    refspec.as_deref(),
                    subdir.as_deref(),
                    &target_dir,
                )?;
            }
            Some("url") => {
                let url = pkg
                    .metadata
                    .get(crate::install::REMOTE_URL_FIELD)
                    .ok_or_else(|| SimpleError::new(format!("{} has no RemoteUrl", pkg.package)))?;
                let expected_sha256 = pkg.metadata.get(crate::install::REMOTE_SHA_FIELD).cloned();
                OUTPUT.status(&format!("Fetching {} from {}", pkg.package, url));
                crate::pkgsource::url::fetch_url_checkout(
                    url,
                    expected_sha256.as_deref(),
                    &target_dir,
                )?;
            }
            other => bail!(
                "{} has an unknown RemoteType `{}`",
                pkg.package,
                other.unwrap_or("<none>")
            ),
        }
        OUTPUT.success(&format!("Fetched {}", pkg.package));
    }
    Ok(())
}

fn download_http_lockfile_packages(
    packages: &[&RprojLockPackage],
    cache_dir: &Path,
) -> Result<(), Box<dyn Error>> {
    // Build download list: (sources, target_path) for each package
    let mut downloads: Vec<(Vec<String>, PathBuf)> = Vec::new();
    for pkg in packages {
        let target_path = cache_dir.join("packages").join(&pkg.target);
        create_parent_dir_if_needed(&target_path)?;
        downloads.push((pkg.sources.clone(), target_path));
    }

    let total = downloads.len();
    if total == 0 {
        return Ok(());
    }

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
    )?;

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
    use crate::dcf::RDepType;
    use crate::rproj::{Dependency, Group, Workspace};
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
            groups: vec![],
            extra_groups: vec![],
            is_project: false,
        }
    }

    fn dep(version: &str) -> Dependency {
        Dependency::Version(version.to_string())
    }

    /// A [`locked`] fixture tagged with the given `groups`/`extra_groups`,
    /// for [`sync_wanted_packages`] tests.
    fn locked_in(name: &str, groups: &[&str], extra_groups: &[&str]) -> RprojLockPackage {
        let mut pkg = locked(name, &[]);
        pkg.groups = groups.iter().map(|g| g.to_string()).collect();
        pkg.extra_groups = extra_groups.iter().map(|g| g.to_string()).collect();
        pkg
    }

    fn sync_opts() -> ProjSyncOptions {
        ProjSyncOptions::default()
    }

    fn names(packages: &[RprojLockPackage]) -> Vec<String> {
        let mut names: Vec<String> = packages.iter().map(|p| p.package.clone()).collect();
        names.sort();
        names
    }

    #[test]
    fn sync_wanted_packages_default_is_main_and_dev_only() {
        let packages = vec![
            locked_in("cli", &["main"], &[]),
            locked_in("devtools", &["dev"], &[]),
            locked_in("testthat", &["test"], &[]),
            locked_in("ggplot2", &[], &["viz"]),
        ];
        let wanted = sync_wanted_packages(&packages, &sync_opts());
        assert_eq!(names(&wanted), vec!["cli", "devtools"]);
    }

    #[test]
    fn sync_wanted_packages_no_dev_keeps_main_only() {
        let packages = vec![
            locked_in("cli", &["main"], &[]),
            locked_in("devtools", &["dev"], &[]),
        ];
        let opts = ProjSyncOptions {
            dev: false,
            ..sync_opts()
        };
        assert_eq!(names(&sync_wanted_packages(&packages, &opts)), vec!["cli"]);
    }

    #[test]
    fn sync_wanted_packages_group_adds_to_the_default_set() {
        let packages = vec![
            locked_in("cli", &["main"], &[]),
            locked_in("devtools", &["dev"], &[]),
            locked_in("testthat", &["test"], &[]),
        ];
        let opts = ProjSyncOptions {
            groups: vec!["test".to_string()],
            ..sync_opts()
        };
        assert_eq!(
            names(&sync_wanted_packages(&packages, &opts)),
            vec!["cli", "devtools", "testthat"]
        );
    }

    #[test]
    fn sync_wanted_packages_no_dev_with_explicit_group_dev_still_installs_dev() {
        let packages = vec![
            locked_in("cli", &["main"], &[]),
            locked_in("devtools", &["dev"], &[]),
        ];
        let opts = ProjSyncOptions {
            dev: false,
            groups: vec!["dev".to_string()],
            ..sync_opts()
        };
        assert_eq!(
            names(&sync_wanted_packages(&packages, &opts)),
            vec!["cli", "devtools"]
        );
    }

    #[test]
    fn sync_wanted_packages_all_groups_installs_every_group() {
        let packages = vec![
            locked_in("cli", &["main"], &[]),
            locked_in("devtools", &["dev"], &[]),
            locked_in("pkgdown", &["docs"], &[]),
            locked_in("ggplot2", &[], &["viz"]),
        ];
        let opts = ProjSyncOptions {
            all_groups: true,
            ..sync_opts()
        };
        assert_eq!(
            names(&sync_wanted_packages(&packages, &opts)),
            vec!["cli", "devtools", "pkgdown"]
        );
    }

    #[test]
    fn sync_wanted_packages_extra_installs_only_that_extra() {
        let packages = vec![
            locked_in("cli", &["main"], &[]),
            locked_in("ggplot2", &[], &["viz"]),
            locked_in("dbi", &[], &["db"]),
        ];
        let opts = ProjSyncOptions {
            extras: vec!["viz".to_string()],
            ..sync_opts()
        };
        assert_eq!(
            names(&sync_wanted_packages(&packages, &opts)),
            vec!["cli", "ggplot2"]
        );
    }

    #[test]
    fn sync_wanted_packages_all_extras_installs_every_extra_but_no_extra_groups() {
        let packages = vec![
            locked_in("cli", &["main"], &[]),
            locked_in("devtools", &["dev"], &[]),
            locked_in("pkgdown", &["docs"], &[]),
            locked_in("ggplot2", &[], &["viz"]),
            locked_in("dbi", &[], &["db"]),
        ];
        let opts = ProjSyncOptions {
            all_extras: true,
            ..sync_opts()
        };
        assert_eq!(
            names(&sync_wanted_packages(&packages, &opts)),
            vec!["cli", "dbi", "devtools", "ggplot2"]
        );
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
    fn add_platform_extends_the_default_platform_set() {
        let opts = ProjLockOptions {
            add_platforms: vec!["ubuntu-24.04".to_string()],
            ..Default::default()
        };
        assert_eq!(
            lock_platform_specs(&opts),
            vec![
                None,
                Some("x86_64-w64-mingw32".to_string()),
                Some("x86_64-unknown-linux-gnu".to_string()),
                Some("aarch64-apple-darwin".to_string()),
                Some("ubuntu-24.04".to_string()),
            ]
        );
    }

    #[test]
    fn add_platform_extends_an_explicit_platform_list() {
        let opts = ProjLockOptions {
            platforms: vec!["macos".to_string()],
            add_platforms: vec!["windows".to_string(), "linux".to_string()],
            ..Default::default()
        };
        assert_eq!(
            lock_platform_specs(&opts),
            vec![
                Some("macos".to_string()),
                Some("windows".to_string()),
                Some("linux".to_string()),
            ]
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
            direct_dependencies: vec![],
            packages: vec![],
        }
    }

    /// A direct dependency requirement, e.g. for [`lock_target_satisfies`]
    /// tests: `direct_dep("dplyr", ">= 1.0")`.
    fn direct_dep(name: &str, constraint: &str) -> DepVersionSpec {
        DepVersionSpec {
            name: name.to_string(),
            types: vec![],
            constraints: crate::rproj::parse_constraints(constraint).unwrap(),
        }
    }

    /// A [`RprojLockPackage`] fixture with a version other than `locked`'s
    /// hard-coded `"1.0.0"`, for [`lock_target_satisfies`] tests.
    fn locked_version(name: &str, version: &str, deps: &[&str]) -> RprojLockPackage {
        let mut pkg = locked(name, deps);
        pkg.version = version.to_string();
        pkg
    }

    /// A git/GitHub-sourced [`RprojLockPackage`] fixture, for
    /// [`lock_target_satisfies`]/[`lock_target_git_sources_fresh`] tests.
    fn locked_git(name: &str, url: &str, sha: &str) -> RprojLockPackage {
        let mut pkg = locked(name, &[]);
        pkg.metadata
            .insert(REMOTE_TYPE_FIELD.to_string(), "git".to_string());
        pkg.metadata.insert(
            crate::install::REMOTE_URL_FIELD.to_string(),
            url.to_string(),
        );
        pkg.metadata.insert(
            crate::install::REMOTE_SHA_FIELD.to_string(),
            sha.to_string(),
        );
        pkg
    }

    fn resolved_git_source(name: &str, url: &str, sha: &str) -> ResolvedGitSource {
        ResolvedGitSource {
            name: name.to_string(),
            version: RegistryPackageVersion::new(name, "1.0.0").unwrap(),
            ranges: HashMap::default(),
            git_source: GitSourceInfo {
                remote_type: "git",
                url: url.to_string(),
                host: None,
                repo: None,
                username: None,
                subdir: None,
                ref_: None,
                sha: sha.to_string(),
                binary: false,
            },
        }
    }

    #[test]
    fn existing_release_refs_reads_the_pinned_tag_and_sha() {
        let mut pkg = locked_git("mypkg", "https://github.com/me/mypkg.git", "abc123");
        pkg.metadata.insert(
            crate::install::REMOTE_REF_FIELD.to_string(),
            "v1.2.0".to_string(),
        );
        let mut t = target("4.6.1", "testos");
        t.packages = vec![pkg];
        let lock = RprojLock {
            version: RPROJ_LOCK_VERSION,
            targets: vec![t],
        };
        let refs = existing_release_refs(&lock);
        assert_eq!(
            refs.get(&("https://github.com/me/mypkg.git".to_string(), None)),
            Some(&("v1.2.0".to_string(), "abc123".to_string()))
        );
    }

    #[test]
    fn lock_target_satisfies_an_unchanged_manifest() {
        let mut t = target("4.6.1", "testos");
        t.packages = vec![locked_version("dplyr", "1.1.0", &[])];
        t.direct_dependencies = vec![LockDirectDependency {
            name: "dplyr".to_string(),
            constraint: ">= 1.0.0".to_string(),
        }];
        let direct_deps = vec![direct_dep("dplyr", ">= 1.0.0")];
        assert!(lock_target_satisfies(&t, &direct_deps));
    }

    #[test]
    fn lock_target_does_not_satisfy_a_tightened_constraint() {
        let mut t = target("4.6.1", "testos");
        t.packages = vec![locked_version("dplyr", "1.1.0", &[])];
        t.direct_dependencies = vec![LockDirectDependency {
            name: "dplyr".to_string(),
            constraint: ">= 1.0.0".to_string(),
        }];
        // The manifest now asks for something newer than what's pinned.
        let direct_deps = vec![direct_dep("dplyr", ">= 2.0.0")];
        assert!(!lock_target_satisfies(&t, &direct_deps));
    }

    #[test]
    fn lock_target_does_not_satisfy_a_newly_added_dependency() {
        let mut t = target("4.6.1", "testos");
        t.packages = vec![locked_version("dplyr", "1.1.0", &[])];
        t.direct_dependencies = vec![LockDirectDependency {
            name: "dplyr".to_string(),
            constraint: "*".to_string(),
        }];
        // `tidyr` was just added to rproj.toml and has no fingerprint entry.
        let direct_deps = vec![direct_dep("dplyr", "*"), direct_dep("tidyr", "*")];
        assert!(!lock_target_satisfies(&t, &direct_deps));
    }

    #[test]
    fn lock_target_does_not_satisfy_a_removed_dependency() {
        let mut t = target("4.6.1", "testos");
        t.packages = vec![
            locked_version("dplyr", "1.1.0", &[]),
            locked_version("tidyr", "1.3.0", &[]),
        ];
        t.direct_dependencies = vec![
            LockDirectDependency {
                name: "dplyr".to_string(),
                constraint: "*".to_string(),
            },
            LockDirectDependency {
                name: "tidyr".to_string(),
                constraint: "*".to_string(),
            },
        ];
        // `tidyr` was just removed from rproj.toml.
        let direct_deps = vec![direct_dep("dplyr", "*")];
        assert!(!lock_target_satisfies(&t, &direct_deps));
    }

    #[test]
    fn lock_target_satisfies_skips_the_numeric_check_for_a_git_dependency() {
        let mut t = target("4.6.1", "testos");
        t.packages = vec![locked_git(
            "mypkg",
            "https://github.com/me/mypkg.git",
            "abc123",
        )];
        t.direct_dependencies = vec![LockDirectDependency {
            name: "mypkg".to_string(),
            constraint: "*".to_string(),
        }];
        let direct_deps = vec![direct_dep("mypkg", "*")];
        assert!(lock_target_satisfies(&t, &direct_deps));
    }

    #[test]
    fn git_sources_are_fresh_when_unchanged() {
        let mut t = target("4.6.1", "testos");
        t.packages = vec![locked_git(
            "mypkg",
            "https://github.com/me/mypkg.git",
            "abc123",
        )];
        let sources = vec![resolved_git_source(
            "mypkg",
            "https://github.com/me/mypkg.git",
            "abc123",
        )];
        assert!(lock_target_git_sources_fresh(&t, &sources));
    }

    #[test]
    fn git_sources_are_stale_when_the_resolved_commit_changed() {
        let mut t = target("4.6.1", "testos");
        t.packages = vec![locked_git(
            "mypkg",
            "https://github.com/me/mypkg.git",
            "abc123",
        )];
        // The branch moved since the lock was last written.
        let sources = vec![resolved_git_source(
            "mypkg",
            "https://github.com/me/mypkg.git",
            "def456",
        )];
        assert!(!lock_target_git_sources_fresh(&t, &sources));
    }

    #[test]
    fn select_sync_target_is_a_noop_with_a_single_target() {
        let this_os = this_os_family();
        let platform = format!("{}-{}", this_os, std::env::consts::ARCH);
        let targets = vec![target("4.6.1", &platform)];
        let picked = select_sync_target(&targets, None, None).unwrap();
        assert_eq!(picked.r_version, "4.6.1");
    }

    #[test]
    fn select_sync_target_ignores_foreign_os_targets() {
        let this_os = this_os_family();
        let other_os = if this_os == "linux" { "macos" } else { "linux" };
        let targets = vec![
            target("4.6.1", &format!("{}-{}", other_os, std::env::consts::ARCH)),
            target("4.5.0", &format!("{}-{}", this_os, std::env::consts::ARCH)),
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
    fn select_sync_target_ignores_foreign_arch_targets() {
        let this_os = this_os_family();
        let other_arch = if std::env::consts::ARCH == "x86_64" {
            "arm64"
        } else {
            "x86_64"
        };
        let targets = vec![
            target("4.6.1", &format!("{}-{}", this_os, other_arch)),
            target("4.5.0", &format!("{}-{}", this_os, std::env::consts::ARCH)),
        ];
        let picked = select_sync_target(&targets, None, None).unwrap();
        assert_eq!(picked.r_version, "4.5.0");
    }

    #[test]
    fn select_sync_target_picks_the_highest_r_version_among_matches() {
        let this_os = this_os_family();
        let platform = format!("{}-{}", this_os, std::env::consts::ARCH);
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
        let platform = format!("{}-{}", this_os, std::env::consts::ARCH);
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
    fn compute_package_groups_keeps_the_non_dev_closure_only() {
        let mut manifest = Rproj::minimal("mypkg");
        manifest.dependencies.insert("cli".to_string(), dep("*"));
        manifest.dependency_groups.insert(
            "dev".to_string(),
            Group {
                include_groups: vec![],
                dependencies: BTreeMap::from([("testthat".to_string(), dep("*"))]),
            },
        );

        let packages = vec![
            locked("cli", &["glue"]),
            locked("glue", &[]),
            locked("testthat", &["waldo", "glue"]),
            locked("waldo", &[]),
        ];

        let groups = compute_package_groups(&manifest.dependency_group_roots().unwrap(), &packages);
        // `glue` is a dev dependency too, but a non-dev one pulls it in
        assert_eq!(groups.get("cli").unwrap(), &vec!["main".to_string()]);
        assert_eq!(
            groups.get("glue").unwrap(),
            &vec!["dev".to_string(), "main".to_string()]
        );
        assert_eq!(groups.get("testthat").unwrap(), &vec!["dev".to_string()]);
        assert_eq!(groups.get("waldo").unwrap(), &vec!["dev".to_string()]);
    }

    #[test]
    fn compute_package_groups_extras_are_tracked_separately_from_groups() {
        let mut manifest = Rproj::minimal("mypkg");
        manifest.dependencies.insert("cli".to_string(), dep("*"));
        manifest.optional_dependencies.insert(
            "viz".to_string(),
            BTreeMap::from([("ggplot2".to_string(), dep("*"))]),
        );

        let packages = vec![locked("cli", &[]), locked("ggplot2", &[])];

        let groups = compute_package_groups(&manifest.main_and_group_roots().unwrap(), &packages);
        assert_eq!(groups.get("cli").unwrap(), &vec!["main".to_string()]);
        assert_eq!(groups.get("ggplot2"), None);

        let extra_groups = compute_package_groups(&manifest.optional_dependency_roots(), &packages);
        assert_eq!(
            extra_groups.get("ggplot2").unwrap(),
            &vec!["viz".to_string()]
        );
        assert_eq!(extra_groups.get("cli"), None);
    }

    /// Write `manifest` into `dir`, creating it, as one project or one
    /// workspace member.
    fn write_manifest(dir: &Path, manifest: &Rproj) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(RPROJ_MANIFEST_FILE), manifest.to_toml().unwrap()).unwrap();
    }

    /// A workspace of two members, `a` and `b`, under `root`: `a` depends on
    /// the sibling `b` and on `cli`, `b` on `glue`. The root manifest is a
    /// member too, and depends on nothing of its own.
    fn two_member_workspace(root: &Path) {
        let mut ws_root = Rproj::minimal("ws");
        ws_root.workspace = Some(Workspace {
            members: vec!["packages/*".to_string()],
            ..Default::default()
        });
        write_manifest(root, &ws_root);

        let mut a = Rproj::minimal("a");
        a.dependencies.insert("b".to_string(), dep("*"));
        a.dependencies.insert("cli".to_string(), dep("*"));
        write_manifest(&root.join("packages/a"), &a);

        let mut b = Rproj::minimal("b");
        b.dependencies.insert("glue".to_string(), dep("*"));
        write_manifest(&root.join("packages/b"), &b);
    }

    #[test]
    fn a_workspace_has_one_solve_root_per_member() {
        let dir = tempfile::tempdir().unwrap();
        two_member_workspace(dir.path());
        let solve = proj_read_solve_roots(dir.path()).unwrap();
        let names: Vec<&str> = solve.roots.iter().map(|r| r.name.as_str()).collect();
        // The workspace root is a member of its own workspace.
        assert_eq!(names, vec!["ws", "a", "b"]);
        assert_eq!(
            solve.members,
            vec![
                dir.path().to_path_buf(),
                dir.path().join("packages/a"),
                dir.path().join("packages/b"),
            ]
        );
    }

    #[test]
    fn a_plain_project_has_one_synthetic_solve_root() {
        let dir = tempfile::tempdir().unwrap();
        write_manifest(dir.path(), &Rproj::minimal("mypkg"));
        let solve = proj_read_solve_roots(dir.path()).unwrap();
        assert_eq!(solve.roots.len(), 1);
        assert_eq!(solve.roots[0].name, PROJECT_ROOT_PKG);
        // `Rproj::minimal` marks the project `type = "project"`, not a
        // package, so there is nothing for another dependency to resolve to.
        assert!(solve.self_alias.is_none());
    }

    /// A [`SolveRoot`] fixture for [`project_lock_package`]/
    /// [`project_entry_fresh`] tests: same shape `self_alias` is built with in
    /// [`proj_read_solve_roots`].
    fn self_alias(name: &str, version: &str, deps: &[&str]) -> SolveRoot {
        SolveRoot {
            name: name.to_string(),
            version: RPackageVersion::from_str(version).unwrap(),
            deps: PackageDependencies {
                dependencies: deps
                    .iter()
                    .map(|d| {
                        let mut dep = direct_dep(d, "*");
                        dep.types = vec![RDepType::Imports];
                        dep
                    })
                    .collect(),
            },
        }
    }

    #[test]
    fn project_lock_package_records_name_version_path_and_sha() {
        let alias = self_alias("mypkg", "1.2.0", &["cli", "rlang"]);
        let root = Path::new("/tmp/mypkg");
        let pkg = project_lock_package(&alias, root, Some("abc123"));

        assert_eq!(pkg.package, "mypkg");
        assert_eq!(pkg.version, "1.2.0");
        assert!(pkg.is_project);
        assert_eq!(names(std::slice::from_ref(&pkg)), vec!["mypkg"]);
        assert_eq!(
            pkg.dependencies.iter().collect::<HashSet<_>>(),
            HashSet::from([&"cli".to_string(), &"rlang".to_string()])
        );
        assert_eq!(pkg.groups, vec!["main".to_string()]);
        assert!(pkg.extra_groups.is_empty());
        assert!(pkg.sources.is_empty());
        assert!(pkg.target.is_empty());
        assert_eq!(
            pkg.metadata.get(REMOTE_TYPE_FIELD),
            Some(&"local".to_string())
        );
        assert_eq!(
            pkg.metadata.get(crate::install::REMOTE_URL_FIELD),
            Some(&root.display().to_string())
        );
        assert_eq!(
            pkg.metadata.get(crate::install::REMOTE_SHA_FIELD),
            Some(&"abc123".to_string())
        );
    }

    #[test]
    fn project_lock_package_drops_r_and_base_packages_from_dependencies() {
        let alias = self_alias("mypkg", "1.0.0", &["R", "methods", "cli"]);
        let pkg = project_lock_package(&alias, Path::new("/tmp/mypkg"), None);
        assert_eq!(pkg.dependencies, vec!["cli".to_string()]);
        assert!(!pkg.metadata.contains_key(crate::install::REMOTE_SHA_FIELD));
    }

    #[test]
    fn project_entry_fresh_with_no_self_alias_always_passes() {
        let t = target("4.6.1", "testos");
        assert!(project_entry_fresh(&t, None, None));
    }

    #[test]
    fn project_entry_fresh_requires_a_project_entry_to_exist() {
        let alias = self_alias("mypkg", "1.0.0", &[]);
        let t = target("4.6.1", "testos");
        assert!(!project_entry_fresh(
            &t,
            Some(&alias),
            Some(&"abc".to_string())
        ));
    }

    #[test]
    fn project_entry_fresh_detects_a_changed_digest() {
        let alias = self_alias("mypkg", "1.0.0", &[]);
        let mut t = target("4.6.1", "testos");
        t.packages = vec![project_lock_package(
            &alias,
            Path::new("/tmp/mypkg"),
            Some("old-sha"),
        )];
        assert!(!project_entry_fresh(
            &t,
            Some(&alias),
            Some(&"new-sha".to_string())
        ));
        assert!(project_entry_fresh(
            &t,
            Some(&alias),
            Some(&"old-sha".to_string())
        ));
    }

    #[test]
    fn project_entry_fresh_detects_a_version_bump() {
        let alias = self_alias("mypkg", "1.0.0", &[]);
        let mut t = target("4.6.1", "testos");
        t.packages = vec![project_lock_package(
            &alias,
            Path::new("/tmp/mypkg"),
            Some("sha"),
        )];
        let bumped = self_alias("mypkg", "2.0.0", &[]);
        assert!(!project_entry_fresh(
            &t,
            Some(&bumped),
            Some(&"sha".to_string())
        ));
    }

    #[test]
    fn sync_wanted_packages_no_install_project_drops_the_project_package() {
        let mut project = locked_in("mypkg", &["main"], &[]);
        project.is_project = true;
        let packages = vec![locked_in("cli", &["main"], &[]), project];
        let opts = ProjSyncOptions {
            install_project: false,
            ..sync_opts()
        };
        assert_eq!(names(&sync_wanted_packages(&packages, &opts)), vec!["cli"]);
    }

    #[test]
    fn sync_wanted_packages_no_install_project_keeps_it_if_still_a_dependency() {
        let mut project = locked_in("mypkg", &["main"], &[]);
        project.is_project = true;
        let mut dependent = locked_in("otherpkg", &["main"], &[]);
        dependent.dependencies = vec!["mypkg".to_string()];
        let packages = vec![dependent, project];
        let opts = ProjSyncOptions {
            install_project: false,
            ..sync_opts()
        };
        assert_eq!(
            names(&sync_wanted_packages(&packages, &opts)),
            vec!["mypkg", "otherpkg"]
        );
    }

    #[test]
    fn a_plain_package_project_gets_a_self_alias() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Rproj::minimal("rlang");
        manifest.project.type_ = Some("package".to_string());
        manifest.project.version = "1.0.1".to_string();
        write_manifest(dir.path(), &manifest);
        let solve = proj_read_solve_roots(dir.path()).unwrap();
        let alias = solve
            .self_alias
            .expect("package project should get a self-alias root");
        assert_eq!(alias.name, "rlang");
        assert_eq!(alias.version, RPackageVersion::from_str("1.0.1").unwrap());
    }

    #[test]
    fn a_workspace_has_no_self_alias() {
        let dir = tempfile::tempdir().unwrap();
        two_member_workspace(dir.path());
        let solve = proj_read_solve_roots(dir.path()).unwrap();
        assert!(solve.self_alias.is_none());
    }

    #[test]
    fn two_members_of_one_name_are_an_error() {
        let dir = tempfile::tempdir().unwrap();
        two_member_workspace(dir.path());
        let mut clash = Rproj::minimal("a");
        clash.project.version = "9.9.9".to_string();
        write_manifest(&dir.path().join("packages/also-a"), &clash);
        let err = proj_read_solve_roots(dir.path()).unwrap_err().to_string();
        assert!(err.contains("Two workspace members"), "{}", err);
        assert!(err.contains("packages"), "{}", err);
        assert!(err.contains("also-a"), "{}", err);
    }

    #[test]
    fn a_member_cannot_be_called_r_or_a_base_package() {
        let dir = tempfile::tempdir().unwrap();
        two_member_workspace(dir.path());
        write_manifest(&dir.path().join("packages/stats"), &Rproj::minimal("stats"));
        let err = proj_read_solve_roots(dir.path()).unwrap_err().to_string();
        assert!(err.contains("stats"), "{}", err);
    }

    #[test]
    fn the_merged_r_requirement_intersects_the_members() {
        let dir = tempfile::tempdir().unwrap();
        two_member_workspace(dir.path());
        let mut b = Rproj::minimal("b");
        b.dependencies.insert("R".to_string(), dep(">= 4.4, < 4.6"));
        write_manifest(&dir.path().join("packages/b"), &b);

        let solve = proj_read_solve_roots(dir.path()).unwrap();
        let r = solve
            .merged
            .dependencies
            .iter()
            .find(|d| d.name == "R")
            .unwrap();
        // `>= 4.1` from the other members and `>= 4.4, < 4.6` from `b`: the
        // solve has to satisfy all of them at once.
        assert!(r.satisfies("4.5.0").unwrap());
        assert!(!r.satisfies("4.6.1").unwrap());
        assert!(!r.satisfies("4.3.0").unwrap());
    }

    #[test]
    fn compute_package_groups_unions_every_members_direct_deps() {
        let dir = tempfile::tempdir().unwrap();
        two_member_workspace(dir.path());

        // `b` is a member, so it is not a lockfile entry: the walk has to get
        // to `glue` from `b`'s own manifest, not by following `a` -> `b`.
        let packages = vec![locked("cli", &[]), locked("glue", &[])];
        let solve = proj_read_solve_roots(dir.path()).unwrap();
        let groups = compute_package_groups(&solve.group_roots, &packages);
        assert_eq!(groups.get("cli").unwrap(), &vec!["main".to_string()]);
        assert_eq!(groups.get("glue").unwrap(), &vec!["main".to_string()]);
    }

    #[test]
    fn a_local_directory_gets_a_stat_digest() {
        let dir = tempfile::tempdir().unwrap();
        let pkg_dir = dir.path().join("mypkg");
        std::fs::create_dir(&pkg_dir).unwrap();
        std::fs::write(
            pkg_dir.join("DESCRIPTION"),
            "Package: mypkg\nVersion: 1.0.0\n",
        )
        .unwrap();

        let (_pkg, source, _remotes) = read_local_package(&pkg_dir).unwrap();
        assert_ne!(source.sha, "");
        assert_eq!(
            source.sha,
            compute_dir_stat_digest(&pkg_dir, false).unwrap()
        );
    }

    #[test]
    fn an_unchanged_directory_has_a_stable_digest() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("DESCRIPTION"), "Package: mypkg\n").unwrap();
        std::fs::create_dir(dir.path().join("R")).unwrap();
        std::fs::write(dir.path().join("R/foo.R"), "foo <- function() 1\n").unwrap();

        let first = compute_dir_stat_digest(dir.path(), false).unwrap();
        let second = compute_dir_stat_digest(dir.path(), false).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn touching_a_files_mtime_changes_the_digest() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("DESCRIPTION");
        std::fs::write(&file, "Package: mypkg\n").unwrap();
        let before = compute_dir_stat_digest(dir.path(), false).unwrap();

        let newer =
            filetime::FileTime::from_unix_time(filetime::FileTime::now().unix_seconds() + 3600, 0);
        filetime::set_file_mtime(&file, newer).unwrap();

        let after = compute_dir_stat_digest(dir.path(), false).unwrap();
        assert_ne!(before, after);
    }

    #[test]
    fn adding_or_removing_a_file_changes_the_digest() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("DESCRIPTION"), "Package: mypkg\n").unwrap();
        let before = compute_dir_stat_digest(dir.path(), false).unwrap();

        let extra = dir.path().join("NEWS.md");
        std::fs::write(&extra, "# mypkg 1.0.0\n").unwrap();
        let with_extra = compute_dir_stat_digest(dir.path(), false).unwrap();
        assert_ne!(before, with_extra);

        std::fs::remove_file(&extra).unwrap();
        let after_removal = compute_dir_stat_digest(dir.path(), false).unwrap();
        assert_eq!(before, after_removal);
    }

    #[test]
    fn rbuildignore_excludes_matching_files_from_the_digest() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("DESCRIPTION"), "Package: mypkg\n").unwrap();
        std::fs::write(dir.path().join(".Rbuildignore"), "^ignored\\.txt$\n").unwrap();
        std::fs::write(dir.path().join("ignored.txt"), "v1").unwrap();

        let before = compute_dir_stat_digest(dir.path(), false).unwrap();
        std::fs::write(dir.path().join("ignored.txt"), "a very different value").unwrap();
        let after = compute_dir_stat_digest(dir.path(), false).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn dot_git_changes_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("DESCRIPTION"), "Package: mypkg\n").unwrap();
        let before = compute_dir_stat_digest(dir.path(), false).unwrap();

        let git_dir = dir.path().join(".git");
        std::fs::create_dir(&git_dir).unwrap();
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();

        let after = compute_dir_stat_digest(dir.path(), false).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn rvenv_and_the_lock_file_are_ignored_at_the_root() {
        // Both are rig's own output, rewritten by every `rig proj lock`/
        // `sync` -- hashing either would make a package project's own digest
        // (see `ProjectSolve::self_alias`) change just from having run.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("DESCRIPTION"), "Package: mypkg\n").unwrap();
        let before = compute_dir_stat_digest(dir.path(), false).unwrap();

        std::fs::write(dir.path().join(RPROJ_LOCK_FILE), "version = 1\n").unwrap();
        let rvenv_dir = dir.path().join(RVENV_DIR);
        std::fs::create_dir(&rvenv_dir).unwrap();
        std::fs::write(rvenv_dir.join("rvenv.cfg"), "r_version = \"4.5.0\"\n").unwrap();

        let after = compute_dir_stat_digest(dir.path(), false).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn skip_description_ignores_the_root_description_only_when_asked() {
        // `rig proj sync` rewrites the project's own `DESCRIPTION` from
        // `rproj.toml` on every sync (`write_description_to`), so hashing it
        // for the project's own `is_project` entry would make `rig proj
        // lock` see a "changed" project after every sync that touched
        // nothing else. An ordinary `path` dependency's `DESCRIPTION`, in
        // contrast, is real content and must still be hashed.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("DESCRIPTION"), "Package: mypkg\n").unwrap();
        let before_skip = compute_dir_stat_digest(dir.path(), true).unwrap();
        let before_hash = compute_dir_stat_digest(dir.path(), false).unwrap();

        std::fs::write(
            dir.path().join("DESCRIPTION"),
            "Package: mypkg\nImports: cli\n",
        )
        .unwrap();

        let after_skip = compute_dir_stat_digest(dir.path(), true).unwrap();
        let after_hash = compute_dir_stat_digest(dir.path(), false).unwrap();
        assert_eq!(before_skip, after_skip);
        assert_ne!(before_hash, after_hash);
    }

    #[test]
    fn a_local_file_is_hashed() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("mypkg_1.0.0.tar.gz");
        {
            let file = std::fs::File::create(&archive).unwrap();
            let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
            let mut ar = tar::Builder::new(enc);
            let contents: &[u8] = b"Package: mypkg\nVersion: 1.0.0\n";
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            ar.append_data(&mut header, "mypkg/DESCRIPTION", contents)
                .unwrap();
            ar.finish().unwrap();
        }

        let (_pkg, source, _remotes) = read_local_package(&archive).unwrap();
        assert_eq!(
            source.sha,
            crate::utils::calculate_file_hash(&archive).unwrap()
        );
        assert_ne!(source.sha, "");
    }
}
