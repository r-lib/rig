use std::collections::HashMap;
use std::error::Error;
use std::fs::{self, File};
use std::path::PathBuf;

use clap::ArgMatches;
use deb822_fast::Deb822;
use log::info;
use pubgrub::{resolve, SelectedDependencies};
use simple_error::*;
use tabular::*;

use crate::cache::get_cache_dir;
use crate::common::get_default_r_version;
use crate::dcf::*;
use crate::download::download_multiple_first_available_;
use crate::install::{install_package_tree, PackageInfo};
use crate::pak::PakLockfile;
use crate::renv::*;
use crate::repos::*;
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
        Some(("deps", s)) => sc_proj_deps(s, args, mainargs),
        Some(("solve", s)) => sc_proj_solve(s, args, mainargs),
        Some(("deploy", s)) => sc_proj_deploy(s, args, mainargs),
        _ => Ok(()), // unreachable
    }
}

fn proj_read_deps(input: &str, dev: bool) -> Result<PackageDependencies, Box<dyn Error>> {
    info!("Reading dependencies from {}", input);
    let df: File = File::open(input)?;
    let desc = Deb822::from_reader(df)?;

    if desc.len() == 0 {
        bail!("Empty DESCRIPTION file");
    }

    if desc.len() > 1 {
        bail!("Invalid DESCRIPTION file, empty lines are not allowed");
    }

    // only one paragraph
    let mut package = Package::from_dcf_paragraph(desc.iter().next().unwrap())?;

    // Filter out Suggests and Enhances if dev is false
    if !dev {
        package.dependencies.dependencies.retain(|dep| {
            !dep.types.contains(&RDepType::Suggests) && !dep.types.contains(&RDepType::Enhances)
        });
    }

    package.dependencies.simplify();

    Ok(package.dependencies)
}

/// Parse dependencies from DESCRIPTION file and print them out
fn sc_proj_deps(
    args: &ArgMatches,
    _libargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let dev = args.get_flag("dev");
    let default_input = "DESCRIPTION".to_string();
    let input: &String = args.get_one::<String>("input").unwrap_or(&default_input);
    let pkg_deps = proj_read_deps(input, dev)?;
    let mut deps = pkg_deps.dependencies;

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

    if args.get_flag("json") || mainargs.get_flag("json") {
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
            let comma = if cst == "" { "" } else { ", " };
            // TODO: should this be an array? Probably
            let types_str = pkg
                .types
                .iter()
                .map(|t| t.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            println!("     \"types\": \"{}\",", types_str);
            println!("     \"package\": \"{}\"{}", pkg.name, comma);
            if cst != "" {
                println!("     \"version\": \"{}\"", cst)
            }
            println!("  }}{}", if i == num - 1 { "" } else { "," });
        }
        println!("]");
    } else {
        let mut tab: Table = Table::new("{:<}   {:<}   {:<}");
        tab.add_row(row!["package", "constraints", "types"]);
        tab.add_heading("------------------------------------------");
        for pkg in deps {
            let mut cst: String = "".to_string();
            for (i, cs) in pkg.constraints.iter().enumerate() {
                if i > 0 {
                    cst += ", ";
                }
                cst += &format!("{} {}", cs.constraint_type, cs.version);
            }
            let types_str = pkg
                .types
                .iter()
                .map(|t| t.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            tab.add_row(row!(pkg.name, cst, types_str));
        }

        print!("{}", tab);
    }

    Ok(())
}

fn sc_proj_solve_latest(
    r_version: &str,
    deps: &PackageDependencies,
) -> Result<(RPackageRegistry, SelectedDependencies<RPackageRegistry>), Box<dyn Error>> {
    info!("Solver with latest package versions");
    let pkgs = repos_get_packages("https://cloud.r-project.org/", "source", "4.5.2")?;
    let reg: RPackageRegistry = RPackageRegistry::default();

    info!("Adding {} packages to the registry", pkgs.len());
    for pkg in pkgs.iter() {
        let v = RegistryPackageVersion {
            name: pkg.name.clone(),
            version: pkg.version.clone(),
        };
        reg.add_package_version(
            pkg.name.clone(),
            v,
            rpackage_version_ranges_from_constraints(&pkg.dependencies, false),
        );
    }

    reg.add_package_version(
        "_project".to_string(),
        RegistryPackageVersion::new("_project", "1.0.0")?,
        rpackage_version_ranges_from_constraints(deps, true),
    );

    // add R itself, for now a hardcoded version
    reg.add_package_version(
        "R".to_string(),
        RegistryPackageVersion::new("R", r_version)?,
        HashMap::with_hasher(rustc_hash::FxBuildHasher::default()),
    );

    // add base packages, these are always available
    for bp in BASE_PKGS.iter() {
        reg.add_package_version(
            bp.to_string(),
            RegistryPackageVersion::new(bp, r_version)?,
            HashMap::with_hasher(rustc_hash::FxBuildHasher::default()),
        );
    }

    let solution = resolve(
        &reg,
        "_project".to_string(),
        RegistryPackageVersion::new("_project", "1.0.0")?,
    );

    match solution {
        Ok(sol) => Ok((reg, sol)),
        Err(e) => bail!("Solution failed with latest package versions: {}", e),
    }
}

fn sc_proj_solve_all(
    r_version: &str,
    deps: &PackageDependencies,
) -> Result<(RPackageRegistry, SelectedDependencies<RPackageRegistry>), Box<dyn Error>> {
    info!("Solver with all package versions");
    let reg: RPackageRegistry = RPackageRegistry::default();

    reg.add_package_version(
        "_project".to_string(),
        RegistryPackageVersion::new("_project", "1.0.0")?,
        rpackage_version_ranges_from_constraints(deps, true),
    );

    // add R itself, for now a hardcoded version
    reg.add_package_version(
        "R".to_string(),
        RegistryPackageVersion::new("R", r_version)?,
        HashMap::with_hasher(rustc_hash::FxBuildHasher::default()),
    );

    // add base packages, these are always available
    for bp in BASE_PKGS.iter() {
        reg.add_package_version(
            bp.to_string(),
            RegistryPackageVersion::new(bp, r_version)?,
            HashMap::with_hasher(rustc_hash::FxBuildHasher::default()),
        );
    }

    let solution = resolve(
        &reg,
        "_project".to_string(),
        RegistryPackageVersion::new("_project", "1.0.0")?,
    );

    match solution {
        Ok(sol) => Ok((reg, sol)),
        Err(e) => bail!("Solution failed with all package versions: {}", e),
    }
}

fn solution_to_sorted_vec(
    solution: &SelectedDependencies<RPackageRegistry>,
) -> Vec<(String, RPackageVersion)> {
    let mut vec: Vec<(String, RPackageVersion)> = solution
        .iter()
        .filter(|(pkg, _ver)| *pkg != "_project")
        .map(|(pkg, ver)| (pkg.clone(), ver.version.clone()))
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

fn sc_proj_solve(
    args: &ArgMatches,
    _libargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let rver = if args.contains_id("r-version") {
        args.get_one::<String>("r-version").unwrap().to_string()
    } else {
        match get_default_r_version()? {
            Some(rv) => rv,
            None => bail!("Cannot determine R version, please specify it with --r-version."),
        }
    };

    // Do this first, to report local errors early
    let dev = args.get_flag("dev");
    let default_input = "DESCRIPTION".to_string();
    let input: &String = args.get_one::<String>("input").unwrap_or(&default_input);
    let mut pkg_deps = proj_read_deps(input, dev)?;

    if args.get_flag("renv") {
        pkg_deps.dependencies.push(DepVersionSpec {
            name: "renv".to_string(),
            constraints: vec![],
            types: vec![RDepType::Depends],
        });
    };

    // try latest version first
    let (registry, solution);
    let try1 = sc_proj_solve_latest(&rver, &pkg_deps);
    match try1 {
        Ok((reg, sol)) => {
            registry = reg;
            solution = sol;
            info!("Solved using latest package versions");
        }
        Err(err) => {
            print!("Failed: {:?}", err);
            let try2 = sc_proj_solve_all(&rver, &pkg_deps);
            match try2 {
                Ok((reg, sol)) => {
                    registry = reg;
                    solution = sol;
                    info!("Solved using all package versions");
                }
                Err(e) => {
                    bail!("Solver failed: {}", e);
                }
            }
        }
    };

    info!("Solution found:");

    if args.get_flag("renv") {
        let renv = REnvLockfile::from_solution(&registry, &solution);
        fs::write("renv.lock", serde_json::to_string_pretty(&renv)?)?;
        info!("Written renv lockfile to renv.lock");
    }

    let lockfile = PakLockfile::from_solution(&registry, &solution);
    fs::write("pkg.lock", serde_json::to_string_pretty(&lockfile)?)?;

    let sorted_solution = solution_to_sorted_vec(&solution);
    let mut tab: Table = Table::new("{:<}   {:<}");
    tab.add_row(row!["package", "version"]);
    tab.add_heading("-------------------------");
    for (pkg, ver) in sorted_solution.iter() {
        tab.add_row(row!(pkg, ver));
    }
    println!("{}", tab);

    Ok(())
}

fn sc_proj_deploy(
    args: &ArgMatches,
    _libargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    // First, download all packages
    info!("Downloading packages...");
    proj_download()?;

    // Read the lockfile to get package information
    let lockfile_content = fs::read_to_string("pkg.lock")?;
    let lockfile: PakLockfile = serde_json::from_str(&lockfile_content)?;

    // Get cache directory where packages were downloaded
    let cache_dir = get_cache_dir()?;

    // Build Vec<PackageInfo> from lockfile
    let mut packages: Vec<PackageInfo> = Vec::new();
    for pkg in &lockfile.packages {
        let file_path = cache_dir.join("packages").join(&pkg.target);
        packages.push(PackageInfo {
            name: pkg.package.clone(),
            file_path,
            dependencies: pkg.dependencies.clone(),
        });
    }

    // Get library path - required argument
    let library_path = PathBuf::from(
        args.get_one::<String>("library")
            .ok_or("--library argument is required")?,
    );

    // Ensure library directory exists
    fs::create_dir_all(&library_path)?;

    // Get R binary path - use argument or default to "R"
    let r_binary = args
        .get_one::<String>("r-binary")
        .map(|s| s.as_str())
        .unwrap_or("R");

    // Set max concurrent installations
    let max_concurrent = args
        .get_one::<usize>("max-concurrent")
        .copied()
        .unwrap_or(8);

    info!(
        "Installing {} packages to {}",
        packages.len(),
        library_path.display()
    );

    // Create a tokio runtime to run the async installation
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(install_package_tree(
        packages,
        &library_path,
        r_binary,
        max_concurrent,
    ))?;

    info!("Deployment complete!");
    Ok(())
}

fn proj_download() -> Result<(), Box<dyn Error>> {
    let lockfile_content = fs::read_to_string("pkg.lock")?;
    let lockfile: PakLockfile = serde_json::from_str(&lockfile_content)?;

    // Get cache directory
    let cache_dir = get_cache_dir()?;

    // Build download list: (sources, target_path) for each package
    let mut downloads: Vec<(Vec<String>, PathBuf)> = Vec::new();
    for pkg in &lockfile.packages {
        let target_path = cache_dir.join("packages").join(&pkg.target);
        create_parent_dir_if_needed(&target_path)?;
        downloads.push((pkg.sources.clone(), target_path));
    }

    // Download all packages concurrently
    info!("Downloading {} packages", downloads.len());
    let results = download_multiple_first_available_(downloads, None, None);

    // Check results and report errors
    let mut success_count = 0;
    for (i, result) in results.iter().enumerate() {
        match result {
            Ok(_) => {
                success_count += 1;
                info!("Downloaded: {}", lockfile.packages[i].package);
            }
            Err(e) => {
                bail!("Failed to download {}: {}", lockfile.packages[i].package, e);
            }
        }
    }

    info!("Download complete: {} packages downloaded", success_count);

    Ok(())
}
