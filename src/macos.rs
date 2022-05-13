#![cfg(target_os = "macos")]

use std::error::Error;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::Command;

use clap::ArgMatches;
use nix::unistd::Gid;
use nix::unistd::Uid;
use regex::Regex;
use semver::Version;

use crate::common::*;
use crate::download::*;
use crate::resolve::resolve_versions;
use crate::rversion::*;
use crate::utils::*;
use crate::escalate::*;

const R_ROOT: &str = "/Library/Frameworks/R.framework/Versions";
const R_CUR: &str = "/Library/Frameworks/R.framework/Versions/Current";

pub fn sc_add(args: &ArgMatches) {
    escalate("adding new R versions");
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
    if target.exists() && not_too_old(&target) {
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

    // This should not happen currently on macOS, a .pkg installer
    // sets the default, but prepare for the future
    set_default_if_none(dirname.to_string());

    sc_system_forget();
    system_no_openmp(Some(vec![dirname.to_string()]));
    system_fix_permissions(None);
    system_make_orthogonal(Some(vec![dirname.to_string()]));
    system_create_lib(Some(vec![dirname.to_string()]));
    sc_system_make_links();

    if !args.is_present("without-cran-mirror") {
        set_cloud_mirror(Some(vec![dirname.to_string()]));
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

pub fn sc_rm(args: &ArgMatches) {
    escalate("removing R versions");
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

pub fn system_add_pak(vers: Option<Vec<String>>, stream: &str, update: bool) {
    let vers = match vers {
        Some(x) => x,
        None => vec![sc_get_default_or_fail()]
    };

    let base = Path::new("/Library/Frameworks/R.framework/Versions");
    let re = Regex::new("[{][}]").unwrap();

    for ver in vers {
        check_installed(&ver);
        if update {
            println!("Installing pak for R {}", ver);
        } else {
            println!("Installing pak for R {} (if not installed yet)", ver);
        }
        check_has_pak(&ver);
        let r = base.join(&ver).join("Resources/R");
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
    escalate("making R-* quick links");
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

pub fn sc_system_allow_core_dumps(args: &ArgMatches) {
    escalate("updating code signature of R and /cores permissions");
    sc_system_allow_debugger(args);
    println!("Updating permissions of /cores");
    Command::new("chmod")
        .args(["1777", "/cores"])
        .output()
        .expect("Failed to update /cores permissions");
}

pub fn sc_system_allow_debugger(args: &ArgMatches) {
    escalate("updating code signature of R");
    let all = args.is_present("all");
    let vers = args.values_of("version");

    let vers: Vec<String> = if all {
        sc_get_list()
    } else if vers.is_none() {
        vec![sc_get_default_or_fail()]
    } else {
        vers.unwrap().map(|v| v.to_string()).collect()
    };

    let tmp_dir = std::env::temp_dir().join("rim");
    match std::fs::create_dir_all(&tmp_dir) {
        Err(err) => {
            let dir = tmp_dir.to_str().unwrap_or_else(|| "???");
            panic!("Cannot craete temporary file in {}: {}", dir, err.to_string());
        }
        _ => {}
    };

    for ver in vers {
        check_installed(&ver);
        let path = Path::new(R_ROOT)
            .join(ver.as_str())
            .join("Resources/bin/exec/R");
        let path = path.to_str().unwrap();
        println!("Updating entitlements of {}", path);

        let out = Command::new("codesign")
            .args(["-d", "--entitlements", ":-", path])
            .output()
            .expect("Failed to query entitlements");
        if ! out.status.success() {
            let stderr = match std::str::from_utf8(&out.stderr) {
                Ok(v) => v,
                Err(e) => panic!("Invalid UTF-8 output from codesign: {}", e),
            };
            if stderr.contains("is not signed") {
                println!("    not signed");
            } else {
                panic!("Cannot query entitlements:\n   {}", stderr);
            }
            continue;
        }

        let titles = tmp_dir.join("r.entitlements");
        let titles_str = titles.to_str().unwrap();
        std::fs::write(&titles, out.stdout)
            .expect("Unable to write entitlement file");

        let out = Command::new("/usr/libexec/PlistBuddy")
            .args(["-c", "Add :com.apple.security.get-task-allow bool true",
                   titles_str])
            .output()
            .expect("Cannot update entitlements");

        if ! out.status.success() {
            let stderr = match std::str::from_utf8(&out.stderr) {
                Ok(v) => v,
                Err(e) => panic!("Invalid UTF-8 output from codesign: {}", e),
            };
            if stderr.contains("Entry Already Exists") {
                println!("    already allows debugging");
                continue;
            } else if stderr.contains("zero-length data") {
                println!("    not signed");
                continue;
            } else {
                panic!("Cannot update entitlements: {}", stderr);
            }
        }

        let out = Command::new("codesign")
            .args(["-s", "-", "-f", "--entitlements", titles_str, path])
            .output()
            .expect("Cannot update entitlements");

        if ! out.status.success() {
            panic!("Cannot update entitlements");
        } else {
            println!("    updated entitlements");
        }
    }
}

pub fn sc_system_make_orthogonal(args: &ArgMatches) {
    escalate("updating the R installations");
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
    escalate("changing system library permissions");
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
        let status = Command::new("chmod")
            .args(["-R", "g-w", path])
            .spawn()
            .expect("Failed to update permissions")
            .wait()
            .expect("Failed to update permissions");

        if !status.success() {
            println!("Failed to update permissions :(");
        }
    }

    let current = Path::new(R_ROOT).join("Current");
    let current = current.to_str().unwrap();
    println!("Fixing permissions and group of {}", current);
    let status = Command::new("chmod")
        .args(["-R", "775", &current])
        .spawn()
        .expect("Failed to update permissions")
        .wait()
        .expect("Failed to update permissions");

    if !status.success() {
        println!("Failed to update permissions :(");
    }

    let status = Command::new("chgrp")
        .args(["admin", &current])
        .spawn()
        .expect("Failed to update group")
        .wait()
        .expect("Failed to update group");

    if !status.success() {
        println!("Failed to update group :(");
    }
}

pub fn sc_system_forget() {
    escalate("forgetting R versions");
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
        let version = resolve_versions(eps, "macos".to_string(), arch, None);
        version[0].to_owned()
    }
}

pub fn sc_system_no_openmp(args: &ArgMatches) {
    escalate("updating R compiler configuration");
    let vers = args.values_of("version");
    if vers.is_none() {
        system_no_openmp(None);
        return;
    } else {
        let vers: Vec<String> = vers.unwrap().map(|v| v.to_string()).collect();
        system_no_openmp(Some(vers));
    }
}

fn system_no_openmp(vers: Option<Vec<String>>) {
    let vers = match vers {
        Some(x) => x,
        None => sc_get_list(),
    };
    let re = Regex::new("[-]fopenmp").unwrap();

    for ver in vers {
        check_installed(&ver);
        let path = Path::new(R_ROOT).join(ver.as_str());
        let makevars = path.join("Resources/etc/Makeconf".to_string());
        if ! makevars.exists() { continue; }

        match replace_in_file(&makevars, &re, "") {
            Ok(_) => { },
            Err(err) => {
                let spath = path.to_str().unwrap();
                panic!("Failed to update {}: {}", spath, err);
            }
        };
    }
}

fn set_cloud_mirror(vers: Option<Vec<String>>) {
    let vers = match vers {
        Some(x) => x,
        None => sc_get_list(),
    };

    for ver in vers {
        check_installed(&ver);
        let path = Path::new(R_ROOT).join(ver.as_str());
        let profile = path.join("Resources/library/base/R/Rprofile".to_string());
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

pub fn sc_clean_registry() {
    // Nothing to do on macOS
}

pub fn sc_rstudio(args: &ArgMatches) {
    let mut ver = args.value_of("version");
    let mut prj = args.value_of("project-file");

    // If the first argument is an R project file, and the second is not,
    // then we switch the two
    if ver.is_some() && ver.unwrap().ends_with(".Rproj") {
        ver = args.value_of("project-file");
        prj = args.value_of("version");
    }

    let mut args = match prj {
        None => vec!["-n", "-a", "RStudio"],
        Some(p) => vec!["-n", p]
    };
    let path;

    if !ver.is_none() {
        let ver = ver.unwrap().to_string();
        check_installed(&ver);
        path = "RSTUDIO_WHICH_R=".to_string() + R_ROOT +
            "/" + &ver + "/Resources/R";
        let mut args2 = vec!["--env", &path];
        args.append(&mut args2);
    }

    println!("Running open {}", args.join(" "));

    let status = Command::new("open")
        .args(args)
        .spawn()
        .expect("Failed to start Rstudio")
        .wait()
        .expect("Failed to start RStudio");


    if !status.success() {
        panic!("`open` exited with status {}", status.to_string());
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

pub fn sc_get_default_() -> Result<Option<String>,Box<dyn Error>> {
    read_version_link(R_CUR)
}

pub fn sc_get_list_() -> Result<Vec<String>, Box<dyn Error>> {
    let mut vers = Vec::new();
    if ! Path::new(R_ROOT).exists() {
        return Ok(vers);
    }

    let paths = std::fs::read_dir(R_ROOT)?;

    for de in paths {
        let path = de.unwrap().path();
        let fname = path.file_name().unwrap();
        if fname != "Current" && fname != ".DS_Store" {
            vers.push(fname.to_str().unwrap().to_string());
        }
    }
    vers.sort();
    Ok(vers)
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
