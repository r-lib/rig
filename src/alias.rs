
use std::error::Error;
#[cfg(target_os = "windows")]
use std::io::Write;
#[cfg(target_os = "windows")]
use std::fs::File;
use std::path::Path;

#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::os::unix::fs::symlink;

use clap::ArgMatches;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use simple_error::*;
use simplelog::*;

#[cfg(target_os = "macos")]
use crate::macos::*;

#[cfg(target_os = "windows")]
use crate::windows::*;

#[cfg(target_os = "linux")]
use crate::linux::*;

use crate::escalate::*;

#[cfg(target_os = "macos")]
pub fn get_alias(args: &ArgMatches) -> Option<String> {
    let str: Option<&String> = args.get_one("str");
    match str {
        None => None,
        Some(str) => {
            match str.as_ref() {
                "oldrel" | "oldrel/1" => Some("oldrel".to_string()),
                "release" | "devel" | "next" => Some(str.to_string()),
                _ => None
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub fn get_alias(args: &ArgMatches) -> Option<String> {
    match args.get_one::<String>("str") {
        None => None,
        Some(str) => {
            match str.as_ref() {
                "oldrel" | "oldrel/1" => Some("oldrel".to_string()),
                "release" => Some(str.to_string()),
                _ => None
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub fn get_alias(args: &ArgMatches) -> Option<String> {
    match args.get_one::<String>("str") {
        None => None,
        Some(str) => {
            match str.as_ref() {
                "oldrel" | "oldrel/1" => Some("oldrel".to_string()),
                "release" | "next" => Some(str.to_string()),
                _ => None
            }
        }
    }
}

#[cfg(target_os = "macos")]
pub fn add_alias(ver: &str, alias: &str) -> Result<(), Box<dyn Error>> {
    let msg = "Adding R-".to_string() + alias + " alias";
    escalate(&msg)?;

    info!("Adding R-{} alias to R {}", alias, ver);

    let rroot = R_ROOT();
    let base = Path::new(&rroot);
    let target = base.join(ver).join("Resources/bin/R");
    let linkfile = Path::new("/usr/local/bin/").join("R-".to_string() + alias);

    // If it exists then we check that it points to the right place
    // Cannot use .exists(), because it follows symlinks
    let meta = std::fs::symlink_metadata(&linkfile);
    if meta.is_ok() {
        match std::fs::read_link(&linkfile) {
            Err(_) => bail!("{} is not a symlink, aborting", linkfile.display()),
            Ok(xtarget) => {
                if xtarget == target {
                    return Ok(())
                } else {
                    debug!("{} is wrong, updating", linkfile.display());
                    match std::fs::remove_file(&linkfile) {
                        Err(err) => {
                            bail!(
                                "Failed to delete {}, cannot update alias: {}",
                                linkfile.display(),
                                err.to_string()
                            );
                        },
                        _ => {}
                    }
                }
            }
        }
    }

    // If we are still here, then we need to create the link
    debug!("Adding {} -> {}", linkfile.display(), target.display());
    match symlink(&target, &linkfile) {
        Err(err) => bail!(
            "Cannot create alias {}: {}",
            linkfile.display(),
            err.to_string()
        ),
        _ => {}
    };

    Ok(())
}

#[cfg(target_os = "windows")]
pub fn add_alias(ver: &str, alias: &str) -> Result<(), Box<dyn Error>> {
    let msg = "Adding R-".to_string() + alias + " alias";
    escalate(&msg)?;
    let rroot = R_ROOT();
    let linkdir = Path::new(RIG_LINKS_DIR);

    // should exist at this point, but make sure
    std::fs::create_dir_all(&linkdir)?;

    let filename = "R-".to_string() + alias + ".bat";
    let linkfile = linkdir.join(&filename);

    let cnt = "@\"".to_string() + &rroot + "\\R-" + &ver + "\\bin\\R\" %*\n";
    let op;
    if linkfile.exists() {
        op = "Updating";
        let orig = std::fs::read_to_string(&linkfile)?;
        if orig == cnt {
            return Ok(());
        }
    } else {
        op = "Adding";
    };
    info!("{} R-{} -> {} alias", op, alias, ver);
    let mut file = File::create(&linkfile)?;
    file.write_all(cnt.as_bytes())?;

    Ok(())
}

#[cfg(target_os = "linux")]
pub fn add_alias(ver: &str, alias: &str) -> Result<(), Box<dyn Error>> {
    let msg = "Adding R-".to_string() + alias + " alias";
    escalate(&msg)?;

    info!("Adding R-{} alias to R {}", alias, ver);

    let base = Path::new(&R_ROOT());
    let target = base.join(ver).join("bin/R");
    let linkfile = Path::new("/usr/local/bin/").join("R-".to_string() + alias);

    // If it exists then we check that it points to the right place
    // Cannot use .exists(), because it follows symlinks
    let meta = std::fs::symlink_metadata(&linkfile);
    if meta.is_ok() {
        match std::fs::read_link(&linkfile) {
            Err(_) => bail!("{} is not a symlink, aborting", linkfile.display()),
            Ok(xtarget) => {
                if xtarget == target {
                    return Ok(())
                } else {
                    debug!("{} is wrong, updating", linkfile.display());
                    match std::fs::remove_file(&linkfile) {
                        Err(err) => {
                            bail!(
                                "Failed to delete {}, cannot update alias: {}",
                                linkfile.display(),
                                err.to_string()
                            );
                        },
                        _ => {}
                    }
                }
            }
        }
    }

    // If we are still here, then we need to create the link
    debug!("Adding {} -> {}", linkfile.display(), target.display());
    match symlink(&target, &linkfile) {
        Err(err) => bail!(
            "Cannot create alias {}: {}",
            linkfile.display(),
            err.to_string()
        ),
        _ => {}
    };

    Ok(())
}
