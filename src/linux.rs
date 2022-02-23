#![cfg(target_os = "linux")]

use regex::Regex;
use std::io::ErrorKind;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::Command;

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

pub fn sc_add(args: &ArgMatches) {
    escalate();
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
    if target.exists() {
        target_str = target.into_os_string().into_string().unwrap();
        println!("{} is cached at\n    {}", filename, target_str);
    } else {
        target_str = target.into_os_string().into_string().unwrap();
        println!("Downloading {} ->\n    {}", url, target_str);
        let client = &reqwest::Client::new();
        download_file(client, url, &target_str);
    }

    if linux.distro == "ubuntu" || linux.distro == "debian" {
	add_deb(target_str);
    }

    system_create_lib(Some(vec![verstr.to_string()]));
    sc_system_make_links();
}

fn add_deb(path: String) {
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
	.args(["-n", &path])
	.spawn()
	.expect("Failed to run gdebi")
	.wait()
	.expect("Failed to run gdebi");

   if !status.success() {
       panic!("gdebi exited with status {}", status.to_string());
   }
}

pub fn sc_rm(args: &ArgMatches) {
    escalate();
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
        let out = Command::new(r)
            .args(["--vanilla", "-s", "-e", "cat(Sys.getenv('R_LIBS_USER'))"])
            .output()
            .expect("Failed to run R to query R_LIBS_USER");
        let lib = match String::from_utf8(out.stdout) {
            Ok(v) => v,
            Err(err) => panic!(
                "Cannot query R_LIBS_USER for R {}: {}",
                ver,
                err.to_string()
            ),
        };

	let re = Regex::new("^~").unwrap();
	let home = match std::env::var("RIM_HOME") {
	    Ok(x) => { x },
	    Err(_) => { get_home() }
	};
	let lib = re.replace(&lib.as_str(), &home).to_string();
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

pub fn sc_system_add_pak(args: &ArgMatches) {
    let devel = args.is_present("devel");
    let all = args.is_present("all");
    let vers = args.values_of("version");
    if all {
        system_add_pak(Some(sc_get_list()), devel);
    } else if vers.is_none() {
        system_add_pak(None, devel);
        return;
    } else {
        let vers: Vec<String> = vers.unwrap().map(|v| v.to_string()).collect();
        system_add_pak(Some(vers), devel);
    }
}

fn system_add_pak(vers: Option<Vec<String>>, devel: bool) {
    let vers = match vers {
        Some(x) => x,
        None => vec![sc_get_default()],
    };

    let base = Path::new(R_ROOT);
    let re = Regex::new("[{][}]").unwrap();
    let stream = if devel { "devel" } else { "stable" };

    for ver in vers {
        println!("Installing pak for R {}", ver);
        check_installed(&ver);
        let r = base.join(&ver).join("bin/R");
        let r = r.to_str().unwrap();
        let cmd = r#"
             dir.create(Sys.getenv('R_LIBS_USER'), showWarnings = FALSE, recursive = TRUE);
             install.packages('pak', repos = 'https://r-lib.github.io/p/pak/{}/')
        "#;
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
    escalate();
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

pub fn sc_get_list() -> Vec<String> {
    let paths = std::fs::read_dir(R_ROOT);
    assert!(paths.is_ok(), "Cannot list directory {}", R_ROOT);
    let paths = paths.unwrap();

    let mut vers = Vec::new();
    for de in paths {
	let path = de.unwrap().path();
	let fname = path.file_name().unwrap();
	if fname != "current" {
	    vers.push(fname.to_str().unwrap().to_string());
	}
    }
    vers.sort();
    vers
}

pub fn sc_set_default(ver: String) {
    escalate();
    check_installed(&ver);

    // Remove current link
    if Path::new(R_CUR).exists() {
	let ret = std::fs::remove_file(R_CUR);
	match ret {
            Err(err) => {
		panic!("Could not remove {}: {}", R_CUR, err)
            }
            Ok(()) => {}
	};
    }

    // Add current link
    let path = Path::new(R_ROOT).join(ver.as_str());
    let ret = std::os::unix::fs::symlink(&path, R_CUR);
    match ret {
        Err(err) => {
            panic!("Could not create {}: {}", path.to_str().unwrap(), err)
        }
        Ok(()) => {}
    };

    // Remove /usr/local/bin/R link
    let r = Path::new("/usr/local/bin/R");
    if r.exists() {
	let ret = std::fs::remove_file(r);
	match ret {
            Err(err) => {
		panic!("Could not remove {}: {}", r.to_str().unwrap(), err)
            }
            Ok(()) => {}
	};
    }

    // Add /usr/local/bin/R link
    let cr = Path::new("/opt/R/current/bin/R");
    let ret = std::os::unix::fs::symlink(&cr, &r);
    match ret {
        Err(err) => {
            panic!("Could not create {}: {}", r.to_str().unwrap(), err)
        }
        Ok(()) => {}
    };

    // Remove /usr/local/bin/Rscript link
    let rs = Path::new("/usr/local/bin/Rscript");
    if rs.exists() {
	let ret = std::fs::remove_file(rs);
	match ret {
            Err(err) => {
		panic!("Could not remove {}: {}", rs.to_str().unwrap(), err)
            }
            Ok(()) => {}
	};
    }

    // Add /usr/local/bin/Rscript link
    let crs = Path::new("/opt/R/current/bin/Rscript");
    let ret = std::os::unix::fs::symlink(&crs, &rs);
    match ret {
        Err(err) => {
            panic!("Could not create {}: {}", rs.to_str().unwrap(), err)
        }
        Ok(()) => {}
    };
}

pub fn sc_get_default() -> String {
    let tgt = std::fs::read_link(R_CUR);
    let tgtbuf = match tgt {
        Err(err) => match err.kind() {
            ErrorKind::NotFound => {
                panic!("File '{}' does not exist", R_CUR)
            }
            ErrorKind::InvalidInput => {
                panic!("File '{}' is not a symbolic link", R_CUR)
            }
            _ => panic!("Error resolving {}: {}", R_CUR, err),
        },
        Ok(tgt) => tgt,
    };

    // file_name() is only None if tgtbuf ends with "..", the we panic...
    let fname = tgtbuf.file_name().unwrap();

    fname.to_str().unwrap().to_string()
}

pub fn sc_show_default() {
    let default = sc_get_default();
    println!("{}", default);
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
				  url: "".to_string() };

    let supported = list_supported_distros();

    let mut good = false;
    for dis in supported {
	if dis.distro == mine.distro && dis.version == mine.version {
	    mine.url = dis.url;
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
		       url: "https://cdn.rstudio.com/r/ubuntu-1804/pkgs/r-{}_1_amd64.deb".to_string() },
	LinuxVersion { distro: "ubuntu".to_string(),
		       version: "20.04".to_string(),
		       url: "https://cdn.rstudio.com/r/ubuntu-2004/pkgs/r-{}_1_amd64.deb".to_string() },
	LinuxVersion { distro: "ubuntu".to_string(),
		       version: "22.04".to_string(),
		       url: "https://cdn.rstudio.com/r/ubuntu-2204/pkgs/r-{}_1_amd64.deb".to_string() },
	LinuxVersion { distro: "debian".to_string(),
		       version: "9".to_string(),
		       url: "https://cdn.rstudio.com/r/debian-9/pkgs/r-${}_1_amd64.deb".to_string() },
	LinuxVersion { distro: "debian".to_string(),
		       version: "10".to_string(),
		       url: "https://cdn.rstudio.com/r/debian-9/pkgs/r-${}_1_amd64.deb".to_string() },
    ]
}

pub fn sc_clean_registry() {
    // Nothing to do on Linux
}
