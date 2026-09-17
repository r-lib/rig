use std::error::Error;
use std::path::Path;

#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::os::unix::fs::symlink;

use clap::ArgMatches;
use log::*;
use regex::Regex;
use simple_error::*;

#[cfg(target_os = "macos")]
use crate::macos::*;

#[cfg(target_os = "windows")]
use crate::windows::*;

#[cfg(target_os = "linux")]
use crate::linux::*;

use crate::escalate::*;
use crate::output::OUTPUT;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use crate::utils::{check_local_bin_path, get_binary_dir};

pub fn validate_name_arg(name: &str) -> Result<(), Box<dyn Error>> {
    let re = Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]*$").unwrap();
    if !re.is_match(name) {
        OUTPUT.error(&format!("Invalid --name value: {}", name));
        error!("Invalid --name value: {}", name);
        bail!(
            "Invalid --name value: {}. Only letters, digits, `.`, `_` and `-` \
            are allowed, and it must not be empty.",
            name
        );
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn get_alias(args: &ArgMatches) -> Option<String> {
    let str: Option<&String> = args.get_one("str");
    // In user mode the installation directory is itself named `devel`/`next`,
    // so a separate alias would be redundant. In admin mode the installation
    // is named after its version number, so we still add the alias to make
    // the build reachable by name.
    let user_mode = crate::utils::get_mode()
        .map(|m| m == crate::utils::Mode::User)
        .unwrap_or(false);
    match str {
        None => None,
        Some(str) => match str.as_ref() {
            "oldrel" | "oldrel/1" => Some("oldrel".to_string()),
            "release" => Some("release".to_string()),
            "devel" | "next" if !user_mode => Some(str.to_string()),
            _ => None,
        },
    }
}

#[cfg(target_os = "linux")]
pub fn get_alias(args: &ArgMatches) -> Option<String> {
    match args.get_one::<String>("str") {
        None => None,
        Some(str) => match str.as_ref() {
            "oldrel" | "oldrel/1" => Some("oldrel".to_string()),
            "release" => Some(str.to_string()),
            _ => None,
        },
    }
}

#[cfg(target_os = "windows")]
pub fn get_alias(args: &ArgMatches) -> Option<String> {
    match args.get_one::<String>("str") {
        None => None,
        Some(str) => match str.as_ref() {
            "oldrel" | "oldrel/1" => Some("oldrel".to_string()),
            "release" | "next" => Some(str.to_string()),
            _ => None,
        },
    }
}

#[cfg(target_os = "macos")]
pub fn add_alias(ver: &str, alias: &str) -> Result<(), Box<dyn Error>> {
    let mode = crate::utils::get_mode()?;
    let msg = "Adding R-".to_string() + alias + " alias";
    if mode == crate::utils::Mode::Admin {
        escalate(&msg)?;
    } else {
        let binary_dir = get_binary_dir()?;
        std::fs::create_dir_all(&binary_dir)?;
    }

    check_local_bin_path()?;

    OUTPUT.status(&format!("Adding R-{} alias to R {}", alias, ver));
    info!("Adding R-{} alias to R {}", alias, ver);

    let binding = get_r_binpath()?.replace("{}", ver);
    let target = Path::new(&get_r_root()?).join(&binding);
    let binary_dir = get_binary_dir()?;
    let linkfile = Path::new(&binary_dir).join("R-".to_string() + alias);

    // If it exists then we check that it points to the right place
    // Cannot use .exists(), because it follows symlinks
    let meta = std::fs::symlink_metadata(&linkfile);
    if meta.is_ok() {
        match std::fs::read_link(&linkfile) {
            Err(_) => {
                OUTPUT.error(&format!(
                    "{} is not a symlink, aborting",
                    linkfile.display()
                ));
                error!("{} is not a symlink, aborting", linkfile.display());
                bail!("{} is not a symlink, aborting", linkfile.display())
            }
            Ok(xtarget) => {
                if xtarget == target {
                    return Ok(());
                } else {
                    debug!("{} is wrong, updating", linkfile.display());
                    if let Err(err) = std::fs::remove_file(&linkfile) {
                        OUTPUT.error(&format!(
                            "Failed to delete {}, cannot update alias: {}",
                            linkfile.display(),
                            err
                        ));
                        error!(
                            "Failed to delete {}, cannot update alias: {}",
                            linkfile.display(),
                            err
                        );
                        bail!(
                            "Failed to delete {}, cannot update alias: {}",
                            linkfile.display(),
                            err.to_string()
                        );
                    }
                }
            }
        }
    }

    // If we are still here, then we need to create the link
    debug!("Adding {} -> {}", linkfile.display(), target.display());
    if let Err(err) = symlink(&target, &linkfile) {
        OUTPUT.error(&format!(
            "Cannot create alias {}: {}",
            linkfile.display(),
            err
        ));
        error!("Cannot create alias {}: {}", linkfile.display(), err);
        bail!(
            "Cannot create alias {}: {}",
            linkfile.display(),
            err.to_string()
        )
    };

    Ok(())
}

#[cfg(target_os = "windows")]
pub fn add_alias(ver: &str, alias: &str) -> Result<(), Box<dyn Error>> {
    let msg = "Adding R-".to_string() + alias + " alias";
    escalate(&msg)?;
    let rroot = get_r_root_for(ver)?;
    let base = version_dir_key(ver);
    let links_dir = get_links_dir()?;
    let linkdir = Path::new(&links_dir);

    // should exist at this point, but make sure
    std::fs::create_dir_all(linkdir)?;

    let filename = "R-".to_string() + alias + ".exe";
    let linkfile = linkdir.join(&filename);

    let target = format!(
        "{}\\{}\\bin\\R.exe",
        rroot,
        get_r_versiondir()?.replace("{}", &base)
    );
    let op;
    if linkfile.exists() {
        op = "Updating";
        if let Some((orig_target, orig_marker)) = read_shim_link(&linkfile) {
            if orig_target == target && orig_marker.is_empty() {
                return Ok(());
            }
        }
    } else {
        op = "Adding";
    };
    OUTPUT.status(&format!("{} R-{} alias to R {}", op, alias, ver));
    info!("{} R-{} -> {} alias", op, alias, ver);
    write_shim_link(&linkfile, &target, "")?;

    Ok(())
}

#[cfg(target_os = "linux")]
pub fn add_alias(ver: &str, alias: &str) -> Result<(), Box<dyn Error>> {
    let mode = crate::utils::get_mode()?;
    let msg = "Adding R-".to_string() + alias + " alias";
    if mode == crate::utils::Mode::Admin {
        escalate(&msg)?;
    } else {
        let binary_dir = get_binary_dir()?;
        std::fs::create_dir_all(&binary_dir)?;
        check_local_bin_path()?;
    }

    OUTPUT.status(&format!("Adding R-{} alias to R {}", alias, ver));
    info!("Adding R-{} alias to R {}", alias, ver);

    let rroot = get_r_root()?;
    let base = Path::new(&rroot);
    let target = base.join(get_r_binpath()?.replace("{}", ver));
    let binary_dir = get_binary_dir()?;
    let linkfile = Path::new(&binary_dir).join("R-".to_string() + alias);

    // If it exists then we check that it points to the right place
    // Cannot use .exists(), because it follows symlinks
    let meta = std::fs::symlink_metadata(&linkfile);
    if meta.is_ok() {
        match std::fs::read_link(&linkfile) {
            Err(_) => {
                OUTPUT.error(&format!(
                    "{} is not a symlink, aborting",
                    linkfile.display()
                ));
                error!("{} is not a symlink, aborting", linkfile.display());
                bail!("{} is not a symlink, aborting", linkfile.display())
            }
            Ok(xtarget) => {
                if xtarget == target {
                    return Ok(());
                } else {
                    debug!("{} is wrong, updating", linkfile.display());
                    if let Err(err) = std::fs::remove_file(&linkfile) {
                        OUTPUT.error(&format!(
                            "Failed to delete {}, cannot update alias: {}",
                            linkfile.display(),
                            err
                        ));
                        error!(
                            "Failed to delete {}, cannot update alias: {}",
                            linkfile.display(),
                            err
                        );
                        bail!(
                            "Failed to delete {}, cannot update alias: {}",
                            linkfile.display(),
                            err.to_string()
                        );
                    }
                }
            }
        }
    }

    // If we are still here, then we need to create the link
    debug!("Adding {} -> {}", linkfile.display(), target.display());
    if let Err(err) = symlink(&target, &linkfile) {
        OUTPUT.error(&format!(
            "Cannot create alias {}: {}",
            linkfile.display(),
            err
        ));
        error!("Cannot create alias {}: {}", linkfile.display(), err);
        bail!(
            "Cannot create alias {}: {}",
            linkfile.display(),
            err.to_string()
        )
    };

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_name_arg_accepts_safe_names() {
        assert!(validate_name_arg("work").is_ok());
        assert!(validate_name_arg("my-r").is_ok());
        assert!(validate_name_arg("4work").is_ok());
        assert!(validate_name_arg("r.4.6").is_ok());
    }

    #[test]
    fn validate_name_arg_rejects_unsafe_names() {
        assert!(validate_name_arg("").is_err());
        assert!(validate_name_arg("../etc").is_err());
        assert!(validate_name_arg("a/b").is_err());
        assert!(validate_name_arg("a b").is_err());
    }
}
