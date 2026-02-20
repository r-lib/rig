use std::collections::HashMap;
use std::error::Error;
use std::fs::{self, File};

use clap::ArgMatches;
use deb822_fast::Deb822;
use log::info;
use pubgrub::{resolve, SelectedDependencies};
use simple_error::*;
use tabular::*;

use crate::common::get_default_r_version;
use crate::dcf::*;
use crate::renv::*;
use crate::repos::*;
use crate::solver::*;

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
        _ => Ok(()), // unreachable
    }
}

fn proj_read_deps(input: &str, dev: bool) -> Result<Vec<DepVersionSpec>, Box<dyn Error>> {
    info!("Reading dependencies from {}", input);
    let df: File = File::open(input)?;
    let desc = Deb822::from_reader(df)?;

    if desc.len() == 0 {
        bail!("Empty DESCRIPTION file");
    }

    if desc.len() > 1 {
        bail!("Invalid DESCRIPTION file, empty lines are not allowed");
    }

    let mut deps: Vec<DepVersionSpec> = vec![];

    for desc0 in desc.iter() {
        let package = Package::from_dcf_paragraph(desc0)?;
        deps.extend(package.dependencies.dependencies);
    }

    // Filter out Suggests and Enhances if dev is false
    if !dev {
        deps.retain(|dep| {
            !dep.types.contains(&RDepType::Suggests)
                && !dep.types.contains(&RDepType::Enhances)
        });
    }

    let mut pkg_deps = PackageDependencies { dependencies: deps };
    pkg_deps.simplify();

    Ok(pkg_deps.dependencies)
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
    let mut deps: Vec<DepVersionSpec> = proj_read_deps(input, dev)?;

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
        let a_types = a.types.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(", ");
        let b_types = b.types.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(", ");
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
            let types_str = pkg.types.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(", ");
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
            let types_str = pkg.types.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(", ");
            tab.add_row(row!(pkg.name, cst, types_str));
        }

        print!("{}", tab);
    }

    Ok(())
}

fn sc_proj_solve_latest(
    r_version: &str,
    deps: &Vec<DepVersionSpec>,
) -> Result<(RPackageRegistry, SelectedDependencies<RPackageRegistry>), Box<dyn Error>> {
    info!("Solver with latest package versions");
    let pkgs = repos_get_packages()?;
    let reg: RPackageRegistry = RPackageRegistry::default();

    info!("Adding {} packages to the registry", pkgs.len());
    for pkg in pkgs.iter() {
        reg.add_package_version(
            pkg.name.clone(),
            pkg.version.clone(),
            rpackage_version_ranges_from_constraints(&pkg.dependencies.dependencies),
        );
    }

    reg.add_package_version(
        "_project".to_string(),
        RPackageVersion::from_str("1.0.0")?,
        rpackage_version_ranges_from_constraints(&deps),
    );

    // add R itself, for now a hardcoded version
    reg.add_package_version(
        "R".to_string(),
        RPackageVersion::from_str(r_version)?,
        HashMap::with_hasher(rustc_hash::FxBuildHasher::default()),
    );

    // add base packages, these are always available
    for bp in BASE_PKGS.iter() {
        reg.add_package_version(
            bp.to_string(),
            RPackageVersion::from_str(r_version)?,
            HashMap::with_hasher(rustc_hash::FxBuildHasher::default()),
        );
    }

    let solution = resolve(
        &reg,
        "_project".to_string(),
        RPackageVersion::from_str("1.0.0")?,
    );

    match solution {
        Ok(sol) => Ok((reg, sol)),
        Err(e) => bail!("Solution failed with latest package versions: {}", e),
    }
}

fn sc_proj_solve_all(
    r_version: &str,
    deps: &Vec<DepVersionSpec>,
) -> Result<(RPackageRegistry, SelectedDependencies<RPackageRegistry>), Box<dyn Error>> {
    info!("Solver with all package versions");
    let reg: RPackageRegistry = RPackageRegistry::default();

    reg.add_package_version(
        "_project".to_string(),
        RPackageVersion::from_str("1.0.0")?,
        rpackage_version_ranges_from_constraints(&deps),
    );

    // add R itself, for now a hardcoded version
    reg.add_package_version(
        "R".to_string(),
        RPackageVersion::from_str(r_version)?,
        HashMap::with_hasher(rustc_hash::FxBuildHasher::default()),
    );

    // add base packages, these are always available
    for bp in BASE_PKGS.iter() {
        reg.add_package_version(
            bp.to_string(),
            RPackageVersion::from_str(r_version)?,
            HashMap::with_hasher(rustc_hash::FxBuildHasher::default()),
        );
    }

    let solution = resolve(
        &reg,
        "_project".to_string(),
        RPackageVersion::from_str("1.0.0")?,
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
    let mut deps: Vec<DepVersionSpec> = proj_read_deps(input, dev)?;

    if args.get_flag("renv") {
        deps.push(DepVersionSpec {
            name: "renv".to_string(),
            constraints: vec![],
            types: vec![RDepType::Depends],
        });
    };

    // try latest version first
    let (registry, solution);
    let try1 = sc_proj_solve_latest(&rver, &deps);
    match try1 {
        Ok((reg, sol)) => {
            registry = reg;
            solution = sol;
            info!("Solved using latest package versions");
        }
        Err(err) => {
            print!("Failed: {:?}", err);
            let try2 = sc_proj_solve_all(&rver, &deps);
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
