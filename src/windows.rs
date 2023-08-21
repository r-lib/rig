#![cfg(target_os = "windows")]

use regex::Regex;
use std::error::Error;
use std::ffi::OsStr;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Write;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{thread, time};

use clap::ArgMatches;
use directories::BaseDirs;
use remove_dir_all::remove_dir_all;
use semver;
use simple_error::{bail, SimpleError};
use simplelog::*;
use whoami::{hostname, username};
use winreg::enums::*;
use winreg::RegKey;

use crate::alias::*;
use crate::common::*;
use crate::download::*;
use crate::escalate::*;
use crate::library::*;
use crate::resolve::resolve_versions;
use crate::rversion::*;
use crate::run::*;
use crate::utils::*;

pub const R_ROOT: &str = "C:\\Program Files\\R";
pub const R_VERSIONDIR: &str = "R-{}";
pub const R_SYSLIBPATH: &str = "R-{}\\library";
pub const R_BINPATH: &str = "R-{}\\bin\\R.exe";

#[warn(unused_variables)]
pub fn sc_add(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    escalate("adding new R version")?;
    let alias = get_alias(args);
    sc_clean_registry()?;
    let str = args.get_one::<String>("str").unwrap();
    if str.len() >= 6 && &str[0..6] == "rtools" {
        return add_rtools(str.to_string());
    }
    let (_version, target) = download_r(&args)?;
    let target_path = Path::new(&target);

    info!("Installing {}", target_path.display());

    let mut cmd_args = vec![os("/VERYSILENT"), os("/SUPPRESSMSGBOXES")];
    if args.get_flag("without-translations") {
	cmd_args.push(os("/components=main,x64,i386"));
    }
    if args.get_flag("with-desktop-icon") {
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
    match dirname {
	None => {},
	Some(ref dirname) => {
	    match alias {
		Some(alias) => add_alias(&dirname, &alias)?,
		None => { }
	    }
	}
    };
    patch_for_rtools()?;
    maybe_update_registry_default()?;

    if !args.get_flag("without-cran-mirror") {
        match dirname {
            None => {
                warn!("Cannot set CRAN mirror, cannot determine installation directory");
            }
            Some(ref dirname) => {
                set_cloud_mirror(Some(vec![dirname.to_string()]))?;
            }
        }
    }

    if !args.get_flag("without-p3m") {
        match dirname {
            None => {
                warn!("Cannot set up P3M, cannot determine installation directory");
            }
            Some(ref dirname) => {
                set_rspm(Some(vec![dirname.to_string()]))?;
            }
        };
    }

    if !args.get_flag("without-pak") {
        match dirname {
            None => {
                warn!("Cannot install pak, cannot determine installation directory");
            }
            Some(ref dirname) => {
                let explicit = args.value_source("pak-version") ==
                    Some(clap::parser::ValueSource::CommandLine);
                system_add_pak(
                    Some(vec![dirname.to_string()]),
                    args.get_one::<String>("pak-version").unwrap(),
                    // If this is specified then we always re-install
                    explicit
                )?;
            }
        }
    }

    Ok(())
}

fn add_rtools(version: String) -> Result<(), Box<dyn Error>> {
    let vers;
    if version == "rtools" {
        vers = get_rtools_needed(None)?;
    } else {
        vers = vec![version.replace("rtools", "")];
    }
    let client = &reqwest::Client::new();
    for ver in vers {
	let rtools43 = &ver[0..2] == "43";
        let rtools42 = &ver[0..2] == "42";
        let rtools4 = &ver[0..1] == "4";
        let filename: String;
        let url: String;
	if rtools43 {
	    let rt43=Path::new("C:\\Rtools43");
	    if rt43.exists() {
		info!("Rtools43 is already installed");
		continue;
	    }
	    filename = "rtools43.exe".to_string();
            url = "https://github.com/r-hub/rtools43/releases/download/latest/rtools43.exe"
                .to_string();
        } else if rtools42 {
	    let rt42=Path::new("C:\\Rtools42");
	    if rt42.exists() {
		info!("Rtools42 is already installed");
		continue;
	    }
            filename = "rtools42.exe".to_string();
            url = "https://github.com/r-hub/rtools42/releases/download/latest/rtools42.exe"
                .to_string();
        } else if rtools4 {
	    let rt40=Path::new("C:\\Rtools40");
	    if rt40.exists() {
		info!("Rtools40 is already installed");
		continue;
	    }
            filename = format!("rtools{}-x86_64.exe", ver);
            url = format!(
                "https://cloud.r-project.org/bin/windows/Rtools/{}",
                filename
            );
        } else {
	    let rt3=Path::new("C:\\Rtools");
	    if rt3.exists() {
		info!("Rtools3x is already installed");
		continue;
	    }
            filename = format!("Rtools{}.exe", ver);
            url = format!(
                "https://cloud.r-project.org/bin/windows/Rtools/{}",
                filename
            );
        };
        let tmp_dir = std::env::temp_dir().join("rig");
        let target = tmp_dir.join(&filename);
        info!("Downloading {} -> {}", url, target.display());
        download_file(client, &url, &target.as_os_str())?;
        info!("Installing {}", target.display());
        run(
            target.into_os_string(),
            vec![os("/VERYSILENT"), os("/SUPPRESSMSGBOXES")],
            "installer"
        )?;
    }

    Ok(())
}

fn patch_for_rtools() -> Result<(), Box<dyn Error>> {
    let base = Path::new(R_ROOT);
    let vers = sc_get_list()?;

    for ver in vers {
	let vver = vec![ver.to_owned()];
	let needed = get_rtools_needed(Some(vver))?;
	if needed[0] == "42" || needed[0] == "43" {
	    continue
	}
	let rtools4 = needed[0] == "40";
	let rdir = "R-".to_string() + &ver;
        let envfile = base
            .join(rdir)
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

fn get_rtools_needed(version: Option<Vec<String>>) -> Result<Vec<String>, Box<dyn Error>> {
    let vers = match version {
	None => sc_get_list()?,
	Some(x) => x
    };
    let base = Path::new(R_ROOT);
    let mut res: Vec<String> = vec![];

    for ver in vers {
        let r = base.join("R-".to_string() + &ver).join("bin").join("R.exe");
        let out = Command::new(r)
            .args(["--vanilla", "-s", "-e", "cat(as.character(getRversion()))"])
            .output()?;
        let ver: String = String::from_utf8(out.stdout)?;
        let v35 = "35".to_string();
        let v40 = "40".to_string();
	let v43 = "43".to_string();
	let sv430 = semver::Version::parse("4.3.0")?;
	let sv = semver::Version::parse(&ver)?;
        if &ver[0..1] == "3" {
            if !res.contains(&v35) {
                res.push(v35);
            }
	} else if sv >= sv430 {
	    if !res.contains(&v43) {
		res.push(v43)
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
        let ver = check_installed(&ver)?;
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
        warn!("P3M does not support this architecture: {}", arch);
        return Ok(());
    }

    let vers = match vers {
        Some(x) => x,
        None => sc_get_list()?,
    };

    let rcode = r#"
options(repos = c(P3M="https://packagemanager.posit.co/cran/latest", getOption("repos")))
"#;

    for ver in vers {
        let ver = check_installed(&ver)?;
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
    let vers = args.get_many::<String>("version");
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
        let ver = check_installed(&verstr)?;

        if let Some(ref default) = default {
            if default == &ver {
                warn!("Removing default version, set new default with \
                       <bold>rig default <version></>");
		match unset_default() {
		    Err(e) => warn!("Failed to unset default version: {}", e.to_string()),
		    _ => {}
		};
            }
        }

        let ver = "R-".to_string() + &ver;
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
    let re_als = re_alias();
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
		if re_als.is_match(&filename) {
		    let rver = find_r_version_in_link(&path.path())?;
		    let realname = "R-".to_string() + &rver + ".bat";
		    if new_links.contains(&realname) {
			continue;
		    }
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

fn re_alias() -> Regex {
    let re = Regex::new("^R-(oldrel|release|next)[.]bat$").unwrap();
    re
}

pub fn find_aliases() -> Result<Vec<Alias>, Box<dyn Error>> {
    debug!("Finding existing aliases");

    let mut result: Vec<Alias> = vec![];
    let bin = Path::new(R_ROOT).join("bin");

    if !bin.exists() {
	return Ok(result);
    }

    let paths = std::fs::read_dir(bin)?;
    let re = re_alias();

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
	    trace!("Checking {}", path.display());
	    let rver = find_r_version_in_link(&path)?;
	    let als = Alias {
		alias: fnamestr[2..fnamestr.len()-4].to_string(),
		version: rver
	    };
	    result.push(als);
	}
    }

    Ok(result)
}

fn find_r_version_in_link(path: &PathBuf) -> Result<String, Box<dyn Error>> {
    let lines = read_lines(path)?;
    if lines.len() == 0 {
	bail!("Invalid R link file: {}", path.display());
    }
    let split = lines[0].split("\\").collect::<Vec<&str>>();
    for s in split {
	if s == "R-devel" {
	    return Ok("devel".to_string());
	}
	if s.starts_with("R-") {
	    return Ok(s[2..].to_string());
	}
    }
    bail!("Cannot extract R version from {}, invalid R link file?", path.display());
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

pub fn sc_system_detect_os(_args: &ArgMatches, _mainargs: &ArgMatches)
                           -> Result<(), Box<dyn Error>> {
    // Nothing to do on Windows
    Ok(())
}

pub fn get_resolve(args: &ArgMatches) -> Result<Rversion, Box<dyn Error>> {
    let str = args.get_one::<String>("str").unwrap();
    let eps = vec![str.to_string()];
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
    let ver = check_installed(&ver.to_string())?;
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

// All this is from https://github.com/rstudio/rstudio/blob/44f09c50d469a14d5a9c3840c7a239f3bf21ace9/src/cpp/core/system/Xdg.cpp#L85
//
// 1. RSTUDIO_CONFIG_HOME might point to the final path.
// 2. XDG_CONFIG_HOME might point to the user config path (append RStudio).
// 3. Otherwise query FOLDERID_RoamingAppData for the user config path.
// 4. Or fall back to `~/.config` if that fails (it really should not).
// 5. Set USER, HOME and HOSTNAME env vars for expansion.
// 6. Expand env vars, both $ENV and ${ENV} types.
// 7. Expand ~ to home dir
//
// 5-6 are only needed if the path has a '$' or '~' character.
// 7 is only needed if the path has a '~' character or if it is empty.

pub fn get_rstudio_config_path() -> Result<std::path::PathBuf, Box<dyn Error>> {
    let mut final_path = true;
    let mut xdg: Option<std::path::PathBuf> = None;

    // RSTUDIO_CONFIG_HOME may point to the final path
    match std::env::var("RSTUDIO_CONFIG_HOME") {
	Ok(x)  => xdg = Some(std::path::PathBuf::from(x)),
	Err(_) => { }
    };

    // XDG_CONFIG_HOME may point to the user config path
    if xdg.is_none() {
	final_path = false;
	match std::env::var("XDG_CONFIG_HOME") {
	    Ok(x) => {
		let mut xdg2 = std::path::PathBuf::new();
		xdg2.push(x);
		xdg = Some(xdg2);
	    },
	    Err(_) => { }
	};
    }

    // Get user config path from system
    if xdg.is_none() {
	if let Some(bd) = BaseDirs::new() {
	    xdg = Some(std::path::PathBuf::from(bd.config_dir()));
	}
    }

    // Fall back to `~/.config`
    if xdg.is_none() {
	xdg = Some(std::path::PathBuf::from("~/.config"));
    }

    // We have a path at this point
    let xdg = xdg.unwrap();

    // Set USER, HOME, HOSTNAME, for expansion, if needed
    let mut path = match xdg.to_str() {
	Some(x) => String::from(x),
	None => bail!("RStudio config path cannot be represented as Unicode. :(")
    };
    let has_dollar = path.contains("$");
    let empty = path.len() == 0;
    let has_tilde = path.contains("~");

    if has_dollar || empty || has_tilde {
	match std::env::var("USER") {
	    Ok(_) => { },
	    Err(_) => {
		let username = username();
		std::env::set_var("USER", username);
	    }
	};
    }

    if has_dollar {
	match std::env::var("HOME") {
	    Ok(_) => { },
	    Err(_) => {
		let bd = BaseDirs::new();
		match bd {
		    Some(x) => {
			std::env::set_var("HOME", x.home_dir().as_os_str());
		    },
		    None    => {
			warn!("Cannot determine HOME directory");
		    }
		};
	    }
	};
    }

    if has_dollar {
	match std::env::var("HOSTNAME") {
	    Ok(_) => { },
	    Err(_) => {
		let hostname = hostname();
		std::env::set_var("HOSTNAME", hostname);
	    }
	};
    }

    // Expand env vars
    if has_dollar {
	path = match shellexpand::env(&path) {
	    Ok(x) => x.to_string(),
	    Err(e) => {
		bail!("RStudio config path contains unknown environment variable: {}", e.var_name);
	    }
	};
    }

    // Expand empty path and ~
    if empty || has_tilde {
	path = shellexpand::tilde(&path).to_string();
    }

    let mut xdg = std::path::PathBuf::from(path);
    if !final_path {
	xdg.push("RStudio");
    }

    Ok(xdg)
}

pub fn sc_rstudio_(version: Option<&str>, project: Option<&str>, arg: Option<&OsStr>)
                   -> Result<(), Box<dyn Error>> {
    debug!("Looking into starting RStudio");

    let def = sc_get_default()?;

    // we only need to restore if 'ver' is given, there is a default and
    // they are different
    let restore = match (version, &def) {
        (Some(v), Some(d)) => v != d,
        _ => false,
    };

    // def is None if there is no default R version set
    let version = match version {
	Some(x) => Some(x.to_string()),
	None    => def
    };

    if let Some(_) = version {
        escalate("updating default version in registry")?;
    }

    let mut args = match project {
        None => vec![os("/c"), os("start"), os("/b"), os("rstudio")],
        Some(p) => vec![os("/c"), os("start"), os("/b"), os(p)],
    };

    if let Some(arg) = arg {
	args.push(arg.to_os_string());
    }

    if let Some(version) = version {
        let ver = version.to_string();
        let ver = check_installed(&ver)?;
	debug!("Updating R version in registry to {}.", ver);
        update_registry_default_to(&ver)?;

	// Update RStudio config, this is needed for newer RStudio versions.
        let config_path = get_rstudio_config_path();
	if config_path.is_ok() {
	    let mut config_path = config_path.unwrap();
	    config_path.push("config.json");
	    if config_path.exists() {
		let re = Regex::new(
		    "\"rExecutablePath\":\\s*\"C:\\\\\\\\Program Files\\\\\\\\R\\\\\\\\R-.*\\\\\\\\bin\\\\\\\\x64\\\\\\\\Rterm.exe\""
		).unwrap();
		let sub = "\"rExecutablePath\": \"C:\\\\Program Files\\\\R\\\\R-".to_string() +
		    &ver + "\\\\bin\\\\x64\\\\Rterm.exe\"";
		match replace_in_file(&config_path, &re, &sub) {
		    Ok(_) => {
			debug!("Updated RStudio config at {}", config_path.display());
		    },
		    Err(x) => {
			warn!(
			    "Cannot update RStudio config file at {}: {}",
			    config_path.display(),
			    x.to_string()
			);
		    }
		}
	    }
	}
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
