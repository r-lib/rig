use std::collections::HashMap;
use std::error::Error;
use std::fs::File;

use clap::ArgMatches;
use deb822_fast::Deb822;
use pubgrub::{resolve, SelectedDependencies};
use simple_error::*;
use simplelog::info;
use tabular::*;

use crate::common::get_default_r_version;
use crate::dcf::*;
use crate::repos::*;
use crate::solver::*;

pub fn sc_proj(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        Some(("deps", s)) => sc_proj_deps(s, args, mainargs),
        Some(("solve", s)) => sc_proj_solve(s, args, mainargs),
        _ => Ok(()), // unreachable
    }
}

fn proj_read_deps() -> Result<Vec<DepVersionSpec>, Box<dyn Error>> {
    info!("Reading dependencies from DESCRIPTION file");
    let df: File = File::open("DESCRIPTION")?;
    let desc = Deb822::from_reader(df)?;

    if desc.len() == 0 {
        bail!("Empty DESCRIPTION file");
    }

    if desc.len() > 1 {
        bail!("Invalid DESCRIPTION file, empty lines are not allowed");
    }

    let mut deps: Vec<DepVersionSpec> = vec![];

    for desc0 in desc.iter() {
        if let Some(dd) = desc0.get("Depends") {
            deps.append(&mut parse_deps(dd, "Depends")?)
        }
        if let Some(di) = desc0.get("Imports") {
            deps.append(&mut parse_deps(di, "Imports")?)
        }
        if let Some(dl) = desc0.get("LinkingTo") {
            deps.append(&mut parse_deps(dl, "LinkingTo")?);
        }
        // if let Some(ds) = desc0.get("Suggests") {
        //     deps.append(&mut parse_deps(ds)?);
        // }
        // if let Some(de) = desc0.get("Enhances") {
        //     deps.append(&mut parse_deps(de)?);
        // }
    }

    let deps = simplify_constraints(deps);

    Ok(deps)
}

/// Parse dependencies from DESCRIPTION file and print them out
fn sc_proj_deps(
    args: &ArgMatches,
    _libargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let deps: Vec<DepVersionSpec> = proj_read_deps()?;

    if args.get_flag("json") || mainargs.get_flag("json") {
        println!("[");
        let num = deps.len();
        for (i, pkg) in deps.iter().enumerate() {
            let mut cst: String = "".to_string();
            for (i, cs) in pkg.constraints.iter().enumerate() {
                if i > 0 {
                    cst += ", ";
                }
                cst += &format!("{} {}", cs.0, cs.1);
            }
            println!(" {{");
            let comma = if cst == "" { "" } else { ", " };
            // TODO: should this be an array? Probably
            println!("     \"types\": \"{}\",", pkg.types.join(", "));
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
                cst += &format!("{} {}", cs.0, cs.1);
            }
            tab.add_row(row!(pkg.name, cst, pkg.types.join(", ")));
        }

        print!("{}", tab);
    }

    Ok(())
}

fn sc_proj_solve_latest(
    r_version: &str,
    deps: &Vec<DepVersionSpec>,
) -> Result<SelectedDependencies<RPackageRegistry>, Box<dyn Error>> {
    info!("Solver with latest package versions");
    let pkgs = repos_get_packages()?;
    let reg: RPackageRegistry = RPackageRegistry::default();

    info!("Adding {} packages to the registry", pkgs.len());
    for pkg in pkgs.iter() {
        reg.add_package_version(
            pkg.name.clone(),
            RPackageVersion::from_str(&pkg.version)?,
            rpackage_version_ranges_from_constraints(&pkg.dependencies),
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
    let base_pkgs = vec![
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
    for bp in base_pkgs.iter() {
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
        Ok(sol) => Ok(sol),
        Err(e) => bail!("Solution failed with latest package versions: {}", e),
    }
}

fn sc_proj_solve_all(
    r_version: &str,
    deps: &Vec<DepVersionSpec>,
) -> Result<SelectedDependencies<RPackageRegistry>, Box<dyn Error>> {
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
    let base_pkgs = vec![
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
    for bp in base_pkgs.iter() {
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
        Ok(sol) => Ok(sol),
        Err(e) => bail!("Solution failed with all package versions: {}", e),
    }
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
    let deps: Vec<DepVersionSpec> = proj_read_deps()?;

    // try latest version first
    let mut solution;
    let try1 = sc_proj_solve_latest(&rver, &deps);
    match try1 {
        Ok(sol) => {
            solution = sol;
            info!("Solved using latest package versions");
        }
        Err(err) => {
            print!("Failed: {:?}", err);
            let try2 = sc_proj_solve_all(&rver, &deps);
            match try2 {
                Ok(sol) => {
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
    let mut tab: Table = Table::new("{:<}   {:<}");
    tab.add_row(row!["package", "version"]);
    tab.add_heading("-------------------------");
    for (pkg, ver) in solution.iter() {
        tab.add_row(row!(pkg, ver));
    }
    println!("{}", tab);

    Ok(())
}
