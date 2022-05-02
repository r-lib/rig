#![cfg(target_os = "windows")]

use regex::Regex;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader};
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::{thread, time};

use clap::ArgMatches;
use remove_dir_all::remove_dir_all;
use winreg::enums::*;
use winreg::RegKey;

use crate::common::*;
use crate::download::*;
use crate::resolve::resolve_versions;
use crate::rversion::Rversion;

const R_ROOT: &str = "C:\\Program Files\\R";

#[warn(unused_variables)]
pub fn sc_add(args: &ArgMatches) {
    elevate("adding new R version");
    sc_clean_registry();
    let str = args.value_of("str").unwrap().to_string();
    if str.len() >= 6 && &str[0..6] == "rtools" {
        return add_rtools(str);
    }
    let (_version, target) = download_r(&args);

    println!("Installing {}", target);
    let status = Command::new(&target)
	.args(["/VERYSILENT", "/SUPPRESSMSGBOXES"])
	.spawn()
	.expect("Failed to run installer")
	.wait()
	.expect("Failed to run installer");

    if !status.success() {
	    panic!("installer exited with status {}", status.to_string());
    }

    system_create_lib(None);
    sc_system_make_links();
    patch_for_rtools();
    maybe_update_registry_default();
}

fn add_rtools(version: String) {
    let vers;
    if version == "rtools" {
        vers = get_rtools_needed();
    } else {
        vers = vec![version.replace("rtools", "")];
    }
    let client = &reqwest::Client::new();
    for ver in vers {
	let rtools42 = &ver[0..2] == "42";
        let rtools4 = &ver[0..1] == "4" || ver == "devel";
	let filename: String;
	let url: String;
        if rtools42 {
	    filename = "rtools42.exe".to_string();
	    url = "https://github.com/r-hub/rtools42/releases/download/latest/rtools42.exe".to_string();
	} else if rtools4 {
            filename = format!("rtools{}-x86_64.exe", ver);
	    url = format!("https://cloud.r-project.org/bin/windows/Rtools/{}", filename);
        } else {
            filename = format!("Rtools{}.exe", ver);
	    url = format!("https://cloud.r-project.org/bin/windows/Rtools/{}", filename);
        };
        let tmp_dir = std::env::temp_dir().join("rim");
        let target = tmp_dir.join(&filename);
        let target_str = target.into_os_string().into_string().unwrap();
        println!("Downloading {} ->\n    {}", url, target_str);
        download_file(client, url, &target_str);
        println!("Installing\n    {}", target_str);
        let status = Command::new(&target_str)
            .args(["/VERYSILENT", "/SUPPRESSMSGBOXES"])
            .spawn()
            .expect("Failed to run Rtools installer")
            .wait()
            .expect("Failed to run RTools installer");

        if !status.success() {
            panic!("Rtools installer exited with status {}", status.to_string());
        }
    }
}

fn patch_for_rtools() {
    let vers = sc_get_list();
    let base = Path::new(R_ROOT);

    for ver in vers {
	let rtools42 = &ver[0..1] == "42";
	// rtools42 does not need any updates
	if rtools42 {
	    continue;
	}

        let rtools4 = &ver[0..1] == "4" || ver == "devel";
	let envfile = base
	    .join("R-".to_string() + &ver)
	    .join("etc")
	    .join("Renviron.site");
	let mut ok = envfile.exists();
	if ok {
	    ok = false;
	    let file = File::open(&envfile).unwrap();
	    let reader = BufReader::new(file);
	    for line in reader.lines() {
		let line2 = line.unwrap();
		if line2.len() >= 14 && &line2[0..14] == "# added by rim" {
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
		.open(&envfile)
		.unwrap();

	    let head = "\n".to_string() +
		"# added by rim, do not update by hand-----\n";
	    let tail = "\n".to_string() +
		"# ----------------------------------------\n";
	    let txt3 = head.to_owned() +
		"PATH=\"C:\\Rtools\\bin;${PATH}\"" +
		&tail;
	    let txt4 = head.to_owned() +
		"PATH=\"${RTOOLS40_HOME}\\ucrt64\\bin;${RTOOLS40_HOME}\\usr\\bin;${PATH}\"" +
		&tail;

	    if let Err(e) = writeln!(file, "{}", if rtools4 { txt4 } else { txt3 }) {
		eprintln!("Couldn't write to Renviron.site file: {}", e);
	    }
	}
    }
}

fn get_rtools_needed() -> Vec<String> {
    let vers = sc_get_list();
    let mut res: Vec<String> = vec![];
    let base = Path::new(R_ROOT);

    for ver in vers {
        let r = base.join("R-".to_string() + &ver).join("bin").join("R.exe");
        let r = r.to_str().unwrap();
        let out = Command::new(r)
            .args(["--vanilla", "-s", "-e", "cat(as.character(getRversion()))"])
            .output()
            .expect("Failed to run R to query R version");
        let ver: String = match String::from_utf8(out.stdout) {
            Ok(v) => v,
            Err(err) => panic!(
                "Cannot query R version for R-{}: {}",
                ver,
                err.to_string()
            ),
        };
        let v35 = "35".to_string();
        let v40 = "40".to_string();
        if &ver[0..1] == "3" {
            if ! res.contains(&v35) {
                res.push(v35);
            }
        } else if &ver[0..1] == "4" {
            if ! res.contains(&v40) {
                res.push(v40);
            }
        }
    }
    res
}

pub fn sc_rm(args: &ArgMatches) {
    elevate("removing R versions");
    let vers = args.values_of("version");
    if vers.is_none() {
        return;
    }
    let vers = vers.unwrap();

    for ver in vers {
	let verstr = ver.to_string();
	if verstr.len() >= 6 && &verstr[0..6] == "rtools" {
	    rm_rtools(verstr);
	    continue;
	}
        check_installed(&verstr);

        let ver = "R-".to_string() + ver;
        let dir = Path::new(R_ROOT);
        let dir = dir.join(ver);
        println!("Removing {}", dir.display());
        match remove_dir_all(&dir) {
            Err(err) => panic!("Cannot remove {}: {}", dir.display(), err.to_string()),
            _ => {}
        }
    }

    sc_clean_registry();
    sc_system_make_links();
}

fn rm_rtools(ver: String) {
    let dir = Path::new("C:\\").join(ver);
    println!("Removing {}", dir.display());
    match remove_dir_all(&dir) {
        Err(_err) => {
	    let cmd = format!("del -recurse -force {}", dir.display());
	    let out = Command::new("powershell")
		.args(["-command", &cmd])
		.output()
		.expect("Failed to run powershell");
	    let stderr = match std::str::from_utf8(&out.stderr) {
                Ok(v) => v,
		Err(_v) => "cannot parse stderr"
	    };
	    if ! out.status.success() {
		panic!("Cannot remove {}: {}", dir.display(), stderr);
	    }
	},
        _ => {}
    }

    sc_clean_registry();
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
        let r = base
	    .join("R-".to_string() + &ver)
	    .join("bin")
	    .join("R.exe");
        let r = r.to_str().unwrap();
        let cmd = r#"
             dir.create(Sys.getenv('R_LIBS_USER'), showWarnings = FALSE, recursive = TRUE);
             install.packages('pak', repos = sprintf('https://r-lib.github.io/p/pak/{}/%s/%s/%s', .Platform$pkgType, R.Version()$os, R.Version()$arch))
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
    let base = Path::new(R_ROOT);

    for ver in vers {
        check_installed(&ver);
        let r = base.join("R-".to_string() + &ver).join("bin").join("R.exe");
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
                "{}: creating library at {}",
                ver,
                lib.display()
            );
            match std::fs::create_dir_all(&lib) {
                Err(err) => panic!(
                    "Cannot create library at {}: {}",
                    lib.display(),
                    err.to_string()
                ),
                _ => {}
            };

        } else {
            println!("{}: library at {} exists.", ver, lib.display());
        }
    }
}

pub fn sc_system_make_links() {
    elevate("making R-* quick shortcuts");
    let vers = sc_get_list();
    let base = Path::new(R_ROOT);
    let bin = base.join("bin");
    let mut new_links: Vec<String> = vec!["RS.bat".to_string(), "R.bat".to_string()];

    std::fs::create_dir_all(bin).unwrap();

    for ver in vers {
        let filename = "R-".to_string() + &ver + ".bat";
        let linkfile = base.join("bin").join(&filename);
        new_links.push(filename);
        let target = base.join("R-".to_string() + &ver);

        let cnt = "@\"C:\\Program Files\\R\\R-".to_string() +
            &ver + "\\bin\\R\" %*\n";
        let op;
        if linkfile.exists() {
            op = "Updating";
            let orig = std::fs::read_to_string(&linkfile).unwrap();
            if orig == cnt { continue; }
        } else {
            op = "Adding";
        };
        println!("{} R-{} -> {}", op, ver, target.display());
        let mut file = File::create(&linkfile).unwrap();
        file.write_all(cnt.as_bytes()).unwrap();
    }

    // Delete the ones we don't need
    let old_links = std::fs::read_dir(base.join("bin")).unwrap();
    for path in old_links {
        let path = path.unwrap();
        let filename = path.file_name();
        let filename_str = filename.to_str().unwrap().to_string();
        if !filename_str.ends_with(".bat") { continue; }
        if !filename_str.starts_with("R-") { continue; }
        if ! new_links.contains(&filename_str) {
            println!("Deleting unused {}", filename_str);
            std::fs::remove_file(path.path()).unwrap();
        }
    }

}

pub fn sc_system_allow_core_dumps(_args: &ArgMatches) {
    // Nothing to do on Windows
}

pub fn sc_system_allow_debugger(_args: &ArgMatches) {
    // Nothing to do on Windows
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

pub fn get_resolve(args: &ArgMatches) -> Rversion {
    let str = args.value_of("str").unwrap().to_string();

    let eps = vec![str];
    let version = resolve_versions(eps, "win".to_string(), "default".to_string(), None);
    version[0].to_owned()
}

// ------------------------------------------------------------------------

pub fn sc_get_list() -> Vec<String> {
  let mut vers = Vec::new();
  if ! Path::new(R_ROOT).exists() {
      return vers
  }

  let paths = std::fs::read_dir(R_ROOT);
  assert!(paths.is_ok(), "Cannot list directory {}", R_ROOT);
  let paths = paths.unwrap();

  for de in paths {
    let path = de.unwrap().path();
    let fname = path.file_name().unwrap();
    let fname = fname.to_str().unwrap().to_string();
    if &fname[0..2] == "R-" {
        let v = fname[2..].to_string();
        vers.push(v);
    }
  }

  vers.sort();
  vers
}

pub fn sc_set_default(ver: String) {
    check_installed(&ver);
    elevate("setting the default R version");
    let base = Path::new(R_ROOT);
    let linkfile = base.join("bin").join("R.bat");
    let cnt = "::".to_string() + &ver + "\n" +
        "@\"C:\\Program Files\\R\\R-" + &ver + "\\bin\\R\" %*\n";
    let mut file = File::create(linkfile).unwrap();
    file.write_all(cnt.as_bytes()).unwrap();

    let linkfile2 = base.join("bin").join("RS.bat");
    let mut file2 = File::create(linkfile2).unwrap();
    file2.write_all(cnt.as_bytes()).unwrap();

    update_registry_default();
}

fn maybe_get_default() -> Option<String> {
    let base = Path::new(R_ROOT);
    let linkfile = base.join("bin").join("R.bat");
    if !linkfile.exists() {
        None
    } else {
	Some(sc_get_default())
    }
}

pub fn sc_get_default() -> String {
    let base = Path::new(R_ROOT);
    let linkfile = base.join("bin").join("R.bat");
    if !linkfile.exists() {
        panic!("No default version is set currently");
    }
    let file = File::open(linkfile).unwrap();
    let reader = BufReader::new(file);

    let mut first = "".to_string();
    for line in reader.lines() {
        first = line.unwrap().replace("::", "");
        break;
    }

    first.to_string()
}

pub fn sc_show_default() {
    let default = sc_get_default();
    println!("{}", default);
}

fn clean_registry_r(key: &RegKey) {
    for nm in key.enum_keys() {
        let nm = nm.unwrap();
        let subkey = key.open_subkey(&nm).unwrap();
        let path: String = subkey.get_value("InstallPath").unwrap();
        let path2 = Path::new(&path);
        if !path2.exists() {
            println!("Cleaning registry: R {} (not in {})", &nm, path);
            key.delete_subkey_all(nm).unwrap();
        }
    }
}

fn clean_registry_rtools(key: &RegKey) {
    for nm in key.enum_keys() {
        let nm = nm.unwrap();
        let subkey = key.open_subkey(&nm).unwrap();
        let path: String = subkey.get_value("InstallPath").unwrap();
        let path2 = Path::new(&path);
        if !path2.exists() {
            println!("Cleaning registry: Rtools {} (not in {})", &nm, path);
            key.delete_subkey_all(nm).unwrap();
        }
    }
}

fn clean_registry_uninst(key: &RegKey) {
    for nm in key.enum_keys().map(|x| x.unwrap())
        .filter(|x| x.starts_with("Rtools") || x.starts_with("R for Windows")) {
            let subkey = key.open_subkey(&nm).unwrap();
            let path: String = subkey.get_value("InstallLocation").unwrap();
            let path2 = Path::new(&path);
            if !path2.exists() {
                println!("Cleaning registry (uninstaller): {}", nm);
                key.delete_subkey_all(nm).unwrap();
            }
    }
}

pub fn sc_clean_registry() {
    elevate("cleaning up the Windows registry");
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);

    let r64r = hklm.open_subkey("SOFTWARE\\R-core\\R");
    if let Ok(x) = r64r { clean_registry_r(&x); };
    let r64r64 = hklm.open_subkey("SOFTWARE\\R-core\\R64");
    if let Ok(x) = r64r64 { clean_registry_r(&x); };
    let r32r = hklm.open_subkey("SOFTWARE\\WOW6432Node\\R-core\\R");
    if let Ok(x) = r32r { clean_registry_r(&x); };
    let r32r32 = hklm.open_subkey("SOFTWARE\\WOW6432Node\\R-core\\R32");
    if let Ok(x) = r32r32 { clean_registry_r(&x); };

    let rtools64 = hklm.open_subkey("SOFTWARE\\R-core\\Rtools");
    if let Ok(x) = rtools64 {
        clean_registry_rtools(&x);
        if x.enum_keys().count() == 0 {
            hklm.delete_subkey("SOFTWARE\\R-core\\Rtools").unwrap();
        }
    };
    let rtools32 = hklm.open_subkey("SOFTWARE\\WOW6432Node\\R-core\\Rtools");
    if let Ok(x) = rtools32 {
        clean_registry_rtools(&x);
        if x.enum_keys().count() == 0 {
            hklm.delete_subkey("SOFTWARE\\WOW6432Node\\R-core\\Rtools").unwrap();
        }
    };

    let uninst = hklm.open_subkey("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall");
    if let Ok(x) = uninst { clean_registry_uninst(&x); };
    let uninst32 = hklm.open_subkey("SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall");
    if let Ok(x) = uninst32 { clean_registry_uninst(&x); };
}

fn maybe_update_registry_default() {
    let base = Path::new(R_ROOT);
    let linkfile = base.join("bin").join("R.bat");
    if linkfile.exists() {
	update_registry_default();
    }
}

fn update_registry_default1(key: &RegKey, ver: &String) {
    match key.set_value("Current Version", ver) {
	Ok(_) => { },
	Err(err) => {
	    panic!("Cannot set default in registry: {}", err.to_string());
	}
    };
    let inst = R_ROOT.to_string() + "\\R-" + ver;

    match key.set_value("InstallPath", &inst) {
	Ok(_) => { },
	Err(err) => {
	    panic!("Cannot set default in registry: {}", err.to_string());
	}
    }
}

fn update_registry_default_to(default: &String) {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let r64r = hklm.create_subkey("SOFTWARE\\R-core\\R");
    if let Ok(x) = r64r {
	let (key, _) = x;
	update_registry_default1(&key, &default);
    }
    let r64r64 = hklm.create_subkey("SOFTWARE\\R-core\\R64");
    if let Ok(x) = r64r64 {
	let (key, _) = x;
	update_registry_default1(&key, &default);
    }
}

fn update_registry_default() {
    elevate("Update registry default");
    let default = sc_get_default();
    update_registry_default_to(&default);
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

    // we only need to restore if 'ver' is given, there is a default and
    // they are different
    let def = maybe_get_default();
    let restore = !ver.is_none() && !def.is_none() &&
	def.unwrap() != ver.unwrap();

    if !ver.is_none() {
	elevate("updating default version in registry");
    }

    let args = match prj {
	None => vec!["/c", "start", "/b", "rstudio"],
	Some(p) => vec!["/c", "start", "/b", p]
    };

    if !ver.is_none() {
	let ver = ver.unwrap().to_string();
	check_installed(&ver);
	update_registry_default_to(&ver);
    }

    println!("Running cmd.exe {}", args.join(" "));

    let status = Command::new("cmd.exe")
	.args(args)
	.spawn()
	.expect("Failed to start RStudio")
	.wait()
	.expect("Failed to start RStusio");

    // Restore registry (well, set default), if we changed it
    // temporarily
    if restore {
	println!("Waiting for RStudio to start");
	let twosecs = time::Duration::from_secs(2);
	thread::sleep(twosecs);
	println!("Restoring default R version in registry");
	maybe_update_registry_default();
    }

    if !status.success() {
        panic!("`open` exited with status {}", status.to_string());
    }
}

fn elevate(task: &str) {
    if is_elevated::is_elevated() { return; }
    let args: Vec<String> = std::env::args().collect();
    println!("Re-running rim as aministrator for {}.", task);
    let exe = std::env::current_exe().unwrap();
    let exedir =  Path::new(&exe).parent();
    let instdir = match exedir {
        Some(d) => d,
        None    => Path::new("/")
    };
    let gsudo = instdir.join("gsudo.exe");
    let code = std::process::Command::new(gsudo)
        .args(args)
        .status()
        .unwrap();
    std::process::exit(code.code().unwrap());
}
