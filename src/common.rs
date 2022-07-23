
use regex::Regex;
use std::error::Error;
use std::path::Path;
use std::path::PathBuf;

use clap::ArgMatches;
use simple_error::bail;
use simplelog::*;

#[cfg(target_os = "macos")]
use crate::macos::*;

#[cfg(target_os = "windows")]
use crate::windows::*;

#[cfg(target_os = "linux")]
use crate::linux::*;

use crate::escalate::escalate;
use crate::renv;
use crate::rversion::*;
use crate::run::*;
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
    let aliases = find_aliases()?;
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
        let mut myaliases: Vec<String> = vec![];
        for a in &aliases {
            if a.version == name {
                myaliases.push(a.alias.to_owned());
            }
        }
        res.push(InstalledVersion {
            name: name.to_string(),
            version: version,
            path: path.to_str().and_then(|x| Some(x.to_string())),
            binary: binary.to_str().and_then(|x| Some(x.to_string())),
            aliases: myaliases
        });
    }

    Ok(res)
}

// -- rig system add-pak (implementation) ---------------------------------

pub fn system_add_pak(
    vers: Option<Vec<String>>,
    stream: &str,
    update: bool,
) -> Result<(), Box<dyn Error>> {
    let vers = match vers {
        Some(x) => x,
        None => vec![sc_get_default_or_fail()?],
    };

    for ver in vers {
        check_installed(&ver)?;
        if update {
            info!("Installing pak for R {}", ver);
        } else {
            info!("Installing pak for R {} (if not installed yet)", ver);
        }
        check_has_pak(&ver)?;

        // We do this to create the user library, because currently there
        // is a bug in the system profile code that creates it, and it is
        // only added after a restart.
        match r(&ver, "invisible()") {
            Ok(_) => {},
            Err(x) => bail!("Failed to to install pak for R {}: {}", ver, x.to_string())
        };

        // The actual pak installation
        let cmd;
        if update {
            cmd = r#"
                install.packages('pak', repos = sprintf('https://r-lib.github.io/p/pak/{}/%s/%s/%s', .Platform$pkgType, R.Version()$os, R.Version()$arch))
            "#;
        } else {
            cmd = r#"
                if (!requireNamespace('pak', quietly = TRUE)) {
                    install.packages('pak', repos = sprintf('https://r-lib.github.io/p/pak/{}/%s/%s/%s', .Platform$pkgType, R.Version()$os, R.Version()$arch))
                }
            "#;
        };
        let cmd = cmd.replace("{}", stream);

        match r(&ver, &cmd) {
            Ok(_) => {},
            Err(x) => bail!("Failed to install pak for R {}: {}", ver, x.to_string())
        };
    }

    Ok(())
}

// -- rig rstudio ---------------------------------------------------------

pub fn sc_rstudio(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let mut ver = args.value_of("version");
    let mut prj = args.value_of("project-file");

    if let Some(ver2) = ver {
        if ver2.ends_with("renv.lock") {
            let lockfile = PathBuf::new().join(ver2);
            let needver = renv::parse_r_version(lockfile)?;
            let usever = renv::match_r_version(&needver)?;
            let realver = usever.version.to_string();

	    // On windows we need to escalate to change the registry
	    // If we don't escalate now, then the info!() will be
	    // printed twive.
	    if std::env::consts::OS == "windows" {
		escalate("updating default version in registry")?;
	    }

            info!("Using {} R {}{}",
                  if needver == realver {
                      "matching version:"
                  } else {
                      "latest minor version:"
                  },
                  usever.name,
                  if realver != usever.name {
                      " (R ".to_string() + &realver + ")"
                  } else {
                      "".to_string()
                  }
            );

            // Seems that it is enough to call with the directory
            // of the lock file, this works reasonable with and without
            // a project file.
            let prdir = Path::new(ver2).parent();
            let prdir = prdir.and_then(|x| Some(x.as_os_str()));
            return sc_rstudio_(Some(&usever.name), None, prdir);
        }
    }

    // If the first argument is an R project file, and the second is not,
    // then we switch the two
    if let Some(ver2) = ver {
        if ver2.ends_with(".Rproj") {
            ver = args.value_of("project-file");
            prj = args.value_of("version");
        }
    }

    sc_rstudio_(ver, prj, None)
}

// ------------------------------------------------------------------------
