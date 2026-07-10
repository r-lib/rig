use regex::Regex;
use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::path::Path;
use std::path::PathBuf;

use clap::ArgMatches;
use log::{debug, error, info, warn};
use semver::Version;
use simple_error::*;
use tabular::*;

#[cfg(target_os = "macos")]
use crate::macos::*;

#[cfg(target_os = "windows")]
use crate::windows::*;

#[cfg(target_os = "linux")]
use crate::linux::*;

#[cfg(target_os = "linux")]
use crate::platform::*;

use crate::download::download_json_sync;
use crate::output::OUTPUT;
use crate::renv;
use crate::run::*;
use crate::rversion::*;
use crate::utils::*;

pub fn check_installed(x: &String) -> Result<String, Box<dyn Error>> {
    let inst = sc_get_list_details()?;

    for ver in inst {
        if &ver.name == x {
            return Ok(ver.name);
        }
        if ver.aliases.contains(x) {
            debug!("Alias {} is resolved to version {}", x, ver.name);
            return Ok(ver.name);
        }
    }

    OUTPUT.error(&format!("R version {} is not installed", x));
    error!("R version {} is not installed", x);
    bail!("R version {} is not installed", &x);
}

// -- rig default ---------------------------------------------------------

// Fail if no default is set

pub fn sc_get_default_or_fail() -> Result<String, Box<dyn Error>> {
    let default = sc_get_default()?;
    match default {
        None => {
            OUTPUT.error("No default R version is set, call `rig default <version>`");
            error!("No default R version is set, call `rig default <version>`");
            bail!("No default R version is set, call `rig default <version>`")
        }
        Some(d) => Ok(d),
    }
}

pub fn set_default_if_none(ver: String) -> Result<(), Box<dyn Error>> {
    debug!("Checking if a default R version is set");
    let cur = sc_get_default()?;
    if cur.is_none() {
        debug!("No default R version is set, setting it to {}", ver);
        sc_set_default(&ver)?;
    }
    Ok(())
}

// -- rig system user-mode ------------------------------------------------
//
// Helpers shared by `sc_system_user_mode` on macOS, Windows and Linux. The
// per-platform code captures the admin-mode setup and reinstalls the versions;
// these cover the parts that are identical on every platform.

// Map an R_STATUS value to the symbolic user-mode directory name used for
// development builds: `devel` for R-devel, `next` for R-next. Returns None for
// released versions, which are named after their version number instead. The
// status is read from the `R_STATUS` macro in `include/Rversion.h`: it is an
// empty string for releases, "Under development (unstable)" for R-devel, and
// another label (e.g. a prerelease string) for R-next.
pub fn user_mode_dev_dirname(status: Option<&str>) -> Option<String> {
    match status {
        Some("Under development (unstable)") => Some("devel".to_string()),
        Some(s) if !s.is_empty() => Some("next".to_string()),
        _ => None,
    }
}

// Switch rig's persisted and in-process mode to user mode. We write the config
// and prime the in-process mode (via the RIG_MODE env var, which child
// processes inherit) so that the subsequent reinstallation targets the user
// location. The persisted mode is read first so we can report whether we
// actually switched.
pub fn switch_to_user_mode() -> Result<(), Box<dyn Error>> {
    let already_user = crate::config::get_global_config_value("mode")?.as_deref() == Some("user");
    env::set_var("RIG_MODE", "user");
    let _ = set_mode(Mode::User);
    crate::config::set_global_config_value("mode", "user")?;
    if already_user {
        OUTPUT.success("rig already in user mode");
        info!("rig already in user mode");
    } else {
        OUTPUT.success("Switched rig to user mode");
        info!("Switched rig to user mode");
    }
    Ok(())
}

// Restore the previously-default R version after reinstalling in user mode.
// `map` maps each admin-mode version name to the user-mode name it was
// reinstalled as; `default` is the admin-mode default (if any).
pub fn restore_user_mode_default(map: &[(String, String)], default: &Option<String>) {
    let adef = match default {
        Some(d) => d,
        None => return,
    };
    match map.iter().find(|(a, _)| a == adef) {
        Some((_, udef)) => {
            OUTPUT.status(&format!("Restoring default R version ({})", udef));
            if let Err(e) = sc_set_default(udef) {
                OUTPUT.warn(&format!("Could not restore default R version: {}", e));
                warn!("Could not restore default R version: {}", e);
            }
        }
        None => {
            OUTPUT.warn(&format!(
                "Could not restore the previous default R version ({})",
                adef
            ));
            warn!("Could not restore default R version {}", adef);
        }
    }
}

pub fn get_default_r_version() -> Result<Option<String>, Box<dyn Error>> {
    let default = sc_get_default()?;
    let re = Regex::new("^Version:[ ]?")?;
    match default {
        None => Ok(None),
        Some(d) => {
            let name = check_installed(&d)?;
            let desc = Path::new(&get_r_root_for(&name)?)
                .join(get_r_syslibpath()?.replace("{}", &version_dir_key(&name)))
                .join("base/DESCRIPTION");
            let lines = read_lines(&desc).unwrap_or_default();
            let idx = grep_lines(&re, &lines);
            let version: Option<String> = if idx.is_empty() {
                None
            } else {
                Some(re.replace(&lines[idx[0]], "").to_string())
            };

            Ok(version)
        }
    }
}

// -- rig list ------------------------------------------------------------

pub fn get_r_version_data_version(name: &str) -> Result<String, Box<dyn Error>> {
    let re = Regex::new("^Version:[ ]?").expect("Invalid regex pattern");
    let desc = Path::new(&get_r_root_for(name)?)
        .join(get_r_syslibpath()?.replace("{}", &version_dir_key(name)))
        .join("base/DESCRIPTION");
    let lines = read_lines(&desc).unwrap_or_default();
    let idx = grep_lines(&re, &lines);
    if idx.is_empty() {
        bail!(
            "Could not find version information for R {} in base/DESCRIPTION file",
            name
        );
    } else {
        Ok(re.replace(&lines[idx[0]], "").to_string())
    }
}

pub fn get_r_version_data(
    name: &str,
    aliases: &[Alias],
) -> Result<InstalledVersion, Box<dyn Error>> {
    let version = match get_r_version_data_version(name) {
        Ok(v) => Some(v),
        Err(e) => {
            OUTPUT.warn(&format!("R installation '{}' looks broken: {}", name, e));
            warn!("R installation '{}' looks broken: {}", name, e);
            None
        }
    };
    let path = Path::new(&get_r_root_for(name)?)
        .join(get_r_versiondir()?.replace("{}", &version_dir_key(name)));
    let binary = Path::new(&get_r_root_for(name)?)
        .join(get_r_binpath()?.replace("{}", &version_dir_key(name)));
    let mut myaliases: Vec<String> = vec![];
    for a in aliases {
        // Don't list an alias that is the same as the installation name,
        // e.g. the `devel`/`next` aliases on a `devel`/`next` install.
        if a.version == name && a.alias != name {
            myaliases.push(a.alias.to_owned());
        }
    }
    Ok(InstalledVersion {
        name: name.to_string(),
        version,
        path: path.to_str().map(|x| x.to_string()),
        binary: binary.to_str().map(|x| x.to_string()),
        aliases: myaliases,
    })
}

pub fn sc_get_list_details() -> Result<Vec<InstalledVersion>, Box<dyn Error>> {
    let names = sc_get_list()?;
    let aliases = find_aliases()?;
    let mut res: Vec<InstalledVersion> = vec![];

    for name in &names {
        res.push(get_r_version_data(name, &aliases)?);
    }

    Ok(res)
}

// -- rig system add-pak (implementation) ---------------------------------

// TODO: we should not hardcode this here...
pub fn check_has_pak(ver: &str) -> Result<bool, Box<dyn Error>> {
    // cur off -arm64 and -x86_64
    let mut ver = Regex::new("-.*$")?.replace(ver, "").to_string();

    // add .0 for macOS minor versions
    let minor = Regex::new("^[0-9]+[.][0-9]+$")?;
    if minor.is_match(&ver) {
        ver += ".0";
    }

    // cut off extra stuff on Windows
    ver = Regex::new("[a-zA-Z][a-zA-Z0-9]*$")?
        .replace(&ver, "")
        .to_string();

    let vv = match Version::parse(&ver) {
        Ok(x) => x,
        Err(_) => return Ok(true), // devel or next, probably
    };

    let v350 = Version::parse("3.5.0")?;
    if vv < v350 {
        OUTPUT.error("Pak is only available for R 3.5.0 or later");
        error!("Pak is only available for R 3.5.0 or later");
        bail!("Pak is only available for R 3.5.0 or later");
    }
    Ok(true)
}

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
        let ver = check_installed(&ver)?;
        let msg = if update {
            format!("Updating pak for R {}", ver)
        } else {
            format!("Installing pak for R {} (if not installed yet)", ver)
        };
        OUTPUT.status(&msg);
        info!("{}", msg);
        check_has_pak(&ver)?;

        // We do this to create the user library, because currently there
        // is a bug in the system profile code that creates it, and it is
        // only added after a restart.
        match r(&ver, "invisible()") {
            Ok(_) => {}
            Err(x) => {
                OUTPUT.error(&format!("Failed to to install pak for R {}: {}", ver, x));
                error!("Failed to to install pak for R {}: {}", ver, x);
                bail!("Failed to to install pak for R {}: {}", ver, x.to_string())
            }
        };

        // The actual pak installation
        let cmd = if update {
            r#"
                install.packages('pak', repos = sprintf('https://r-lib.github.io/p/pak/{}/%s/%s/%s', .Platform$pkgType, R.Version()$os, R.Version()$arch))
            "#
        } else {
            r#"
                if (!requireNamespace('pak', quietly = TRUE)) {
                    install.packages('pak', repos = sprintf('https://r-lib.github.io/p/pak/{}/%s/%s/%s', .Platform$pkgType, R.Version()$os, R.Version()$arch))
                }
            "#
        };
        let cmd = cmd.replace("{}", stream);

        match r(&ver, &cmd) {
            Ok(_) => {}
            Err(x) => {
                OUTPUT.error(&format!("Failed to install pak for R {}: {}", ver, x));
                error!("Failed to install pak for R {}: {}", ver, x);
                bail!("Failed to install pak for R {}: {}", ver, x.to_string())
            }
        };
    }

    Ok(())
}

// -- rig rstudio ---------------------------------------------------------

fn look_for_file(p: &Path, re: Regex) -> Result<Option<PathBuf>, Box<dyn Error>> {
    let paths = std::fs::read_dir(p)?;
    for file in paths {
        let path = file?.path();
        let pathstr = match path.file_name() {
            Some(x) => x,
            None => continue,
        };
        let pathstr = match pathstr.to_str() {
            Some(x) => x,
            None => continue,
        };
        if re.is_match(pathstr) {
            return Ok(Some(path));
        }
    }

    Ok(None)
}

// This mirrors how RStudio resolves the user config directory on Linux and
// macOS, see
// https://github.com/rstudio/rstudio/blob/44f09c50d469a14d5a9c3840c7a239f3bf21ace9/src/cpp/core/system/Xdg.cpp#L85
//
// 1. RSTUDIO_CONFIG_HOME might point to the final path.
// 2. XDG_CONFIG_HOME might point to the user config path (append `rstudio`).
// 3. Otherwise fall back to `~/.config` (append `rstudio`).
//
// Environment variables (`$VAR` / `${VAR}`) and a leading `~` are expanded in
// the resolved path. (The Windows version of this function lives in
// `src/windows/mod.rs` and uses `FOLDERID_RoamingAppData` and `RStudio`.)
#[cfg(unix)]
pub fn get_rstudio_config_path() -> Result<PathBuf, Box<dyn Error>> {
    let mut final_path = true;
    let mut xdg: Option<PathBuf> = None;

    // RSTUDIO_CONFIG_HOME may point to the final path
    if let Ok(x) = env::var("RSTUDIO_CONFIG_HOME") {
        if !x.is_empty() {
            xdg = Some(PathBuf::from(x));
        }
    }

    // XDG_CONFIG_HOME may point to the user config path
    if xdg.is_none() {
        final_path = false;
        if let Ok(x) = env::var("XDG_CONFIG_HOME") {
            if !x.is_empty() {
                xdg = Some(PathBuf::from(x));
            }
        }
    }

    // Fall back to `~/.config`
    let xdg = xdg.unwrap_or_else(|| PathBuf::from("~/.config"));

    // Expand environment variables and a leading `~`
    let path = match xdg.to_str() {
        Some(x) => x,
        None => {
            OUTPUT.error("RStudio config path cannot be represented as Unicode. :(");
            error!("RStudio config path cannot be represented as Unicode. :(");
            bail!("RStudio config path cannot be represented as Unicode. :(")
        }
    };
    let path = match shellexpand::full(path) {
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

    let mut xdg = PathBuf::from(path);
    if !final_path {
        xdg.push("rstudio");
    }

    Ok(xdg)
}

pub fn sc_rstudio(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let mut ver: Option<&String> = args.get_one("version");
    let mut prj: Option<&String> = args.get_one("project-file");

    if args.get_flag("config-path") {
        let cp = get_rstudio_config_path();
        match cp {
            Ok(x) => println!("{}", x.display()),
            Err(x) => {
                OUTPUT.error(&format!("Error: {}", x));
                error!("Error: {}", x);
                bail!("Error: {}", x.to_string())
            }
        };
        return Ok(());
    }

    // If the first argument is an existing path, and the second is not,
    // then we switch the two
    if let Some(ver2) = ver {
        let path = Path::new(ver2);
        if path.exists() && (prj.is_none() || !Path::new(prj.unwrap()).exists()) {
            ver = args.get_one("project-file");
            prj = args.get_one("version");
        }
    }

    sc_rstudio2(ver, prj)
}

pub fn sc_rstudio2(ver: Option<&String>, prj: Option<&String>) -> Result<(), Box<dyn Error>> {
    let mut prj = prj;
    let mut prj2;
    if let Some(p) = prj {
        let path = Path::new(p);
        if path.exists() && path.is_dir() && !p.ends_with("/") {
            prj2 = Some(p.to_string() + "/").map(|x| x.to_string());
            prj = prj2.as_ref();
        }
    };
    if let Some(p) = prj {
        if !p.starts_with("/") && !p.starts_with(".") {
            prj2 = Some("./".to_string() + p).map(|x| x.to_string());
            prj = prj2.as_ref();
        }
    }

    // If there is a path, find its directory
    let (fver, fproj, farg) = match (ver, prj) {
        (None, None) => (None, None, None),
        (Some(v), None) => (Some(v.to_owned()), None, None),
        (Some(v), Some(p)) => {
            let pf = find_project_file(p)?;
            (Some(v.to_owned()), pf.0, pf.1)
        }
        (None, Some(p)) => {
            let pf = find_project_file(p)?;
            let v = get_project_version(p)?;
            (v, pf.0, pf.1)
        }
    };

    debug!("RStudio start: {:?}, {:?}, {:?}", fver, fproj, farg);
    sc_rstudio_(fver.as_deref(), fproj.as_deref(), farg.as_deref())
}

fn find_project_dir(path: &str) -> Result<PathBuf, Box<dyn Error>> {
    debug!("Find project dir in {}", path);
    let ppath = Path::new(path);
    if !ppath.exists() {
        OUTPUT.error(&format!("Could not find path {}", path));
        error!("Could not find path {}", path);
        bail!("Could not find path {}", path);
    }
    let ret = if ppath.is_dir() {
        ppath.to_path_buf()
    } else {
        match ppath.parent() {
            None => Path::new("/").to_path_buf(),
            Some(x) => x.to_path_buf(),
        }
    };

    debug!("Project dir: {:?}", ret.display());
    Ok(ret)
}

// Returns the project file, and also the original file if it
// was not the project file but another file

fn find_project_file(
    path: &str,
) -> Result<(Option<String>, Option<std::ffi::OsString>), Box<dyn Error>> {
    if path.ends_with(".Rproj") && !Path::new(path).is_dir() {
        Ok((Some(path.to_string()), None))
    } else {
        let dir = find_project_dir(path)?;
        let proj = look_for_file(&dir, Regex::new("[.]Rproj$").unwrap())?;
        let projstr = proj
            .as_ref()
            .and_then(|x| x.to_str())
            .map(|x| x.to_string());
        Ok((projstr, Some(std::ffi::OsString::from(path))))
    }
}

// Look at the project dir, and check if there is an renv.lock file

fn get_project_version(path: &str) -> Result<Option<String>, Box<dyn Error>> {
    let dir = find_project_dir(path)?;
    let renv = dir.join("renv.lock");
    if renv.exists() {
        let needver = renv::parse_r_version(renv)?;
        let usever = renv::match_r_version(&needver)?;
        let realver = usever.version.to_string();

        let msg = format!(
            "Using {} R {}{}",
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
        OUTPUT.info(&msg);
        info!("{}", msg);

        Ok(Some(usever.name.to_owned()))
    } else {
        Ok(None)
    }
}

// -- rig avilable --------------------------------------------------------

pub(crate) fn normalize_rig_platform(rp: &str) -> String {
    // "ubuntu-24.04" (one dash, not a known non-linux shorthand) -> "linux-ubuntu-24.04"
    if rp.matches('-').count() == 1 && rp != "macos" && rp != "windows" && !rp.starts_with("linux-")
    {
        format!("linux-{}", rp)
    } else {
        rp.to_string()
    }
}

pub fn get_platform(args: &ArgMatches) -> Result<String, Box<dyn Error>> {
    // rig add does not have a --platform argument, only auto-detect
    if args.try_contains_id("platform").is_ok() {
        let platform = args.get_one::<String>("platform");
        if let Some(x) = platform {
            return Ok(x.to_string());
        }
    };

    if let Ok(rp) = env::var("RIG_PLATFORM") {
        let rp = normalize_rig_platform(&rp);
        debug!("Using RIG_PLATFORM: {}.", rp);
        return Ok(rp);
    }

    #[allow(unused_mut)]
    let mut os = env::consts::OS.to_string();

    #[cfg(target_os = "linux")]
    {
        if os == "linux" {
            if get_mode()? == Mode::User {
                // User mode installs a portable build (manylinux/musllinux),
                // selected by libc rather than by the host distro.
                os = crate::linux::user_mode_platform()?;
            } else {
                let dist = detect_platform()?;
                if let (Some(distro), Some(version)) = (dist.distro, dist.version) {
                    os = format!("linux-{}-{}", distro, version);
                }
            }
        }
    }

    debug!("Auto-detected platform: {}.", os);

    Ok(os)
}

pub fn get_arch(platform: &str, args: &ArgMatches) -> String {
    #[allow(unused_mut)]
    // For rig add we don't have --arch, except on macOS, only auto-detect
    let arch = match args.try_contains_id("arch") {
        Ok(_) => args.get_one::<String>("arch"),
        Err(_) => None,
    };

    let arch = match arch {
        Some(x) => x.to_string(),
        None => env::consts::ARCH.to_string(),
    };

    // Prefer 'arm64' on macos, but 'aarch64' on linux and windows
    if platform == "macos" && arch == "aarch64" {
        "arm64".to_string()
    } else if arch == "arm64" {
        "aarch64".to_string()
    } else {
        arch
    }
}

pub fn sc_available(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    #[allow(unused_mut)]
    if args.get_flag("list-distros") {
        return sc_available_distros(args, mainargs);
    }

    if args.get_flag("list-rtools-versions") {
        return sc_available_rtools_versions(args, mainargs);
    }

    let platform = get_platform(args)?;
    let arch = get_arch(&platform, args);

    let url = "https://api.r-hub.io/rversions/available/".to_string() + &platform + "/" + &arch;
    let resp = download_json_sync(vec![url])?;
    let resp = resp[0].as_array().unwrap();

    let mut vers: Vec<Available> = vec![];
    for item in resp.iter().rev() {
        let date = unquote(&item["date"].to_string());
        let rtype = unquote(&item["type"].to_string());
        let new = Available {
            name: unquote(&item["name"].to_string()),
            version: unquote(&item["version"].to_string()),
            date: if date == "null" { None } else { Some(date) },
            url: Some(unquote(&item["url"].to_string())),
            rtype: Some(rtype),
        };

        if !args.get_flag("all") && !vers.is_empty() && new.name != "next" && new.name != "devel" {
            let lstnam = &vers[vers.len() - 1].name;
            let v300 = Version::parse("3.0.0")?;
            let lstver = Version::parse(&vers[vers.len() - 1].version)?;
            let thsver = Version::parse(&new.version)?;
            // drop old versions
            if thsver < v300 {
                continue;
            }
            // drop outdated minor versions
            if lstver.major == thsver.major
                && lstver.minor == thsver.minor
                && lstnam != "next"
                && lstnam != "devel"
            {
                continue;
            }
        }
        vers.push(new);
    }

    if args.get_flag("json") || mainargs.get_flag("json") {
        println!("[");
        let num = vers.len();
        for (idx, ver) in vers.iter().rev().enumerate() {
            let date = match &ver.date {
                None => "null".to_string(),
                Some(d) => "\"".to_string() + d + "\"",
            };
            let rtype = match &ver.rtype {
                None => "null".to_string(),
                Some(x) => "\"".to_string() + x + "\"",
            };
            let url = match &ver.url {
                None => "null".to_string(),
                Some(x) => "\"".to_string() + x + "\"",
            };
            println!("  {{");
            println!("    \"name\": \"{}\",", ver.name);
            println!("    \"date\": {},", date);
            println!("    \"version\": \"{}\",", ver.version);
            println!("    \"type\": {},", rtype);
            println!("    \"url\": {}", url);
            println!("  }}{}", if idx == num - 1 { "" } else { "," });
        }
        println!("]");
    } else {
        let mut tab = Table::new("{:<}  {:<}  {:<}  {:<}");
        tab.add_row(row!["name", "version", "release date", "type"]);
        tab.add_heading("------------------------------------------");
        for item in vers.iter().rev() {
            let date = match &item.date {
                None => "".to_string(),
                Some(d) => d[..10].to_string(),
            };
            let rtype = match &item.rtype {
                None => "".to_string(),
                Some(x) => x.to_string(),
            };
            tab.add_row(row!(&item.name, &item.version, date, rtype));
        }
        print!("{}", tab);
    }
    Ok(())
}

#[derive(Hash, Eq, PartialEq, Debug, Clone)]
struct Distro {
    name: String,
    version: String,
    id: String,
    ppm: bool,
    retired: bool,
    eol: String,
    last: Option<String>,
}

fn get_distros() -> Result<Vec<Distro>, Box<dyn Error>> {
    let mut distros: Vec<Distro> = vec![];

    let url = "https://api.r-hub.io/rversions/linux-distros".to_string();
    let resp = download_json_sync(vec![url])?;
    let resp = resp[0].as_array().unwrap();

    let mut distro_aliases: HashMap<String, Distro> = HashMap::new();
    for item in resp.iter() {
        // these are always there
        let name = item["name"].to_string();
        let version = item["version"].to_string();
        let id = item["id"].to_string();
        let eol = item["eol"].to_string();

        if item["implementation"].is_null() {
            // these are not there for aliases
            let ppm = item["ppm-binaries"].as_bool().unwrap_or_default();
            let retired = item["retired"].as_bool().unwrap_or_default();
            let last = item["last-build"].as_str().map(|s| s.to_string());
            let d = Distro {
                name,
                version,
                id: id.clone(),
                ppm,
                retired,
                eol,
                last,
            };
            distro_aliases.insert(id, d.clone());
            distros.push(d);
        } else {
            let imp = item["implementation"].to_string();
            let alias = distro_aliases.get(&imp);
            if let Some(alias2) = alias {
                let d = Distro {
                    name,
                    version,
                    id,
                    ppm: alias2.ppm,
                    retired: alias2.retired,
                    eol,
                    last: alias2.last.clone(),
                };
                distros.push(d);
            };
        }
    }

    Ok(distros)
}

fn sc_available_distros(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let distros = get_distros()?;

    if args.get_flag("json") || mainargs.get_flag("json") {
        let num = distros.len();
        println!("[");
        for (idx, item) in distros.iter().enumerate() {
            let last = match &item.last {
                Some(v) => "\"".to_string() + v + "\"",
                None => "null".to_string(),
            };
            println!("{{");
            println!("  \"name\": {},", item.name);
            println!("  \"version\": {},", item.version);
            println!("  \"id\": {},", item.id);
            println!("  \"ppm-binaries\": {},", item.ppm);
            println!("  \"retired\": {},", item.retired);
            println!("  \"eol\": {},", item.eol);
            println!("  \"last-build\": {}", last);
            println!("}}{}", if idx == num - 1 { "" } else { "," });
        }
        println!("]");
    } else {
        let mut tab = Table::new("{:<}  {:<}  {:<}  {:<}  {:<}  {:<}");
        tab.add_row(row!["name", "version", "id", "PPM", "retired", "eol"]);
        tab.add_heading(
            "-------------------------------------------------------------------------------",
        );
        for item in distros.iter() {
            tab.add_row(row!(
                unquote(&item.name),
                unquote(&item.version),
                unquote(&item.id),
                item.ppm.to_string(),
                item.retired.to_string(),
                unquote(&item.eol)
            ));
        }

        print!("{}", tab);
    }

    Ok(())
}

fn sc_available_rtools_versions(
    args: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let mut url = "https://api.r-hub.io/rversions/rtools-versions".to_string();
    let arch = get_arch("windows", args);
    if arch != "x86_64" {
        url = url + "/" + &arch;
    }
    let resp = download_json_sync(vec![url])?;
    let resp = resp[0].as_array().unwrap();
    let all = args.get_flag("all");

    fn show(ver: &str) -> bool {
        let iver = ver.parse::<i32>();
        match iver {
            Ok(x) => x >= 35 && !(210..=215).contains(&x),
            Err(_) => true,
        }
    }

    if args.get_flag("json") || mainargs.get_flag("json") {
        let num = resp.len();
        println!("[");
        for (idx, item) in resp.iter().enumerate() {
            let ver = unquote(&item["version"].to_string());
            if all || show(&ver) {
                println!("{{");
                println!("  \"version\": {},", item["version"]);
                println!("  \"first\": {},", item["first"]);
                println!("  \"last\": {},", item["last"]);
                println!("  \"url\": {}", item["url"]);
                println!("}}{}", if idx == num - 1 { "" } else { "," });
            }
        }
        println!("]");
    } else {
        let mut tab = Table::new("{:<}  {:<}  {:<}  {:<}");
        tab.add_row(row!["version", "from R", "to R", "URL"]);
        tab.add_heading("--------------------------------------------------------------");
        for item in resp.iter() {
            let ver = unquote(&item["version"].to_string());
            if all || show(&ver) {
                tab.add_row(row![
                    ver,
                    unquote(&item["first"].to_string()),
                    unquote(&item["last"].to_string()),
                    unquote(&item["url"].to_string())
                ]);
            }
        }
        print!("{}", tab);
    }

    Ok(())
}

// ------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_rig_platform_short_linux() {
        assert_eq!(normalize_rig_platform("ubuntu-24.04"), "linux-ubuntu-24.04");
        assert_eq!(normalize_rig_platform("fedora-40"), "linux-fedora-40");
    }

    #[test]
    fn test_normalize_rig_platform_already_prefixed() {
        assert_eq!(
            normalize_rig_platform("linux-ubuntu-22.04"),
            "linux-ubuntu-22.04"
        );
    }

    #[test]
    fn test_normalize_rig_platform_non_linux() {
        assert_eq!(normalize_rig_platform("macos"), "macos");
        assert_eq!(normalize_rig_platform("windows"), "windows");
    }
}
