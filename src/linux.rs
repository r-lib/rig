#![cfg(target_os = "linux")]

use regex::Regex;
use std::error::Error;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::{Command, Stdio};

use clap::ArgMatches;
use nix::unistd::Gid;
use nix::unistd::Uid;

use crate::resolve::resolve_versions;
use crate::rversion::*;

use crate::common::*;
use crate::download::*;
use crate::escalate::*;
use crate::utils::*;

const R_ROOT: &str = "/opt/R";
const R_CUR: &str = "/opt/R/current";

#[cfg(target_arch = "x86_64")]
const UBUNTU_1804_URL: &str = "https://cdn.rstudio.com/r/ubuntu-1804/pkgs/r-{}_1_amd64.deb";
#[cfg(target_arch = "x86_64")]
const UBUNTU_2004_URL: &str = "https://cdn.rstudio.com/r/ubuntu-2004/pkgs/r-{}_1_amd64.deb";
#[cfg(target_arch = "x86_64")]
const UBUNTU_2204_URL: &str = "https://cdn.rstudio.com/r/ubuntu-2204/pkgs/r-{}_1_amd64.deb";
#[cfg(target_arch = "x86_64")]
const DEBIAN_9_URL: &str = "https://cdn.rstudio.com/r/debian-9/pkgs/r-{}_1_amd64.deb";
#[cfg(target_arch = "x86_64")]
const DEBIAN_10_URL: &str = "https://cdn.rstudio.com/r/debian-10/pkgs/r-{}_1_amd64.deb";
#[cfg(target_arch = "x86_64")]
const DEBIAN_11_URL: &str = "https://cdn.rstudio.com/r/debian-11/pkgs/r-{}_1_amd64.deb";

#[cfg(target_arch = "aarch64")]
const UBUNTU_1804_URL: &str = "https://github.com/r-hub/R/releases/download/v{}/R-rstudio-ubuntu-1804-{}_1_arm64.deb";
#[cfg(target_arch = "aarch64")]
const UBUNTU_2004_URL: &str = "https://github.com/r-hub/R/releases/download/v{}/R-rstudio-ubuntu-2004-{}_1_arm64.deb";
#[cfg(target_arch = "aarch64")]
const UBUNTU_2204_URL: &str = "https://github.com/r-hub/R/releases/download/v{}/R-rstudio-ubuntu-2204-{}_1_arm64.deb";
#[cfg(target_arch = "aarch64")]
const DEBIAN_9_URL: &str = "https://github.com/r-hub/R/releases/download/v{}/R-rstudio-debian-9-{}_1_arm64.deb";
#[cfg(target_arch = "aarch64")]
const DEBIAN_10_URL: &str = "https://github.com/r-hub/R/releases/download/v{}/R-rstudio-debian-10-{}_1_arm64.deb";
#[cfg(target_arch = "aarch64")]
const DEBIAN_11_URL: &str = "https://github.com/r-hub/R/releases/download/v{}/R-rstudio-debian-11-{}_1_arm64.deb";

const UBUNTU_1804_RSPM: &str = "https://packagemanager.rstudio.com/all/__linux__/bionic/latest";
const UBUNTU_2004_RSPM: &str = "https://packagemanager.rstudio.com/all/__linux__/focal/latest";
const UBUNTU_2204_RSPM: &str = "https://packagemanager.rstudio.com/all/__linux__/jammy/latest";

pub fn sc_add(args: &ArgMatches) {
    escalate("adding new R versions");
    let linux = detect_linux();
    let version = get_resolve(args);
    let ver = version.version.to_owned();
    let verstr = match ver {
        Some(ref x) => x,
        None => "???"
    };

    let url: String = match &version.url {
        Some(s) => s.to_string(),
        None => panic!("Cannot find a download url for R version {}", verstr),
    };

    let filename = basename(&url).unwrap();
    let tmp_dir = std::env::temp_dir().join("rim");
    let target = tmp_dir.join(&filename);
    let target_str;
    if target.exists() && not_too_old(&target) {
        target_str = target.into_os_string().into_string().unwrap();
        println!("{} is cached at\n    {}", filename, target_str);
    } else {
        target_str = target.into_os_string().into_string().unwrap();
        println!("Downloading {} ->\n    {}", url, target_str);
        let client = &reqwest::Client::new();
        download_file(client, url, &target_str);
    }

    let dirname;
    if linux.distro == "ubuntu" || linux.distro == "debian" {
	add_deb(&target_str);
        dirname = get_install_dir_deb(&target_str);
    } else {
        panic!("Only Ubuntu and Debian Linux are supported currently");
    }

    set_default_if_none(dirname.to_string());

    system_create_lib(Some(vec![dirname.to_string()]));
    sc_system_make_links();

    if !args.is_present("without-cran-mirror") {
        set_cloud_mirror(Some(vec![dirname.to_string()]));
    }

    if !args.is_present("without-rspm") {
        set_rspm(Some(vec![dirname.to_string()]), linux);
    }

    if !args.is_present("without-pak") {
        system_add_pak(
            Some(vec![dirname.to_string()]),
            args.value_of("pak-version").unwrap(),
            // If this is specified then we always re-install
            args.occurrences_of("pak-version") > 0
        );
    }
}

fn get_install_dir_deb(path: &str) -> String {
    let out = Command::new("dpkg")
        .args(["-I", path])
        .output()
        .expect("Failed to run dpkg -I on DEB package");
    let std = match String::from_utf8(out.stdout) {
        Ok(v) => v,
        Err(err) => panic!("Cannot extract version from .deb file: {}", err.to_string())
    };

    let lines = std.lines();
    let re = Regex::new("^[ ]*Package: r-(.*)$").unwrap();
    let lines: Vec<&str> = lines.filter(|l| re.is_match(l)).collect();
    let ver = re.replace(lines[0], "${1}");

    ver.to_string()
}

fn add_deb(path: &str) {
    let status = Command::new("apt-get")
	.args(["update"])
	.spawn()
	.expect("Failed to run apt-get update")
	.wait()
	.expect("Failed to run apt-get update");

   if !status.success() {
       panic!("apt-get install exited with status {}", status.to_string());
   }

    let status = Command::new("apt-get")
	.args(["install", "-y", "gdebi-core"])
	.spawn()
	.expect("Failed to install gdebi-core")
	.wait()
	.expect("Failed to install gdebi-core");

    if !status.success() {
        panic!("apt-get exited with status {}", status.to_string());
    }

    let status = Command::new("gdebi")
	.args(["-n", path])
	.spawn()
	.expect("Failed to run gdebi")
	.wait()
	.expect("Failed to run gdebi");

   if !status.success() {
       panic!("gdebi exited with status {}", status.to_string());
   }
}

pub fn sc_rm(args: &ArgMatches) {
    escalate("removing R versions");
    let vers = args.values_of("version");
    if vers.is_none() {
        return;
    }
    let vers = vers.unwrap();

    for ver in vers {
        check_installed(&ver.to_string());

	let pkgname = "r-".to_string() + ver;
	let out = Command::new("dpkg")
	    .args(["-s",  &pkgname])
	    .output()
	    .expect("Failed to run dpkg -s");

	if out.status.success() {
	    println!("Removing {} package", pkgname);
	    let status = Command::new("apt-get")
		.args(["remove", "-y", "--purge", &pkgname])
		.spawn()
		.expect("Failed to run apt-get remove")
		.wait()
		.expect("Failed to run apt-get remove");

	    if !status.success() {
		panic!("Failed to run apt-get remove");
	    }
	} else {
	    println!("{} package is not installed", pkgname);
	}

        let dir = Path::new(R_ROOT);
        let dir = dir.join(&ver);
	if dir.exists() {
            println!("Removing {}", dir.display());
            match std::fs::remove_dir_all(&dir) {
		Err(err) => panic!("Cannot remove {}: {}", dir.display(), err.to_string()),
		_ => {}
            };
	}
    }

    sc_system_make_links();
}

pub fn system_create_lib(vers: Option<Vec<String>>) {
    let vers = match vers {
        Some(x) => x,
        None => sc_get_list(),
    };
    let base = Path::new(R_ROOT);

    let user = get_user();
    for ver in vers {
        check_installed(&ver);
        let r = base.join(&ver).join("bin/R");
        let r = r.to_str().unwrap();
	let out;
	if user.sudo {
            out = Command::new("su")
		.args([&user.user, "--", r, "--vanilla", "-s", "-e", "cat(Sys.getenv('R_LIBS_USER'))"])
		.output()
		.expect("Failed to run R to query R_LIBS_USER");
	} else {
            out = Command::new(r)
		.args(["--vanilla", "-s", "-e", "cat(Sys.getenv('R_LIBS_USER'))"])
		.output()
		.expect("Failed to run R to query R_LIBS_USER");
	}
        let lib = match String::from_utf8(out.stdout) {
            Ok(v) => v,
            Err(err) => panic!(
                "Cannot query R_LIBS_USER for R {}: {}",
                ver,
                err.to_string()
            ),
        };

	let re = Regex::new("^~").unwrap();
	let lib = re.replace(&lib.as_str(), &user.dir).to_string();
        let lib = Path::new(&lib);
        if !lib.exists() {
            println!(
                "{}: creating library at {} for user {}",
                ver,
                lib.display(),
                user.user
            );
            match std::fs::create_dir_all(&lib) {
                Err(err) => panic!(
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
                Err(err) => panic!("Cannot set owner on {}: {}", lib.display(), err.to_string()),
                _ => {}
            };
        } else {
            println!("{}: library at {} exists.", ver, lib.display());
        }
    }
}

pub fn system_add_pak(vers: Option<Vec<String>>, stream: &str, update: bool) {
    let vers = match vers {
        Some(x) => x,
        None => vec![sc_get_default_or_fail()],
    };

    let base = Path::new(R_ROOT);
    let re = Regex::new("[{][}]").unwrap();

    for ver in vers {
        check_installed(&ver);
        if update {
            println!("Installing pak for R {}", ver);
        } else {
            println!("Installing pak for R {} (if not installed yet)", ver);
        }
        let r = base.join(&ver).join("bin/R");
        let r = r.to_str().unwrap();
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
        let cmd = Regex::new("[\n\r]")
            .unwrap()
            .replace_all(&cmd, "")
            .to_string();
        let status = Command::new(r)
            .args(["--vanilla", "-s", "-e", &cmd])
            .spawn()
            .expect("Failed to run R to install pak")
            .wait()
            .expect("Failed to run R to install pak");

        if !status.success() {
            panic!("Failed to run R {} to install pak", ver);
        }
    }
}

pub fn sc_system_make_links() {
    escalate("making R-* quick links");
    let vers = sc_get_list();
    let base = Path::new(R_ROOT);

    // Create new links
    for ver in vers {
	let linkfile = Path::new("/usr/local/bin").join("R-".to_string() + &ver);
	let target = base.join(&ver).join("bin/R");
	if !linkfile.exists() {
            println!("Adding {} -> {}", linkfile.display(), target.display());
            match symlink(&target, &linkfile) {
                Err(err) => panic!(
                    "Cannot create symlink {}: {}",
                    linkfile.display(),
                    err.to_string()
                ),
                _ => {}
            };
        }
    }

    // Remove dangling links
    let paths = std::fs::read_dir("/usr/local/bin").unwrap();
    let re = Regex::new("^R-[0-9]+[.][0-9]+").unwrap();
    for file in paths {
        let path = file.unwrap().path();
        let pathstr = path.to_str().unwrap();
        let fnamestr = path.file_name().unwrap().to_str().unwrap();
        if re.is_match(&fnamestr) {
            match std::fs::read_link(&path) {
                Err(_) => println!("{} is not a symlink", pathstr),
                Ok(target) => {
                    if !target.exists() {
                        let targetstr = target.to_str().unwrap();
                        println!("Cleaning up {}", targetstr);
                        match std::fs::remove_file(&path) {
                            Err(err) => {
                                println!("Failed to remove {}: {}", pathstr, err.to_string())
                            }
                            _ => {}
                        }
                    }
                }
            };
        }
    }
}

pub fn get_resolve(args: &ArgMatches) -> Rversion {
    let str = args.value_of("str").unwrap().to_string();

    let eps = vec![str];
    let me = detect_linux();
    let version = resolve_versions(eps, "linux".to_string(), "default".to_string(), Some(me));
    version[0].to_owned()
}

pub fn sc_get_list_() -> Result<Vec<String>, Box<dyn Error>> {
    let mut vers = Vec::new();
    if ! Path::new(R_ROOT).exists() {
        return Ok(vers)
    }

    let paths = std::fs::read_dir(R_ROOT)?;

    for de in paths {
	let path = de?.path();
	let fname = path.file_name().unwrap();
	if fname != "current" {
	    vers.push(fname.to_str().unwrap().to_string());
	}
    }
    vers.sort();
    Ok(vers)
}

pub fn sc_set_default_(ver: &str) -> Result<(), Box<dyn Error>> {
    escalate("setting the default R version");
    check_installed(&ver.to_string());

    // Remove current link
    if Path::new(R_CUR).exists() {
        std::fs::remove_file(R_CUR)?;
    }

    // Add current link
    let path = Path::new(R_ROOT).join(ver);
    std::os::unix::fs::symlink(&path, R_CUR)?;

    // Remove /usr/local/bin/R link
    let r = Path::new("/usr/local/bin/R");
    if r.exists() {
	std::fs::remove_file(r)?;
    }

    // Add /usr/local/bin/R link
    let cr = Path::new("/opt/R/current/bin/R");
    std::os::unix::fs::symlink(&cr, &r)?;

    // Remove /usr/local/bin/Rscript link
    let rs = Path::new("/usr/local/bin/Rscript");
    if rs.exists() {
	std::fs::remove_file(rs)?;
    }

    // Add /usr/local/bin/Rscript link
    let crs = Path::new("/opt/R/current/bin/Rscript");
    std::os::unix::fs::symlink(&crs, &rs)?;

    Ok(())
}

pub fn sc_get_default_() -> Result<Option<String>,Box<dyn Error>> {
    read_version_link(R_CUR)
}

fn set_cloud_mirror(vers: Option<Vec<String>>) {
    let vers = match vers {
        Some(x) => x,
        None => sc_get_list(),
    };

    for ver in vers {
        check_installed(&ver);
        let path = Path::new(R_ROOT).join(ver.as_str());
        let profile = path.join("lib/R/library/base/R/Rprofile".to_string());
        if ! profile.exists() { continue; }

        match append_to_file(
            &profile,
            vec!["options(repos = c(CRAN = \"https://cloud.r-project.org\"))".to_string()]
        ) {
            Ok(_) => { },
            Err(err) => {
                let spath = path.to_str().unwrap();
                panic!("Failed to update {}: {}", spath, err);
            }
        };
    }
}

fn set_rspm(vers: Option<Vec<String>>, linux: LinuxVersion) {
    let arch = std::env::consts::ARCH;
    if arch != "x86_64" {
	println!("RSPM does not support this architecture: {}", arch);
	return;
    }

    if !linux.rspm {
	println!(
	    "RSPM (or rim) does not support this distro: {} {}",
	    linux.distro,
	    linux.version
	);
	return;
    }

    let vers = match vers {
        Some(x) => x,
        None => sc_get_list(),
    };

    let rcode = r#"
options(repos = c(RSPM="%url%", getOption("repos")))
options(HTTPUserAgent = sprintf("R/%s R (%s)", getRversion(), paste(getRversion(), R.version$platform, R.version$arch, R.version$os)))
"#;

    let rcode = rcode.to_string().replace("%url%", &linux.rspm_url);

    for ver in vers {
        check_installed(&ver);
        let path = Path::new(R_ROOT).join(ver.as_str());
        let profile = path.join("lib/R/library/base/R/Rprofile".to_string());
        if ! profile.exists() { continue; }

        match append_to_file(&profile, vec![rcode.to_string()]) {
            Ok(_) => { },
            Err(err) => {
                let spath = path.to_str().unwrap();
                panic!("Failed to update {}: {}", spath, err);
            }
        };
    }
}

pub fn sc_system_allow_core_dumps(_args: &ArgMatches) {
    // Nothing to do on Linux
}

pub fn sc_system_allow_debugger(_args: &ArgMatches) {
    // Nothing to do on Linux
}

pub fn sc_system_make_orthogonal(_args: &ArgMatches) {
    // Nothing to do on Windows
}

pub fn sc_system_fix_permissions(_args: &ArgMatches) {
    // Nothing to do on Windows
}

pub fn sc_system_forget() {
    // Nothing to do on Windows
}

pub fn sc_system_no_openmp(_args: &ArgMatches) {
    // Nothing to do on Windows
}

fn detect_linux() -> LinuxVersion {
    let release_file = Path::new("/etc/os-release");
    let lines = match read_lines(release_file) {
        Ok(x) => { x },
        Err(_err) => { panic!("Unknown Linux, no /etc/os-release"); }
    };

    let re_id = Regex::new("^ID=").unwrap();
    let wid_line = grep_lines(&re_id, &lines);
    if wid_line.len() == 0 {
        panic!("Unknown Linux distribution");
    }
    let id_line = &lines[wid_line[0]];
    let id = re_id.replace(&id_line, "").to_string();
    let id = unquote(&id);

    let re_ver = Regex::new("^VERSION_ID=").unwrap();
    let wver_line = grep_lines(&re_ver, &lines);
    if wver_line.len() == 0 {
        panic!("Unknown {} Linux version", id);
    }
    let ver_line = &lines[wver_line[0]];
    let ver = re_ver.replace(&ver_line, "").to_string();
    let ver = unquote(&ver);

    let mut mine = LinuxVersion { distro: id.to_owned(),
				  version: ver.to_owned(),
				  url: "".to_string(),
                                  rspm: false,
				  rspm_url: "".to_string() };

    let supported = list_supported_distros();

    let mut good = false;
    for dis in supported {
	if dis.distro == mine.distro && dis.version == mine.version {
	    mine.url = dis.url;
	    mine.rspm = dis.rspm;
	    mine.rspm_url = dis.rspm_url;
	    good = true;
	}
    }

    if ! good {
	panic!(
	    "Unsupported distro: {} {}, see rim list-supported",
	    &id,
	    &ver
	);
    }

    mine
}

fn list_supported_distros() -> Vec<LinuxVersion> {
    vec![
	LinuxVersion { distro: "ubuntu".to_string(),
		       version: "18.04".to_string(),
		       url: UBUNTU_1804_URL.to_string(),
                       rspm: true,
		       rspm_url: UBUNTU_1804_RSPM.to_string() },
	LinuxVersion { distro: "ubuntu".to_string(),
		       version: "20.04".to_string(),
		       url: UBUNTU_2004_URL.to_string(),
                       rspm: true,
		       rspm_url: UBUNTU_2004_RSPM.to_string() },
	LinuxVersion { distro: "ubuntu".to_string(),
		       version: "22.04".to_string(),
		       url: UBUNTU_2204_URL.to_string(),
                       rspm: true,
		       rspm_url: UBUNTU_2204_RSPM.to_string() },
	LinuxVersion { distro: "debian".to_string(),
		       version: "9".to_string(),
		       url: DEBIAN_9_URL.to_string(),
                       rspm: false,
		       rspm_url: "".to_string() },
	LinuxVersion { distro: "debian".to_string(),
		       version: "10".to_string(),
		       url: DEBIAN_10_URL.to_string(),
                       rspm: false,
		       rspm_url: "".to_string() },
	LinuxVersion { distro: "debian".to_string(),
		       version: "11".to_string(),
		       url: DEBIAN_11_URL.to_string(),
                       rspm: false,
		       rspm_url: "".to_string() },
    ]
}

pub fn sc_clean_registry() {
    // Nothing to do on Linux
}

pub fn sc_rstudio_(version: Option<&str>, project: Option<&str>)
                   -> Result<(), Box<dyn Error>> {
    let cmd;
    let args;
    if project.is_none() {
        cmd = "rstudio";
        args = vec![];
    } else {
        cmd = "xdg-open";
        args = vec![project.unwrap()];
    }

    let mut envname = "dummy";
    let mut path = "".to_string();
    if !version.is_none() {
        let ver = version.unwrap().to_string();
        check_installed(&ver);
        envname = "RSTUDIO_WHICH_R";
        path = R_ROOT.to_string() + "/" + &ver + "/bin/R"
    }

    println!("Running {} {}", cmd, args.join(" "));

    Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env(envname, &path)
        .spawn()?;

    Ok(())
}
