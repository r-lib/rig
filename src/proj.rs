use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::ArgMatches;
use deb822_fast::Deb822;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use log::{error, info};
use pubgrub::{resolve, SelectedDependencies};
use simple_error::*;
use tabular::*;

use crate::built::BuiltCache;
use crate::cache::get_cache_dir;
use crate::common::{get_arch, get_default_r_version, get_platform};
use crate::dcf::*;
use crate::download::download_multiple_first_available_with_progress;
use crate::install::{
    install_packages, parse_linkingto, PackageInfo, REMOTE_HASH_FIELD, REMOTE_LINKINGTO_FIELD,
};
use crate::output::OUTPUT;
use crate::pak::{PakLockfile, PakLockfilePackage};
use crate::pkg::deps::{
    dep_count, print_deps_json, print_deps_recursive, print_header, type_list, walk_deps,
};
use crate::pkg::install::{plan_installs, print_plan};
use crate::pkg::list::read_installed;
use crate::pkg::tree::proj_tree;
use crate::platform::{detect_platform, parse_platform_string};
use crate::renv::*;
use crate::repos::binaries::loader::{BinaryTarget, P3mBinaryLoader};
use crate::repos::*;
use crate::resolve::resolve_versions;
use crate::rproj::{Rproj, RprojLock, RprojLockTarget, RPROJ_LOCK_VERSION, RPROJ_MANIFEST_FILE};
use crate::rvenv::{
    existing_targets, find_project_root, project_library, rvenv_init, write_sync_stamp,
    RPROJ_LOCK_FILE,
};
use crate::solver::*;
use crate::utils::create_parent_dir_if_needed;

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
        Some(("deps", s)) => sc_proj_deps(s, args, mainargs),
        Some(("tree", s)) => sc_proj_tree(s, args, mainargs),
        Some(("lock", s)) => sc_proj_lock(s, args, mainargs),
        Some(("sync", s)) => sc_proj_sync(s, args, mainargs),
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

/// Import a `DESCRIPTION` file's dependencies into `rproj.toml`, creating a
/// minimal manifest first if none exists yet.
fn sc_proj_import(
    args: &ArgMatches,
    _projargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let default_input = "DESCRIPTION".to_string();
    let input: &String = args.get_one::<String>("input").unwrap_or(&default_input);
    let pkg = proj_read_deps(input, true)?;

    let path = Path::new(RPROJ_MANIFEST_FILE);
    let mut manifest = if path.exists() {
        toml::from_str::<Rproj>(&fs::read_to_string(path)?)?
    } else {
        OUTPUT.status(&format!(
            "{} does not exist, creating a new one",
            RPROJ_MANIFEST_FILE
        ));
        info!("{} does not exist, creating a new one", RPROJ_MANIFEST_FILE);
        Rproj::minimal(&pkg.name)
    };

    let count = pkg.dependencies.dependencies.len();
    manifest.merge_description(&pkg);
    fs::write(path, toml::to_string_pretty(&manifest)?)?;

    OUTPUT.success(&format!(
        "Imported {} dependencies from {} into {}",
        count, input, RPROJ_MANIFEST_FILE
    ));
    info!(
        "Imported {} dependencies from {} into {}",
        count, input, RPROJ_MANIFEST_FILE
    );
    Ok(())
}

/// Read the project's manifest, e.g. its `DESCRIPTION` file, and return it as a
/// package, with the soft dependencies dropped unless `dev`.
fn proj_read_deps(input: &str, dev: bool) -> Result<Package, Box<dyn Error>> {
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

    // only one paragraph
    let mut package = Package::from_dcf_paragraph(desc.iter().next().unwrap())?;

    // Filter out Suggests and Enhances if dev is false. A package that is also
    // a hard dependency stays: it needs to be installed either way.
    if !dev {
        package
            .dependencies
            .dependencies
            .retain(|dep| !dep.types.iter().all(|t| DEP_TYPES_SOFT.contains(t)));
    }

    package.dependencies.simplify();

    Ok(package)
}

/// Read the project's `rproj.toml` manifest and return its name, version and
/// dependencies, with the soft dependencies dropped unless `dev`. The manifest
/// is read from `root`, the project directory.
fn proj_read_manifest_deps(
    root: &Path,
    dev: bool,
) -> Result<(String, RPackageVersion, PackageDependencies), Box<dyn Error>> {
    let path = root.join(RPROJ_MANIFEST_FILE);
    if !path.exists() {
        OUTPUT.error(&format!(
            "{} not found, run `rig proj init` first",
            RPROJ_MANIFEST_FILE
        ));
        error!("{} not found", RPROJ_MANIFEST_FILE);
        bail!("{} not found", RPROJ_MANIFEST_FILE);
    }

    OUTPUT.status(&format!(
        "Reading dependencies from {}",
        RPROJ_MANIFEST_FILE
    ));
    info!("Reading dependencies from {}", RPROJ_MANIFEST_FILE);
    let manifest: Rproj = toml::from_str(&fs::read_to_string(path)?).map_err(|e| {
        OUTPUT.error(&format!("Cannot parse {}: {}", RPROJ_MANIFEST_FILE, e));
        error!("Cannot parse {}: {}", RPROJ_MANIFEST_FILE, e);
        e
    })?;

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
    let platform = match platform {
        Some(p) if p == "source" => {
            info!("Solving for source packages only");
            return Ok(None);
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

    match &target {
        Some(target) => info!("Solving for binary target {}", target.name()),
        None => {
            let name = platform.rig_platform.as_deref().unwrap_or(&platform.os);
            OUTPUT.warn(&format!(
                "No binary packages for {}, using source packages",
                name
            ));
        }
    }
    Ok(target)
}

pub(crate) fn sc_proj_solve_deps(
    r_version: &str,
    deps: &PackageDependencies,
    target: Option<BinaryTarget>,
    prefer_binary: Option<usize>,
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
    OUTPUT.status("Downloading binary package metadata");
    let roots: Vec<String> = deps.dependencies.iter().map(|d| d.name.clone()).collect();
    reg.prefetch_binaries(&roots);

    OUTPUT.status("Solving dependencies");
    let solution = resolve(
        &reg,
        "_project".to_string(),
        RegistryPackageVersion::new("_project", "1.0.0")?,
    );

    match solution {
        Ok(sol) => Ok((reg, sol)),
        Err(e) => {
            OUTPUT.error(&format!("Solver failed: {}", e));
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
    r_version: Option<String>,
    platform: Option<String>,
    prefer_binary: Option<usize>,
    dev: bool,
    renv: bool,
}

impl Default for ProjLockOptions {
    fn default() -> Self {
        ProjLockOptions {
            r_version: None,
            platform: None,
            prefer_binary: None,
            // dev dependencies are included unless --no-dev is given
            dev: true,
            renv: false,
        }
    }
}

fn sc_proj_lock(
    args: &ArgMatches,
    _libargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let opts = ProjLockOptions {
        r_version: args.get_one::<String>("r-version").cloned(),
        platform: args.get_one::<String>("platform").cloned(),
        prefer_binary: args.get_one::<usize>("prefer-binary").copied(),
        dev: !args.get_flag("no-dev"),
        renv: args.get_flag("renv"),
    };
    proj_lock(Path::new("."), &opts)
}

/// Solve the dependencies of the project in `root` and write `rproj.lock`
/// (and `renv.lock` with `--renv`) into it.
fn proj_lock(root: &Path, opts: &ProjLockOptions) -> Result<(), Box<dyn Error>> {
    let rver = match &opts.r_version {
        Some(rv) => rv.to_string(),
        None => match get_default_r_version()? {
            Some(rv) => rv,
            None => {
                OUTPUT.error("Cannot determine R version, please specify it with --r-version.");
                error!("Cannot determine R version, please specify it with --r-version.");
                bail!("Cannot determine R version, please specify it with --r-version.")
            }
        },
    };

    // Do this first, to report local errors early
    let dev = opts.dev;
    let (_name, _version, mut pkg_deps) = proj_read_manifest_deps(root, dev)?;

    if opts.renv {
        pkg_deps.dependencies.push(DepVersionSpec {
            name: "renv".to_string(),
            constraints: vec![],
            types: vec![RDepType::Depends],
        });
    };

    let target = proj_binary_target(opts.platform.as_ref(), &rver)?;

    let prefer_binary = opts.prefer_binary;
    if prefer_binary.is_some() && target.is_none() {
        OUTPUT.warn("There are no binary packages to prefer, ignoring --prefer-binary");
        info!("Ignoring --prefer-binary: solving for source packages only");
    }

    // A single solver over the full CRAN version history: it picks the latest
    // in-range version of each package first and only falls back to older
    // versions when a constraint forces it, so the common case still resolves
    // to the latest versions. With `--prefer-binary` it also falls back to an
    // older version to get a binary package instead of a source one.
    let (registry, solution) = sc_proj_solve_deps(&rver, &pkg_deps, target, prefer_binary)?;
    OUTPUT.success("Solved dependencies");
    info!("Solved dependencies");

    if opts.renv {
        let renv = REnvLockfile::from_solution(&registry, &solution);
        fs::write(root.join("renv.lock"), serde_json::to_string_pretty(&renv)?)?;
        OUTPUT.success("Written renv lockfile to renv.lock");
        info!("Written renv lockfile to renv.lock");
    }

    // Single-target for now: one `(r_version, platform)` entry. The matrix
    // form (solving for several targets into one `rproj.lock`) is follow-up
    // work; `sc_proj_sync` already reads `targets[0]` unconditionally to
    // match.
    let lockfile = PakLockfile::from_solution(&registry, &solution);
    let rproj_lock = RprojLock {
        version: RPROJ_LOCK_VERSION,
        targets: vec![RprojLockTarget {
            r_version: lockfile.r_version.clone(),
            platform: lockfile.platform.clone(),
            packages: lockfile.packages.clone(),
        }],
    };
    fs::write(
        root.join(RPROJ_LOCK_FILE),
        toml::to_string_pretty(&rproj_lock)?,
    )?;
    OUTPUT.success("Written project lockfile to rproj.lock");
    info!("Written project lockfile to rproj.lock");

    let sorted_solution = solution_to_sorted_vec(&solution);
    let mut tab: Table = Table::new("{:<}   {:<}   {:<}   {:<}");
    tab.add_row(row!["package", "version", "type", ""]);
    tab.add_heading("-------------------------------------");
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
        let note = match registry.held_back_from(pkg, ver) {
            Some(latest) => {
                info!(
                    "Held {} back to {} for a binary package, latest is {}",
                    pkg, ver.version, latest
                );
                format!("held back for a binary package, latest is {}", latest)
            }
            None => String::new(),
        };
        tab.add_row(row!(pkg, &ver.version, kind, note));
    }
    println!("{}", tab);

    Ok(())
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

    // Read the lockfile to get package information. Single-target for now:
    // always the first (and, today, only) entry `rig proj lock` wrote; the
    // "pick the entry matching this machine, hard error if none match" logic
    // for a real multi-target `rproj.lock` is follow-up work.
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
        proj_lock(&root, &ProjLockOptions::default())?;
    }

    let lock_content = fs::read_to_string(&lock_path)?;
    let lock: RprojLock = toml::from_str(&lock_content)?;
    let target = lock.targets.first().ok_or("rproj.lock has no targets")?;

    // Library path: --library, or the project library by default. The
    // project library is created by `rig proj init`, together with the
    // `.gitignore` files that keep it in version control, so do not create
    // it here.
    let library_path = match args.get_one::<String>("library") {
        Some(lib) => PathBuf::from(lib),
        None => {
            let lib = project_library(&root);
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

    // A package already in the library, at the version and provenance the
    // lockfile asks for, does not need to be downloaded or reinstalled.
    let already_installed = if library_path.exists() {
        read_installed(&library_path)?
    } else {
        vec![]
    };
    let plan = plan_installs(&target.packages, &already_installed, false);
    print_plan(&format!("({})", library_path.display()), &plan);
    let todo: Vec<&PakLockfilePackage> = plan
        .iter()
        .filter(|p| p.install)
        .map(|p| p.package)
        .collect();

    // The shim package in the project library compares this stamp to
    // `rproj.lock` and warns in every R session while they differ, so it has
    // to be updated even when there was nothing to install.
    let stamp_lib = if library_path == project_library(&root) {
        Some(library_path.clone())
    } else {
        None
    };

    if todo.is_empty() {
        if let Some(lib) = &stamp_lib {
            write_sync_stamp(lib, &lock_path)?;
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
    let to_download: Vec<PakLockfilePackage> = todo.iter().map(|p| (*p).clone()).collect();
    download_lockfile_packages(&to_download)?;

    // Get cache directory where packages were downloaded
    let cache_dir = get_cache_dir()?;

    // Ensure library directory exists
    fs::create_dir_all(&library_path)?;

    // Get R binary path - use argument or default to "R"
    let r_binary = args
        .get_one::<String>("r-binary")
        .map(|s| s.as_str())
        .unwrap_or("R");

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

    // Set max concurrent installations
    let max_concurrent = args
        .get_one::<usize>("max-concurrent")
        .copied()
        .unwrap_or(8);

    let total_packages = packages.len();
    OUTPUT.status(&format!(
        "Installing {} of {} packages to {}",
        total_packages,
        target.packages.len(),
        library_path.display()
    ));
    info!(
        "Installing {} of {} packages to {}",
        total_packages,
        target.packages.len(),
        library_path.display()
    );

    let installed = install_packages(packages, &library_path, r_binary, max_concurrent)?;

    if let Some(lib) = &stamp_lib {
        write_sync_stamp(lib, &lock_path)?;
    }

    OUTPUT.success(&format!(
        "Deployment complete, installed {} packages",
        installed
    ));
    info!("Deployment complete, installed {} packages", installed);
    Ok(())
}

/// What to install for one lockfile entry, including the provenance
/// `PakLockfile::from_solution` recorded in its `metadata`.
///
/// `built` is the cache of packages rig compiled itself, and only a source
/// package has anything to do with it: a binary is already built, and rig has
/// nothing to add to it.
pub(crate) fn lockfile_package_info(
    pkg: &PakLockfilePackage,
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

/// Read `pkg.lock` (the pak-compatible JSON lockfile `rig pkg install`
/// writes/reads) and download everything it names. Used by the hidden `rig
/// test download-lockfile` diagnostic; unrelated to `rproj.lock` / `rig proj
/// sync`, which call [`download_lockfile_packages`] directly instead.
pub fn proj_download() -> Result<(), Box<dyn Error>> {
    let lockfile_content = fs::read_to_string("pkg.lock")?;
    let lockfile: PakLockfile = serde_json::from_str(&lockfile_content)?;
    download_lockfile_packages(&lockfile.packages)
}

/// Download every package a lockfile names into the package cache.
///
/// Takes a plain package slice, not a whole lockfile, so any caller with a
/// `Vec<PakLockfilePackage>` can use it directly — `rig pkg install`, which
/// solves in memory and never writes a lockfile, and `rig proj sync`, which
/// reads one target's packages out of `rproj.lock`.
pub(crate) fn download_lockfile_packages(
    packages: &[PakLockfilePackage],
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
