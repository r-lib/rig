#![cfg(target_os = "windows")]

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::process::Command;

use clap::ArgMatches;

use crate::common::*;
use crate::download::*;
use crate::resolve::resolve_versions;
use crate::rversion::Rversion;

const R_ROOT: &str = "C:\\Program Files\\R";

#[warn(unused_variables)]
pub fn sc_add(args: &ArgMatches) {
    let (version, target) = download_r(&args);

    let status = Command::new(&target)
	.args(["/VERYSILENT", "/SUPPRESSMSGBOXES"])
	.spawn()
	.expect("Failed to run installer")
	.wait()
	.expect("Failed to run installer");

    if !status.success() {
	    panic!("installer exited with status {}", status.to_string());
    }

    // system_create_lib(Some(vec![version.version]));
    // sc_system_make_links();
}

pub fn sc_rm(args: &ArgMatches) {
    let vers = args.values_of("version");
    if vers.is_none() {
        return;
    }
    let vers = vers.unwrap();

    for ver in vers {
        check_installed(&ver.to_string());

        let ver = "R-".to_string() + ver;
        let dir = Path::new(R_ROOT);
        let dir = dir.join(ver);
        println!("Removing {}", dir.display());
        // TODO: remove from the registry as well
        match std::fs::remove_dir_all(&dir) {
            Err(err) => panic!("Cannot remove {}: {}", dir.display(), err.to_string()),
            _ => {}
        }
    }

    // sc_system_make_links();
}

pub fn sc_system_add_pak(args: &ArgMatches) {
    unimplemented!();
}

pub fn sc_system_create_lib(args: &ArgMatches) {
    unimplemented!();
}

pub fn sc_system_make_links() {
    let vers = sc_get_list();
    let base = Path::new(R_ROOT);

    for ver in vers {
        let linkfile = base.join("bin").join("R-".to_string() + &ver + ".bat");
        let target = base.join("R-".to_string() + &ver);
        let op = if !linkfile.exists() { "Updating" } else { "Adding" };
        println!("Adding R-{} -> {}", ver, target.display());
        let mut file = File::create(linkfile).unwrap();
        let cnt = "@\"C:\\Program Files\\R\\R-".to_string() +
            &ver + "\\bin\\R\" %*\n";
        file.write_all(cnt.as_bytes()).unwrap();
    }
}

pub fn sc_system_make_orthogonal(_args: &ArgMatches) {
    // Nothing to do on Windows
}

pub fn sc_system_fix_permissions(args: &ArgMatches) {
    // Nothing to do on Windows
}

pub fn sc_system_clean_system_lib() {
    unimplemented!();
}

pub fn sc_system_forget() {
    // Nothing to do on Windows
}

pub fn get_resolve(args: &ArgMatches) -> Rversion {
    let str = args.value_of("str").unwrap().to_string();
    let arch = match args.value_of("arch") {
        Some(a) => a.to_string(),
        None => "default".to_string(),
    };

    if !valid_windows_archs().contains(&arch) {
	panic!("Unknown Windows arch: {}", arch);
    }
    let arch = match arch.as_str() {
	"default" => "msvcrt",
	other => other,
    }.to_string();

    let eps = vec![str];
    let version = resolve_versions(eps, "win".to_string(), arch);
    version[0].to_owned()
}

// ------------------------------------------------------------------------

fn valid_windows_archs() -> Vec<String> {
    vec![
	"msvcrt".to_string(),
	"ucrt".to_string(),
	"default".to_string()
    ]
}

pub fn sc_get_list() -> Vec<String> {
  let paths = std::fs::read_dir(R_ROOT);
  assert!(paths.is_ok(), "Cannot list directory {}", R_ROOT);
  let paths = paths.unwrap();

  let mut vers = Vec::new();
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
    unimplemented!();
}

pub fn sc_get_default() -> String {
    unimplemented!();
}

pub fn sc_show_default() -> String {
    unimplemented!();
}
