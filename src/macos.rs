#![cfg(target_os = "macos")]

use rand::Rng;
use std::error::Error;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::ArgMatches;
use regex::Regex;
use semver::Version;
use simple_error::*;
use simplelog::{debug, info, warn};

use crate::alias::*;
use crate::common::*;
use crate::download::*;
use crate::escalate::*;
use crate::library::*;
use crate::resolve::resolve_versions;
use crate::rversion::*;
use crate::run::*;
use crate::utils::*;

pub const R_ROOT: &str = "/Library/Frameworks/R.framework/Versions";
pub const R_VERSIONDIR: &str = "{}";
pub const R_SYSLIBPATH: &str = "{}/Resources/library";
pub const R_BINPATH: &str = "{}/Resources/R";
const R_CUR: &str = "/Library/Frameworks/R.framework/Versions/Current";

pub fn sc_add(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    escalate("adding new R versions")?;
    let mut version = get_resolve(args)?;
    let alias = get_alias(args);
    let ver = version.version.to_owned();
    let verstr = match ver {
        Some(ref x) => x,
        None => "???",
    };
    let url: String = match &version.url {
        Some(s) => s.to_string(),
        None => {
            let archarg = args
                .value_of("arch")
                .ok_or(SimpleError::new("Internal argument error"))?;
            bail!("Cannot find a download url for R version {}, {}", verstr, archarg);
        }
    };
    let arch = version.arch.to_owned();
    let prefix = match arch {
        Some(ref x) => x.to_owned(),
        None => calculate_hash(&url),
    };
    let filename = prefix + "-" + basename(&url).unwrap_or("foo");
    let tmp_dir = std::env::temp_dir().join("rig");
    let target = tmp_dir.join(&filename);
    let cache = target.exists() && not_too_old(&target);
    let target_str = target.to_owned().into_os_string();
    let target_dsp = target.display();
    if cache {
        info!("{} is cached at {}", filename, target_dsp);
    } else {
        info!("Downloading {} -> {}", url, target_dsp);
        let client = &reqwest::Client::new();
        download_file(client, &url, &target_str)?;
    }

    sc_system_forget()?;

    // If installed from URL, then we'll need to extract the version + arch
    match ver {
        Some(_) => {}
        None => {
            let fver = extract_pkg_version(&target_str)?;
            version.version = fver.version;
            version.arch = fver.arch;
        }
    };

    let dirname = &get_install_dir(&version)?;

    // Install without changing default
    safe_install(target, dirname, arch)?;

    // This should not happen currently on macOS, a .pkg installer
    // sets the default, but prepare for the future
    set_default_if_none(dirname.to_string())?;

    sc_system_forget()?;
    system_no_openmp(Some(vec![dirname.to_string()]))?;
    system_fix_permissions(None)?;
    library_update_rprofile(&dirname.to_string())?;
    sc_system_make_links()?;
    match alias {
        Some(alias) => add_alias(dirname, &alias)?,
        None => { }
    };

    if !args.is_present("without-cran-mirror") {
        set_cloud_mirror(Some(vec![dirname.to_string()]))?;
    }

    if !args.is_present("without-pak") {
        system_add_pak(
            Some(vec![dirname.to_string()]),
            args.value_of("pak-version")
                .ok_or(SimpleError::new("Internal argument error"))?,
            // If this is specified then we always re-install
            args.occurrences_of("pak-version") > 0,
        )?;
    }

    Ok(())
}

fn random_string() -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ\
                            abcdefghijklmnopqrstuvwxyz";
    const PASSWORD_LEN: usize = 10;
    let mut rng = rand::thread_rng();

    let password: String = (0..PASSWORD_LEN)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect();

    password
}

fn safe_install(target: std::path::PathBuf, ver: &str, arch: Option<String>)
                -> Result<(), Box<dyn Error>> {
    let dir = target.parent().ok_or(SimpleError::new("Internal error"))?;
    let tmpf = random_string();
    let tmp = dir.join(tmpf);

    let output = Command::new("pkgutil")
        .arg("--expand")
        .arg(&target)
        .arg(&tmp)
        .output()?;
    if !output.status.success() {
        bail!("pkgutil exited with {}", output.status.to_string());
    }

    let wd1 = tmp.join("r.pkg");
    let wd2 = tmp.join("R-fw.pkg");
    let wd = if wd2.exists() {
        wd2
    } else if wd1.exists() {
        wd1
    } else {
        bail!("Failed to patch installer, could not find framework");
    };
    let output = Command::new("sh")
        .current_dir(&wd)
        .args(["-c", "gzip -dcf Payload | cpio -i"])
        .output()?;
    if !output.status.success() {
        bail!("Failed to extract installer: {}", output.status.to_string());
    }

    let link = wd.join("R.framework").join("Versions").join("Current");
    make_orthogonal_(&link, ver)?;

    std::fs::remove_file(&link)?;

    let output = Command::new("sh")
        .current_dir(&wd)
        .args(["-c", "find R.framework | cpio -oz > Payload"])
        .output()?;
    if !output.status.success() {
        bail!(
            "Failed to re-package installer (cpio): {}",
            output.status.to_string()
        );
    }

    let rf = wd.join("R.framework");
    std::fs::remove_dir_all(&rf)?;

    let pkgf = random_string() + ".pkg";
    let pkg = dir.join(pkgf);
    let output = Command::new("pkgutil")
        .arg("--flatten")
        .arg(&tmp)
        .arg(&pkg)
        .output()?;
    if !output.status.success() {
        bail!(
            "Failed to re-package installer (pkgutil): {}",
            output.status.to_string()
        );
    }

    let mut cmd: OsString = os("installer");
    let mut args: Vec<OsString> = vec![];

    match arch {
        Some(arch) => {
            if arch == "arm64" {
                cmd = os("arch");
                args = vec![os("-arm64"), os("installer")];
            }
        },
        None => { }
    };

    args.push(os("-pkg"));
    args.push(pkg.to_owned().into_os_string());
    args.push(os("-target"));
    args.push(os("/"));

    info!("Running installer");
    run(cmd.into(), args, "installer")?;

    if let Err(err) = std::fs::remove_file(&pkg) {
        warn!(
            "Failed to remove temporary file {}: {}",
            pkg.display(),
            err.to_string()
        );
    }
    if let Err(err) = std::fs::remove_dir_all(&tmp) {
        warn!(
            "Failed to remove temporary directory {}: {}",
            tmp.display(),
            err.to_string()
        );
    }

    Ok(())
}

pub fn sc_rm(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    escalate("removing R versions")?;
    let vers = args.values_of("version");
    if vers.is_none() {
        return Ok(());
    }
    let vers = vers.ok_or(SimpleError::new("Internal argument error"))?;
    let default = sc_get_default()?;

    for ver in vers {

        let ver = check_installed(&ver.to_string())?;

        if let Some(ref default) = default {
            if default == &ver {
                warn!("Removing default version, set new default with \
                       <bold>rig default <version></>");
            }
        }

        let dir = Path::new(R_ROOT);
        let dir = dir.join(&ver);
        info!("Removing {}", dir.display());
        sc_system_forget()?;
        match std::fs::remove_dir_all(&dir) {
            Err(err) => bail!("Cannot remove {}: {}", dir.display(), err.to_string()),
            _ => {}
        };
    }

    sc_system_make_links()?;

    Ok(())
}

pub fn sc_system_make_links() -> Result<(), Box<dyn Error>> {
    escalate("making R-* quick links")?;
    let vers = sc_get_list()?;
    let base = Path::new(R_ROOT);

    info!("Adding R-* quick links (if needed)");

    // Create new links
    for ver in vers {
        let linkfile = Path::new("/usr/local/bin/").join("R-".to_string() + &ver);
        let target = base.join(&ver).join("Resources/bin/R");
        if !linkfile.exists() {
            debug!("Adding {} -> {}", linkfile.display(), target.display());
            match symlink(&target, &linkfile) {
                Err(err) => bail!(
                    "Cannot create symlink {}: {}",
                    linkfile.display(),
                    err.to_string()
                ),
                _ => {}
            };
        }
    }

    // Remove dangling links
    let paths = std::fs::read_dir("/usr/local/bin")?;
    let re = Regex::new("^R-[0-9]+[.][0-9]+")?;
    for file in paths {
        let path = file?.path();
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
        if re.is_match(&fnamestr) {
            match std::fs::read_link(&path) {
                Err(_) => debug!("{} is not a symlink", path.display()),
                Ok(target) => {
                    if !target.exists() {
                        debug!("Cleaning up {}", target.display());
                        match std::fs::remove_file(&path) {
                            Err(err) => {
                                warn!("Failed to remove {}: {}", path.display(), err.to_string())
                            }
                            _ => {}
                        }
                    }
                }
            };
        }
    }

    Ok(())
}

pub fn find_aliases() -> Result<Vec<Alias>, Box<dyn Error>> {
    debug!("Finding existing aliaes");

    let paths = std::fs::read_dir("/usr/local/bin")?;
    let re = re_alias();
    let mut result: Vec<Alias> = vec![];

    for file in paths {
        let path = file?.path();
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
        if re.is_match(&fnamestr) {
            match std::fs::read_link(&path) {
                Err(_) => debug!("{} is not a symlink", path.display()),
                Ok(target) => {
                    if !target.exists() {
                        debug!("Target does not exist at {}", target.display());

                    } else {
                        let version = version_from_link(target);
                        match version {
                            None => continue,
                            Some(version) => {
                                let als = Alias {
                                    alias: fnamestr[2..].to_string(),
                                    version: version.to_string()
                                };
                                result.push(als);
                            }
                        };
                    }
                }
            };
        }
    }

    Ok(result)
}

// Might not be needed in the end
pub fn resolve_alias(alias: &str) -> Result<String, Box<dyn Error>> {
    let path = Path::new("/usr/local/bin").join("R-".to_string() + alias);
    if !path.exists() {
        bail!("Could not find alias: {}", alias);
    }
    match std::fs::read_link(&path) {
        Err(_) => bail!("{} is not a symlink", path.display()),
        Ok(target) => {
            if !target.exists() {
                bail!("Target does not exist at {}", target.display());

            } else {
                let version = version_from_link(target);
                match version {
                    None => bail!("target file name not UTF-8"),
                    Some(v) => return Ok(v.to_string())
                };
            }
        }
    };
}

// /Library/Frameworks/R.framework/Versions/4.2-arm64/Resources/bin/R ->
// 4.2-arm64
fn version_from_link(pb: PathBuf) -> Option<String> {
    let osver = match pb.parent()
        .and_then(|x| x.parent())
        .and_then(|x| x.parent())
        .and_then(|x| x.file_name()) {
        None => None,
        Some(s) => Some(s.to_os_string())
    };

    let s = match osver {
        None => None,
        Some(os) => os.into_string().ok()
    };

    s
}

pub fn sc_system_allow_core_dumps(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    escalate("updating code signature of R and /cores permissions")?;
    sc_system_allow_debugger(args)?;
    info!("Updating permissions of /cores");
    Command::new("chmod").args(["1777", "/cores"]).output()?;
    Ok(())
}

pub fn sc_system_allow_debugger(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    escalate("updating code signature of R")?;
    let all = args.is_present("all");
    let vers = args.values_of("version");

    let vers: Vec<String> = if all {
        sc_get_list()?
    } else if vers.is_none() {
        vec![sc_get_default_or_fail()?]
    } else {
        vers.ok_or(SimpleError::new("Internal argument error"))?
            .map(|v| v.to_string())
            .collect()
    };

    for ver in vers {
        let ver = check_installed(&ver)?;
        let path = PathBuf::new()
            .join(R_ROOT)
            .join(ver.as_str())
            .join("Resources/bin/exec/R");
        update_entitlements(path)?;
    }

    Ok(())
}

pub fn sc_system_allow_debugger_rstudio(_args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let rsess = PathBuf::new().
        join("/Applications/RStudio.app/Contents/MacOS/rsession");

    if !rsess.exists() {
        bail!("RStudio is not installed, at least not in /Applications/RStudio.app");
    }

    update_entitlements(rsess)?;

    let rsessarm64 = PathBuf::new()
        .join("/Applications/RStudio.app/Contents/MacOS/rsession-arm64");

    if rsessarm64.exists() {
        update_entitlements(rsessarm64)?;
    }

    Ok(())
}

pub fn update_entitlements(path: PathBuf) -> Result<(), Box<dyn Error>> {

    let tmp_dir = std::env::temp_dir().join("rig");
    match std::fs::create_dir_all(&tmp_dir) {
        Err(err) => {
            let dir = tmp_dir.to_str().unwrap_or_else(|| "???");
            bail!(
                "Cannot craete temporary file in {}: {}",
                dir,
                err.to_string()
            );
        }
        _ => {}
    };

    info!("Updating entitlements of {}", path.display());

    let out = Command::new("codesign")
        .args(["-d", "--entitlements", ":-"])
        .arg(&path)
        .output()?;
    if !out.status.success() {
        let stderr = match std::str::from_utf8(&out.stderr) {
            Ok(v) => v,
            Err(e) => bail!("Invalid UTF-8 output from codesign: {}", e),
        };
        if stderr.contains("is not signed") {
            info!("    not signed");
        } else {
            bail!("Cannot query entitlements:\n   {}", stderr);
        }
        return Ok(());
    }

    let titles = tmp_dir.join("r.entitlements");
    std::fs::write(&titles, out.stdout)?;

    let out = Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Add :com.apple.security.get-task-allow bool true"])
        .arg(&titles)
        .output()?;

    if !out.status.success() {
        let stderr = match std::str::from_utf8(&out.stderr) {
            Ok(v) => v,
            Err(e) => bail!("Invalid UTF-8 output from codesign: {}", e),
        };
        if stderr.contains("Entry Already Exists") {
            info!("    already allows debugging");
            return Ok(());
        } else if stderr.contains("zero-length data") {
            info!("    not signed");
            return Ok(());
        } else {
            bail!("Cannot update entitlements: {}", stderr);
        }
    }

    let out = Command::new("codesign")
        .args(["-s", "-", "-f", "--entitlements"])
        .arg(&titles)
        .arg(&path)
        .output()?;

    if !out.status.success() {
        bail!("Cannot update entitlements");
    } else {
        info!("    updated entitlements");
    }

    Ok(())
}

pub fn sc_system_make_orthogonal(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    escalate("updating the R installations")?;
    let vers = args.values_of("version");
    if vers.is_none() {
        system_make_orthogonal(None)
    } else {
        let vers: Vec<String> = vers
            .ok_or(SimpleError::new("Internal argument error"))?
            .map(|v| v.to_string())
            .collect();
        system_make_orthogonal(Some(vers))
    }
}

fn system_make_orthogonal(vers: Option<Vec<String>>) -> Result<(), Box<dyn Error>> {
    let vers = match vers {
        Some(x) => {
            let str = x.join(", ");
            info!(
                "Making R version{} {} orthogonal",
                if x.len() > 1 { "s" } else { "" },
                str
            );
            x
        }
        None => {
            info!("Making all R versions orthogonal");
            sc_get_list()?
        }
    };

    for ver in vers {
        let ver = check_installed(&ver)?;
        debug!("Making R {} orthogonal", ver);
        let base = Path::new(R_ROOT).join(&ver);
        make_orthogonal_(&base, &ver)?;
    }

    Ok(())
}

fn is_orthogonal(ver: &str) -> Result<bool, Box<dyn Error>> {
    let base = Path::new(R_ROOT).join(&ver);
    let re = Regex::new("R[.]framework/Resources")?;
    let re2 = Regex::new("[-]F/Library/Frameworks/R[.]framework/[.][.]")?;
    let rfile = base.join("Resources/bin/R");
    let lines = read_lines(&rfile)?;
    let mch = grep_lines(&re, &lines);
    let mch2 = grep_lines(&re2, &lines);
    Ok(mch.len() == 0 && mch2.len() == 0)
}

fn make_orthogonal_(base: &Path, ver: &str) -> Result<(), Box<dyn Error>> {
    let re = Regex::new("R[.]framework/Resources")?;
    let re2 = Regex::new("[-]F/Library/Frameworks/R[.]framework/[.][.]")?;

    let sub = "R.framework/Versions/".to_string() + &ver + "/Resources";

    let rfile = base.join("Resources/bin/R");
    replace_in_file(&rfile, &re, &sub).ok();

    let efile = base.join("Resources/etc/Renviron");
    replace_in_file(&efile, &re, &sub).ok();

    let ffile = base.join("Resources/fontconfig/fonts/fonts.conf");
    replace_in_file(&ffile, &re, &sub).ok();

    let mfile = base.join("Resources/etc/Makeconf");
    let sub = "-F/Library/Frameworks/R.framework/Versions/".to_string() + &ver;
    replace_in_file(&mfile, &re2, &sub).ok();

    let fake = base.join("R.framework");
    let fake = fake.as_path();
    // TODO: only ignore failure if files already exist
    std::fs::create_dir_all(&fake).ok();
    symlink("../Headers", fake.join("Headers")).ok();
    symlink("../Resources/lib", fake.join("Libraries")).ok();
    symlink("../PrivateHeaders", fake.join("PrivateHeaders")).ok();
    symlink("../R", fake.join("R")).ok();
    symlink("../Resources", fake.join("Resources")).ok();

    Ok(())
}

pub fn sc_system_fix_permissions(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    escalate("changing system library permissions")?;
    let vers = args.values_of("version");
    if vers.is_none() {
        system_fix_permissions(None)
    } else {
        let vers: Vec<String> = vers
            .ok_or(SimpleError::new("Internal argument error"))?
            .map(|v| v.to_string())
            .collect();
        system_fix_permissions(Some(vers))
    }
}

fn system_fix_permissions(vers: Option<Vec<String>>) -> Result<(), Box<dyn Error>> {
    let vers = match vers {
        Some(x) => x,
        None => sc_get_list()?,
    };

    info!("Fixing permissions");

    for ver in vers {
        let ver = check_installed(&ver)?;
        let path = Path::new(R_ROOT).join(ver.as_str());
        debug!("Fixing permissions in {}", path.display());
        let output = Command::new("chmod")
            .args(["-R", "g-w"])
            .arg(path)
            .output()?;

        if !output.status.success() {
            bail!("Failed to update permissions :(");
        }
    }

    let current = Path::new(R_ROOT).join("Current");
    debug!(
        "Fixing permissions and group of {}",
        current.display()
    );
    let output = Command::new("chmod")
        .args(["-R", "775"])
        .arg(&current)
        .output()?;

    if !output.status.success() {
        bail!("Failed to update permissions :(");
    }

    let output = Command::new("chgrp")
        .args(["admin"])
        .arg(&current)
        .output()?;

    if !output.status.success() {
        bail!("Failed to update group :(");
    }

    Ok(())
}

pub fn sc_system_forget() -> Result<(), Box<dyn Error>> {
    escalate("forgetting R versions")?;
    let out = Command::new("sh")
        .args(["-c", "pkgutil --pkgs | grep -i r-project | grep -v clang"])
        .output()?;

    let output = match String::from_utf8(out.stdout) {
        Ok(v) => v,
        Err(_) => bail!("Invalid UTF-8 output from pkgutil"),
    };

    if output.lines().count() > 0 {
        info!("Forgetting installed versions");
    }

    // TODO: this can fail, but if it fails it will still have exit
    // status 0, so we would need to check stderr to see if it failed.
    for line in output.lines() {
        debug!("Calling pkgutil --forget {}", line.trim());
        Command::new("pkgutil")
            .args(["--forget", line.trim()])
            .output()?;
    }

    Ok(())
}

pub fn get_resolve(args: &ArgMatches) -> Result<Rversion, Box<dyn Error>> {
    let str = args
        .value_of("str")
        .ok_or(SimpleError::new("Internal argument error"))?
        .to_string();
    let arch = args
        .value_of("arch")
        .ok_or(SimpleError::new("Internal argument error"))?;

    if str.len() > 8 && (&str[..7] == "http://" || &str[..8] == "https://") {
        Ok(Rversion {
            version: None,
            url: Some(str.to_string()),
            arch: None,
        })
    } else {
        let eps = vec![str];
        let version = resolve_versions(eps, "macos".to_string(), arch.to_string(), None)?;
        Ok(version[0].to_owned())
    }
}

pub fn sc_system_no_openmp(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    escalate("updating R compiler configuration")?;
    let vers = args.values_of("version");
    if vers.is_none() {
        system_no_openmp(None)
    } else {
        let vers: Vec<String> = vers
            .ok_or(SimpleError::new("Internal argument error"))?
            .map(|v| v.to_string())
            .collect();
        system_no_openmp(Some(vers))
    }
}

fn system_no_openmp(vers: Option<Vec<String>>) -> Result<(), Box<dyn Error>> {
    let vers = match vers {
        Some(x) => x,
        None => sc_get_list()?,
    };
    let re = Regex::new("[-]fopenmp")?;

    for ver in vers {
        let ver = check_installed(&ver)?;
        let path = Path::new(R_ROOT).join(ver.as_str());
        let makevars = path.join("Resources/etc/Makeconf".to_string());
        if !makevars.exists() {
            continue;
        }

        match replace_in_file(&makevars, &re, "") {
            Ok(_) => {}
            Err(err) => {
                bail!("Failed to update {}: {}", path.display(), err);
            }
        };
    }

    Ok(())
}

fn set_cloud_mirror(vers: Option<Vec<String>>) -> Result<(), Box<dyn Error>> {
    let vers = match vers {
        Some(x) => x,
        None => sc_get_list()?,
    };

    info!("Setting default CRAN mirror");

    for ver in vers {
        let ver = check_installed(&ver)?;
        let path = Path::new(R_ROOT).join(ver.as_str());
        let profile = path.join("Resources/library/base/R/Rprofile".to_string());
        if !profile.exists() {
            continue;
        }

        match append_to_file(
            &profile,
            vec!["options(repos = c(CRAN = \"https://cloud.r-project.org\"))".to_string()],
        ) {
            Ok(_) => {}
            Err(err) => {
                bail!("Failed to update {}: {}", path.display(), err);
            }
        };
    }

    Ok(())
}

pub fn sc_clean_registry() -> Result<(), Box<dyn Error>> {
    // Nothing to do on macOS
    Ok(())
}

pub fn sc_system_update_rtools40() -> Result<(), Box<dyn Error>> {
    Ok(())
}

pub fn sc_rstudio_(version: Option<&str>,
                   project: Option<&str>,
                   arg: Option<&OsStr>)
                   -> Result<(), Box<dyn Error>> {

    let mut args = match project {
        None => vec![os("-n"), os("-a"), os("RStudio")],
        Some(p) => vec![os("-n"), os(p)],
    };
    let path;

    if let Some(ver) = version {
        let ver = check_installed(&ver.to_string())?;
        if !is_orthogonal(&ver)? {
            bail!("R {} is not orthogonal, it cannot run as a non-default. \
                   Run `rig system make-orthogonal`.", ver)
        }
        path = "RSTUDIO_WHICH_R=".to_string() + R_ROOT + "/" + &ver + "/Resources/R";
        let mut args2 = vec![os("--env"), os(&path)];
        args.append(&mut args2);
    }

    if let Some(arg) = arg { args.push(arg.to_os_string()); }

    info!("Running open {}", osjoin(args.to_owned(), " "));

    match run(os("open"), args, "open") {
        Err(e) => { bail!("RStudio failed to start: {}", e.to_string()); },
        _ => {}
    };

    Ok(())
}

// ------------------------------------------------------------------------

pub fn check_has_pak(ver: &String) -> Result<bool, Box<dyn Error>> {
    let ver = Regex::new("-.*$")?.replace(ver, "").to_string();
    let ver = ver + ".0";
    let v330 = Version::parse("3.2.0")?;
    let vv = Version::parse(&ver)?;
    if vv <= v330 {
        bail!("Pak is only available for R 3.3.0 or later");
    }
    Ok(true)
}

pub fn sc_set_default(ver: &str) -> Result<(), Box<dyn Error>> {
    let ver = check_installed(&ver.to_string())?;
    // Maybe it does not exist, ignore error here
    match std::fs::remove_file(R_CUR) {
        _ => {}
    };
    let path = Path::new(R_ROOT).join(ver);
    std::os::unix::fs::symlink(&path, R_CUR)?;

    let r = Path::new("/usr/local/bin/R");
    if !r.exists() {
        debug!("Creating {}", r.display());
        let tgt = Path::new("/Library/Frameworks/R.framework/Resources/bin/R");
        match std::os::unix::fs::symlink(&tgt, &r) {
            Err(e) => warn!("Cannot create missing /usr/local/bin/R: {}", e.to_string()),
            _ => {}
        };
    }

    let rscript = Path::new("/usr/local/bin/Rscript");
    if !rscript.exists() {
        debug!("Creating {}", rscript.display());
        let tgt = Path::new("/Library/Frameworks/R.framework/Resources/bin/Rscript");
        match std::os::unix::fs::symlink(&tgt, &rscript) {
            Err(e) => warn!("Cannot create missing /usr/local/bin/Rscript: {}", e.to_string()),
            _ => {}
        };
    }

    Ok(())
}

pub fn sc_get_default() -> Result<Option<String>, Box<dyn Error>> {
    read_version_link(R_CUR)
}

pub fn sc_get_list() -> Result<Vec<String>, Box<dyn Error>> {
    let mut vers = Vec::new();
    if !Path::new(R_ROOT).exists() {
        return Ok(vers);
    }

    let paths = std::fs::read_dir(R_ROOT)?;

    for de in paths {
        let path = de?.path();
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
        if fnamestr == "Current" || fnamestr == ".DS_Store" {
            continue;
        }
        // If there is no Resources/bin/R, then this is not an R installation
        let rbin = path.join("Resources").join("bin").join("R");
        if !rbin.exists() {
            continue;
        }

        // Ok
        vers.push(fnamestr.to_string());
    }
    vers.sort();
    Ok(vers)
}

fn get_install_dir(ver: &Rversion) -> Result<String, Box<dyn Error>> {
    let version = match &ver.version {
        Some(x) => x,
        None => bail!("Cannot calculate install dir for unknown R version"),
    };
    let arch = match &ver.arch {
        Some(x) => x,
        None => bail!("Cannot calculate install dir for unknown arch"),
    };
    let minor = get_minor_version(&version)?;
    if arch == "x86_64" {
        Ok(minor)
    } else if arch == "arm64" {
        Ok(minor + "-arm64")
    } else {
        bail!("Unknown macOS arch: {}", arch);
    }
}

fn get_minor_version(ver: &str) -> Result<String, Box<dyn Error>> {
    let re = Regex::new("[.][^.]*$")?;
    Ok(re.replace(ver, "").to_string())
}

fn extract_pkg_version(filename: &OsStr) -> Result<Rversion, Box<dyn Error>> {
    let out = Command::new("installer")
        .args(["-pkginfo", "-pkg"])
        .arg(filename)
        .output()?;
    let std = match String::from_utf8(out.stdout) {
        Ok(v) => v,
        Err(err) => bail!("Cannot extract version from .pkg file: {}", err.to_string()),
    };

    let lines = std.lines();
    let re = Regex::new("^R ([0-9]+[.][0-9]+[.][0-9])+.*$")?;
    let lines: Vec<&str> = lines.filter(|l| re.is_match(l)).collect();

    if lines.len() == 0 {
        bail!("Cannot extract version from .pkg file");
    }

    let arm64 = Regex::new("ARM64")?;
    let ver = re.replace(lines[0], "${1}");
    let arch = if arm64.is_match(lines[0]) {
        "arm64"
    } else {
        "x86_64"
    };

    let res = Rversion {
        version: Some(ver.to_string()),
        url: None,
        arch: Some(arch.to_string()),
    };

    Ok(res)
}

pub fn get_r_binary(rver: &str) -> Result<PathBuf, Box<dyn Error>> {
    debug!("Finding R binary for R {}", rver);
    let bin = Path::new(R_ROOT).join(rver).join("Resources/R");
    debug!("R {} binary is at {}", rver, bin.display());
    Ok(bin)
}

#[allow(dead_code)]
pub fn get_system_renviron(rver: &str) -> Result<PathBuf, Box<dyn Error>> {
    let renviron = Path::new(R_ROOT).join(rver).join("Resources/etc/Renviron");
    Ok(renviron)
}

pub fn get_system_profile(rver: &str) -> Result<PathBuf, Box<dyn Error>> {
    let profile = Path::new(R_ROOT)
        .join(rver)
        .join("Resources/library/base/R/Rprofile");
    Ok(profile)
}

pub fn is_arm64_machine() -> bool {
    let proc = std::process::Command::new("arch")
        .args(["-arm64", "true"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    if let Ok(mut proc) = proc {
        let out = proc.wait();
        if let Ok(out) = out {
            if out.success() {
                true
            } else {
                false
            }
        } else {
            false
        }
    } else {
        false
    }
}
