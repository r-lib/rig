
use std::io::ErrorKind;
use std::path::Path;
use std::process::Command;

use clap::ArgMatches;

use crate::resolve::resolve_versions;
use crate::rversion::Rversion;
use crate::utils::*;
use crate::download::download_file;

#[cfg(target_os = "macos")]
const R_ROOT: &str = "/Library/Frameworks/R.framework/Versions";

#[cfg(target_os = "macos")]
const R_CUR:  &str = "/Library/Frameworks/R.framework/Versions/Current";

#[allow(unused_variables)]
pub fn sc_add(args: &ArgMatches) {
    let version = get_resolve(args);
    let ver = version.version;
    let url: String = match version.url {
        Some(s) => s.to_string(),
        None => panic!("Cannot find a download url for R version {}", ver)
    };
    let filename = basename(&url).unwrap();
    let tmp_dir = std::env::temp_dir().join("rim");
    let target = tmp_dir.join(filename);
    let target_str;
    if target.exists() {
        target_str = target.into_os_string().into_string().unwrap();
        println!("{} is cached at\n    {}", filename, target_str);
    } else {
        target_str = target.into_os_string().into_string().unwrap();
        println!("Downloading {} ->\n    {}", url, target_str);
        let client = reqwest::Client::new();
        let client = &client;
        download_file(client, url, &target_str);
    }

    sc_system_forget();

    let status = Command::new("installer")
        .args(["-pkg", &target_str, "-target", "/"])
        .spawn()
        .expect("Failed to run installer")
        .wait()
        .expect("Failed to run installer");

    if ! status.success() {
        println!("WARNING: installer exited with status {}", status.to_string());
    }

    sc_system_forget();
    sc_system_fix_permissions();

    // TODO: make orthogonal
    // TODO: create quick links
    // TODO: create user libs
}

#[cfg(target_os = "macos")]
pub fn sc_default(args: &ArgMatches) {
    if args.is_present("version") {
        let ver = args.value_of("version").unwrap().to_string();
        sc_set_default(ver);
    } else {
        sc_show_default();
    }
}

#[cfg(target_os = "macos")]
pub fn sc_list() {
    let vers = sc_get_list();
    for ver in vers {
        println!("{}", ver);
    }
}

#[allow(unused_variables)]
pub fn sc_rm(args: &ArgMatches) {
    unimplemented!();
}

#[cfg(target_os = "macos")]
pub fn sc_system_add_pak() {
    unimplemented!();
}

#[cfg(target_os = "macos")]
pub fn sc_system_create_lib() {
    unimplemented!();
}

#[cfg(target_os = "macos")]
pub fn sc_system_make_links() {
    unimplemented!();
}

#[cfg(target_os = "macos")]
pub fn sc_system_make_orthogonal() {
    unimplemented!();
}

#[cfg(target_os = "macos")]
pub fn sc_system_fix_permissions() {
    check_root();
    let vers = sc_get_list();
    for ver in vers {
        let path = Path::new(R_ROOT).join(ver.as_str());
        let path = path.to_str().unwrap();
        println!("Fixing permissions in {}", path);
        Command::new("chmod")
            .args(["-R", "g-w", path])
            .output()
            .expect("Failed to update permissions");
    }
}

#[cfg(target_os = "macos")]
pub fn sc_system_clean_system_lib() {
    unimplemented!();
}

#[cfg(target_os = "macos")]
pub fn sc_system_forget() {
    check_root();
    let out = Command::new("sh")
        .args(["-c", "pkgutil --pkgs | grep -i r-project | grep -v clang"])
        .output()
        .expect("failed to run pkgutil");

    let output = match String::from_utf8(out.stdout) {
        Ok(v) => v,
        Err(_) => panic!("Invalid UTF-8 output from pkgutil")
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

#[cfg(target_os = "macos")]
pub fn sc_resolve(args: &ArgMatches) {
    let version = get_resolve(args);
    let url: String = match version.url {
        Some(s) => s.to_string(),
        None => "NA".to_string()
    };
    println!("{} {}", version.version, url);
}

fn get_resolve(args: &ArgMatches) -> Rversion {
    let str = args.value_of("str").unwrap().to_string();
    let eps = vec![str];
    let version = resolve_versions(eps, "macos".to_string());
    version[0].to_owned()
}

// ------------------------------------------------------------------------

fn check_installed(ver: &String) -> bool {
    let inst = sc_get_list();
    assert!(
        inst.contains(&ver),
        "Version {} is not installed, see 'rim list'",
        ver);
    true
}

#[cfg(target_os = "macos")]
fn sc_set_default(ver: String) {
    check_installed(&ver);
    let ret = std::fs::remove_file(R_CUR);
    match ret {
        Err(err) => {
            panic!("Could not remove {}: {}", R_CUR, err)
        },
        Ok(()) => { }
    };

    let path = Path::new(R_ROOT).join(ver.as_str());
    let ret = std::os::unix::fs::symlink(&path, R_CUR);
    match ret {
        Err(err) => {
            panic!("Could not create {}: {}", path.to_str().unwrap(), err)
        },
        Ok(()) => { }
    };
}

#[cfg(target_os = "macos")]
fn sc_show_default() {
    let tgt = std::fs::read_link(R_CUR);
    let tgtbuf = match tgt {
        Err(err) => {
            match err.kind() {
                ErrorKind::NotFound => {
                    panic!("File '{}' does not exist", R_CUR)
                },
                ErrorKind::InvalidInput => {
                    panic!("File '{}' is not a symbolic link", R_CUR)
                },
                _ => panic!("Error resolving {}: {}", R_CUR, err),
            }
        },
        Ok(tgt) => tgt
    };

    // file_name() is only None if tgtbuf ends with "..", the we panic...
    let fname = tgtbuf.file_name().unwrap();

    println!("{}", fname.to_str().unwrap());
}

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
fn check_root() {
    let euid = nix::unistd::geteuid();
    if ! euid.is_root() {
        panic!("Not enough permissions, you probably need 'sudo'");
    }
}
