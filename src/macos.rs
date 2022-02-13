#![cfg(target_os = "macos")]

use std::io::ErrorKind;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::Command;

use clap::ArgMatches;
use nix::unistd::Gid;
use nix::unistd::Uid;
use regex::Regex;
use semver::Version;
use sudo::escalate_if_needed;

use crate::common::*;
use crate::download::*;
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
    escalate();
    let mut version = get_resolve(args);
    let ver = version.version.to_owned();
    let verstr = match ver {
        Some(ref x) => x,
        None => "???"
    };
    let url: String = match &version.url {
        Some(s) => s.to_string(),
        None => panic!("Cannot find a download url for R version {}", verstr),
    };
    let arch = version.arch.to_owned();
    let prefix = match arch {
        Some(x) => x,
        None => calculate_hash(&url)
    };
    let filename = prefix + "-" + basename(&url).unwrap();
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

    // If installed from URL, then we'll need to extract the version + arch
    match ver {
        Some(_) => { },
        None => {
            let fver = extract_pkg_version(&target_str);
            version.version = fver.version;
            version.arch = fver.arch;
        }
    };

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
    system_make_orthogonal(Some(vec![dirname.to_string()]));
    system_create_lib(Some(vec![dirname.to_string()]));
    sc_system_make_links();
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

        let dir = Path::new(R_ROOT);
        let dir = dir.join(&ver);
        println!("Removing {}", dir.display());
        sc_system_forget();
        match std::fs::remove_dir_all(&dir) {
            Err(err) => panic!("Cannot remove {}: {}", dir.display(), err.to_string()),
            _ => {}
        };
    }

    sc_system_make_links();
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

    let base = Path::new("/Library/Frameworks/R.framework/Versions");
    let re = Regex::new("[{][}]").unwrap();
    let stream = if devel { "devel" } else { "stable" };

    for ver in vers {
        println!("Installing pak for R {}", ver);
        check_installed(&ver);
        check_has_pak(&ver);
        let r = base.join(&ver).join("Resources/R");
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

pub fn system_create_lib(vers: Option<Vec<String>>) {
    let vers = match vers {
        Some(x) => x,
        None => sc_get_list(),
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
    escalate();
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

pub fn sc_system_make_orthogonal(args: &ArgMatches) {
    escalate();
    let vers = args.values_of("version");
    if vers.is_none() {
        system_make_orthogonal(None);
        return;
    } else {
        let vers: Vec<String> = vers.unwrap().map(|v| v.to_string()).collect();
        system_make_orthogonal(Some(vers));
    }
}

fn system_make_orthogonal(vers: Option<Vec<String>>) {
    let vers = match vers {
        Some(x) => x,
        None => sc_get_list(),
    };

    let re = Regex::new("R[.]framework/Resources").unwrap();
    let re2 = Regex::new("[-]F/Library/Frameworks/R[.]framework/[.][.]").unwrap();
    for ver in vers {
        check_installed(&ver);
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
    escalate();
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
        None => sc_get_list(),
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

pub fn sc_system_forget() {
    escalate();
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

pub fn get_resolve(args: &ArgMatches) -> Rversion {
    let str = args.value_of("str").unwrap().to_string();
    let arch = match args.value_of("arch") {
        Some(a) => a.to_string(),
        None => "x86_64".to_string(),
    };
    if !valid_macos_archs().contains(&arch) {
        panic!("Unknown macOS arch: {}", arch);
    }
     if str.len() > 8 && (&str[..7] == "http://" || &str[..8] == "https://") {
        Rversion {
            version: None,
            url: Some(str.to_string()),
            arch: None
        }
    } else {
        let eps = vec![str];
        let version = resolve_versions(eps, "macos".to_string(), arch);
        version[0].to_owned()
    }
}

// ------------------------------------------------------------------------

fn valid_macos_archs() -> Vec<String> {
    vec!["x86_64".to_string(), "arm64".to_string()]
}

fn check_has_pak(ver: &String) -> bool {
    let ver = Regex::new("-.*$").unwrap().replace(ver, "").to_string();
    let ver = ver + ".0";
    let v330 = Version::parse("3.2.0").unwrap();
    let vv = Version::parse(&ver).unwrap();
    assert!(vv > v330, "Pak is only available for R 3.3.0 or later");
    true
}

pub fn sc_set_default(ver: String) {
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

pub fn sc_get_list() -> Vec<String> {
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
    let version = match &ver.version {
        Some(x) => x,
        None => panic!("Cannot calculate install dir for unknown R version")
    };
    let arch = match &ver.arch {
        Some(x) => x,
        None => panic!("Cannot calculate install dir for unknown arch")
    };
    let minor = get_minor_version(&version);
    if arch == "x86_64" {
        minor
    } else if arch == "arm64" {
        minor + "-arm64"
    } else {
        panic!("Unknown macOS arch: {}", arch);
    }
}

fn get_minor_version(ver: &str) -> String {
    let re = Regex::new("[.][^.]*$").unwrap();
    re.replace(ver, "").to_string()
}

fn extract_pkg_version(filename: &str) -> Rversion {
    let out = Command::new("installer")
        .args(["-pkginfo", "-pkg", filename])
        .output()
        .expect("Failed to extract version from .pkg file");
    let std = match String::from_utf8(out.stdout) {
        Ok(v) => v,
        Err(err) => panic!("Cannot extract version from .pkg file: {}", err.to_string())
    };

    let lines = std.lines();
    let re = Regex::new("^R ([0-9]+[.][0-9]+[.][0-9])+.*$").unwrap();
    let lines: Vec<&str> = lines.filter(|l| re.is_match(l)).collect();

    if lines.len() == 0 {
        panic!("Cannot extract version from .pkg file");
    }

    let arm64 = Regex::new("ARM64").unwrap();
    let ver = re.replace(lines[0], "${1}");
    let arch = if arm64.is_match(lines[0]) { "arm64" } else { "x86_64" };

    let res = Rversion {
        version: Some(ver.to_string()),
        url: None,
        arch: Some(arch.to_string())
    };

    res
}

fn escalate() {
    let need_sudo = match sudo::check() {
        sudo::RunningAs::Root => { false },
        sudo::RunningAs::User => { true },
        sudo::RunningAs::Suid => { true }
    };
    if need_sudo {
        println!("Sorry, rim needs your password for this.");
        escalate_if_needed().unwrap();
    }
}
