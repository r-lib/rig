use std::env;
use std::error::Error;

use clap::ArgMatches;
use simple_error::*;
use tabular::*;

use crate::common::*;
use crate::hardcoded::*;
use crate::repositories::*;

#[cfg(target_os = "macos")]
use crate::macos::*;

#[cfg(target_os = "windows")]
use crate::windows::*;

#[cfg(target_os = "linux")]
use crate::linux::*;

mod config;
pub use config::{get_repos_config, RepoEntry, Repository};
mod interpret_repos_args;
pub use interpret_repos_args::interpret_repos_args;
mod repos_available;
use repos_available::sc_repos_available;
mod repos_list;
use repos_list::sc_repos_list;
pub mod cranlike_metadata;
pub use cranlike_metadata::repos_get_packages;
mod setup;
pub use setup::repos_setup;
mod crandb;
pub use crandb::get_all_cran_package_versions;

pub fn sc_repos(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        // Some(("add", s)) => sc_repos_add(s, args, mainargs),
        Some(("available", s)) => sc_repos_available(s, args, mainargs),
        // Some(("disable", s)) => sc_repos_disable(s, args, mainargs),
        // Some(("enable", s)) => sc_repos_enable(s, args, mainargs),
        Some(("list", s)) => sc_repos_list(s, args, mainargs),
        Some(("package-list", s)) => sc_repos_package_list(s, args, mainargs),
        Some(("package-info", s)) => sc_repos_package_info(s, args, mainargs),
        Some(("package-versions", s)) => sc_repos_package_versions(s, args, mainargs),
        // Some(("reset", s)) => sc_repos_reset(s, args, mainargs),
        // Some(("rm", s)) => sc_repos_rm(s, args, mainargs),
        Some(("setup", s)) => sc_repos_setup(s, args, mainargs),
        _ => Ok(()), // unreachable
    }
}

pub fn r_version_to_bioc_version(rver: &str) -> Result<String, Box<dyn Error>> {
    match env::var("R_BIOC_VERSION") {
        Ok(biocver) => Ok(biocver),
        Err(_) => {
            let minor = rver.split('.').take(2).collect::<Vec<&str>>().join(".");
            match HC_R_VERSION_TO_BIOC_VERSION.get(&minor) {
                Some(biocver) => Ok(biocver.to_string()),
                None => {
                    bail!(
                        "Cannot determine Bioconductor version for R version {}, \n\
                        set R_BIOC_VERSION environment variable to override.",
                        rver
                    );
                }
            }
        }
    }
}

// pub fn sc_repos_add(
//     args: &ArgMatches,
//     _libargs: &ArgMatches,
//     _mainargs: &ArgMatches,
// ) -> Result<(), Box<dyn Error>> {
//     panic!("TODO: implement sc_repos_add");
//     Ok(())
// }

// pub fn sc_repos_disable(
//     args: &ArgMatches,
//     _libargs: &ArgMatches,
//     _mainargs: &ArgMatches,
// ) -> Result<(), Box<dyn Error>> {
//     panic!("TODO: implement sc_repos_disable");
//     Ok(())
// }

// pub fn sc_repos_enable(
//     args: &ArgMatches,
//     _libargs: &ArgMatches,
//     _mainargs: &ArgMatches,
// ) -> Result<(), Box<dyn Error>> {
//     panic!("TODO: implement sc_repos_enable");
//     Ok(())
// }

// pub fn sc_repos_reset(
//     args: &ArgMatches,
//     _libargs: &ArgMatches,
//     _mainargs: &ArgMatches,
// ) -> Result<(), Box<dyn Error>> {
//     panic!("TODO: implement sc_repos_reset");
//     Ok(())
// }

// pub fn sc_repos_rm(
//     args: &ArgMatches,
//     _libargs: &ArgMatches,
//     _mainargs: &ArgMatches,
// ) -> Result<(), Box<dyn Error>> {
//     panic!("TODO: implement sc_repos_rm");
//     Ok(())
// }

fn sc_repos_setup(
    args: &ArgMatches,
    _libargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let vers: Vec<String> = if args.contains_id("r-version") {
        vec![args.get_one::<String>("r-version").unwrap().to_string()]
    } else {
        sc_get_list()?
    };

    let setup = interpret_repos_args(args, false);
    repos_setup(Some(vers), setup)
}

fn sc_repos_package_list(
    args: &ArgMatches,
    _libargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let platform = if args.contains_id("platform") {
        crate::platform::parse_platform_string(
            &args.get_one::<String>("platform").unwrap().to_string(),
        )?
    } else {
        crate::platform::detect_platform()?
    };
    let r_version = if args.contains_id("r-version") {
        args.get_one::<String>("r-version").unwrap().to_string()
    } else {
        get_default_r_version()?.ok_or("Cannot determine default R version")?
    };
    let pkg_type = if args.contains_id("pkg-type") {
        match crate::platform::resolve_package_type_synonyms(
            &platform,
            &r_version,
            &args.get_one::<String>("pkg-type").unwrap().to_string(),
        ) {
            Some(pt) => pt,
            None => "source".to_string(),
        }
    } else {
        "source".to_string()
    };
    let packages = repos_get_packages("https://cloud.r-project.org", &pkg_type, &r_version)?;

    if args.get_flag("json") || mainargs.get_flag("json") {
        // TODO
    } else {
        let mut tab: Table = Table::new("{:<}   {:<}   {:<}");
        tab.add_row(row!("Package", "Version", "Dependencies"));
        tab.add_heading("------------------------------------------------------------------------");
        for pkg in packages.iter() {
            let deps_str: String = pkg
                .dependencies
                .dependencies
                .iter()
                .map(|x| format!("{}", x))
                .collect::<Vec<String>>()
                .join(", ");
            tab.add_row(row!(&pkg.name, &pkg.version, deps_str));
        }

        print!("{}", tab);
    }

    Ok(())
}

fn sc_repos_package_info(
    args: &ArgMatches,
    _libargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let package: String = args.get_one::<String>("package").unwrap().to_string();
    let ver = if args.contains_id("version") {
        args.get_one::<String>("version").unwrap().to_string()
    } else {
        "latest".to_string()
    };

    let info = crandb::get_cran_package_version(&package, &ver)?;
    if args.get_flag("json") {
        let json = serde_json::to_string_pretty(&info)?;
        println!("{}", json);
    } else {
        let mut tab: Table = Table::new("{:<}   {:<}");
        tab.add_row(row!("Field", "Value"));
        tab.add_heading("------------------------------------------------------------------------");
        for (k, v) in info.iter() {
            tab.add_row(row!(k, v));
        }
        print!("{}", tab);
    }

    Ok(())
}

fn sc_repos_package_versions(
    args: &ArgMatches,
    _libargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let package: String = args.get_one::<String>("package").unwrap().to_string();

    let mut rows = crandb::get_all_cran_package_versions(&package, None)?;
    rows.sort_by(|a, b| a.version.cmp(&b.version));

    let mut tab: Table = Table::new("{:<}   {:<}   {:<}");
    tab.add_row(row!("Package", "Version", "Dependencies"));
    tab.add_heading("------------------------------------------------------------------------");
    for row in rows {
        let deps_str: String = row
            .dependencies
            .dependencies
            .iter()
            .map(|x| format!("{}", x))
            .collect::<Vec<String>>()
            .join(", ");

        tab.add_row(row!(&row.name, &row.version, &deps_str));
    }

    print!("{}", tab);

    Ok(())
}
