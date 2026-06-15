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
use log::{debug, error, info, trace, warn};
use owo_colors::OwoColorize;
use remove_dir_all::remove_dir_all;
use semver;
use simple_error::{bail, SimpleError};
use tabular::*;
use whoami::{fallible::hostname, username};
use winreg::enums::*;
use winreg::RegKey;

use crate::alias::*;
use crate::common::*;
use crate::download::*;
use crate::escalate::*;
use crate::library::*;
use crate::output::OUTPUT;
use crate::repos::*;
use crate::resolve::*;
use crate::run::*;
use crate::rversion::*;
use crate::utils::*;
use crate::windows_arch::*;

// get_r_root(): returns the directory where the R versions are installed
// get_links_dir(): returns the directory where the quick links are created
// get_r_versiondir(): name template for a single R version directory inside R_ROOT()
// get_r_syslibpath(): path to the system library of an R version from R_ROOT()
// get_r_binpath(): path of the R executable from R_ROOT()

pub fn get_links_dir() -> Result<String, Box<dyn Error>> {
    get_binary_dir()
}

macro_rules! osvec {
    // match a list of expressions separated by comma:
    ($($str:expr),*) => ({
        // create a Vec with this list of expressions,
        // calling String::from on each:
        vec![$(OsString::from($str),)*] as Vec<OsString>
    });
}

pub fn get_r_root() -> Result<String, Box<dyn Error>> {
    get_r_root_arch(get_native_arch())
}

pub fn get_r_root_arch(arch: &str) -> Result<String, Box<dyn Error>> {
    if get_mode()? == Mode::User {
        let base = match get_r_install_dir()? {
            Some(dir) => dir,
            None => {
                let appdata = std::env::var("APPDATA")?;
                format!("{}\\rig\\data\\r", appdata)
            }
        };
        // User mode: all arches share the same root; arch is encoded in the directory name
        return Ok(base);
    }
    const R_ROOT_: &str = "C:\\Program Files\\R";
    const R_X86_ROOT_: &str = "C:\\Program Files (x86)\\R";
    const R_AARCH64_ROOT_: &str = "C:\\Program Files\\R-aarch64";
    Ok(match arch {
        "aarch64" | "arm64" => R_AARCH64_ROOT_.to_string(),
        "x86_64" if get_native_arch() == "aarch64" => R_X86_ROOT_.to_string(),
        _ => R_ROOT_.to_string(),
    })
}

// Strip a trailing -x86_64, -aarch64, or -arm64 arch suffix from a rig name.
pub fn base_version(name: &str) -> String {
    for suffix in &["-x86_64", "-aarch64", "-arm64"] {
        if let Some(base) = name.strip_suffix(suffix) {
            return base.to_string();
        }
    }
    name.to_string()
}

// Determine the architecture implied by a rig version name.
// Names with a -x86_64 suffix are x86_64; -aarch64/-arm64 are aarch64.
// An unsuffixed name is the native architecture.
pub fn arch_of_name(name: &str) -> &'static str {
    if name.ends_with("-x86_64") {
        "x86_64"
    } else if name.ends_with("-aarch64") || name.ends_with("-arm64") {
        "aarch64"
    } else {
        get_native_arch()
    }
}

// Return the R root directory for a given rig version name.
pub fn get_r_root_for(name: &str) -> Result<String, Box<dyn Error>> {
    get_r_root_arch(arch_of_name(name))
}

// Return the directory key used inside the root.
// In user mode, all arches share one root and the arch suffix stays in the key.
// In admin mode, arches use different roots so the suffix is stripped here.
pub fn version_dir_key(name: &str) -> String {
    if get_mode().unwrap_or(Mode::Admin) == Mode::User {
        name.to_string()
    } else {
        base_version(name)
    }
}

// Build the rig name from a bare version string and an architecture.
// On x86_64 hosts or for the native arch on aarch64, no suffix is added.
fn rig_name_for_arch(base: &str, arch: &str) -> String {
    let native = get_native_arch();
    if native == "aarch64" && arch == "x86_64" {
        base.to_string() + "-x86_64"
    } else {
        base.to_string()
    }
}

pub fn get_r_syslibpath() -> Result<String, Box<dyn Error>> {
    if get_mode()? == Mode::User {
        Ok("{}\\library".to_string())
    } else {
        Ok("R-{}\\library".to_string())
    }
}

pub fn get_r_binpath() -> Result<String, Box<dyn Error>> {
    if get_mode()? == Mode::User {
        Ok("{}\\bin\\R.exe".to_string())
    } else {
        Ok("R-{}\\bin\\R.exe".to_string())
    }
}

pub fn get_r_base_profile() -> Result<String, Box<dyn Error>> {
    if get_mode()? == Mode::User {
        Ok("{}\\library\\base\\R\\Rprofile".to_string())
    } else {
        Ok("R-{}\\library\\base\\R\\Rprofile".to_string())
    }
}

pub fn get_r_etc_path() -> Result<String, Box<dyn Error>> {
    if get_mode()? == Mode::User {
        Ok("{}\\etc".to_string())
    } else {
        Ok("R-{}\\etc".to_string())
    }
}

pub fn get_r_versiondir() -> Result<String, Box<dyn Error>> {
    if get_mode()? == Mode::User {
        Ok("{}".to_string())
    } else {
        Ok("R-{}".to_string())
    }
}

// Build the version's installation directory name: "<base>" in user mode, "R-<base>" in admin.
fn r_dirname(key: &str) -> Result<String, Box<dyn Error>> {
    if get_mode()? == Mode::User {
        Ok(key.to_string())
    } else {
        Ok(format!("R-{}", key))
    }
}

#[warn(unused_variables)]
pub fn sc_add(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    escalate("adding new R version")?;
    let alias = get_alias(args);
    sc_clean_registry()?;
    let str = args.get_one::<String>("str").unwrap();
    if str.len() >= 6 && &str[0..6] == "rtools" {
        // For bare "rtools" (install all needed), only honour --arch when the user
        // explicitly passed it; the flag's native-arch default should not filter out
        // cross-arch installations.
        let explicit_arch = args.value_source("arch")
            == Some(clap::parser::ValueSource::CommandLine);
        let arch = if explicit_arch {
            args.get_one::<String>("arch").map(|s| normalize_arch(s))
        } else {
            None
        };
        return add_rtools(str.to_string(), arch);
    }
    let (version_info, target) = download_r(&args)?;
    let installed_arch = version_info.arch.clone().unwrap_or_else(|| get_native_arch().to_string());
    let target_path = Path::new(&target);

    OUTPUT.status(&format!("Installing {}", target_path.display()));
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

    let dirname = if get_mode()? == Mode::User {
        // In user mode: install into APPDATA, no registry lookup needed.
        let base = match &version_info.version {
            Some(v) => v.clone(),
            None => bail!("Cannot determine R version to install in user mode"),
        };
        let r_root = get_r_root_arch(&installed_arch)?;
        let rig_name = rig_name_for_arch(&base, &installed_arch);
        // User mode: directory name is the rig name itself (no R- prefix, arch suffix included)
        let install_dir = format!("{}\\{}", r_root, &rig_name);
        std::fs::create_dir_all(&install_dir)?;
        cmd_args.push(os("/CURRENTUSER"));
        cmd_args.push(OsString::from(format!("/DIR={}", install_dir)));
        run(target, cmd_args, "installer")?;
        Some(rig_name)
    } else {
        run(target, cmd_args, "installer")?;
        get_latest_install_path(&installed_arch)?
    };

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
        None => {}
        Some(ref dirname) => match alias {
            Some(alias) => add_alias(&dirname, &alias)?,
            None => {}
        },
    };
    patch_for_rtools()?;
    maybe_update_registry_default()?;

    match dirname {
        None => {
            OUTPUT.warn("Cannot set up repositories, cannot determine installation directory");
            warn!("Cannot set up repositories, cannot determine installation directory");
        }
        Some(ref dirname) => {
            let setup = interpret_repos_args(args, true);
            repos_setup(Some(vec![dirname.to_string()]), setup)?;
        }
    };

    if !args.get_flag("without-pak") {
        match dirname {
            None => {
                OUTPUT.warn("Cannot install pak, cannot determine installation directory");
                warn!("Cannot install pak, cannot determine installation directory");
            }
            Some(ref dirname) => {
                let explicit = args.value_source("pak-version")
                    == Some(clap::parser::ValueSource::CommandLine);
                system_add_pak(
                    Some(vec![dirname.to_string()]),
                    args.get_one::<String>("pak-version").unwrap(),
                    // If this is specified then we always re-install
                    explicit,
                )?;
            }
        }
    }

    Ok(())
}

fn normalize_arch(arch: &str) -> String {
    match arch {
        "arm64" => "aarch64".to_string(),
        other => other.to_string(),
    }
}

struct NeededRtools {
    version: String,
    arch: String,
}

fn add_rtools(version: String, arch: Option<String>) -> Result<(), Box<dyn Error>> {
    let needed: Vec<NeededRtools>;
    if version == "rtools" {
        needed = get_rtools_needed(None, arch.as_deref())?;
    } else {
        let ver = version.replace("rtools", "");
        let a = arch.unwrap_or_else(|| get_native_arch().to_string());
        needed = vec![NeededRtools { version: ver, arch: a }];
    }
    let client = &reqwest::Client::new();
    for item in needed {
        let versuffix = if &item.version[0..1] != "3" { item.version.as_str() } else { "" };
        let archsuffix = if item.arch == "aarch64" { "-aarch64" } else { "" };
        let instdir = "C:\\Rtools".to_string() + versuffix + archsuffix;
        let instdirpath = Path::new(&instdir);
        if instdirpath.exists() {
            OUTPUT.success(&format!(
                "Rtools{} ({}) is already installed",
                &item.version, &item.arch
            ));
            info!("Rtools{} ({}) is already installed", &item.version, &item.arch);
            continue;
        }
        let rtver = get_rtools_version(&item.version, &item.arch)?;
        let url = rtver.url;
        let filename = "rtools-".to_string() + &item.version + "-" + &item.arch + ".exe";

        let tmp_dir = std::env::temp_dir().join("rig");
        let target = tmp_dir.join(&filename);
        OUTPUT.status(&format!("Downloading {} -> {}", url, target.display()));
        info!("Downloading {} -> {}", url, target.display());
        download_file(client, &url, &target.as_os_str())?;
        OUTPUT.status(&format!("Installing {}", target.display()));
        info!("Installing {}", target.display());
        run(
            target.into_os_string(),
            vec![os("/VERYSILENT"), os("/SUPPRESSMSGBOXES")],
            "installer",
        )?;
    }

    Ok(())
}

fn patch_for_rtools() -> Result<(), Box<dyn Error>> {
    let vers = sc_get_list()?;

    for ver in vers {
        let vver = vec![ver.to_owned()];
        let needed = get_rtools_needed(Some(vver), None)?;
        if needed.is_empty() || (needed[0].version != "35" && needed[0].version != "40") {
            continue;
        }
        let rtools4 = needed[0].version == "40";
        let ver_rroot = get_r_root_for(&ver)?;
        let ver_base = version_dir_key(&ver);
        let envfile = Path::new(&ver_rroot).join(r_dirname(&ver_base)?).join("etc").join("Renviron.site");
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
                OUTPUT.warn(&format!("Couldn't write to Renviron.site file: {}", e));
                warn!("Couldn't write to Renviron.site file: {}", e);
            }
        }
    }

    Ok(())
}

fn get_rtools_needed(
    version: Option<Vec<String>>,
    arch_filter: Option<&str>,
) -> Result<Vec<NeededRtools>, Box<dyn Error>> {
    let vers = match version {
        None => sc_get_list()?,
        Some(x) => x,
    };
    let mut res: Vec<NeededRtools> = vec![];
    let errmsg = "Cannot parse list of Rtools versions.";

    for ver in vers {
        let r_arch = normalize_arch(arch_of_name(&ver));
        if let Some(filter) = arch_filter {
            if r_arch != filter {
                continue;
            }
        }
        let r = get_r_binary(&ver)?;
        let out = Command::new(r)
            .args(["--vanilla", "-s", "-e", "cat(as.character(getRversion()))"])
            .output()?;
        let rver_str: String = String::from_utf8(out.stdout)?;
        let sver = semver::Version::parse(&rver_str)?;
        debug!("Get Rtools version for R {}.", rver_str);

        let rtversval = get_available_rtools_versions(&r_arch);
        let rtvers = match rtversval.as_array() {
            Some(x) => x,
            None => {
                OUTPUT.error(errmsg);
                error!("{}", errmsg);
                bail!("{}", errmsg)
            }
        };

        for rtver in rtvers {
            let first = rtver["first"].as_str().ok_or(errmsg)?;
            let last = rtver["last"].as_str().ok_or(errmsg)?;
            let first = semver::Version::parse(first)?;
            let last = semver::Version::parse(last)?;
            if first <= sver && sver <= last {
                let rtverver = rtver["version"].as_str().ok_or(errmsg)?.to_string();
                debug!("R {} needs Rtools {} ({}).", rver_str, rtverver, r_arch);
                if !res.iter().any(|x| x.version == rtverver && x.arch == r_arch) {
                    res.push(NeededRtools { version: rtverver, arch: r_arch.clone() });
                }
            }
        }
    }
    Ok(res)
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
            rm_rtools(verstr, None)?;
            continue;
        }
        let ver = check_installed(&verstr)?;

        if let Some(ref default) = default {
            if default == &ver {
                OUTPUT.warn(&format!(
                    "Removing default version, set new default with {}",
                    "rig default <version>".bold(),
                ));
                warn!(
                    "Removing default version, set new default with {}",
                    "rig default <version>".bold()
                );
                match unset_default() {
                    Err(e) => warn!("Failed to unset default version: {}", e.to_string()),
                    _ => {}
                };
            }
        }

        let rroot = get_r_root_for(&ver)?;
        let base = version_dir_key(&ver);
        let dir = Path::new(&rroot).join(r_dirname(&base)?);
        OUTPUT.status(&format!("Removing {}", dir.display()));
        info!("Removing {}", dir.display());
        remove_dir_all(&dir)?;
    }

    sc_clean_registry()?;
    sc_system_make_links()?;

    Ok(())
}

fn rm_rtools(ver: String, arch: Option<String>) -> Result<(), Box<dyn Error>> {
    let arch = arch.unwrap_or_else(|| get_native_arch().to_string());
    let ver = if arch == "aarch64" {
        ver + "-aarch64"
    } else {
        ver
    };
    let dir = Path::new("C:\\").join(ver);
    OUTPUT.status(&format!("Removing {}", dir.display()));
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
                OUTPUT.error(&format!("Failed to remove {}: {}", dir.display(), stderr));
                error!("Failed to remove {}: {}", dir.display(), stderr);
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
    let links_dir = get_links_dir()?;
    let linkdir = Path::new(&links_dir);
    let mut new_links: Vec<String> = vec![
        "RS.bat".to_string(),
        "R.bat".to_string(),
        "Rscript.bat".to_string(),
    ];
    std::fs::create_dir_all(linkdir)?;

    for ver in vers {
        let filename = "R-".to_string() + &ver + ".bat";
        let linkfile = linkdir.join(&filename);
        new_links.push(filename);
        let ver_rroot = get_r_root_for(&ver)?;
        let ver_base = version_dir_key(&ver);
        let ver_dir = r_dirname(&ver_base)?;
        let target = Path::new(&ver_rroot).join(&ver_dir);

        let cnt = format!("@\"{}\\{}\\bin\\R\" %*\n", ver_rroot, ver_dir);
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
        OUTPUT.status(&format!("{} R-{} -> {}", op, ver, target.display()));
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
                    OUTPUT.status(&format!("Deleting unused {}", filename));
                    info!("Deleting unused {}", filename);
                    match std::fs::remove_file(path.path()) {
                        Ok(_) => {}
                        Err(e) => {
                            OUTPUT.warn(&format!(
                                "Failed to remove {}: {}",
                                filename,
                                e.to_string()
                            ));
                            warn!("Failed to remove {}: {}", filename, e.to_string());
                        }
                    }
                }
            }
        };
    }

    if get_mode()? == Mode::User {
        add_user_bin_to_path()?;
    }

    Ok(())
}

fn re_alias() -> Regex {
    let re = Regex::new("^R-(oldrel|release|next)[.]bat$").unwrap();
    re
}

pub fn find_aliases() -> Result<Vec<Alias>, Box<dyn Error>> {
    let mut result: Vec<Alias> = vec![];
    let links_dir = get_links_dir()?;
    let bin = Path::new(&links_dir);
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
                alias: fnamestr[2..fnamestr.len() - 4].to_string(),
                version: rver,
            };
            result.push(als);
        }
    }

    Ok(result)
}

fn find_r_version_in_link(path: &PathBuf) -> Result<String, Box<dyn Error>> {
    let lines = read_lines(path)?;
    if lines.len() == 0 {
        OUTPUT.error(&format!("Invalid R link file: {}", path.display()));
        error!("Invalid R link file: {}", path.display());
        bail!("Invalid R link file: {}", path.display());
    }
    let line = &lines[0];

    if get_mode().unwrap_or(Mode::Admin) == Mode::User {
        // User mode: @"<root>\<version>\bin\R" %* — extract the component after the root
        let user_root = get_r_root()?;
        let prefix = format!("@\"{}\\", user_root);
        let line_lower = line.to_lowercase();
        let prefix_lower = prefix.to_lowercase();
        if line_lower.starts_with(&prefix_lower) {
            let rest = &line[prefix.len()..];
            if let Some(slash_pos) = rest.find('\\') {
                return Ok(rest[..slash_pos].to_string());
            }
        }
        OUTPUT.error(&format!(
            "Cannot extract R version from {}, invalid R link file?",
            path.display(),
        ));
        error!(
            "Cannot extract R version from {}, invalid R link file?",
            path.display()
        );
        bail!(
            "Cannot extract R version from {}, invalid R link file?",
            path.display()
        );
    }

    // Admin mode: @"<root>\R-<base>\bin\R" %*
    // Determine which root this link points into, so we can re-attach the right suffix.
    let x86_root = get_r_root_arch("x86_64")?;
    let is_x86 = get_native_arch() == "aarch64"
        && line
            .to_lowercase()
            .starts_with(&("@\"".to_string() + &x86_root.to_lowercase()));

    let split = line.split("\\").collect::<Vec<&str>>();
    for s in split {
        if s == "R-devel" {
            let base = "devel".to_string();
            return Ok(if is_x86 {
                base + "-x86_64"
            } else {
                base
            });
        }
        // Skip the R-aarch64 root directory name itself
        if s != "R-aarch64" && s.starts_with("R-") {
            let base = s[2..].to_string();
            return Ok(if is_x86 {
                base + "-x86_64"
            } else {
                base
            });
        }
    }
    OUTPUT.error(&format!(
        "Cannot extract R version from {}, invalid R link file?",
        path.display(),
    ));
    error!(
        "Cannot extract R version from {}, invalid R link file?",
        path.display()
    );
    bail!(
        "Cannot extract R version from {}, invalid R link file?",
        path.display()
    );
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

// ------------------------------------------------------------------------

fn list_r_in_root(root: &str, suffix: &str, vers: &mut Vec<String>) -> Result<(), Box<dyn Error>> {
    if !Path::new(root).exists() {
        return Ok(());
    }
    let user_mode = get_mode().unwrap_or(Mode::Admin) == Mode::User;
    for de in std::fs::read_dir(root)? {
        let path = de?.path();
        if !path.is_dir() {
            continue;
        }
        match path.file_name() {
            None => continue,
            Some(fname) => match fname.to_str() {
                None => continue,
                Some(fname) => {
                    if user_mode {
                        // User mode: no R- prefix; directory name is the version
                        vers.push(fname.to_string() + suffix);
                    } else if fname.len() > 2 && &fname[0..2] == "R-" {
                        let v = fname[2..].to_string() + suffix;
                        vers.push(v);
                    }
                }
            },
        }
    }
    Ok(())
}

pub fn sc_get_list() -> Result<Vec<String>, Box<dyn Error>> {
    let mut vers = Vec::new();

    if get_mode()? == Mode::User {
        // User mode: all arches share one root; version names already include arch suffix
        list_r_in_root(&get_r_root()?, "", &mut vers)?;
    } else {
        // Admin mode: native root, and on aarch64 also a separate x86_64 root
        list_r_in_root(&get_r_root()?, "", &mut vers)?;
        if get_native_arch() == "aarch64" {
            let x86_root = get_r_root_arch("x86_64")?;
            list_r_in_root(&x86_root, "-x86_64", &mut vers)?;
        }
    }

    vers.sort();
    Ok(vers)
}

pub fn sc_set_default(ver: &str) -> Result<(), Box<dyn Error>> {
    let ver = check_installed(&ver.to_string())?;
    escalate("setting the default R version")?;
    let rroot = get_r_root_for(&ver)?;
    let base = version_dir_key(&ver);
    let links_dir = get_links_dir()?;
    let linkdir = Path::new(&links_dir);
    std::fs::create_dir_all(&linkdir)?;

    let linkfile = linkdir.join("R.bat");
    let base_dir = r_dirname(&base)?;
    let cnt =
        "::".to_string() + &ver + "\n" + "@\"" + &rroot + "\\" + &base_dir + "\\bin\\R\" %*\n";
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
        + "@\""
        + &rroot
        + "\\"
        + &base_dir
        + "\\bin\\Rscript\" %*\n";
    file3.write_all(cnt3.as_bytes())?;

    update_registry_default()?;

    Ok(())
}

pub fn unset_default() -> Result<(), Box<dyn Error>> {
    escalate("unsetting the default R version")?;

    let links_dir = get_links_dir()?;
    let linkdir = Path::new(&links_dir);

    let try_rm = |x: &str| {
        let f = linkdir.join(x);
        if f.exists() {
            match std::fs::remove_file(&f) {
                Err(e) => {
                    OUTPUT.warn(&format!(
                        "Failed to remove {}: {}",
                        f.display(),
                        e.to_string()
                    ));
                    warn!("Failed to remove {}: {}", f.display(), e.to_string())
                }
                _ => {}
            };
        }
    };

    try_rm("R.bat");
    try_rm("RS.bat");
    try_rm("Rscript.bat");

    unset_registry_default()?;

    Ok(())
}

pub fn sc_get_default() -> Result<Option<String>, Box<dyn Error>> {
    let links_dir = get_links_dir()?;
    let linkdir = Path::new(&links_dir);
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

fn r_registry_hive() -> Result<RegKey, Box<dyn Error>> {
    if get_mode()? == Mode::User {
        Ok(RegKey::predef(HKEY_CURRENT_USER))
    } else {
        Ok(RegKey::predef(HKEY_LOCAL_MACHINE))
    }
}

pub fn sc_clean_registry() -> Result<(), Box<dyn Error>> {
    escalate("cleaning up the Windows registry")?;

    OUTPUT.status("Cleaning leftover registry entries");
    info!("Cleaning leftover registry entries");

    let hive = r_registry_hive()?;

    let r64r = hive.open_subkey("SOFTWARE\\R-core\\R");
    if let Ok(x) = r64r {
        clean_registry_r(&x)?;
    };
    let r64r64 = hive.open_subkey("SOFTWARE\\R-core\\R64");
    if let Ok(x) = r64r64 {
        clean_registry_r(&x)?;
    };
    let r32r = hive.open_subkey("SOFTWARE\\WOW6432Node\\R-core\\R");
    if let Ok(x) = r32r {
        clean_registry_r(&x)?;
    };
    let r32r32 = hive.open_subkey("SOFTWARE\\WOW6432Node\\R-core\\R32");
    if let Ok(x) = r32r32 {
        clean_registry_r(&x)?;
    };
    let r32r64 = hive.open_subkey("SOFTWARE\\WOW6432Node\\R-core\\R64");
    if let Ok(x) = r32r64 {
        clean_registry_r(&x)?;
    };

    // Rtools and uninstall entries only exist in HKLM
    if get_mode()? == Mode::Admin {
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
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

        let uninst =
            hklm.open_subkey("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall");
        if let Ok(x) = uninst {
            clean_registry_uninst(&x)?;
        };
        let uninst32 = hklm.open_subkey(
            "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
        );
        if let Ok(x) = uninst32 {
            clean_registry_uninst(&x)?;
        };
    }

    Ok(())
}

fn maybe_update_registry_default() -> Result<(), Box<dyn Error>> {
    let links_dir = get_links_dir()?;
    let linkdir = Path::new(&links_dir);
    let linkfile = linkdir.join("R.bat");
    if linkfile.exists() {
        update_registry_default()?;
    }
    Ok(())
}

fn update_registry_default1(key: &RegKey, ver: &String) -> Result<(), Box<dyn Error>> {
    let base = version_dir_key(ver);
    let rroot = get_r_root_for(ver)?;
    key.set_value("Current Version", &base)?;
    let inst = rroot + "\\" + &r_dirname(&base)?;
    key.set_value("InstallPath", &inst)?;
    Ok(())
}

fn update_registry_default_to(default: &String) -> Result<(), Box<dyn Error>> {
    let hive = r_registry_hive()?;
    let native = get_native_arch();
    let arch = arch_of_name(default);

    if native == "aarch64" && arch == "x86_64" {
        // x86_64 R on aarch64 host: update the WOW6432Node key
        let key_path = "SOFTWARE\\WOW6432Node\\R-core\\R64";
        let r = hive.create_subkey(key_path);
        if let Ok(x) = r {
            let (key, _) = x;
            update_registry_default1(&key, default)?;
        }
    } else {
        // native arch: update both R and R64 keys
        let r64r = hive.create_subkey("SOFTWARE\\R-core\\R");
        if let Ok(x) = r64r {
            let (key, _) = x;
            update_registry_default1(&key, default)?;
        }
        let r64r64 = hive.create_subkey("SOFTWARE\\R-core\\R64");
        if let Ok(x) = r64r64 {
            let (key, _) = x;
            update_registry_default1(&key, default)?;
        }
    }
    Ok(())
}

fn update_registry_default() -> Result<(), Box<dyn Error>> {
    escalate("Update registry default")?;
    let default = sc_get_default_or_fail()?;
    update_registry_default_to(&default)
}

fn unset_registry_default() -> Result<(), Box<dyn Error>> {
    let hive = r_registry_hive()?;
    let r64r = hive.create_subkey("SOFTWARE\\R-core\\R");
    if let Ok(x) = r64r {
        let (key, _) = x;
        let _ = key.delete_value("Current Version");
        let _ = key.delete_value("InstallPath");
    }
    let r64r64 = hive.create_subkey("SOFTWARE\\R-core\\R64");
    if let Ok(x) = r64r64 {
        let (key, _) = x;
        let _ = key.delete_value("Current Version");
        let _ = key.delete_value("InstallPath");
    }
    Ok(())
}

fn add_user_bin_to_path() -> Result<(), Box<dyn Error>> {
    let bin_dir = get_binary_dir()?;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let env_key = hkcu.open_subkey_with_flags(
        "Environment",
        winreg::enums::KEY_READ | winreg::enums::KEY_WRITE,
    )?;

    // Read current PATH (handles both REG_SZ and REG_EXPAND_SZ).
    let raw = env_key.get_raw_value("Path").unwrap_or(winreg::RegValue {
        bytes: Vec::new(),
        vtype: winreg::enums::REG_EXPAND_SZ,
    });

    // Decode UTF-16 LE registry string (strip null terminator).
    let words: Vec<u16> = raw
        .bytes
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .take_while(|&c| c != 0)
        .collect();
    let current_path = String::from_utf16_lossy(&words);

    // Check if bin_dir is already a segment (case-insensitive on Windows).
    let bin_lower = bin_dir.to_lowercase();
    let already_present = current_path
        .split(';')
        .any(|s| s.trim().to_lowercase() == bin_lower);

    if already_present {
        return Ok(());
    }

    let new_path = if current_path.is_empty() {
        bin_dir.clone()
    } else {
        format!("{};{}", bin_dir, current_path)
    };

    // Encode back to UTF-16 LE with null terminator, preserving original REG type.
    let encoded: Vec<u8> = new_path
        .encode_utf16()
        .chain(std::iter::once(0u16))
        .flat_map(|c| c.to_le_bytes())
        .collect();
    env_key.set_raw_value(
        "Path",
        &winreg::RegValue {
            bytes: encoded,
            vtype: raw.vtype,
        },
    )?;

    OUTPUT.status(&format!("Added {} to user PATH", bin_dir));
    info!("Added {} to user PATH", bin_dir);
    OUTPUT.warn("Restart your terminal (or sign out and back in) for the PATH change to take effect.");
    warn!("Restart your terminal for the PATH change to take effect.");

    Ok(())
}

pub fn sc_system_update_rtools40() -> Result<(), Box<dyn Error>> {
    run(
        "c:\\rtools40\\usr\\bin\\bash.exe".into(),
        vec![
            "--login".into(),
            "-c".into(),
            "pacman -Syu --noconfirm".into(),
        ],
        "Rtools40 update",
    )
}

pub fn sc_system_rtools(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        Some(("add", s)) => sc_rtools_add(s, mainargs),
        Some(("list", s)) => sc_rtools_ls(s, mainargs),
        Some(("rm", s)) => sc_rtools_rm(s, mainargs),
        _ => Ok(()), // unreachable
    }
}

fn sc_rtools_add(args: &ArgMatches, _mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    escalate("adding Rtools")?;
    let ver = args.get_one::<String>("version").unwrap();
    let arch = args.get_one::<String>("arch").map(|s| normalize_arch(s));
    if ver == "all" {
        add_rtools("rtools".to_string(), arch)
    } else if ver.starts_with("rtools") {
        add_rtools(ver.to_string(), arch)
    } else {
        add_rtools("rtools".to_string() + ver, arch)
    }
}

fn sc_rtools_rm(args: &ArgMatches, _mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    escalate("removing Rtools")?;
    let vers = args.get_many::<String>("version");
    if vers.is_none() {
        return Ok(());
    }
    let vers = vers.ok_or(SimpleError::new("Internal argument error"))?;
    let arch = args.get_one::<String>("arch").map(|s| normalize_arch(s));

    for ver in vers {
        if ver == "all" {
            rm_rtools("rtools".to_string(), arch.clone())?;
        } else if ver.starts_with("rtools") {
            rm_rtools(ver.to_string(), arch.clone())?;
        } else {
            rm_rtools("rtools".to_string() + ver, arch.clone())?;
        }
    }

    Ok(())
}

#[derive(Default, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RtoolsVersion {
    pub name: String,
    pub version: String,
    pub fullversion: String,
    pub path: String,
    pub arch: String,
}

fn get_rtools_versions(rtoolskey: &RegKey) -> Result<Vec<RtoolsVersion>, Box<dyn Error>> {
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
        // derive arch from install path: -aarch64 in path => aarch64, else x86_64
        let arch = if path.to_lowercase().contains("-aarch64") {
            "aarch64".to_string()
        } else {
            "x86_64".to_string()
        };
        versions.push(RtoolsVersion {
            name,
            version,
            fullversion,
            path,
            arch,
        });
    }
    Ok(versions)
}

fn sc_rtools_ls(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
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
        println!("{}", serde_json::to_string_pretty(&versions)?);
    } else {
        let mut tab = Table::new("{:<}  {:<}  {:<}  {:<}  {:<}");
        tab.add_row(row!["name", "version", "full-version", "arch", "path"]);
        tab.add_heading("------------------------------------------------------");
        for item in versions {
            tab.add_row(row!(item.name, item.version, item.fullversion, item.arch, item.path));
        }
        println!("{}", tab);
    }

    Ok(())
}

fn get_latest_install_path(installed_arch: &str) -> Result<Option<String>, Box<dyn Error>> {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let native = get_native_arch();
    // Choose the registry key written by the installer for this arch.
    // x86_64 R on x86_64 host  -> SOFTWARE\R-core\R64
    // aarch64 R on aarch64 host -> SOFTWARE\R-core\R
    // x86_64 R on aarch64 host  -> SOFTWARE\WOW6432Node\R-core\R64
    let key = if native == "aarch64" && installed_arch == "x86_64" {
        "SOFTWARE\\WOW6432Node\\R-core\\R64"
    } else if native == "aarch64" {
        "SOFTWARE\\R-core\\R"
    } else {
        "SOFTWARE\\R-core\\R64"
    };
    let regkey = hklm.open_subkey(key);
    if let Ok(k) = regkey {
        let ip: Result<String, _> = k.get_value("InstallPath");
        if let Ok(fp) = ip {
            let ufp = fp.replace("\\", "/");
            let p = match basename(&ufp) {
                None => return Ok(None),
                Some(p) => p,
            };
            let re = Regex::new("^R-")?;
            let base = re.replace(p, "").to_string();
            let name = rig_name_for_arch(&base, installed_arch);
            return Ok(Some(name));
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
        Ok(x) => xdg = Some(std::path::PathBuf::from(x)),
        Err(_) => {}
    };

    // XDG_CONFIG_HOME may point to the user config path
    if xdg.is_none() {
        final_path = false;
        match std::env::var("XDG_CONFIG_HOME") {
            Ok(x) => {
                let mut xdg2 = std::path::PathBuf::new();
                xdg2.push(x);
                xdg = Some(xdg2);
            }
            Err(_) => {}
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
        None => {
            OUTPUT.error("RStudio config path cannot be represented as Unicode. :(");
            error!("RStudio config path cannot be represented as Unicode. :(");
            bail!("RStudio config path cannot be represented as Unicode. :(")
        }
    };
    let has_dollar = path.contains("$");
    let empty = path.len() == 0;
    let has_tilde = path.contains("~");

    if has_dollar || empty || has_tilde {
        match std::env::var("USER") {
            Ok(_) => {}
            Err(_) => {
                let username = username();
                std::env::set_var("USER", username);
            }
        };
    }

    if has_dollar {
        match std::env::var("HOME") {
            Ok(_) => {}
            Err(_) => {
                let bd = BaseDirs::new();
                match bd {
                    Some(x) => {
                        std::env::set_var("HOME", x.home_dir().as_os_str());
                    }
                    None => {
                        OUTPUT.warn("Cannot determine HOME directory");
                        warn!("Cannot determine HOME directory");
                    }
                };
            }
        };
    }

    if has_dollar {
        match std::env::var("HOSTNAME") {
            Ok(_) => {}
            Err(_) => {
                let hostname = match hostname() {
                    Ok(x) => x,
                    Err(_) => "localhost".to_string(),
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
                OUTPUT.error(&format!(
                    "RStudio config path contains unknown environment variable: {}",
                    e.var_name
                ));
                error!(
                    "RStudio config path contains unknown environment variable: {}",
                    e.var_name
                );
                bail!(
                    "RStudio config path contains unknown environment variable: {}",
                    e.var_name
                );
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

pub fn sc_rstudio_(
    version: Option<&str>,
    project: Option<&str>,
    arg: Option<&OsStr>,
) -> Result<(), Box<dyn Error>> {
    debug!("Looking into starting RStudio");

    let def = sc_get_default()?;

    // def is None if there is no default R version set
    let version = match version {
        Some(x) => Some(x.to_string()),
        None => def,
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
        OUTPUT.status(&format!("Setting RSTUDIO_WHICH_R=\"{}\"", bin.display()));
        info!("Setting RSTUDIO_WHICH_R=\"{}\"", bin.display());
        std::env::set_var("RSTUDIO_WHICH_R", bin);
    }

    let cmdline = osjoin(args.to_owned(), " ");
    OUTPUT.status(&format!("Running cmd.exe {}", cmdline));
    info!("Running cmd.exe {}", cmdline);
    let status = run("cmd.exe".into(), args, "start");

    // restore version env var
    if let Some(_) = version {
        match old {
            Ok(v) => std::env::set_var("RSTUDIO_WHICH_R", v),
            Err(_) => std::env::remove_var("RSTUDIO_WHICH_R"),
        };
    }

    match status {
        Err(e) => {
            OUTPUT.error(&format!("`start` failed: {}", e.to_string()));
            error!("`start` failed: {}", e.to_string());
            bail!("`start` failed: {}", e.to_string());
        }
        _ => {}
    };

    Ok(())
}

pub fn get_system_profile(rver: &str) -> Result<PathBuf, Box<dyn Error>> {
    let rroot = get_r_root_for(rver)?;
    let base = version_dir_key(rver);
    let path = Path::new(&rroot).join(r_dirname(&base)?);
    let profile = path.join("library/base/R/Rprofile");
    Ok(profile)
}

pub fn get_r_binary(rver: &str) -> Result<PathBuf, Box<dyn Error>> {
    debug!("Finding R {} binary", rver);
    let rroot = get_r_root_for(rver)?;
    let base = version_dir_key(rver);
    let bin = Path::new(&rroot)
        .join(r_dirname(&base)?)
        .join("bin")
        .join("R.exe");
    debug!("R {} binary: {}", rver, bin.display());
    Ok(bin)
}

pub fn get_r_binary_x64(rver: &str) -> Result<PathBuf, Box<dyn Error>> {
    debug!("Finding R {} binary", rver);
    let rroot = get_r_root_for(rver)?;
    let base = version_dir_key(rver);
    let bin = Path::new(&rroot)
        .join(r_dirname(&base)?)
        .join("bin")
        .join("x64")
        .join("R.exe");
    debug!("R {} binary: {}", rver, bin.display());
    Ok(bin)
}
