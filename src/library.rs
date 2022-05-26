
use regex::Regex;
use std::error::Error;
use std::path::PathBuf;

use clap::ArgMatches;
use simple_error::{bail,SimpleError};
use simplelog::info;

#[cfg(target_os = "macos")]
use crate::macos::*;

#[cfg(target_os = "windows")]
use crate::windows::*;

#[cfg(target_os = "linux")]
use crate::linux::*;

use crate::escalate::*;
use crate::rversion::*;
use crate::utils::*;

pub fn sc_library_ls(args: &ArgMatches, libargs: &ArgMatches, mainargs: &ArgMatches)
                     -> Result<(), Box<dyn Error>> {
    let libs = sc_library_get_list(None, false)?;
    let mut names: Vec<String> = libs.iter().map(|x| {
        if x.default {
            x.name.to_owned() + " (default)"
        } else {
            x.name.to_owned()
        }
    }).collect();
    names.sort();

    if args.is_present("json") || libargs.is_present("json") ||
        mainargs.is_present("json") {
            println!("[");
            let num = libs.len();
            for (idx, lib) in libs.iter().enumerate() {
		let path = lib.path.display().to_string();
                println!("  {{");
                println!("    \"name\": \"{}\",", lib.name);
                println!("    \"path\": \"{}\",", path.replace("\\", "/"));
                println!(
                    "    \"default\": {}",
                    if lib.default { "true" } else { "false" }
                );
                println!("  }}{}", if idx == num - 1 { "" } else { "," });
            }
            println!("]");

        } else {
            for name in names {
                println!("{}", name);
            }
        }

    Ok(())
}

fn sc_library_get_list(rver: Option<String>, cache: bool)
                       -> Result<Vec<PkgLibrary>, Box<dyn Error>> {
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

    let (main, default) = get_library_path(&rver, cache)?;
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

        if fnamestr == "___default" {
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
    let libs = sc_library_get_list(Some(rver.to_string()), false)?;
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
    let libs = sc_library_get_list(Some(rver.to_string()), false)?;

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

pub fn sc_library_default(args: &ArgMatches, libargs: &ArgMatches,
                          mainargs: &ArgMatches)
                          -> Result<(), Box<dyn Error>> {
    if args.is_present("lib-name") {
        let name = args.value_of("lib-name")
            .ok_or(SimpleError::new("Internal argument error"))?.to_string();
        sc_library_set_default(&name)
    } else {
        let default = sc_library_get_default()?;
        if args.is_present("json") || libargs.is_present("json") ||
            mainargs.is_present("json") {
		let path = default.path.display().to_string();
		println!("{{");
		println!("  \"name\": \"{}\",", default.name);
		println!("  \"path\": \"{}\"", path.replace("\\", "/"));
		println!("}}");
            } else {
		println!("{}", default.name);
            }
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

    let (_main, default) = get_library_path(&rver, false)?;
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
    let rver = match sc_get_default()? {
        Some(x) => x,
        None => {
            bail!("Need to set default R version for `rig library`.")
        }
    };
    let libs = sc_library_get_list(Some(rver.to_string()), false)?;

    let mut path: Option<PathBuf> = None;
    for lib in libs {
        if lib.name == name {
            path = Some(lib.path);
        }
    }

    let mut path = match path {
        None => bail!("No such library: {}, for R {}", name, rver),
        Some(x) => x
    };

    // Create ___default file in the library
    if let Some(last) = path.file_name() {
        let last = last.to_str();
        if let Some(last) = last {
            if &last[..2] == "__" {
                path = path.parent().unwrap().to_path_buf();
            }
        }
    }

    let def_file = path.join("___default");
    std::fs::write(def_file, name)?;

    // Update the R installation if needed
    let rprofile = get_system_profile(&rver)?;
    let lines = read_lines(&rprofile)?;
    let re_start = Regex::new("^## rig R_LIBS_USER start")?;
    let idx_start = grep_lines(&re_start, &lines);
    if idx_start.len() == 0 {
        bail!("Library config not set up yet, call `rig system create-lib`");
    }

    Ok(())
}

pub fn library_update_rprofile(rver: &str) -> Result<(), Box<dyn Error>> {
    escalate("updating user library configuration")?;

    let rprofile = get_system_profile(&rver)?;
    let lines = read_lines(&rprofile)?;
    let re_start = Regex::new("^## rig R_LIBS_USER start")?;
    let re_end = Regex::new("^## rig R_LIBS_USER end")?;
    let idx_start = grep_lines(&re_start, &lines);
    let idx_end = grep_lines(&re_end, &lines);
    let nmarkers = idx_start.len() + idx_end.len();
    if nmarkers != 0 && nmarkers != 2 {
        bail!("Invalid system Rprofile file at {}. Must include a \
               single pair of '## rig R_LIBS_USER start' and \
               '## rig R_LIBS_USER end'", rprofile.display());
    }

    if nmarkers == 0 {
        let newlines = r#"
## rig R_LIBS_USER start
local({
  userlibs <- strsplit(Sys.getenv("R_LIBS_USER"), .Platform$path.sep)[[1]]
  if (userlibs[[1]] == "NULL") return()
  userlib1 <- userlibs[1]
  dir.create(userlib1, recursive = TRUE, showWarnings = FALSE)
  deffile <- file.path(userlib1, "___default", fsep = "/")
  if (file.exists(deffile)) {
    def <- readLines(deffile, warn = FALSE)
    def <- if (def == "main") "" else paste0("/__", def)
    userlibs[1] <- file.path(userlib1, def, fsep = "/")
    dir.create(userlibs[1], recursive = TRUE, showWarnings = FALSE)
    Sys.setenv("R_LIBS_USER" = paste(userlibs, collapse = .Platform$path.sep))
    invisible(.libPaths(c(
      unlist(strsplit(Sys.getenv("R_LIBS"), .Platform$path.sep)),
      userlibs
    )))
  }
})
## rig R_LIBS_USER end
"#;

	append_to_file(&rprofile, vec![newlines.to_string()])?;
    }

    Ok(())
}
