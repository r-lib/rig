use regex::Regex;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{file, line};

use clap::ArgMatches;
use simple_error::*;
use simplelog::{debug, info, warn};

#[cfg(target_os = "macos")]
use crate::macos::*;

#[cfg(target_os = "windows")]
use crate::windows::*;

#[cfg(target_os = "linux")]
use crate::linux::*;

use crate::config::*;
use crate::escalate::*;
use crate::rversion::*;
use crate::utils::*;

pub fn sc_library_ls(
    args: &ArgMatches,
    libargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let libs = sc_library_get_list(None, false)?;
    let mut names: Vec<String> = libs
        .iter()
        .map(|x| {
            if x.default {
                x.name.to_owned() + " (default)"
            } else {
                x.name.to_owned()
            }
        })
        .collect();
    names.sort();

    if args.is_present("json") || libargs.is_present("json") || mainargs.is_present("json") {
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

pub fn sc_library_get_list(
    rver: Option<String>,
    cache: bool,
) -> Result<Vec<PkgLibrary>, Box<dyn Error>> {
    let rver = match rver {
        Some(x) => x,
        None => match sc_get_default()? {
            Some(x) => x,
            None => {
                bail!("Need to set default R version for `rig library`.")
            }
        },
    };

    let (main, default) = get_library_path(&rver, cache)?;
    let paths = try_with!(
        std::fs::read_dir(&main),
        "Cannot read directory {} @{}:{}",
        main.display(),
        file!(),
        line!()
    );

    let mut libs = Vec::new();

    libs.push(PkgLibrary {
        rversion: rver.to_string(),
        name: "main".to_string(),
        path: main.to_owned(),
        default: main == default,
    });

    for de in paths {
        let path = try_with!(
            de,
            "Cannot read directory {} @{}:{}",
            main.display(),
            file!(),
            line!()
        )
        .path();

        // If no path name, then path ends with ..., so we can skip
        let fnamestr = match path.file_name() {
            Some(x) => x,
            None => continue,
        };
        // If the path is not UTF-8, we'll skip it, this should not happen
        let fnamestr = match fnamestr.to_str() {
            Some(x) => x,
            None => continue,
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
            default: path == default,
        });
    }

    Ok(libs)
}

pub fn sc_library_add(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let new = require_with!(args.value_of("lib-name"), "clap error").to_string();
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
            try_with!(
                std::fs::create_dir_all(&dir),
                "Cannot create directory {} @{}:{}",
                dir.display(),
                file!(),
                line!()
            );
        }
    };

    Ok(())
}

pub fn sc_library_rm(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let torm = require_with!(args.value_of("lib-name"), "clap error").to_string();
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
            try_with!(
                std::fs::remove_dir_all(&dir.as_path()),
                "Cannot delete directory {} @ {}:{}",
                dir.display(),
                file!(),
                line!()
            );
        }
    };

    Ok(())
}

pub fn sc_library_default(
    args: &ArgMatches,
    libargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    if args.is_present("lib-name") {
        let name = require_with!(args.value_of("lib-name"), "clap error").to_string();
        sc_library_set_default(&name)
    } else {
        let default = sc_library_get_default()?;
        if args.is_present("json") || libargs.is_present("json") || mainargs.is_present("json") {
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
        default: true,
    })
}

pub fn sc_library_set_default(name: &str) -> Result<(), Box<dyn Error>> {
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
        Some(x) => x,
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
    try_with!(
        std::fs::write(&def_file, name),
        "Cannot write file {} @{}:{}",
        def_file.display(),
        file!(),
        line!()
    );

    // Update the R installation if needed
    let rprofile = get_system_profile(&rver)?;
    let lines = match read_lines(&rprofile) {
        Ok(x) => x,
        Err(e) => {
            bail!(
                "Cannot read lines from file {} @{}:{}, {}",
                rprofile.display(),
                file!(),
                line!(),
                e.to_string()
            )
        }
    };
    let re_start = Regex::new("^## rig R_LIBS_USER start")?;
    let idx_start = grep_lines(&re_start, &lines);
    if idx_start.len() == 0 {
        bail!("Library config not set up yet, call `rig system create-lib`");
    }

    // This if for the Rig.app, to update the title in the status bar.
    // It watches the current version, so we change that to trigger an update.
    #[cfg(target_os = "macos")]
    {
        match sc_set_default(&rver) {
            Err(_) => {}
            Ok(_) => {}
        };
    }

    Ok(())
}

pub fn library_update_rprofile(rver: &str) -> Result<(), Box<dyn Error>> {
    let rprofile = get_system_profile(&rver)?;
    let lines = match read_lines(&rprofile) {
        Ok(x) => x,
        Err(e) => {
            bail!(
                "Cannot read lines from file {} @{}:{}, {}",
                rprofile.display(),
                file!(),
                line!(),
                e.to_string()
            )
        }
    };
    let re_start = Regex::new("^## rig R_LIBS_USER start")?;
    let re_end = Regex::new("^## rig R_LIBS_USER end")?;
    let idx_start = grep_lines(&re_start, &lines);
    let idx_end = grep_lines(&re_end, &lines);
    let nmarkers = idx_start.len() + idx_end.len();
    if nmarkers != 0 && nmarkers != 2 {
        bail!(
            "Invalid system Rprofile file at {}. Must include a \
               single pair of '## rig R_LIBS_USER start' and \
               '## rig R_LIBS_USER end'",
            rprofile.display()
        );
    }

    if nmarkers == 0 {
        escalate("updating user library configuration")?;
        let newlines = r#"
## rig R_LIBS_USER start
invisible(local({
  userlibs <- strsplit(Sys.getenv("R_LIBS_USER"), .Platform$path.sep)[[1]]
  if (length(userlibs) == 0) return()
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
  }
  invisible(.libPaths(c(
    unlist(strsplit(Sys.getenv("R_LIBS"), .Platform$path.sep)),
    userlibs
  )))
}))
## rig R_LIBS_USER end
"#;

        match append_to_file(&rprofile, vec![newlines.to_string()]) {
            Err(e) => {
                bail!(
                    "Cannot update file {} @{}:{}, {}",
                    rprofile.display(),
                    file!(),
                    line!(),
                    e.to_string()
                );
            }
            _ => {}
        };
    }

    Ok(())
}

pub fn get_library_path(rver: &str, cache: bool) -> Result<(PathBuf, PathBuf), Box<dyn Error>> {
    match cache {
        true => get_library_path_cache(rver),
        false => get_library_path_nocache(rver),
    }
}

pub fn get_library_path_cache(rver: &str) -> Result<(PathBuf, PathBuf), Box<dyn Error>> {
    debug!("Finding library path (R {}) in cache", rver);
    let default = get_config(rver, "userlibrary");
    let main = match default {
        Err(e) => {
            info!(
                "Failed to read location of library from cache: {}",
                e.to_string()
            );
            return get_library_path_nocache(rver);
        }
        Ok(main) => match main {
            None => return get_library_path_nocache(rver),
            Some(main) => main,
        },
    };

    let main_path = Path::new(&main);
    let config_path = main_path.join("___default");
    if !main_path.exists() || !config_path.exists() {
        return Ok((main_path.to_path_buf(), main_path.to_path_buf()));
    }

    let conf_lines = match read_lines(&config_path) {
        Ok(x) => x,
        Err(e) => {
            bail!(
                "Cannot read lines from file {} @{}:{}, {}",
                config_path.display(),
                file!(),
                line!(),
                e.to_string()
            )
        }
    };
    let def_path = if conf_lines.len() > 0 {
        if conf_lines[0] == "main" {
            main_path.to_path_buf()
        } else {
            main_path.join("__".to_string() + &conf_lines[0])
        }
    } else {
        warn!("Defaults library setup is broken, selecting main library");
        main_path.to_path_buf()
    };
    if !def_path.exists() {
        Ok((main_path.to_path_buf(), main_path.to_path_buf()))
    } else {
        Ok((main_path.to_path_buf(), def_path.to_path_buf()))
    }
}

pub fn get_library_path_nocache(rver: &str) -> Result<(PathBuf, PathBuf), Box<dyn Error>> {
    debug!("Finding library path (R {}) without cache", rver);
    let r = get_r_binary(rver)?;
    let out = try_with!(
        Command::new(r)
            .args([
                "--vanilla",
                "-s",
                "-e",
                "cat(strsplit(Sys.getenv('R_LIBS_USER'), .Platform$path.sep)[[1]][1])"
            ])
            .output(),
        "Failed to run R {} to get library path @{}:{}",
        rver,
        file!(),
        line!()
    );
    let lib = match String::from_utf8(out.stdout) {
        Ok(v) => v,
        Err(err) => bail!(
            "Cannot query R_LIBS_USER for R {}: {}",
            rver,
            err.to_string()
        ),
    };

    let defaultstr = shellexpand::tilde(&lib.as_str()).to_string();
    let default = Path::new(&defaultstr);
    let mut main = Path::new(&defaultstr);

    // If it ends with a __dir component, then drop that
    if let Some(last) = main.file_name() {
        let last = last.to_str();
        if let Some(last) = last {
            if &last[..2] == "__" {
                if let Some(dirn) = main.parent() {
                    main = Path::new(dirn);
                }
            }
        }
    }

    let mainstr = main.to_owned().into_os_string().into_string();
    match mainstr {
        Ok(mainstr) => {
            match save_config(rver, "userlibrary", Some(&mainstr)) {
                Ok(x) => x,
                Err(e) => {
                    bail!(
                        "Failed to save config @{}:{}, {}",
                        file!(),
                        line!(),
                        e.to_string()
                    );
                }
            };
        }
        Err(_) => warn!(
            "Failed to save non-UTF-8 location of library: {}",
            main.display()
        ),
    };

    debug!(
        "R library path: main: {}, default: {}",
        main.display(),
        default.display()
    );
    Ok((main.to_path_buf(), default.to_path_buf()))
}
