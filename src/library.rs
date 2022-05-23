
use regex::Regex;
use std::error::Error;
use std::path::PathBuf;

use clap::ArgMatches;
use simple_error::{bail,SimpleError};
use simplelog::info;

#[cfg(target_os = "macos")]
use crate::macos::*;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
use windows::*;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux::*;

use crate::escalate::*;
use crate::rversion::*;
use crate::utils::*;

pub fn sc_library_ls() -> Result<(), Box<dyn Error>> {
    let libs = sc_library_get_list(None)?;
    let mut names: Vec<String> = libs.iter().map(|x| {
        if x.default {
            x.name.to_owned() + " (default)"
        } else {
            x.name.to_owned()
        }
    }).collect();
    names.sort();

    for name in names {
        println!("{}", name);
    }

    Ok(())
}

fn sc_library_get_list(rver: Option<String>) -> Result<Vec<PkgLibrary>, Box<dyn Error>> {
    let rver = match rver {
        Some(x) => x,
        None => {
            match sc_get_default()? {
                Some(x) => x,
                None => {
                    bail!("Need to set default R version for `rig library`.")
                }
            }
        }
    };

    let (main, default) = get_library_path(&rver)?;
    let paths = std::fs::read_dir(&main)?;

    let mut libs = Vec::new();

    libs.push(PkgLibrary {
        rversion: rver.to_string(),
        name: "main".to_string(),
        path: main.to_owned(),
        default: main == default
    });

    for de in paths {
        let path = de?.path();

        // If no path name, then path ends with ..., so we can skip
        let fnamestr = match path.file_name() {
            Some(x) => x,
            None => continue
        };
        // If the path is not UTF-8, we'll skip it, this should not happen
        let fnamestr = match fnamestr.to_str() {
            Some(x) => x,
            None => continue
        };

        if &fnamestr[..2] != "__" {
            continue;
        }

        if fnamestr.len() < 3 {
            continue;
        }

        // Ok
        libs.push(PkgLibrary {
            rversion: rver.to_string(),
            name: fnamestr[2..].to_string(),
            path: path.to_owned(),
            default: path == default
        });
    }

    Ok(libs)
}

pub fn sc_library_add(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let new = args.value_of("lib-name")
        .ok_or(SimpleError::new("Internal argument error"))?.to_string();

    let rver = match sc_get_default()? {
        Some(x) => x,
        None => {
            bail!("Need to set default R version for `rig library`.")
        }
    };
    let libs = sc_library_get_list(Some(rver.to_string()))?;
    let names: Vec<String> = libs.iter().map(|x| x.name.to_owned()).collect();
    if names.contains(&new) {
        bail!("Library '{}' already exists for R {}", new, rver);
    }

    let mut main: Option<PathBuf> = None;
    for lib in libs {
        if lib.name == "main" {
            main = Some(lib.path);
        }
    }
    match main {
        None => bail!("Internal error, no main library for R {}", rver),
        Some(main) => {
            let dir = main.as_path().join("__".to_string() + &new);
            std::fs::create_dir_all(&dir)?;
        }
    };

    Ok(())
}

pub fn sc_library_rm(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let torm = args.value_of("lib-name")
        .ok_or(SimpleError::new("Internal argument error"))?.to_string();

    if torm == "main" {
        bail!("Cannot remove the main library");
    }

    let rver = match sc_get_default()? {
        Some(x) => x,
        None => {
            bail!("Need to set default R version for `rig library`.")
        }
    };
    let libs = sc_library_get_list(Some(rver.to_string()))?;

    let mut dir: Option<PathBuf> = None;
    for lib in libs {
        if lib.name == torm {
            if lib.default {
                bail!("Cannot remove the default library");
            }
            dir = Some(lib.path);
        }
    }

    match dir {
        None => bail!("Library {} does not exist for R {}", torm, rver),
        Some(dir) => {
            info!("Deleting library {} for R {}", torm, rver);
            std::fs::remove_dir_all(&dir.as_path())?;
        }
    };

    Ok(())
}

pub fn sc_library_default(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    if args.is_present("lib-name") {
        let name = args.value_of("lib-name")
            .ok_or(SimpleError::new("Internal argument error"))?.to_string();
        sc_library_set_default(&name)
    } else {
        let default = sc_library_get_default()?;
        println!("{}", default.name);
        Ok(())
    }
}

fn sc_library_get_default() -> Result<PkgLibrary, Box<dyn Error>> {
    let rver = match sc_get_default()? {
        Some(x) => x,
        None => {
            bail!("Need to set default R version for `rig library`.")
        }
    };

    let (_main, default) = get_library_path(&rver)?;
    let mut name = "main".to_string();

    if let Some(last) = default.file_name() {
        let last = last.to_str();
        if let Some(last) = last {
            if &last[..2] == "__" {
                name = last[2..].to_string();
            }
        }
    }

    Ok(PkgLibrary {
        rversion: rver.to_string(),
        name: name,
        path: default,
        default: true
    })
}

fn sc_library_set_default(name: &str) -> Result<(), Box<dyn Error>> {
    escalate("updating default library")?;
    let rver = match sc_get_default()? {
        Some(x) => x,
        None => {
            bail!("Need to set default R version for `rig library`.")
        }
    };
    let libs = sc_library_get_list(Some(rver.to_string()))?;

    let mut path: Option<PathBuf> = None;
    for lib in libs {
        if lib.name == name {
            path = Some(lib.path);
        }
    }

    match path {
        None => bail!("No such library: {}, for R {}", name, rver),
        Some(_path) => {

            let mut pre_lines: Vec<String> = vec![
                "## rig R_LIBS_USER start".to_string(),
                "R_LIBS_USER_ORIG=${R_LIBS_USER}".to_string()
            ];

            let name = if name == "main" {
                "".to_string()
            } else {
                "/__".to_string() + name
            };

            let mut post_lines: Vec<String> = vec![
                "R_LIBS_USER_DEFAULT=${R_LIBS_USER}".to_string(),
                format!("R_LIBS_USER_SELECTED=${{R_LIBS_USER_SELECTED-\"{}\"}}", name),
                "R_LIBS_USER_SELECTED_FULL=${R_LIBS_USER}${R_LIBS_USER_SELECTED}".to_string(),
                "R_LIBS_USER=${R_LIBS_USER_ORIG-${R_LIBS_USER_SELECTED_FULL}}".to_string(),
                "## rig R_LIBS_USER end".to_string()
            ];

            let renviron = get_system_renviron(&rver)?;
            let lines = read_lines(&renviron)?;
            let re_start = Regex::new("^## rig R_LIBS_USER start")?;
            let re_end = Regex::new("^## rig R_LIBS_USER end")?;
            let re_def = Regex::new("^R_LIBS_USER=\\$\\{R_LIBS_USER:?\\-")?;
            let idx_start = grep_lines(&re_start, &lines);
            let idx_end = grep_lines(&re_end, &lines);
            let nmarkers = idx_start.len() + idx_end.len();
            let idx_def = grep_lines(&re_def, &lines);
            if nmarkers != 0 && nmarkers != 2 {
                bail!("Invalid system Renviron file at {}. Must include a \
                       single pair of '## rig R_LIBS_USER start' and \
                       '## rig R_LIBS_USER end'", renviron.display());
            }

            let mut lines2: Vec<String>;

            if nmarkers == 0 {
                if idx_def.len() != 1 {
                    bail!("Invalid system Renviron file at {}, Must \
                           include exactly one line single line that \
                           sets `R_LIBS_USER`.");
                }

                let idx = idx_def[0];
                lines2 = lines[..idx].to_vec();
                lines2.append(&mut pre_lines);
                lines2.push(lines[idx].to_string());
                lines2.append(&mut post_lines);
                lines2.append(&mut lines[(idx+1)..].to_vec());

            } else {

                let idx = idx_def[0];
                let idx1 = idx_start[0];
                let idx2 = idx_end[0];
                lines2 = lines[..idx1].to_vec();
                lines2.append(&mut pre_lines);
                lines2.push(lines[idx].to_string());
                lines2.append(&mut post_lines);
                lines2.append(&mut lines[(idx2+1)..].to_vec());

            }

            update_file(&renviron, &lines2)?;
        }
    }

    Ok(())
}
