
use regex::Regex;
use std::error::Error;
use std::path::Path;

use clap::ArgMatches;
use simple_error::bail;

#[cfg(target_os = "macos")]
use crate::macos::*;

#[cfg(target_os = "windows")]
use crate::windows::*;

#[cfg(target_os = "linux")]
use crate::linux::*;

use crate::rversion::*;
use crate::utils::*;

pub fn check_installed(ver: &String) -> Result<bool, Box<dyn Error>> {
    let inst = sc_get_list()?;
    if !inst.contains(&ver) {
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
        Some(d) => Ok(d),
    }
}

pub fn set_default_if_none(ver: String) -> Result<(), Box<dyn Error>> {
    let cur = sc_get_default()?;
    if cur.is_none() {
        sc_set_default(&ver)?;
    }
    Ok(())
}

// -- rig list ------------------------------------------------------------

pub fn sc_get_list_details() -> Result<Vec<InstalledVersion>, Box<dyn Error>> {
    let names = sc_get_list()?;
    let mut res: Vec<InstalledVersion> = vec![];
    let re = Regex::new("^Version:[ ]?")?;

    for name in names {
        let desc = Path::new(R_ROOT)
            .join(R_SYSLIBPATH.replace("{}", &name))
            .join("base/DESCRIPTION");
        let lines = match read_lines(&desc) {
            Ok(x) => x,
            Err(_) => vec![],
        };
        let idx = grep_lines(&re, &lines);
        let version: Option<String> = if idx.len() == 0 {
            None
        } else {
            Some(re.replace(&lines[idx[0]], "").to_string())
        };
        let path = Path::new(R_ROOT).join(R_VERSIONDIR.replace("{}", &name));
        let binary = Path::new(R_ROOT).join(R_BINPATH.replace("{}", &name));
        res.push(InstalledVersion {
            name: name.to_string(),
            version: version,
            path: path.to_str().and_then(|x| Some(x.to_string())),
            binary: binary.to_str().and_then(|x| Some(x.to_string()))
        });
    }

    Ok(res)
}

// -- rig rstudio ---------------------------------------------------------

pub fn sc_rstudio(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let mut ver = args.value_of("version");
    let mut prj = args.value_of("project-file");

    // If the first argument is an R project file, and the second is not,
    // then we switch the two
    if let Some(ver2) = ver {
        if ver2.ends_with(".Rproj") {
            ver = args.value_of("project-file");
            prj = args.value_of("version");
        }
    }

    sc_rstudio_(ver, prj)
}

// ------------------------------------------------------------------------
