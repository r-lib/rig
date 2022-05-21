
use std::error::Error;

use clap::ArgMatches;
use simple_error::bail;

#[cfg(target_os = "macos")]
use crate::macos::*;

#[cfg(target_os = "windows")]
use crate::windows::*;

#[cfg(target_os = "linux")]
use crate::linux::*;

pub fn check_installed(ver: &String) -> Result<bool, Box<dyn Error>> {
    let inst = sc_get_list()?;
    if ! inst.contains(&ver) {
        bail!("R version <b>{}</b> is not installed", &ver);
    }
    Ok(true)
}

// -- rig default ---------------------------------------------------------

// Fail if no default is set

pub fn sc_get_default_or_fail() -> Result<String, Box<dyn Error>> {
    let default = sc_get_default()?;
    match default {
        None => bail!("No default R version is set, call <b>rig default <version></b>"),
        Some(d) => Ok(d)
    }
}

pub fn set_default_if_none(ver: String) -> Result<(), Box<dyn Error>> {
    let cur = sc_get_default()?;
    if cur.is_none() {
        sc_set_default(&ver)?;
    }
    Ok(())
}

// -- rig rstudio ---------------------------------------------------------

pub fn sc_rstudio(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let mut ver = args.value_of("version");
    let mut prj = args.value_of("project-file");

    // If the first argument is an R project file, and the second is not,
    // then we switch the two
    if let Some(_) = ver {
        ver = args.value_of("project-file");
        prj = args.value_of("version");
    }

    sc_rstudio_(ver, prj)
}

// ------------------------------------------------------------------------
