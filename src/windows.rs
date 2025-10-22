#![cfg(target_os = "windows")]

use regex::Regex;
use std::error::Error;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Write;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::ArgMatches;
use directories::BaseDirs;
use remove_dir_all::remove_dir_all;
use semver;
use simple_error::{bail, SimpleError};
use simplelog::*;
use tabular::*;
use whoami::{fallible::hostname, username};
use winreg::enums::*;
use winreg::RegKey;

use crate::alias::*;
use crate::common::*;
use crate::download::*;
use crate::escalate::*;
use crate::library::*;
use crate::resolve::*;
use crate::rversion::*;
use crate::run::*;
use crate::utils::*;
use crate::windows_arch::*;

// get_r_root(): returns the directory where the R versions are installed
// RIG_LINKS_DIR: directory where the quick links are created
// R_VERSION_DIR: name of the directory of a single R version inside R_ROOT()
// R_SYSLIBPATH: path to the system library of an R version from R_ROOT()
// R_BINPATH: path of the R executable from R_ROOT()

pub const RIG_LINKS_DIR: &str = "C:\\Program Files\\R\\bin";
pub const R_VERSIONDIR: &str = "R-{}";
pub const R_SYSLIBPATH: &str = "R-{}\\library";
pub const R_BINPATH: &str = "R-{}\\bin\\R.exe";

macro_rules! osvec {
    // match a list of expressions separated by comma:
    ($($str:expr),*) => ({
        // create a Vec with this list of expressions,
        // calling String::from on each:
        vec![$(OsString::from($str),)*] as Vec<OsString>
    });
}

pub fn get_r_root() -> String {
    // x86_64 R on x86_64 Windows
    const R_ROOT_: &str = "C:\\Program Files\\R";
    // x86_64 R on aarch64 Windows
    // const R_X86_ROOT_: &str = "C:\\Program Files (x86)\\R";
    // aarch64 R on aarch64 Windows
    const R_AARCH64_ROOT_: &str = "C:\\Program Files\\R-aarch64";
    if get_native_arch() == "aarch64" {
	R_AARCH64_ROOT_.to_string()
    } else {
	R_ROOT_.to_string()
    }
}

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
    let myarch = get_native_arch();
    let client = &reqwest::Client::new();
    for ver in vers {
	let versuffix = if &ver[0..1] != "3" { &ver } else { "" };
	let archsuffix = if myarch == "aarch64" { "-aarch64" } else { "" };
	let instdir = "C:\\Rtools".to_string() + versuffix + archsuffix;
	let instdirpath = Path::new(&instdir);
	if instdirpath.exists() {
	    info!("Rtools{} is already installed", ver);
	    continue;
	}
	let rtver = get_rtools_version(&ver, &myarch)?;
	let url = rtver.url;
	let filename = "rtools-".to_string() + &ver + "-" + &myarch + ".exe";

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
    let rroot = get_r_root();
    let base = Path::new(&rroot);
    let vers = sc_get_list()?;

    for ver in vers {
	let vver = vec![ver.to_owned()];
	let needed = get_rtools_needed(Some(vver))?;
	if needed[0] != "35" && needed[0] != "40" {
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
    let rroot = get_r_root();
    let base = Path::new(&rroot);
    let mut res: Vec<String> = vec![];
    let errmsg = "Cannot parse list of Rtools versions.";
    let rtversval = get_available_rtools_versions(&get_native_arch());
    let rtvers = match rtversval.as_array() {
	Some(x) => x,
	None => bail!(errmsg)
    };

    for ver in vers {
        let r = base.join("R-".to_string() + &ver).join("bin").join("R.exe");
        let out = Command::new(r)
            .args(["--vanilla", "-s", "-e", "cat(as.character(getRversion()))"])
            .output()?;
        let ver: String = String::from_utf8(out.stdout)?;
	let sver = semver::Version::parse(&ver)?;
	debug!("Get Rtools version for R {}.", ver);
	for rtver in rtvers {
	    let first = rtver["first"].as_str().ok_or(errmsg)?;
	    let last = rtver["last"].as_str().ok_or(errmsg)?;
	    let first = semver::Version::parse(first)?;
	    let last = semver::Version::parse(last)?;
	    if first <= sver && sver <= last {
		let rtverver = rtver["version"].as_str().ok_or(errmsg)?.to_string();
		debug!("R {} needs Rtools {}.", ver, rtverver);
		if !res.contains(&rtverver) {
		    res.push(rtverver);
		}
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

    for ver in vers {
	info!("Setting default CRAN mirror for R-{}", ver);
        let ver = check_installed(&ver)?;
        let path = Path::new(&get_r_root()).join("R-".to_string() + ver.as_str());
        let profile = path.join("library/base/R/Rprofile".to_string());
        if !profile.exists() {
	    warn!(
		"Cannot set default CRAN mirror, profile at {} does not exist.",
		profile.display()
	    );
            continue;
        }

        append_to_file(
            &profile,
            vec![
r#"if (Sys.getenv("RSTUDIO") != "1" && Sys.getenv("POSITRON") != "1") {
  options(repos = c(CRAN = "https://cloud.r-project.org"))
}"#.to_string()
            ],
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
if (Sys.getenv("RSTUDIO") != "1" && Sys.getenv("POSITRON") != "1") {
  options(repos = c(P3M="https://packagemanager.posit.co/cran/latest", getOption("repos")))
}
"#;

    for ver in vers {
        let ver = check_installed(&ver)?;
        let path = Path::new(&get_r_root()).join("R-".to_string() + ver.as_str());
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
	let rroot = get_r_root();
        let dir = Path::new(&rroot);
        let dir = dir.join(ver);
        info!("Removing {}", dir.display());
        remove_dir_all(&dir)?;
    }

    sc_clean_registry()?;
    sc_system_make_links()?;

    Ok(())
}

fn rm_rtools(ver: String) -> Result<(), Box<dyn Error>> {
    let arch = get_native_arch();
    let ver = if arch == "aarch64" { ver + "-aarch64" } else { ver };
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
    let linkdir = Path::new(RIG_LINKS_DIR);
    let mut new_links: Vec<String> = vec![
        "RS.bat".to_string(),
        "R.bat".to_string(),
        "Rscript.bat".to_string(),
    ];
    let rroot = get_r_root();
    let base = Path::new(&rroot);

    std::fs::create_dir_all(linkdir)?;

    for ver in vers {
        let filename = "R-".to_string() + &ver + ".bat";
        let linkfile = linkdir.join(&filename);
        new_links.push(filename);
        let target = base.join("R-".to_string() + &ver);

        let cnt = "@\"".to_string() + &rroot + "\\R-" + &ver + "\\bin\\R\" %*\n";
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
    let old_links = std::fs::read_dir(linkdir)?;
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
    let mut result: Vec<Alias> = vec![];
    let bin = Path::new(RIG_LINKS_DIR);
    debug!("Finding existing aliases in {}", bin.display());

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
	debug!("Alias candidate: {}", fnamestr);
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
	if s != "R-aarch64" && s.starts_with("R-") {
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

pub fn sc_system_detect_platform(_args: &ArgMatches, _mainargs: &ArgMatches)
                                 -> Result<(), Box<dyn Error>> {
    // Nothing to do on Windows
    Ok(())
}

// ------------------------------------------------------------------------

pub fn sc_get_list() -> Result<Vec<String>, Box<dyn Error>> {
    let mut vers = Vec::new();
    if !Path::new(&get_r_root()).exists() {
        return Ok(vers);
    }

    let paths = std::fs::read_dir(&get_r_root())?;

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
    let rroot = get_r_root();
    let linkdir = Path::new(RIG_LINKS_DIR);
    std::fs::create_dir_all(&linkdir)?;

    let linkfile = linkdir.join("R.bat");
    let cnt =
        "::".to_string() + &ver + "\n" + "@\"" + &rroot + "\\R-" + &ver + "\\bin\\R\" %*\n";
    let mut file = File::create(linkfile)?;
    file.write_all(cnt.as_bytes())?;

    let linkfile2 = linkdir.join("RS.bat");
    let mut file2 = File::create(linkfile2)?;
    file2.write_all(cnt.as_bytes())?;

    let linkfile3 = linkdir.join("Rscript.bat");
    let mut file3 = File::create(linkfile3)?;
    let cnt3 = "::".to_string()
        + &ver
        + "\n"
        + "@\"" + &rroot + "\\R-"
        + &ver
        + "\\bin\\Rscript\" %*\n";
    file3.write_all(cnt3.as_bytes())?;

    update_registry_default()?;

    Ok(())
}

pub fn unset_default() -> Result<(), Box<dyn Error>> {
    escalate("unsetting the default R version")?;

    fn try_rm(x: &str) {
	let linkdir = Path::new(RIG_LINKS_DIR);
	let f = linkdir.join(x);
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
    let linkdir = Path::new(RIG_LINKS_DIR);
    let linkfile = linkdir.join("R.bat");
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
    let r32r64 = hklm.open_subkey("SOFTWARE\\WOW6432Node\\R-core\\R64");
    if let Ok(x) = r32r64 {
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
    let linkdir = Path::new(RIG_LINKS_DIR);
    let linkfile = linkdir.join("R.bat");
    if linkfile.exists() {
        update_registry_default()?;
    }
    Ok(())
}

fn update_registry_default1(key: &RegKey, ver: &String) -> Result<(), Box<dyn Error>> {
    key.set_value("Current Version", ver)?;
    let inst = get_r_root().to_string() + "\\R-" + ver;
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

pub fn sc_system_rtools(args: &ArgMatches, mainargs: &ArgMatches)
                        -> Result<(), Box<dyn Error>> {

    match args.subcommand() {
	Some(("add", s)) => sc_rtools_add(s, mainargs),
	Some(("list", s)) => sc_rtools_ls(s, mainargs),
	Some(("rm", s)) => sc_rtools_rm(s, mainargs),
	_ => Ok(()), // unreachable
    }
}

fn sc_rtools_add(args: &ArgMatches, _mainargs: &ArgMatches)
                 -> Result<(), Box<dyn Error>> {
    escalate("adding Rtools")?;
    let ver = args.get_one::<String>("version").unwrap();
    if ver == "all" {
	add_rtools("rtools".to_string())
    } else if ver.starts_with("rtools") {
	add_rtools(ver.to_string())
    } else {
	add_rtools("rtools".to_string() + ver)
    }
}

fn sc_rtools_rm(args: &ArgMatches, _mainargs: &ArgMatches)
		-> Result<(), Box<dyn Error>> {
    escalate("removing Rtools")?;
    let vers = args.get_many::<String>("version");
    if vers.is_none() {
	return Ok(())
    }
    let vers = vers.ok_or(SimpleError::new("Internal argument error"))?;

    for ver in vers {
	if ver == "all" {
	    rm_rtools("rtools".to_string())?;
	} else if ver.starts_with("rtools") {
	    rm_rtools(ver.to_string())?;
	} else {
	    rm_rtools("rtools".to_string() + ver)?;
	}
    }

    Ok(())
}

#[derive(Default, Debug, Clone)]
pub struct RtoolsVersion {
    pub name: String,
    pub version: String,
    pub fullversion: String,
    pub path: String
}


fn get_rtools_versions(rtoolskey: &RegKey)
		       -> Result<Vec<RtoolsVersion>, Box<dyn Error>> {
    let mut versions: Vec<RtoolsVersion> = vec![];
    for nm in rtoolskey.enum_keys() {
	let nm = nm?;
	let subkey = rtoolskey.open_subkey(&nm)?;
	// e.g. 4.3.5948.5818
	let fullversion: String = subkey.get_value("FullVersion")?;
	let path: String = subkey.get_value("InstallPath")?;
	let verparts: Vec<_> = nm.split(".").collect();
	// e.g. 4.3
	let version = verparts[0..2].join(".");
	// e.g. 43
	let name = verparts[0..2].join("");
	versions.push(RtoolsVersion {
	    name,
	    version,
	    fullversion,
	    path
	});
    }
    Ok(versions)
}

fn sc_rtools_ls(args: &ArgMatches, mainargs: &ArgMatches)
                -> Result<(), Box<dyn Error>> {
    escalate("listing Rtools in the registry")?;
    let mut versions: Vec<RtoolsVersion> = vec![];

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let rtools32 = hklm.open_subkey("SOFTWARE\\WOW6432Node\\R-core\\Rtools");
    if let Ok(key) = rtools32 {
	versions.append(&mut get_rtools_versions(&key)?);
    }
    let rtools64 = hklm.open_subkey("SOFTWARE\\R-core\\Rtools");
    if let Ok(key) = rtools64 {
	versions.append(&mut get_rtools_versions(&key)?);
    }

    let json = args.get_flag("json") || mainargs.get_flag("json");
    if json {
	let num = versions.len();
	println!("[");
	for (idx, item) in versions.into_iter().enumerate() {
            println!("{{");
            println!("  \"name\": \"{}\",", item.name);
            println!("  \"version\": \"{}\",", item.version);
            println!("  \"fullversion\": \"{}\",", item.fullversion);
            println!("  \"path\": \"{}\",", item.path);
            println!("}}{}", if idx == num - 1 { "" } else { "," });
	}
	println!("]");
    } else {
	let mut tab = Table::new("{:<}  {:<}  {:<}  {:<}");
	tab.add_row(row!["name", "version", "full-version", "path"]);
        tab.add_heading("------------------------------------------");
	for item in versions {
	    tab.add_row(row!(item.name, item.version, item.fullversion, item.path));
	}
	println!("{}", tab);
    }

    Ok(())
}

fn get_latest_install_path() -> Result<Option<String>, Box<dyn Error>> {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let arch = get_native_arch();
    let key = if arch == "aarch64" { "SOFTWARE\\R-core\\R" } else { "SOFTWARE\\R-core\\R64" };
    let r64r64 = hklm.open_subkey(key);
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
		let hostname = match hostname() {
		    Ok(x) => x,
		    Err(_) => "localhost".to_string()
		};
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

pub fn sc_rstudio_(version: Option<&str>,
		   project: Option<&str>,
		   arg: Option<&OsStr>)
                   -> Result<(), Box<dyn Error>> {
    debug!("Looking into starting RStudio");

    let def = sc_get_default()?;

    // def is None if there is no default R version set
    let version = match version {
	Some(x) => Some(x.to_string()),
	None    => def
    };

    let mut args = match project {
        None => osvec!["/c", "start", "/b", "rstudio"],
        Some(p) => osvec!["/c", "start", "/b", p],
    };

    if let Some(arg) = arg {
	args.push(arg.to_os_string());
    }

    // set version env var if needed
    let old = std::env::var("RSTUDIO_WHICH_R");
    if let Some(ref v) = version {
	let ver = v.to_string();
	let ver = check_installed(&ver)?;
	// TODO: this does not work aarch64 windows
	let bin = get_r_binary_x64(&ver)?;
	info!("Setting RSTUDIO_WHICH_R=\"{}\"", bin.display());
	std::env::set_var("RSTUDIO_WHICH_R", bin);
    }

    info!("Running cmd.exe {}", osjoin(args.to_owned(), " "));
    let status = run("cmd.exe".into(), args, "start");

    // restore version env var
    if let Some(_) = version {
	match old {
	    Ok(v) => std::env::set_var("RSTUDIO_WHICH_R", v),
	    Err(_) => std::env::remove_var("RSTUDIO_WHICH_R")
	};
    }

    match status {
        Err(e) => { bail!("`start` failed: {}", e.to_string()); },
        _ => {}
    };

    Ok(())
}

pub fn get_system_profile(rver: &str) -> Result<PathBuf, Box<dyn Error>> {
    let path = Path::new(&get_r_root()).join("R-".to_string() + rver);
    let profile = path.join("library/base/R/Rprofile");
    Ok(profile)
}

pub fn get_r_binary(rver: &str) -> Result<PathBuf, Box<dyn Error>> {
    debug!("Finding R {} binary", rver);
    let rroot = get_r_root();
    let base = Path::new(&rroot);
    let bin = base
        .join("R-".to_string() + &rver)
        .join("bin")
        .join("R.exe");
    debug!("R {} binary: {}", rver, bin.display());
    Ok(bin)
}

pub fn get_r_binary_x64(rver: &str) -> Result<PathBuf, Box<dyn Error>> {
    debug!("Finding R {} binary", rver);
    let rroot = get_r_root();
    let base = Path::new(&rroot);
    let bin = base
        .join("R-".to_string() + &rver)
        .join("bin")
	.join("x64")
        .join("R.exe");
    debug!("R {} binary: {}", rver, bin.display());
    Ok(bin)
}
