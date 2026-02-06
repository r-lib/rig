#![cfg(target_os = "linux")]

use regex::Regex;
use std::error::Error;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::{file, line};

use clap::ArgMatches;
use simple_error::*;
use simplelog::{debug, info, trace, warn};

use crate::rversion::*;

use crate::alias::*;
use crate::common::*;
use crate::download::*;
use crate::escalate::*;
use crate::library::*;
use crate::resolve::get_resolve;
use crate::run::*;
use crate::utils::*;

pub const R_ROOT_: &str = "/opt/R";
pub const R_VERSIONDIR: &str = "{}";
pub const R_SYSLIBPATH: &str = "{}/lib/R/library";
pub const R_BINPATH: &str = "{}/bin/R";
const R_CUR: &str = "/opt/R/current";

const PPM_URL: &str = "https://packagemanager.posit.co/cran";

macro_rules! strvec {
    // match a list of expressions separated by comma:
    ($($str:expr),*) => ({
        // create a Vec with this list of expressions,
        // calling String::from on each:
        vec![$(String::from($str),)*] as Vec<String>
    });
}

macro_rules! osvec {
    // match a list of expressions separated by comma:
    ($($str:expr),*) => ({
        // create a Vec with this list of expressions,
        // calling String::from on each:
        vec![$(OsString::from($str),)*] as Vec<OsString>
    });
}

pub fn get_r_root() -> String {
    R_ROOT_.to_string()
}

pub fn sc_add(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    escalate("adding new R versions")?;

    // This is needed to fix statix linking on Arm Linux :(
    let uid = nix::unistd::getuid().as_raw();
    if false {
        println!("{}", uid);
    }

    let platform = detect_linux()?;
    let version = get_resolve(args)?;
    let alias = get_alias(args);
    let ver = version.version.to_owned();
    let verstr = match ver {
        Some(ref x) => x,
        None => "???",
    };

    let url: String = match &version.url {
        Some(s) => s.to_string(),
        None => bail!("Cannot find a download url for R version {}", verstr),
    };

    let filename = basename(&url).unwrap_or_else(|| "foo");
    let tmp_dir = std::env::temp_dir().join("rig");
    let target = tmp_dir.join(&filename);
    if target.exists() && not_too_old(&target) {
        info!("{} is cached at {}", filename, target.display());
    } else {
        info!("Downloading {} -> {}", url, target.display());
        let client = &reqwest::Client::new();
        download_file(client, &url, &target.as_os_str())?;
    }

    let dirname;
    dirname = add_package(target.as_os_str(), &platform)?;

    set_default_if_none(dirname.to_string())?;

    library_update_rprofile(&dirname.to_string())?;
    check_usr_bin_sed(&dirname.to_string())?;
    sc_system_make_links()?;
    match alias {
        Some(alias) => add_alias(&dirname, &alias)?,
        None => {}
    };

    if !args.get_flag("without-cran-mirror") {
        set_cloud_mirror(Some(vec![dirname.to_string()]))?;
    }

    if !args.get_flag("without-p3m") {
        set_ppm(Some(vec![dirname.to_string()]), &platform, &version)?;
    }

    if args.get_flag("without-sysreqs") {
        set_sysreqs_false(Some(vec![dirname.to_string()]))?;
    }

    if !args.get_flag("without-pak") {
        let explicit =
            args.value_source("pak-version") == Some(clap::parser::ValueSource::CommandLine);
        system_add_pak(
            Some(vec![dirname.to_string()]),
            require_with!(args.get_one::<String>("pak-version"), "clap error"),
            // If this is specified then we always re-install
            explicit,
        )?;
    }

    Ok(())
}

fn select_linux_tools(platform: &OsVersion) -> Result<LinuxTools, Box<dyn Error>> {
    if platform.distro == "debian" || platform.distro == "ubuntu" {
        Ok(LinuxTools {
            package_name: "r-{}".to_string(),
            install: vec![
                strvec!["apt-get", "update"],
                strvec![
                    "apt",
                    "install",
                    "--reinstall",
                    "-y",
                    "-o=Dpkg::Use-Pty=0",
                    "-o=Apt::Cmd::Disable-Script-Warning=1",
                    "{}"
                ],
            ],
            get_package_name: strvec!["dpkg", "--field", "{}", "Package"],
            delete: strvec![
                "apt-get",
                "remove",
                "-y",
                "-o=Dpkg::Use-Pty=0",
                "--purge",
                "{}"
            ],
            is_installed: strvec!["dpkg", "-s", "{}"],
        })
    } else if platform.distro == "opensuse" || platform.distro == "sles" {
        Ok(LinuxTools {
            package_name: "R-{}".to_string(),
            install: vec![strvec!["zypper", "--no-gpg-checks", "install", "-y", "{}"]],
            get_package_name: strvec!["rpm", "-q", "--qf", "%{NAME}", "-p", "{}"],
            is_installed: strvec!["rpm", "-q", "{}"],
            delete: strvec!["zypper", "remove", "-y", "{}"],
        })
    } else if platform.distro == "fedora" {
        Ok(LinuxTools {
            package_name: "R-{}".to_string(),
            install: vec![strvec!["dnf", "install", "-y", "{}"]],
            get_package_name: strvec!["rpm", "-q", "--qf", "%{NAME}", "-p", "{}"],
            is_installed: strvec!["rpm", "-q", "{}"],
            delete: strvec!["dnf", "remove", "-y", "{}"],
        })
    } else if (platform.distro == "centos")
        && (platform.version == "7" || platform.version.starts_with("7."))
    {
        Ok(LinuxTools {
            package_name: "R-{}".to_string(),
            install: vec![
                strvec!["yum", "install", "-y", "epel-release"],
                strvec!["yum", "install", "-y", "{}"],
            ],
            get_package_name: strvec!["rpm", "-q", "--qf", "%{NAME}", "-p", "{}"],
            is_installed: strvec!["rpm", "-q", "{}"],
            delete: strvec!["yum", "remove", "-y", "{}"],
        })
    } else if (platform.distro == "rhel")
        && (platform.version == "7" || platform.version.starts_with("7."))
    {
        Ok(LinuxTools{
            package_name: "R-{}".to_string(),
            install: vec![
                strvec!["bash", "-c", "rpm -q epel-release || yum install -y https://dl.fedoraproject.org/pub/epel/epel-release-latest-7.noarch.rpm"],
                strvec!["yum", "--enablerepo", "rhel-7-server-optional-rpms", "install", "-y", "{}"]
            ],
            get_package_name:
                strvec!["rpm", "-q", "--qf", "%{NAME}", "-p", "{}"],
            is_installed:
                strvec!["rpm", "-q", "{}"],
            delete:
                strvec!["yum", "remove", "-y", "{}"]
        })
    } else if (platform.distro == "rhel"
        || platform.distro == "almalinux"
        || platform.distro == "rocky")
        && (platform.version == "8" || platform.version.starts_with("8."))
    {
        Ok(LinuxTools {
            package_name: "R-{}".to_string(),
            install: vec![strvec!["dnf", "install", "-y", "{}"]],
            get_package_name: strvec!["rpm", "-q", "--qf", "%{NAME}", "-p", "{}"],
            is_installed: strvec!["rpm", "-q", "{}"],
            delete: strvec!["dnf", "remove", "-y", "{}"],
        })
    } else if (platform.distro == "almalinux" || platform.distro == "rocky")
        && (platform.version == "9" || platform.version.starts_with("9."))
    {
        Ok(LinuxTools {
            package_name: "R-{}".to_string(),
            install: vec![
                strvec!["dnf", "install", "-y", "dnf-plugins-core"],
                strvec!["dnf", "config-manager", "--set-enabled", "crb"],
                strvec!["dnf", "install", "-y", "epel-release"],
                strvec!["dnf", "install", "-y", "{}"],
            ],
            get_package_name: strvec!["rpm", "-q", "--qf", "%{NAME}", "-p", "{}"],
            is_installed: strvec!["rpm", "-q", "{}"],
            delete: strvec!["dnf", "remove", "-y", "{}"],
        })
    } else if (platform.distro == "rhel")
        && (platform.version == "9" || platform.version.starts_with("9."))
    {
        let crb = "codeready-builder-for-rhel-9-".to_string() + &platform.arch + "-rpms";
        Ok(LinuxTools{
                package_name: "R-{}".to_string(),
                install: vec![
            strvec!["bash", "-c", "rpm -q epel-release || dnf install -y https://dl.fedoraproject.org/pub/epel/epel-release-latest-9.noarch.rpm"],
                    strvec!["dnf", "install", "--enablerepo", crb, "-y", "{}"]
                ],
                get_package_name:
                    strvec!["rpm", "-q", "--qf", "%{NAME}", "-p", "{}"],
                is_installed:
                    strvec!["rpm", "-q", "{}"],
                delete:
                    strvec!["dnf", "remove", "-y", "{}"]
            })
    } else if platform.distro == "almalinux"
        || platform.distro == "rocky"
        || platform.distro == "rhel"
    {
        Ok(LinuxTools {
            package_name: "R-{}".to_string(),
            install: vec![strvec!["dnf", "install", "-y", "{}"]],
            get_package_name: strvec!["rpm", "-q", "--qf", "%{NAME}", "-p", "{}"],
            is_installed: strvec!["rpm", "-q", "{}"],
            delete: strvec!["yum", "remove", "-y", "{}"],
        })
    } else {
        bail!(
            "I don't know how to install a system package on {} {}",
            platform.distro,
            platform.version
        );
    }
}

fn add_package(path: &OsStr, platform: &OsVersion) -> Result<String, Box<dyn Error>> {
    let tools = select_linux_tools(platform)?;

    for cmd in tools.install.iter() {
        let cmd = format_cmd_args(cmd.to_vec(), path);
        info!("Running {:?}", cmd.join(&OsString::from(" ")));
        let cmd0 = cmd[0].to_owned();
        run(cmd0, cmd[1..].to_vec(), "installation")?;
    }

    let get_package_name = format_cmd_args(tools.get_package_name, path);
    let cmd0 = get_package_name[0].to_owned();
    let oscmd = get_package_name.join(&OsString::from(" "));
    let out = try_with!(
        Command::new(cmd0)
            .args(get_package_name[1..].to_vec())
            .output(),
        "Failed to run {:?} @{}:{}",
        oscmd,
        file!(),
        line!()
    );

    let std = try_with!(
        String::from_utf8(out.stdout),
        "Non-UTF-8 output from {:?} @{}:{}",
        oscmd,
        file!(),
        line!()
    );

    let re = Regex::new("^[rR]-(.*)\\s*$")?;
    let ver = re.replace(&std, "${1}");

    Ok(ver.to_string())
}

pub fn sc_rm(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    escalate("removing R versions")?;
    let vers = args.get_many::<String>("version");
    if vers.is_none() {
        return Ok(());
    }
    let vers = require_with!(vers, "clap error");

    let platform = detect_linux()?;
    let tools = select_linux_tools(&platform)?;
    for ver in vers {
        let ver = check_installed(&ver.to_string())?;

        let pkgname = tools.package_name.replace("{}", &ver);
        let opkgname = OsStr::new(&pkgname);
        let cmd = format_cmd_args(tools.is_installed.clone(), opkgname);
        let oscmd = cmd.join(&OsString::from(" "));
        let cmd0 = cmd[0].to_owned();
        let out = try_with!(
            Command::new(cmd0).args(cmd[1..].to_vec()).output(),
            "Failed to run {:?} @{}:{}",
            oscmd,
            file!(),
            line!()
        );

        if out.status.success() {
            info!("Removing {} package", pkgname);
            let cmd = format_cmd_args(tools.delete.clone(), opkgname);
            let cmd0 = cmd[0].to_owned();
            run(cmd0, cmd[1..].to_vec(), "deleting system package")?;
        } else {
            info!("{} package is not installed", pkgname);
        }

        let rroot = get_r_root();
        let dir = Path::new(&rroot);
        let dir = dir.join(&ver);
        if dir.exists() {
            info!("Removing {}", dir.display());
            try_with!(
                std::fs::remove_dir_all(&dir),
                "Failed to remove {} @{}:{}",
                dir.display(),
                file!(),
                line!()
            );
        }
    }

    sc_system_make_links()?;

    Ok(())
}

pub fn sc_system_make_links() -> Result<(), Box<dyn Error>> {
    escalate("making R-* quick links")?;
    let vers = sc_get_list()?;
    let rroot = get_r_root();
    let base = Path::new(&rroot);

    // Create new links
    for ver in vers {
        let linkfile = Path::new("/usr/local/bin").join("R-".to_string() + &ver);
        let target = base.join(&ver).join("bin/R");
        if !linkfile.exists() {
            info!("Adding {} -> {}", linkfile.display(), target.display());
            symlink(&target, &linkfile)?;
        }
    }

    // Remove dangling links
    let paths = std::fs::read_dir("/usr/local/bin")?;
    let re = Regex::new("^R-([0-9]+[.][0-9]+[.][0-9]+|oldrel|next|release|devel)$")?;
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
                Err(_) => warn!("<magenra>[WARN]</> {} is not a symlink", path.display()),
                Ok(target) => {
                    if !target.exists() {
                        info!("Cleaning up {}", target.display());
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

pub fn re_alias() -> Regex {
    let re = Regex::new("^R-(release|oldrel)$").unwrap();
    re
}

pub fn find_aliases() -> Result<Vec<Alias>, Box<dyn Error>> {
    debug!("Finding existing aliases");

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
            trace!("Checking {}", path.display());
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
                                trace!("{} -> {}", fnamestr, version);
                                let als = Alias {
                                    alias: fnamestr[2..].to_string(),
                                    version: version.to_string(),
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

fn version_from_link(pb: PathBuf) -> Option<String> {
    let osver = match pb
        .parent()
        .and_then(|x| x.parent())
        .and_then(|x| x.file_name())
    {
        None => None,
        Some(s) => Some(s.to_os_string()),
    };

    let s = match osver {
        None => None,
        Some(os) => os.into_string().ok(),
    };

    s
}

pub fn sc_get_list() -> Result<Vec<String>, Box<dyn Error>> {
    let mut vers = Vec::new();
    if !Path::new(&get_r_root()).exists() {
        return Ok(vers);
    }

    let paths = std::fs::read_dir(get_r_root())?;

    for de in paths {
        let path = de?.path();
        // If no path name, then path ends with ..., so we can skip
        let fname = match path.file_name() {
            Some(x) => x,
            None => continue,
        };
        // If the path is not UTF-8, we'll skip it, this should not happen
        let fname = match fname.to_str() {
            Some(x) => x,
            None => continue,
        };
        if fname == "current" {
            continue;
        }
        // If there is no bin/R, then this is not an R installation
        let rbin = path.join("bin").join("R");
        if !rbin.exists() {
            continue;
        }

        vers.push(fname.to_string());
    }
    vers.sort();
    Ok(vers)
}

pub fn sc_set_default(ver: &str) -> Result<(), Box<dyn Error>> {
    escalate("setting the default R version")?;
    let ver = check_installed(&ver.to_string())?;
    trace!("Setting default version to {}", ver);

    // Remove current link
    // We do not check if it exists, because that follows the symlink
    trace!("Removing current at {}", R_CUR);
    std::fs::remove_file(R_CUR).ok();

    // Add current link
    let path = Path::new(&get_r_root()).join(ver);
    trace!("Adding symlink at {}", R_CUR);
    std::os::unix::fs::symlink(&path, R_CUR)?;

    // Remove /usr/local/bin/R link
    let r = Path::new("/usr/local/bin/R");
    trace!("Removing link at {}", r.display());
    std::fs::remove_file(r).ok();

    // Add /usr/local/bin/R link
    let cr = Path::new("/opt/R/current/bin/R");
    trace!("Adding /usr/local/bin/R link");
    std::os::unix::fs::symlink(&cr, &r)?;

    // Remove /usr/local/bin/Rscript link
    let rs = Path::new("/usr/local/bin/Rscript");
    trace!("Removing /usr/local/bin/Rscript link");
    std::fs::remove_file(rs).ok();

    // Add /usr/local/bin/Rscript link
    let crs = Path::new("/opt/R/current/bin/Rscript");
    trace!("Adding /usr/local/bin/Rscript link");
    std::os::unix::fs::symlink(&crs, &rs)?;

    Ok(())
}

pub fn sc_get_default() -> Result<Option<String>, Box<dyn Error>> {
    read_version_link(R_CUR)
}

fn set_cloud_mirror(vers: Option<Vec<String>>) -> Result<(), Box<dyn Error>> {
    let vers = match vers {
        Some(x) => x,
        None => sc_get_list()?,
    };

    info!("Setting default CRAN mirror");

    for ver in vers {
        let ver = check_installed(&ver)?;
        let path = Path::new(&get_r_root()).join(ver.as_str());
        let profile = path.join("lib/R/library/base/R/Rprofile".to_string());
        if !profile.exists() {
            continue;
        }

        append_to_file(
            &profile,
            vec![
                r#"if (Sys.getenv("RSTUDIO") != "1" && Sys.getenv("POSITRON") != "1") {
  options(repos = c(CRAN = "https://cloud.r-project.org"))
}"#
                .to_string(),
            ],
        )?;
    }
    Ok(())
}

fn set_ppm(
    vers: Option<Vec<String>>,
    platform: &OsVersion,
    version: &Rversion,
) -> Result<(), Box<dyn Error>> {
    if !version.ppm || version.ppmurl.is_none() {
        info!(
            "P3M (or rig) does not support this distro: {} {} or architecture: {}",
            platform.distro, platform.version, platform.arch
        );
        return Ok(());
    }

    info!("Setting up P3M");

    let vers = match vers {
        Some(x) => x,
        None => sc_get_list()?,
    };

    let rcode = r#"
if (Sys.getenv("RSTUDIO") != "1" && Sys.getenv("POSITRON") != "1") {
  options(repos = c(P3M="%url%", getOption("repos")))
  options(HTTPUserAgent = sprintf("R/%s R (%s)", getRversion(), paste(getRversion(), R.version$platform, R.version$arch, R.version$os)))
}
"#;

    let ppm_url =
        PPM_URL.to_string() + "/__linux__/" + &version.ppmurl.clone().unwrap() + "/latest";
    let rcode = rcode.to_string().replace("%url%", &ppm_url);

    for ver in vers {
        let ver = check_installed(&ver)?;
        let path = Path::new(&get_r_root()).join(ver.as_str());
        let profile = path.join("lib/R/library/base/R/Rprofile".to_string());
        if !profile.exists() {
            continue;
        }

        append_to_file(&profile, vec![rcode.to_string()])?;
    }
    Ok(())
}

fn set_sysreqs_false(vers: Option<Vec<String>>) -> Result<(), Box<dyn Error>> {
    info!("Setting up automatic system requirements installation.");

    let vers = match vers {
        Some(x) => x,
        None => sc_get_list()?,
    };

    let rcode = r#"
if (Sys.getenv("PKG_SYSREQS") == "") Sys.setenv(PKG_SYSREQS = "false")
"#;

    for ver in vers {
        let ver = check_installed(&ver)?;
        let path = Path::new(&get_r_root()).join(ver.as_str());
        let profile = path.join("lib/R/library/base/R/Rprofile".to_string());
        if !profile.exists() {
            continue;
        }

        append_to_file(&profile, vec![rcode.to_string()])?;
    }
    Ok(())
}

pub fn sc_system_allow_core_dumps(_args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    // Nothing to do on Linux
    Ok(())
}

pub fn sc_system_allow_debugger(_args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    // Nothing to do on Linux
    Ok(())
}

pub fn sc_system_allow_debugger_rstudio(_args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    // Nothing to do on Linux
    Ok(())
}

pub fn sc_system_make_orthogonal(_args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    // Nothing to do on Linux
    Ok(())
}

pub fn sc_system_fix_permissions(_args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    // Nothing to do on Linux
    Ok(())
}

pub fn sc_system_forget() -> Result<(), Box<dyn Error>> {
    // Nothing to do on Linux
    Ok(())
}

pub fn sc_system_no_openmp(_args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    // Nothing to do on Linux
    Ok(())
}

pub fn detect_linux() -> Result<OsVersion, Box<dyn Error>> {
    let release_file = Path::new("/etc/os-release");
    let lines = read_lines(release_file)?;

    let mut id;
    let mut ver;

    let rig_platform = match std::env::var("RIG_PLATFORM") {
        Ok(x) => Some(x),
        Err(_) => None,
    };

    if rig_platform.is_some() {
        let mut rig_platform2 = rig_platform.clone().unwrap();
        if rig_platform2.starts_with("linux-") {
            rig_platform2 = rig_platform2.strip_prefix("linux-").unwrap().to_string();
        }

        (id, ver) = match rig_platform2.split_once("-") {
            Some((x, y)) => (x.to_string(), y.to_string()),
            None => (rig_platform2, "".to_string()),
        };
    } else {
        let re_id = Regex::new("^ID=")?;
        let wid_line = grep_lines(&re_id, &lines);
        id = if wid_line.len() == 0 {
            "".to_string()
        } else {
            let id_line = &lines[wid_line[0]];
            let id = re_id.replace(&id_line, "").to_string();
            unquote(&id)
        };

        let re_ver = Regex::new("^VERSION_ID=")?;
        let wver_line = grep_lines(&re_ver, &lines);
        ver = if wver_line.len() == 0 {
            "".to_string()
        } else {
            let ver_line = &lines[wver_line[0]];
            let ver = re_ver.replace(&ver_line, "").to_string();
            unquote(&ver)
        };

        // workaround for a node-rversions bug
        if id == "opensuse-leap" {
            id = "opensuse".to_string()
        }
        if id == "opensuse" {
            ver = ver.replace(".", "");
        }
    }

    let arch = std::env::consts::ARCH.to_string();
    let vendor = "unknown".to_string();
    let os = "linux".to_string();
    let distro = id.to_owned();
    let version = ver.to_owned();

    Ok(OsVersion {
        rig_platform,
        arch,
        vendor,
        os,
        distro,
        version,
    })
}

pub fn sc_clean_registry() -> Result<(), Box<dyn Error>> {
    // Nothing to do on Linux
    Ok(())
}

pub fn sc_system_update_rtools40() -> Result<(), Box<dyn Error>> {
    Ok(())
}

pub fn sc_system_rtools(_args: &ArgMatches, _mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    Ok(())
}

pub fn sc_rstudio_(
    version: Option<&str>,
    project: Option<&str>,
    arg: Option<&OsStr>,
) -> Result<(), Box<dyn Error>> {
    let cwd = std::env::current_dir();
    let (cmd, mut args) = match project {
        Some(p) => ("xdg-open", osvec![p]),
        None => match cwd {
            Ok(x) => ("rstudio", osvec![x]),
            Err(_) => ("rstudio", osvec![]),
        },
    };

    let mut envname = "dummy";
    let mut path = "".to_string();
    if let Some(ver) = version {
        let ver = check_installed(&ver.to_string())?;
        envname = "RSTUDIO_WHICH_R";
        path = get_r_root().to_string() + "/" + &ver + "/bin/R"
    };

    if let Some(arg) = arg {
        if project.is_none() {
            args.push(arg.into());
        }
    }

    info!("Running {} {:?}", cmd, args.join(&os(" ")));

    Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env(envname, &path)
        .spawn()?;

    Ok(())
}

pub fn get_r_binary(rver: &str) -> Result<PathBuf, Box<dyn Error>> {
    debug!("Finding R binary for R {}", rver);
    let bin = Path::new(&get_r_root()).join(rver).join("bin/R");
    debug!("R {} binary is at {}", rver, bin.display());
    Ok(bin)
}

#[allow(dead_code)]
pub fn get_system_renviron(rver: &str) -> Result<PathBuf, Box<dyn Error>> {
    let renviron = Path::new(&get_r_root())
        .join(rver)
        .join("lib/R/etc/Renviron");
    Ok(renviron)
}

pub fn get_system_profile(rver: &str) -> Result<PathBuf, Box<dyn Error>> {
    let profile = Path::new(&get_r_root())
        .join(rver)
        .join("lib/R/library/base/R/Rprofile");
    Ok(profile)
}

// /usr/bin/sed might not be available, and R will need it (issue 119#)

fn check_usr_bin_sed(rver: &str) -> Result<(), Box<dyn Error>> {
    let usrbinsed = Path::new("/usr/bin/sed");
    let binsed = Path::new("/bin/sed");

    debug!("Checking if SED = /usr/bin/sed if OK");

    // in these cases we don't need to or cannot do anything
    if usrbinsed.exists() {
        debug!("/usr/bin/sed exists giving up");
        return Ok(());
    }
    if !binsed.exists() {
        debug!("/bin/sed missing, giving up");
        return Ok(());
    }

    let makeconf = Path::new(&get_r_root())
        .join(rver)
        .join("lib/R/etc/Makeconf");
    let lines: Vec<String> = match read_lines(&makeconf) {
        Ok(x) => x,
        Err(_) => {
            // this should not happen, but if it does, then
            // something weird is going on and we'll bail out
            // silently
            debug!("Cannot read Makeconf, giving up");
            return Ok(());
        }
    };
    let re_sed = Regex::new("^SED = /usr/bin/sed$")?;
    let idx_sed: Vec<usize> = grep_lines(&re_sed, &lines);
    if idx_sed.len() == 0 {
        debug!("SED is not set in Makeconf, bailing.");
        return Ok(());
    }

    bail!(
        "This version of R was compiled using sed at /usr/bin/sed\n        \
           but it is missing on your system.\n        \
           Run `ln -s /bin/sed /usr/bin/sed` as the root user to fix this,\n        \
           and then run rig again."
    );
}

pub fn sc_system_detect_platform(
    args: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let linux = detect_linux()?;

    if args.get_flag("json") || mainargs.get_flag("json") {
        println!("{}", serde_json::to_string_pretty(&linux)?);
    } else {
        println!("Detected platform:");
        println!("Architecture: {}", linux.arch);
        println!("OS:           {}", linux.os);
        println!("Distribution: {}", linux.distro);
        println!("Version:      {}", linux.version);
    }
    Ok(())
}

pub fn set_cert_envvar() {
    match std::env::var("SSL_CERT_FILE") {
        Ok(_) => {
            debug!("SSL_CERT_FILE is already set, keeping it.");
            return;
        }
        Err(_) => {
            let scertpath = "/usr/local/share/rig/cacert.pem";
            let certpath = std::path::Path::new(scertpath);
            if certpath.exists() {
                debug!("Using embedded SSL certificates via SSL_CERT_FILE");
                std::env::set_var("SSL_CERT_FILE", scertpath);
            } else {
                debug!(
                    "{} does not exist, using system SSL certificates",
                    scertpath
                );
            }
        }
    };
}
