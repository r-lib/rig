#![cfg(target_os = "windows")]

use regex::Regex;
use std::error::Error;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Write;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{thread, time};

use clap::ArgMatches;
use remove_dir_all::remove_dir_all;
use simple_error::{bail, SimpleError};
use simplelog::{debug, info, warn};
use winreg::enums::*;
use winreg::RegKey;

use crate::common::*;
use crate::download::*;
use crate::escalate::*;
use crate::library::*;
use crate::resolve::resolve_versions;
use crate::rversion::Rversion;
use crate::run::*;
use crate::utils::*;

pub const R_ROOT: &str = "C:\\Program Files\\R";
pub const R_VERSIONDIR: &str = "R-{}";
pub const R_SYSLIBPATH: &str = "R-{}\\library";
pub const R_BINPATH: &str = "R-{}\\bin\\R.exe";

#[warn(unused_variables)]
pub fn sc_add(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    escalate("adding new R version")?;
    sc_clean_registry()?;
    let str = args
        .value_of("str")
        .ok_or(SimpleError::new("Internal argument error"))?
        .to_string();
    if str.len() >= 6 && &str[0..6] == "rtools" {
        return add_rtools(str);
    }
    let (_version, target) = download_r(&args)?;
    let target_path = Path::new(&target);

    info!("Installing {}", target_path.display());

    let mut cmd_args = vec![os("/VERYSILENT"), os("/SUPPRESSMSGBOXES")];
    if args.is_present("without-translations") {
	cmd_args.push(os("/components=main,x64,i386"));
    }
    if args.is_present("with-desktop-icon") {
	cmd_args.push(os("/mergetasks=desktopicon"));
    } else {
	cmd_args.push(os("/mergetasks=!desktopicon"));
    }

    run(target, cmd_args, "installer")?;

    let dirname = get_latest_install_path()?;

    match dirname {
        None => {
            let vers = sc_get_list()?;
            for ver in vers {
                library_update_rprofile(&ver)?;
            }
        }
        Some(ref dirname) => {
            set_default_if_none(dirname.to_string())?;
            library_update_rprofile(&dirname.to_string())?;
        }
    };
    sc_system_make_links()?;
    patch_for_rtools()?;
    maybe_update_registry_default()?;

    if !args.is_present("without-cran-mirror") {
        match dirname {
            None => {
                warn!("Cannot set CRAN mirror, cannot determine installation directory");
            }
            Some(ref dirname) => {
                set_cloud_mirror(Some(vec![dirname.to_string()]))?;
            }
        }
    }

    if !args.is_present("without-rspm") {
        match dirname {
            None => {
                warn!("Cannot set up RSPM, cannoe determine installation directory");
            }
            Some(ref dirname) => {
                set_rspm(Some(vec![dirname.to_string()]))?;
            }
        };
    }

    if !args.is_present("without-pak") {
        match dirname {
            None => {
                warn!("Cannot install pak, cannot determine installation directory");
            }
            Some(ref dirname) => {
                system_add_pak(
                    Some(vec![dirname.to_string()]),
                    args.value_of("pak-version")
                        .ok_or(SimpleError::new("Internal argument error"))?,
                    // If this is specified then we always re-install
                    args.occurrences_of("pak-version") > 0,
                )?;
            }
        }
    }

    Ok(())
}

fn add_rtools(version: String) -> Result<(), Box<dyn Error>> {
    let vers;
    if version == "rtools" {
        vers = get_rtools_needed()?;
    } else {
        vers = vec![version.replace("rtools", "")];
    }
    let client = &reqwest::Client::new();
    for ver in vers {
        let rtools42 = &ver[0..2] == "42";
        let rtools4 = &ver[0..1] == "4" || ver == "devel";
        let filename: String;
        let url: String;
        if rtools42 {
            filename = "rtools42.exe".to_string();
            url = "https://github.com/r-hub/rtools42/releases/download/latest/rtools42.exe"
                .to_string();
        } else if rtools4 {
            filename = format!("rtools{}-x86_64.exe", ver);
            url = format!(
                "https://cloud.r-project.org/bin/windows/Rtools/{}",
                filename
            );
        } else {
            filename = format!("Rtools{}.exe", ver);
            url = format!(
                "https://cloud.r-project.org/bin/windows/Rtools/{}",
                filename
            );
        };
        let tmp_dir = std::env::temp_dir().join("rig");
        let target = tmp_dir.join(&filename);
        info!("Downloading {} ->\n    {}", url, target.display());
        download_file(client, &url, &target.as_os_str())?;
        info!("Installing\n    {}", target.display());
        run(
            target.into_os_string(),
            vec![os("/VERYSILENT"), os("/SUPPRESSMSGBOXES")],
            "installer"
        )?;
    }

    Ok(())
}

fn patch_for_rtools() -> Result<(), Box<dyn Error>> {
    let vers = sc_get_list()?;
    let base = Path::new(R_ROOT);

    for ver in vers {
        let rtools42 = &ver[0..1] == "42";
        // rtools42 does not need any updates
        if rtools42 {
            continue;
        }

        let rtools4 = &ver[0..1] == "4" || ver == "devel";
        let envfile = base
            .join("R-".to_string() + &ver)
            .join("etc")
            .join("Renviron.site");
        let mut ok = envfile.exists();
        if ok {
            ok = false;
            let file = File::open(&envfile)?;
            let reader = BufReader::new(file);
            for line in reader.lines() {
                let line2 = line?;
                if line2.len() >= 14 && &line2[0..14] == "# added by rig" {
                    ok = true;
                    break;
                }
            }
        }
        if !ok {
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .append(true)
                .open(&envfile)?;

            let head = "\n".to_string() + "# added by rig, do not update by hand-----\n";
            let tail = "\n".to_string() + "# ----------------------------------------\n";
            let txt3 = head.to_owned() + "PATH=\"C:\\Rtools\\bin;${PATH}\"" + &tail;
            let txt4 = head.to_owned()
                + "PATH=\"${RTOOLS40_HOME}\\ucrt64\\bin;${RTOOLS40_HOME}\\usr\\bin;${PATH}\""
                + &tail;

            if let Err(e) = writeln!(file, "{}", if rtools4 { txt4 } else { txt3 }) {
                warn!("Couldn't write to Renviron.site file: {}", e);
            }
        }
    }

    Ok(())
}

fn get_rtools_needed() -> Result<Vec<String>, Box<dyn Error>> {
    let vers = sc_get_list()?;
    let mut res: Vec<String> = vec![];
    let base = Path::new(R_ROOT);

    for ver in vers {
        let r = base.join("R-".to_string() + &ver).join("bin").join("R.exe");
        let out = Command::new(r)
            .args(["--vanilla", "-s", "-e", "cat(as.character(getRversion()))"])
            .output()?;
        let ver: String = String::from_utf8(out.stdout)?;
        let v35 = "35".to_string();
        let v40 = "40".to_string();
        if &ver[0..1] == "3" {
            if !res.contains(&v35) {
                res.push(v35);
            }
        } else if &ver[0..1] == "4" {
            if !res.contains(&v40) {
                res.push(v40);
            }
        }
    }
    Ok(res)
}

fn set_cloud_mirror(vers: Option<Vec<String>>) -> Result<(), Box<dyn Error>> {
    let vers = match vers {
        Some(x) => x,
        None => sc_get_list()?,
    };

    info!("Setting default CRAN mirror");

    for ver in vers {
        check_installed(&ver)?;
        let path = Path::new(R_ROOT).join("R-".to_string() + ver.as_str());
        let profile = path.join("library/base/R/Rprofile".to_string());
        if !profile.exists() {
            continue;
        }

        append_to_file(
            &profile,
            vec!["options(repos = c(CRAN = \"https://cloud.r-project.org\"))".to_string()],
        )?;
    }

    Ok(())
}

fn set_rspm(vers: Option<Vec<String>>) -> Result<(), Box<dyn Error>> {
    let arch = std::env::consts::ARCH;
    if arch != "x86_64" {
        warn!("RSPM does not support this architecture: {}", arch);
        return Ok(());
    }

    let vers = match vers {
        Some(x) => x,
        None => sc_get_list()?,
    };

    let rcode = r#"
options(repos = c(RSPM="https://packagemanager.rstudio.com/all/latest", getOption("repos")))
"#;

    for ver in vers {
        check_installed(&ver)?;
        let path = Path::new(R_ROOT).join("R-".to_string() + ver.as_str());
        let profile = path.join("library/base/R/Rprofile".to_string());
        if !profile.exists() {
            continue;
        }

        append_to_file(&profile, vec![rcode.to_string()])?;
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
        let verstr = ver.to_string();
        if verstr.len() >= 6 && &verstr[0..6] == "rtools" {
            rm_rtools(verstr)?;
            continue;
        }
        check_installed(&verstr)?;

        if let Some(ref default) = default {
            if default == ver {
                warn!("Removing default version, set new default with \
                       <bold>rig default <version></>");
		match unset_default() {
		    Err(e) => warn!("Failed to unset default version: {}", e.to_string()),
		    _ => {}
		};
            }
        }

        let ver = "R-".to_string() + ver;
        let dir = Path::new(R_ROOT);
        let dir = dir.join(ver);
        info!("Removing {}", dir.display());
        remove_dir_all(&dir)?;
    }

    sc_clean_registry()?;
    sc_system_make_links()?;

    Ok(())
}

fn rm_rtools(ver: String) -> Result<(), Box<dyn Error>> {
    let dir = Path::new("C:\\").join(ver);
    info!("Removing {}", dir.display());
    match remove_dir_all(&dir) {
        Err(_err) => {
            let cmd = format!("del -recurse -force {}", dir.display());
            let out = Command::new("powershell")
                .args(["-command", &cmd])
                .output()?;
            let stderr = match std::str::from_utf8(&out.stderr) {
                Ok(v) => v,
                Err(_v) => "cannot parse stderr",
            };
            if !out.status.success() {
                bail!("Cannot remove {}: {}", dir.display(), stderr);
            }
        }
        _ => {}
    }

    sc_clean_registry()?;

    Ok(())
}

pub fn sc_system_make_links() -> Result<(), Box<dyn Error>> {
    escalate("making R-* quick shortcuts")?;
    let vers = sc_get_list()?;
    let base = Path::new(R_ROOT);
    let bin = base.join("bin");
    let mut new_links: Vec<String> = vec![
        "RS.bat".to_string(),
        "R.bat".to_string(),
        "Rscript.bat".to_string(),
    ];

    std::fs::create_dir_all(bin)?;

    for ver in vers {
        let filename = "R-".to_string() + &ver + ".bat";
        let linkfile = base.join("bin").join(&filename);
        new_links.push(filename);
        let target = base.join("R-".to_string() + &ver);

        let cnt = "@\"C:\\Program Files\\R\\R-".to_string() + &ver + "\\bin\\R\" %*\n";
        let op;
        if linkfile.exists() {
            op = "Updating";
            let orig = std::fs::read_to_string(&linkfile)?;
            if orig == cnt {
                continue;
            }
        } else {
            op = "Adding";
        };
        info!("{} R-{} -> {}", op, ver, target.display());
        let mut file = File::create(&linkfile)?;
        file.write_all(cnt.as_bytes())?;
    }

    // Delete the ones we don't need
    let old_links = std::fs::read_dir(base.join("bin"))?;
    for path in old_links {
        let path = path?;
        match path.file_name().into_string() {
            Err(_) => continue,
            Ok(filename) => {
                if !filename.ends_with(".bat") {
                    continue;
                }
                if !filename.starts_with("R-") {
                    continue;
                }
                if !new_links.contains(&filename) {
                    info!("Deleting unused {}", filename);
                    match std::fs::remove_file(path.path()) {
                        Ok(_) => {}
                        Err(e) => {
                            warn!("Failed to remove {}: {}", filename, e.to_string());
                        }
                    }
                }
            }
        };
    }

    Ok(())
}

pub fn sc_system_allow_core_dumps(_args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    // Nothing to do on Windows
    Ok(())
}

pub fn sc_system_allow_debugger(_args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    // Nothing to do on Windows
    Ok(())
}

pub fn sc_system_allow_debugger_rstudio(_args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    // Nothing to do on Windows
    Ok(())
}

pub fn sc_system_make_orthogonal(_args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    // Nothing to do on Windows
    Ok(())
}

pub fn sc_system_fix_permissions(_args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    // Nothing to do on Windows
    Ok(())
}

pub fn sc_system_forget() -> Result<(), Box<dyn Error>> {
    // Nothing to do on Windows
    Ok(())
}

pub fn sc_system_no_openmp(_args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    // Nothing to do on Windows
    Ok(())
}

pub fn get_resolve(args: &ArgMatches) -> Result<Rversion, Box<dyn Error>> {
    let str = args
        .value_of("str")
        .ok_or(SimpleError::new("Internal argument error"))?
        .to_string();

    let eps = vec![str];
    let version = resolve_versions(eps, "win".to_string(), "default".to_string(), None)?;
    Ok(version[0].to_owned())
}

// ------------------------------------------------------------------------

pub fn sc_get_list() -> Result<Vec<String>, Box<dyn Error>> {
    let mut vers = Vec::new();
    if !Path::new(R_ROOT).exists() {
        return Ok(vers);
    }

    let paths = std::fs::read_dir(R_ROOT)?;

    for de in paths {
        let path = de?.path();
        match path.file_name() {
            None => continue,
            Some(fname) => {
                match fname.to_str() {
                    None => continue,
                    Some(fname) => {
                        if &fname[0..2] == "R-" {
                            let v = fname[2..].to_string();
                            vers.push(v);
                        }
                    }
                };
            }
        }
    }

    vers.sort();
    Ok(vers)
}

pub fn sc_set_default(ver: &str) -> Result<(), Box<dyn Error>> {
    check_installed(&ver.to_string())?;
    escalate("setting the default R version")?;
    let base = Path::new(R_ROOT);
    let bin = base.join("bin");
    std::fs::create_dir_all(&bin)?;

    let linkfile = bin.join("R.bat");
    let cnt =
        "::".to_string() + &ver + "\n" + "@\"C:\\Program Files\\R\\R-" + &ver + "\\bin\\R\" %*\n";
    let mut file = File::create(linkfile)?;
    file.write_all(cnt.as_bytes())?;

    let linkfile2 = base.join("bin").join("RS.bat");
    let mut file2 = File::create(linkfile2)?;
    file2.write_all(cnt.as_bytes())?;

    let linkfile3 = base.join("bin").join("Rscript.bat");
    let mut file3 = File::create(linkfile3)?;
    let cnt3 = "::".to_string()
        + &ver
        + "\n"
        + "@\"C:\\Program Files\\R\\R-"
        + &ver
        + "\\bin\\Rscript\" %*\n";
    file3.write_all(cnt3.as_bytes())?;

    update_registry_default()?;

    Ok(())
}

pub fn unset_default() -> Result<(), Box<dyn Error>> {
    escalate("unsetting the default R version")?;

    fn try_rm(x: &str) {
	let bin = Path::new(R_ROOT).join("bin");
	let f = bin.join(x);
	if f.exists() {
	    match std::fs::remove_file(&f) {
		Err(e) => warn!("Failed to remove {}: {}", f.display(), e.to_string()),
		_ => {}
	    };
	}
    }

    try_rm("R.bat");
    try_rm("RS.bat");
    try_rm("Rscript.bat");

    unset_registry_default()?;

    Ok(())
}

pub fn sc_get_default() -> Result<Option<String>, Box<dyn Error>> {
    let base = Path::new(R_ROOT);
    let linkfile = base.join("bin").join("R.bat");
    if !linkfile.exists() {
        return Ok(None);
    }
    let file = File::open(linkfile)?;
    let reader = BufReader::new(file);

    let mut first = "".to_string();
    for line in reader.lines() {
        first = line?.replace("::", "");
        break;
    }

    Ok(Some(first.to_string()))
}

fn clean_registry_r(key: &RegKey) -> Result<(), Box<dyn Error>> {
    for nm in key.enum_keys() {
        let nm = nm?;
        let subkey = key.open_subkey(&nm)?;
        let path: String = subkey.get_value("InstallPath")?;
        let path2 = Path::new(&path);
        if !path2.exists() {
            debug!("Cleaning registry: R {} (not in {})", &nm, path);
            key.delete_subkey_all(nm)?;
        }
    }
    Ok(())
}

fn clean_registry_rtools(key: &RegKey) -> Result<(), Box<dyn Error>> {
    for nm in key.enum_keys() {
        let nm = nm?;
        let subkey = key.open_subkey(&nm)?;
        let path: String = subkey.get_value("InstallPath")?;
        let path2 = Path::new(&path);
        if !path2.exists() {
            debug!("Cleaning registry: Rtools {} (not in {})", &nm, path);
            key.delete_subkey_all(nm)?;
        }
    }
    Ok(())
}

fn clean_registry_uninst(key: &RegKey) -> Result<(), Box<dyn Error>> {
    for nm in key
        .enum_keys()
        .map(|x| x.unwrap())
        .filter(|x| x.starts_with("Rtools") || x.starts_with("R for Windows"))
    {
        let subkey = key.open_subkey(&nm).unwrap();
        let path: String = subkey.get_value("InstallLocation").unwrap();
        let path2 = Path::new(&path);
        if !path2.exists() {
            debug!("Cleaning registry (uninstaller): {}", nm);
            key.delete_subkey_all(nm).unwrap();
        }
    }
    Ok(())
}

pub fn sc_clean_registry() -> Result<(), Box<dyn Error>> {
    escalate("cleaning up the Windows registry")?;

    info!("Cleaning leftover registry entries");

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);

    let r64r = hklm.open_subkey("SOFTWARE\\R-core\\R");
    if let Ok(x) = r64r {
        clean_registry_r(&x)?;
    };
    let r64r64 = hklm.open_subkey("SOFTWARE\\R-core\\R64");
    if let Ok(x) = r64r64 {
        clean_registry_r(&x)?;
    };
    let r32r = hklm.open_subkey("SOFTWARE\\WOW6432Node\\R-core\\R");
    if let Ok(x) = r32r {
        clean_registry_r(&x)?;
    };
    let r32r32 = hklm.open_subkey("SOFTWARE\\WOW6432Node\\R-core\\R32");
    if let Ok(x) = r32r32 {
        clean_registry_r(&x)?;
    };

    let rtools64 = hklm.open_subkey("SOFTWARE\\R-core\\Rtools");
    if let Ok(x) = rtools64 {
        clean_registry_rtools(&x)?;
        if x.enum_keys().count() == 0 {
            hklm.delete_subkey("SOFTWARE\\R-core\\Rtools")?;
        }
    };
    let rtools32 = hklm.open_subkey("SOFTWARE\\WOW6432Node\\R-core\\Rtools");
    if let Ok(x) = rtools32 {
        clean_registry_rtools(&x)?;
        if x.enum_keys().count() == 0 {
            hklm.delete_subkey("SOFTWARE\\WOW6432Node\\R-core\\Rtools")?;
        }
    };

    let uninst = hklm.open_subkey("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall");
    if let Ok(x) = uninst {
        clean_registry_uninst(&x)?;
    };
    let uninst32 =
        hklm.open_subkey("SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall");
    if let Ok(x) = uninst32 {
        clean_registry_uninst(&x)?;
    };

    Ok(())
}

fn maybe_update_registry_default() -> Result<(), Box<dyn Error>> {
    let base = Path::new(R_ROOT);
    let linkfile = base.join("bin").join("R.bat");
    if linkfile.exists() {
        update_registry_default()?;
    }
    Ok(())
}

fn update_registry_default1(key: &RegKey, ver: &String) -> Result<(), Box<dyn Error>> {
    key.set_value("Current Version", ver)?;
    let inst = R_ROOT.to_string() + "\\R-" + ver;
    key.set_value("InstallPath", &inst)?;
    Ok(())
}

fn update_registry_default_to(default: &String) -> Result<(), Box<dyn Error>> {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let r64r = hklm.create_subkey("SOFTWARE\\R-core\\R");
    if let Ok(x) = r64r {
        let (key, _) = x;
        update_registry_default1(&key, &default)?;
    }
    let r64r64 = hklm.create_subkey("SOFTWARE\\R-core\\R64");
    if let Ok(x) = r64r64 {
        let (key, _) = x;
        update_registry_default1(&key, &default)?;
    }
    Ok(())
}

fn update_registry_default() -> Result<(), Box<dyn Error>> {
    escalate("Update registry default")?;
    let default = sc_get_default_or_fail()?;
    update_registry_default_to(&default)
}

fn unset_registry_default() -> Result<(), Box<dyn Error>> {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let r64r = hklm.create_subkey("SOFTWARE\\R-core\\R");
    if let Ok(x) = r64r {
        let (key, _) = x;
	key.delete_value("Current Version")?;
	key.delete_value("InstallPath")?;
    }
    let r64r64 = hklm.create_subkey("SOFTWARE\\R-core\\R64");
    if let Ok(x) = r64r64 {
        let (key, _) = x;
	key.delete_value("Current Version")?;
	key.delete_value("InstallPath")?;
    }
    Ok(())
}

pub fn sc_system_update_rtools40() -> Result<(), Box<dyn Error>> {
    run(
	"c:\\rtools40\\usr\\bin\\bash.exe".into(),
        vec!["--login".into(), "-c".into(), "pacman -Syu --noconfirm".into()],
	"Rtools40 update"
    )
}

fn get_latest_install_path() -> Result<Option<String>, Box<dyn Error>> {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let r64r64 = hklm.open_subkey("SOFTWARE\\R-core\\R64");
    if let Ok(key) = r64r64 {
        let ip: Result<String, _> = key.get_value("InstallPath");
        if let Ok(fp) = ip {
            let ufp = fp.replace("\\", "/");
            let p = match basename(&ufp) {
                None => return Ok(None),
                Some(p) => p,
            };
            let re = Regex::new("^R-")?;
            let v = re.replace(p, "").to_string();
            return Ok(Some(v));
        }
    }
    Ok(None)
}

pub fn sc_rstudio_(version: Option<&str>, project: Option<&str>) -> Result<(), Box<dyn Error>> {
    // we only need to restore if 'ver' is given, there is a default and
    // they are different
    let def = sc_get_default()?;
    let restore = match (version, def) {
        (Some(v), Some(d)) => v != d,
        _ => false,
    };

    if let Some(_) = version {
        escalate("updating default version in registry")?;
    }

    let args = match project {
        None => vec![os("/c"), os("start"), os("/b"), os("rstudio")],
        Some(p) => vec![os("/c"), os("start"), os("/b"), os(p)],
    };

    if let Some(version) = version {
        let ver = version.to_string();
        check_installed(&ver)?;
        update_registry_default_to(&ver)?;
    }

    info!("Running cmd.exe {}", osjoin(args.to_owned(), " "));

    let status = run("cmd.exe".into(), args, "start");

    // Restore registry (well, set default), if we changed it
    // temporarily
    if restore {
        debug!("Waiting for RStudio to start");
        let twosecs = time::Duration::from_secs(2);
        thread::sleep(twosecs);
        debug!("Restoring default R version in registry");
        maybe_update_registry_default()?;
    }

    match status {
        Err(e) => { bail!("`start` failed: {}", e.to_string()); },
        _ => {}
    };

    Ok(())
}

pub fn get_system_profile(rver: &str) -> Result<PathBuf, Box<dyn Error>> {
    let path = Path::new(R_ROOT).join("R-".to_string() + rver);
    let profile = path.join("library/base/R/Rprofile");
    Ok(profile)
}

pub fn get_r_binary(rver: &str) -> Result<PathBuf, Box<dyn Error>> {
    debug!("Finding R {} binary", rver);
    let base = Path::new(R_ROOT);
    let bin = base
        .join("R-".to_string() + &rver)
        .join("bin")
        .join("R.exe");
    debug!("R {} binary: {}", rver, bin.display());
    Ok(bin)
}

pub fn check_has_pak(_rver: &str) -> Result<(), Box<dyn Error>> {
    // TODO: actually check. Right now the install will fail
    Ok(())
}
