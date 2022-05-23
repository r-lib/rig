#![cfg(target_os = "macos")]

use std::error::Error;
use std::ffi::OsStr;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::Command;
use rand::Rng;

use clap::ArgMatches;
use nix::unistd::Gid;
use nix::unistd::Uid;
use regex::Regex;
use semver::Version;
use simple_error::{bail, SimpleError};
use simplelog::{info,warn};

use crate::common::*;
use crate::download::*;
use crate::resolve::resolve_versions;
use crate::rversion::*;
use crate::utils::*;
use crate::escalate::*;

const R_ROOT: &str = "/Library/Frameworks/R.framework/Versions";
const R_CUR: &str = "/Library/Frameworks/R.framework/Versions/Current";

pub fn sc_add(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    escalate("adding new R versions")?;
    let mut version = get_resolve(args)?;
    let ver = version.version.to_owned();
    let verstr = match ver {
        Some(ref x) => x,
        None => "???"
    };
    let url: String = match &version.url {
        Some(s) => s.to_string(),
        None => bail!("Cannot find a download url for R version {}", verstr)
    };
    let arch = version.arch.to_owned();
    let prefix = match arch {
        Some(x) => x,
        None => calculate_hash(&url)
    };
    let filename = prefix + "-" + basename(&url).unwrap_or("foo");
    let tmp_dir = std::env::temp_dir().join("rig");
    let target = tmp_dir.join(&filename);
    let cache = target.exists() && not_too_old(&target);
    let target_str = target.to_owned().into_os_string();
    let target_dsp = target.display();
    if cache {
        info!("<cyan>[INFO]</> {} is cached at {}", filename, target_dsp);
    } else {
        info!("<cyan>[INFO]</> Downloading {} -> {}", url, target_dsp);
        let client = &reqwest::Client::new();
        download_file(client, url, &target_str)?;
    }

    sc_system_forget()?;

    // If installed from URL, then we'll need to extract the version + arch
    match ver {
        Some(_) => { },
        None => {
            let fver = extract_pkg_version(&target_str)?;
            version.version = fver.version;
            version.arch = fver.arch;
        }
    };

    let dirname = &get_install_dir(&version)?;

    // Install without changing default
    safe_install(target, dirname)?;

    // This should not happen currently on macOS, a .pkg installer
    // sets the default, but prepare for the future
    set_default_if_none(dirname.to_string())?;

    sc_system_forget()?;
    system_no_openmp(Some(vec![dirname.to_string()]))?;
    system_fix_permissions(None)?;
    system_create_lib(Some(vec![dirname.to_string()]))?;
    sc_system_make_links()?;

    if !args.is_present("without-cran-mirror") {
        set_cloud_mirror(Some(vec![dirname.to_string()]))?;
    }

    if !args.is_present("without-pak") {
        system_add_pak(
            Some(vec![dirname.to_string()]),
            args.value_of("pak-version")
                .ok_or(SimpleError::new("Internal argument error"))?,
            // If this is specified then we always re-install
            args.occurrences_of("pak-version") > 0
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

fn safe_install(target: std::path::PathBuf, ver: &str) -> Result<(), Box<dyn Error>> {

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
        bail!("Failed to re-package installer (cpio): {}", output.status.to_string());
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
        bail!("Failed to re-package installer (pkgutil): {}", output.status.to_string());
    }

    println!("--nnn-- Start of installer output -----------------");
    let status = Command::new("installer")
        .arg("-pkg")
        .arg(&pkg)
        .args(["-target", "/"])
        .spawn()?
        .wait()?;
    println!("--uuu-- End of installer output -------------------");

    if !status.success() {
        bail!("installer exited with status {}", status.to_string());
    }

    if let Err(err) = std::fs::remove_file(&pkg) {
        warn!(
            "<magenta>[WARN]</> Failed to remove temporary file {}: {}",
            pkg.display(),
            err.to_string()
        );
    }
    if let Err(err) = std::fs::remove_dir_all(&tmp) {
        warn!(
            "<magenta>[WARN]</> Failed to remove temporary directory {}: {}",
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

    for ver in vers {
        check_installed(&ver.to_string())?;

        let dir = Path::new(R_ROOT);
        let dir = dir.join(&ver);
        info!("<cyan>[INFO]</> Removing {}", dir.display());
        sc_system_forget()?;
        match std::fs::remove_dir_all(&dir) {
            Err(err) => bail!("Cannot remove {}: {}", dir.display(), err.to_string()),
            _ => {}
        };
    }

    sc_system_make_links()?;

    Ok(())
}

pub fn system_add_pak(vers: Option<Vec<String>>, stream: &str, update: bool)
                      -> Result<(), Box<dyn Error>> {
    let vers = match vers {
        Some(x) => x,
        None => vec![sc_get_default_or_fail()?]
    };

    let base = Path::new("/Library/Frameworks/R.framework/Versions");
    let re = Regex::new("[{][}]")?;

    for ver in vers {
        check_installed(&ver)?;
        if update {
            info!("<cyan>[INFO]</> Installing pak for R {}", ver);
        } else {
            info!("<cyan>[INFO]</> Installing pak for R {} (if not installed yet)", ver);
        }
        check_has_pak(&ver)?;
        let r = base.join(&ver).join("Resources/R");
        let cmd;
        if update {
            cmd = r#"
               dir.create(Sys.getenv('R_LIBS_USER'), showWarnings = FALSE, recursive = TRUE);
               install.packages("pak", repos = sprintf("https://r-lib.github.io/p/pak/{}/%s/%s/%s", .Platform$pkgType, R.Version()$os, R.Version()$arch))
             "#;
        } else {
            cmd = r#"
               dir.create(Sys.getenv('R_LIBS_USER'), showWarnings = FALSE, recursive = TRUE);
               if (!requireNamespace("pak", quietly = TRUE)) {
                 install.packages("pak", repos = sprintf("https://r-lib.github.io/p/pak/{}/%s/%s/%s", .Platform$pkgType, R.Version()$os, R.Version()$arch))
               }
             "#;
        }

        let cmd = re.replace(cmd, stream).to_string();
        let cmd = Regex::new("[\n\r]")?
            .replace_all(&cmd, "")
            .to_string();
        println!("--nnn-- Start of R output -------------------------");
        let status = Command::new(r)
            .args(["--vanilla", "-s", "-e", &cmd])
            .spawn()?
            .wait()?;
        println!("--uuu-- End of R output ---------------------------");

        if !status.success() {
            bail!("Failed to run R {} to install pak", ver);
        }
    }

    Ok(())
}

pub fn system_create_lib(vers: Option<Vec<String>>) -> Result<(), Box<dyn Error>> {
    let vers = match vers {
        Some(x) => x,
        None => sc_get_list()?,
    };
    let base = Path::new("/Library/Frameworks/R.framework/Versions");

    let user = get_user()?;
    for ver in vers {
        check_installed(&ver)?;
        let r = base.join(&ver).join("Resources/R");
        let out = Command::new(r)
            .args(["--vanilla", "-s", "-e", "cat(Sys.getenv('R_LIBS_USER'))"])
            .output()?;
        let lib = match String::from_utf8(out.stdout) {
            Ok(v) => v,
            Err(err) => bail!(
                "Cannot query R_LIBS_USER for R {}: {}",
                ver,
                err.to_string()
            ),
        };

        let lib = shellexpand::tilde(&lib.as_str()).to_string();
        let lib = Path::new(&lib);
        if !lib.exists() {
            info!(
                "<cyan>[INFO]</> {}: creating library at {} for user {}",
                ver,
                lib.display(),
                user.user
            );
            match std::fs::create_dir_all(&lib) {
                Err(err) => bail!(
                    "Cannot create library at {}: {}",
                    lib.display(),
                    err.to_string()
                ),
                _ => {}
            };
            match nix::unistd::chown(
                lib,
                Some(Uid::from_raw(user.uid)),
                Some(Gid::from_raw(user.gid)),
            ) {
                Err(err) => bail!("Cannot set owner on {}: {}", lib.display(), err.to_string()),
                _ => {}
            };
        } else {
            info!("<cyan>[INFO]</> {}: library at {} exists.", ver, lib.display());
        }
    }

    Ok(())
}

pub fn sc_system_make_links() -> Result<(), Box<dyn Error>> {
    escalate("making R-* quick links")?;
    let vers = sc_get_list()?;
    let base = Path::new("/Library/Frameworks/R.framework/Versions/");

    // Create new links
    for ver in vers {
        let linkfile = Path::new("/usr/local/bin/").join("R-".to_string() + &ver);
        let target = base.join(&ver).join("Resources/bin/R");
        if !linkfile.exists() {
            info!("<cyan>[INFO]</> Adding {} -> {}", linkfile.display(), target.display());
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

    // Remove danglink links
    let paths = std::fs::read_dir("/usr/local/bin")?;
    let re = Regex::new("^R-[0-9]+[.][0-9]+")?;
    for file in paths {
        let path = file?.path();
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
        if re.is_match(&fnamestr) {
            match std::fs::read_link(&path) {
                Err(_) => info!("<cyan>[INFO]</> {} is not a symlink", path.display()),
                Ok(target) => {
                    if !target.exists() {
                        info!("<cyan>[INFO]</> Cleaning up {}", target.display());
                        match std::fs::remove_file(&path) {
                            Err(err) => {
                                warn!("<magenta>[WARN]</> Failed to remove {}: {}", path.display(), err.to_string())
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

pub fn sc_system_allow_core_dumps(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    escalate("updating code signature of R and /cores permissions")?;
    sc_system_allow_debugger(args)?;
    info!("<cyan>[INFO]</> Updating permissions of /cores");
    Command::new("chmod")
        .args(["1777", "/cores"])
        .output()?;
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
            .map(|v| v.to_string()).collect()
    };

    let tmp_dir = std::env::temp_dir().join("rig");
    match std::fs::create_dir_all(&tmp_dir) {
        Err(err) => {
            let dir = tmp_dir.to_str().unwrap_or_else(|| "???");
            bail!("Cannot craete temporary file in {}: {}", dir, err.to_string());
        }
        _ => {}
    };

    for ver in vers {
        check_installed(&ver)?;
        let path = Path::new(R_ROOT)
            .join(ver.as_str())
            .join("Resources/bin/exec/R");
        info!("<cyan>[INFO]</> Updating entitlements of {}", path.display());

        let out = Command::new("codesign")
            .args(["-d", "--entitlements", ":-"])
            .arg(&path)
            .output()?;
        if ! out.status.success() {
            let stderr = match std::str::from_utf8(&out.stderr) {
                Ok(v) => v,
                Err(e) => bail!("Invalid UTF-8 output from codesign: {}", e),
            };
            if stderr.contains("is not signed") {
                info!("<cyan>[INFO]</>     not signed");
            } else {
                bail!("Cannot query entitlements:\n   {}", stderr);
            }
            continue;
        }

        let titles = tmp_dir.join("r.entitlements");
        std::fs::write(&titles, out.stdout)?;

        let out = Command::new("/usr/libexec/PlistBuddy")
            .args(["-c", "Add :com.apple.security.get-task-allow bool true"])
            .arg(&titles)
            .output()?;

        if ! out.status.success() {
            let stderr = match std::str::from_utf8(&out.stderr) {
                Ok(v) => v,
                Err(e) => bail!("Invalid UTF-8 output from codesign: {}", e),
            };
            if stderr.contains("Entry Already Exists") {
                info!("<cyan>[INFO]</>     already allows debugging");
                continue;
            } else if stderr.contains("zero-length data") {
                info!("<cyan>[INFO]</>     not signed");
                continue;
            } else {
                bail!("Cannot update entitlements: {}", stderr);
            }
        }

        let out = Command::new("codesign")
            .args(["-s", "-", "-f", "--entitlements"])
            .arg(&titles)
            .arg(&path)
            .output()?;

        if ! out.status.success() {
            bail!("Cannot update entitlements");
        } else {
            info!("<cyan>[INFO]</>     updated entitlements");
        }
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
            .map(|v| v.to_string()).collect();
        system_make_orthogonal(Some(vers))
    }
}

fn system_make_orthogonal(vers: Option<Vec<String>>) -> Result<(), Box<dyn Error>> {
    let vers = match vers {
        Some(x) => x,
        None => sc_get_list()?,
    };

    for ver in vers {
        check_installed(&ver)?;
        info!("<cyan>[INFO]</> Making R {} orthogonal", ver);
        let base = Path::new("/Library/Frameworks/R.framework/Versions/").join(&ver);
        make_orthogonal_(&base, &ver)?;
    }

    Ok(())
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
            .map(|v| v.to_string()).collect();
        system_fix_permissions(Some(vers))
    }
}

fn system_fix_permissions(vers: Option<Vec<String>>) -> Result<(), Box<dyn Error>> {
    let vers = match vers {
        Some(x) => x,
        None => sc_get_list()?,
    };

    for ver in vers {
        check_installed(&ver)?;
        let path = Path::new(R_ROOT).join(ver.as_str());
        info!("<cyan>[INFO]</> Fixing permissions in {}", path.display());
        let status = Command::new("chmod")
            .args(["-R", "g-w"])
            .arg(path)
            .spawn()?
            .wait()?;

        if !status.success() {
            bail!("Failed to update permissions :(");
        }
    }

    let current = Path::new(R_ROOT).join("Current");
    info!("<cyan>[INFO]</> Fixing permissions and group of {}", current.display());
    let status = Command::new("chmod")
        .args(["-R", "775"])
        .arg(&current)
        .spawn()?
        .wait()?;

    if !status.success() {
        bail!("Failed to update permissions :(");
    }

    let status = Command::new("chgrp")
        .args(["admin"])
        .arg(&current)
        .spawn()?
        .wait()?;

    if !status.success() {
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

    // TODO: this can fail, but if it fails it will still have exit
    // status 0, so we would need to check stderr to see if it failed.
    for line in output.lines() {
        info!("<cyan>[INFO]</> Calling pkgutil --forget {}", line.trim());
        Command::new("pkgutil")
            .args(["--forget", line.trim()])
            .output()?;
    }

    Ok(())
}

pub fn get_resolve(args: &ArgMatches) -> Result<Rversion, Box<dyn Error>> {
    let str = args.value_of("str")
        .ok_or(SimpleError::new("Internal argument error"))?.to_string();
    let arch = args.value_of("arch")
        .ok_or(SimpleError::new("Internal argument error"))?;

    if str.len() > 8 && (&str[..7] == "http://" || &str[..8] == "https://") {
        Ok(Rversion {
            version: None,
            url: Some(str.to_string()),
            arch: None
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
            .map(|v| v.to_string()).collect();
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
        check_installed(&ver)?;
        let path = Path::new(R_ROOT).join(ver.as_str());
        let makevars = path.join("Resources/etc/Makeconf".to_string());
        if ! makevars.exists() { continue; }

        match replace_in_file(&makevars, &re, "") {
            Ok(_) => { },
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

    for ver in vers {
        check_installed(&ver)?;
        let path = Path::new(R_ROOT).join(ver.as_str());
        let profile = path.join("Resources/library/base/R/Rprofile".to_string());
        if ! profile.exists() { continue; }

        match append_to_file(
            &profile,
            vec!["options(repos = c(CRAN = \"https://cloud.r-project.org\"))".to_string()]
        ) {
            Ok(_) => { },
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

pub fn sc_rstudio_(version: Option<&str>, project: Option<&str>)
                   -> Result<(), Box<dyn Error>> {

    let mut args = match project {
        None => vec!["-n", "-a", "RStudio"],
        Some(p) => vec!["-n", p]
    };
    let path;

    if let Some(ver) = version {
        check_installed(&ver.to_string())?;
        path = "RSTUDIO_WHICH_R=".to_string() + R_ROOT +
            "/" + &ver + "/Resources/R";
        let mut args2 = vec!["--env", &path];
        args.append(&mut args2);
    }

    info!("<cyan>[INFO]</> Running open {}", args.join(" "));

    let status = Command::new("open")
        .args(args)
        .spawn()?
        .wait()?;

    if !status.success() {
        bail!("RStudio failed with status {}", status.to_string());
    } else {
        Ok(())
    }
}

// ------------------------------------------------------------------------

fn check_has_pak(ver: &String) -> Result<bool, Box<dyn Error>> {
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
    check_installed(&ver.to_string())?;
    // Maybe it does not exist, ignore error here
    match std::fs::remove_file(R_CUR) { _ => { } };
    let path = Path::new(R_ROOT).join(ver);
    std::os::unix::fs::symlink(&path, R_CUR)?;
    Ok(())
}

pub fn sc_get_default() -> Result<Option<String>,Box<dyn Error>> {
    read_version_link(R_CUR)
}

pub fn sc_get_list() -> Result<Vec<String>, Box<dyn Error>> {
    let mut vers = Vec::new();
    if ! Path::new(R_ROOT).exists() {
        return Ok(vers);
    }

    let paths = std::fs::read_dir(R_ROOT)?;

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
        if fnamestr != "Current" && fnamestr != ".DS_Store" {
            vers.push(fnamestr.to_string());
        }
    }
    vers.sort();
    Ok(vers)
}

#[allow(dead_code)]
pub fn sc_get_list_with_versions()
       -> Result<Vec<InstalledVersion>, Box<dyn Error>> {

    let names = sc_get_list()?;
    let mut res: Vec<InstalledVersion> = vec![];
    let re = Regex::new("^Version:[ ]?")?;

    for name in names {
        let desc = Path::new(R_ROOT).join(&name).join("Resources/library/base/DESCRIPTION");
        let lines = match read_lines(&desc) {
            Ok(x) => x,
            Err(_) => vec![]
        };
        let idx = grep_lines(&re, &lines);
        let version: Option<String> = if idx.len() == 0 {
            None
        } else {
            Some(re.replace(&lines[idx[0]], "").to_string())
        };
        res.push(InstalledVersion { name: name.to_string(), version: version });
    }

    Ok(res)
}

fn get_install_dir(ver: &Rversion) -> Result<String, Box<dyn Error>> {
    let version = match &ver.version {
        Some(x) => x,
        None => bail!("Cannot calculate install dir for unknown R version")
    };
    let arch = match &ver.arch {
        Some(x) => x,
        None => bail!("Cannot calculate install dir for unknown arch")
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
        Err(err) => bail!("Cannot extract version from .pkg file: {}", err.to_string())
    };

    let lines = std.lines();
    let re = Regex::new("^R ([0-9]+[.][0-9]+[.][0-9])+.*$")?;
    let lines: Vec<&str> = lines.filter(|l| re.is_match(l)).collect();

    if lines.len() == 0 {
        bail!("Cannot extract version from .pkg file");
    }

    let arm64 = Regex::new("ARM64")?;
    let ver = re.replace(lines[0], "${1}");
    let arch = if arm64.is_match(lines[0]) { "arm64" } else { "x86_64" };

    let res = Rversion {
        version: Some(ver.to_string()),
        url: None,
        arch: Some(arch.to_string())
    };

    Ok(res)
}
