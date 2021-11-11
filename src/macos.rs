#![cfg(target_os = "macos")]

use std::io::ErrorKind;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::Command;

use clap::ArgMatches;
use nix::unistd::Gid;
use nix::unistd::Uid;
use regex::Regex;

use crate::download::download_file;
use crate::resolve::resolve_versions;
use crate::rversion::Rversion;
use crate::utils::*;

struct User {
    user: String,
    uid: u32,
    gid: u32,
}

const R_ROOT: &str = "/Library/Frameworks/R.framework/Versions";
const R_CUR: &str = "/Library/Frameworks/R.framework/Versions/Current";

pub fn sc_add(args: &ArgMatches) {
    let version = get_resolve(args);
    let ver = version.version.to_owned();
    let url: String = match &version.url {
        Some(s) => s.to_string(),
        None => panic!("Cannot find a download url for R version {}", ver),
    };
    let filename = version.arch.to_owned() + "-" + basename(&url).unwrap();
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

    sc_system_forget();

    let status = Command::new("installer")
        .args(["-pkg", &target_str, "-target", "/"])
        .spawn()
        .expect("Failed to run installer")
        .wait()
        .expect("Failed to run installer");

    if !status.success() {
        panic!("installer exited with status {}", status.to_string());
    }

    let dirname = &get_install_dir(&version);

    sc_system_forget();
    system_fix_permissions(Some(vec![dirname.to_string()]));
    sc_system_make_orthogonal();
    system_create_lib(Some(vec![dirname.to_string()]));
    sc_system_make_links();
}

pub fn sc_default(args: &ArgMatches) {
    if args.is_present("version") {
        let ver = args.value_of("version").unwrap().to_string();
        sc_set_default(ver);
    } else {
        sc_show_default();
    }
}

pub fn sc_list() {
    let vers = sc_get_list();
    for ver in vers {
        println!("{}", ver);
    }
}

pub fn sc_rm(args: &ArgMatches) {
    let vers = args.values_of("version");
    if vers.is_none() { return; }
    let vers = vers.unwrap();

    for ver in vers {
        check_installed(&ver.to_string());

        let dir = Path::new("/Library/Frameworks/R.framework/Versions");
        let dir = dir.join(&ver);
        println!("Removing {}", dir.display());
        sc_system_forget();
        match std::fs::remove_dir_all(&dir) {
            Err(err) => panic!("Cannot remove {}: {}", dir.display(), err.to_string()),
            _ => {}
        };
    }
}

pub fn sc_system_add_pak() {
    unimplemented!();
}

pub fn sc_system_create_lib(args: &ArgMatches) {
    let vers = args.values_of("version");
    if vers.is_none() {
        system_create_lib(None);
        return;
    } else {
        let vers: Vec<String> = vers.unwrap().map(|v| v.to_string()).collect();
        system_create_lib(Some(vers));
    }
}

fn system_create_lib(vers: Option<Vec<String>>) {
    let vers = match vers {
        Some(x) => x,
        None => sc_get_list()
    };
    let base = Path::new("/Library/Frameworks/R.framework/Versions");

    let user = get_user();
    for ver in vers {
        check_installed(&ver);
        let r = base.join(&ver).join("Resources/R");
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

        let lib = shellexpand::tilde(&lib.as_str()).to_string();
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

pub fn sc_system_make_links() {
    let vers = sc_get_list();
    let base = Path::new("/Library/Frameworks/R.framework/Versions/");

    // Create new links
    for ver in vers {
        let linkfile = Path::new("/usr/local/bin/").join("R-".to_string() + &ver);
        let target = base.join(&ver).join("Resources/bin/R");
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

    // Remove danglink links
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

pub fn sc_system_make_orthogonal() {
    let vers = sc_get_list();
    let re = Regex::new("R[.]framework/Resources").unwrap();
    let re2 = Regex::new("[-]F/Library/Frameworks/R[.]framework/[.][.]").unwrap();
    for ver in vers {
        println!("Making R {} orthogonal", ver);
        let base = Path::new("/Library/Frameworks/R.framework/Versions/");
        let sub = "R.framework/Versions/".to_string() + &ver + "/Resources";

        let rfile = base.join(&ver).join("Resources/bin/R");
        replace_in_file(&rfile, &re, &sub).ok();

        let efile = base.join(&ver).join("Resources/etc/Renviron");
        replace_in_file(&efile, &re, &sub).ok();

        let ffile = base
            .join(&ver)
            .join("Resources/fontconfig/fonts/fonts.conf");
        replace_in_file(&ffile, &re, &sub).ok();

        let mfile = base.join(&ver).join("Resources/etc/Makeconf");
        let sub = "-F/Library/Frameworks/R.framework/Versions/".to_string() + &ver;
        replace_in_file(&mfile, &re2, &sub).ok();

        let fake = base.join(&ver).join("R.framework");
        let fake = fake.as_path();
        // TODO: only ignore failure if files already exist
        std::fs::create_dir_all(&fake).ok();
        symlink("../Headers", fake.join("Headers")).ok();
        symlink("../Resources/lib", fake.join("Libraries")).ok();
        symlink("../PrivateHeaders", fake.join("PrivateHeaders")).ok();
        symlink("../R", fake.join("R")).ok();
        symlink("../Resources", fake.join("Resources")).ok();
    }
}

pub fn sc_system_fix_permissions(args: &ArgMatches) {
    check_root();
    let vers = args.values_of("version");
    if vers.is_none() {
        system_fix_permissions(None);
        return;
    } else {
        let vers: Vec<String> = vers.unwrap().map(|v| v.to_string()).collect();
        system_fix_permissions(Some(vers));
    }
}

fn system_fix_permissions(vers: Option<Vec<String>>) {
    let vers = match vers {
        Some(x) => x,
        None => sc_get_list()
    };

    for ver in vers {
        check_installed(&ver);
        let path = Path::new(R_ROOT).join(ver.as_str());
        let path = path.to_str().unwrap();
        println!("Fixing permissions in {}", path);
        Command::new("chmod")
            .args(["-R", "g-w", path])
            .output()
            .expect("Failed to update permissions");
    }
}

pub fn sc_system_clean_system_lib() {
    unimplemented!();
}

pub fn sc_system_forget() {
    check_root();
    let out = Command::new("sh")
        .args(["-c", "pkgutil --pkgs | grep -i r-project | grep -v clang"])
        .output()
        .expect("failed to run pkgutil");

    let output = match String::from_utf8(out.stdout) {
        Ok(v) => v,
        Err(_) => panic!("Invalid UTF-8 output from pkgutil"),
    };

    // TODO: this can fail, but if it fails it will still have exit
    // status 0, so we would need to check stderr to see if it failed.
    for line in output.lines() {
        println!("Calling pkgutil --forget {}", line.trim());
        Command::new("pkgutil")
            .args(["--forget", line.trim()])
            .output()
            .expect("pkgutil failed --forget call");
    }
}

pub fn sc_resolve(args: &ArgMatches) {
    let version = get_resolve(args);
    let url: String = match version.url {
        Some(s) => s.to_string(),
        None => "NA".to_string(),
    };
    println!("{} {}", version.version, url);
}

fn get_resolve(args: &ArgMatches) -> Rversion {
    let str = args.value_of("str").unwrap().to_string();
    let arch = match args.value_of("arch") {
        Some(a) => a.to_string(),
        None => "x86_64".to_string(),
    };
    if !valid_macos_archs().contains(&arch) {
        panic!("Unknown macOS arch: {}", arch);
    }
    let eps = vec![str];
    let version = resolve_versions(eps, "macos".to_string(), arch);
    version[0].to_owned()
}

// ------------------------------------------------------------------------

fn valid_macos_archs() -> Vec<String> {
    vec!["x86_64".to_string(), "arm64".to_string()]
}

fn check_installed(ver: &String) -> bool {
    let inst = sc_get_list();
    assert!(
        inst.contains(&ver),
        "Version {} is not installed, see 'rim list'",
        ver
    );
    true
}

fn sc_set_default(ver: String) {
    check_installed(&ver);
    let ret = std::fs::remove_file(R_CUR);
    match ret {
        Err(err) => {
            panic!("Could not remove {}: {}", R_CUR, err)
        }
        Ok(()) => {}
    };

    let path = Path::new(R_ROOT).join(ver.as_str());
    let ret = std::os::unix::fs::symlink(&path, R_CUR);
    match ret {
        Err(err) => {
            panic!("Could not create {}: {}", path.to_str().unwrap(), err)
        }
        Ok(()) => {}
    };
}

fn sc_show_default() {
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

    println!("{}", fname.to_str().unwrap());
}

fn sc_get_list() -> Vec<String> {
    let paths = std::fs::read_dir(R_ROOT);
    assert!(paths.is_ok(), "Cannot list directory {}", R_ROOT);
    let paths = paths.unwrap();

    let mut vers = Vec::new();
    for de in paths {
        let path = de.unwrap().path();
        let fname = path.file_name().unwrap();
        if fname != "Current" {
            vers.push(fname.to_str().unwrap().to_string());
        }
    }
    vers.sort();
    vers
}

fn check_root() {
    let euid = nix::unistd::geteuid();
    if !euid.is_root() {
        panic!("Not enough permissions, you probably need 'sudo'");
    }
}

fn get_user() -> User {
    let uid;
    let gid;
    let user;

    let euid = nix::unistd::geteuid();
    let sudo_uid = std::env::var_os("SUDO_UID");
    let sudo_gid = std::env::var_os("SUDO_GID");
    let sudo_user = std::env::var_os("SUDO_USER");
    if euid.is_root() && sudo_uid.is_some() && sudo_gid.is_some() && sudo_user.is_some() {
        uid = match sudo_uid {
            Some(x) => x.to_str().unwrap().parse::<u32>().unwrap(),
            _ => {
                unreachable!();
            }
        };
        gid = match sudo_gid {
            Some(x) => x.to_str().unwrap().parse::<u32>().unwrap(),
            _ => {
                unreachable!();
            }
        };
        user = match sudo_user {
            Some(x) => x.to_str().unwrap().to_string(),
            _ => {
                unreachable!();
            }
        };
    } else {
        uid = nix::unistd::getuid().as_raw();
        gid = nix::unistd::getgid().as_raw();
        user = match std::env::var_os("USER") {
            Some(x) => x.to_str().unwrap().to_string(),
            None => "Current user".to_string(),
        };
    }
    User { user, uid, gid }
}

fn get_install_dir(ver: &Rversion) -> String {
    let minor = get_minor_version(&ver.version);
    if ver.arch == "x86_64" {
        minor
    } else if ver.arch == "arm64" {
        minor + "-arm64"
    } else {
        panic!("Unknown macOS arch: {}", ver.arch);
    }
}

fn get_minor_version(ver: &str) -> String {
    let re = Regex::new("[.][^.]*$").unwrap();
    re.replace(ver, "").to_string()
}
