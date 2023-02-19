
use std::collections::HashMap;
use std::error::Error;
#[cfg(target_os = "macos")]
use std::path::Path;

use clap::ArgMatches;
use lazy_static::lazy_static;
#[cfg(target_os = "macos")]
use simple_error::*;
#[cfg(target_os = "macos")]
use simplelog::*;
use tabular::*;

#[cfg(target_os = "macos")]
use crate::download::*;
#[cfg(target_os = "macos")]
use crate::escalate::*;
#[cfg(target_os = "macos")]
use crate::run::*;
#[cfg(target_os = "macos")]
use crate::utils::*;

#[cfg(target_os = "macos")]
use crate::macos::*;

#[derive(PartialEq, Clone, Debug)]
pub struct SysReq {
    pub name: String,
    pub description: String
}

lazy_static! {
    static ref SYSREQS: Vec<&'static str> = vec![
        "checkbashisms",
        "gfortran",
        "pkgconfig",
        "tidy-html5",
    ];

    static ref SYSREQS_INFO: HashMap<String, SysReq> = {
        let mut info = std::collections::HashMap::new();
        info.insert(
            "checkbashisms".to_string(),
            SysReq {
                name: "checkbashisms".to_string(),
                description: r#"
    Checks for bashisms in shell scripts. Via HomeBrew.
"#.to_string()
            }
        );
        info.insert(
            "gfortran".to_string(),
            SysReq {
                name: "gfortran".to_string(),
                description: r#"
    GNU fortran compiler.

    On x86_64 R it is version 8.2 from
    https://github.com/fxcoudert/gfortran-for-macOS
    This is compatible with CRAN's R 3.4 and above, even though some of these
    were built with gfortran 6.1.0.

    On arm64 R, this is version 12.0.1 from
    https://github.com/R-macos/gcc-darwin-arm64

    See https://mac.r-project.org/tools/ for the latest information about
    gfortran on macOS.
"#.to_string()
            }
        );
        info.insert(
            "pkgconfig".to_string(),
            SysReq {
                name: "pkgconfig".to_string(),
                description: r#"
    pkg-config: Manage compile and link flags for libraries. Via Homebrew.
"#.to_string()
            }
        );
        info.insert(
            "tidy-html5".to_string(),
            SysReq {
                name: "tidy-html5".to_string(),
                description: r#"
    Granddaddy of HTML tools, with support for modern standards. Via Homebrew
"#.to_string()
            }
        );
        info
    };
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
pub fn sc_sysreqs(_args: &ArgMatches, _mainargs: &ArgMatches)
              -> Result<(), Box<dyn Error>> {
    // Cannot be called
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn sc_sysreqs(args: &ArgMatches, mainargs: &ArgMatches)
              -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        Some(("add", s)) => sc_sysreqs_add(s, args, mainargs),
        Some(("info", s)) => sc_sysreqs_info(s, args, mainargs),
        Some(("list", s)) => sc_sysreqs_list(s, args, mainargs),
        _ => Ok(()), // unreachable
    }
}

#[cfg(target_os = "macos")]
pub fn sc_sysreqs_info(
    args: &ArgMatches,
    libargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {

    let name = args.get_one::<String>("name").unwrap();
    let info = match SYSREQS_INFO.get(name) {
        None => bail!("Unknown sysreqs: {}", name),
        Some(x) => x
    };

    if args.get_flag("json") || libargs.get_flag("json") || mainargs.get_flag("json") {
        println!("{{");
        println!("  \"name\": \"{}\",", info.name);
        println!("  \"description\": \"{}\"", escape_json(&info.description));
        println!("}}");

    } else {
        let mut tab = Table::new("{:<} {:<}");
        tab.add_row(row!(&info.name, &info.description));
        print!("{}", tab);
    }

    Ok(())
}

#[cfg(target_os = "macos")]
pub fn sc_sysreqs_list(
    args: &ArgMatches,
    libargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {

    if args.get_flag("json") || libargs.get_flag("json") || mainargs.get_flag("json") {
        let num = SYSREQS.len();
        println!("[");
        for (idx, sr) in SYSREQS.iter().enumerate() {
            println!("  {{");
            println!("    \"name\": \"{}\"", sr);
            println!("  }}{}", if idx == num - 1 { "" } else { "," });
        }
        println!("]");
    } else {
        for sr in SYSREQS.iter() {
            println!("{}", sr);
        }
    }

    Ok(())
}

#[cfg(target_os = "macos")]
pub fn sc_sysreqs_add(
    args: &ArgMatches,
    _libargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {

    let mut gfortran = false;
    let mut srs: Vec<String> = vec![];
    match args.get_many::<String>("name") {
        None => {
            debug!("No system package to install");
            return Ok(());
        },
        Some(x) => {
            for sr in x {
                if sr == "gfortran" {
                    gfortran = true;
                    continue;
                }
                if !SYSREQS.contains(&&sr[..]) {
                    bail!("Unknown system package: {}", sr);
                }
                srs.push(sr.to_string());
            }
        }
    };

    let arch = args.get_one::<String>("arch").unwrap();

    // Need to do this up front, so we sudo and won't call brew twice
    if gfortran {
        escalate("installing gfortran")?;
        macos_install_gfortran(&arch)?;
    }

    if srs.len() > 0 {
        brew_install(&arch, srs)?;
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn find_brew(arch: &str) -> Result<(String, Vec<String>), Box<dyn Error>> {
    let brew = if arch == "arm64" {
        "/opt/homebrew/bin/brew"
    } else {
        "/usr/local/bin/brew"
    };

    if !Path::new(brew).exists() {
        bail!("Cannot find {} brew at {}", arch, brew);
    }

    debug!("Found {} brew at {}", arch, brew);

    if is_arm64_machine() {
        Ok((
            "arch".to_string(),
            vec![("-".to_string() + arch).into(), brew.into()]
        ))
    } else {
        Ok((brew.to_string(), vec![]))
    }
}

#[cfg(target_os = "macos")]
fn brew_install(arch: &str, pkgs: Vec<String>)
                -> Result<(), Box<dyn Error>> {

    let brew = find_brew(&arch)?;
    let mut args: Vec<String> = brew.1;
    args.push("install".into());
    for p in pkgs {
        args.push(p.into());
    }

    run_as_user(brew.0, args, "brew")?;

    Ok(())
}

#[cfg(target_os = "macos")]
fn macos_install_gfortran(arch: &str) -> Result<(), Box<dyn Error>> {
    if arch == "arm64" {
        macos_install_gfortran_arm64()
    } else {
        macos_install_gfortran_intel()
    }
}

#[cfg(target_os = "macos")]
fn macos_install_gfortran_arm64() -> Result<(), Box<dyn Error>> {
    let url = "https://github.com/R-macos/gcc-darwin-arm64/releases/download/R-4.2.0-release/gfortran-12.0.1-20220312-is-darwin20-arm64.tar.xz";
    let filename = basename(url).unwrap_or("gfortran-arm64");
    let target = download_file_sync(url, filename, true)?;

    let old = Path::new("/opt/R/arm64/gfortran");
    if old.exists() {
        info!("Removing current gfortran installation from {}", old.display());
        match std::fs::remove_dir_all(&old) {
            Ok(_) => {},
            Err(err) => bail!("Failed to remove {}: {}", old.display(), err.to_string())
        };
    }

    info!("Unpacking gfortran");
    run("tar".into(), vec![os("fxz"), target, os("-C"), os("/")], "tar")?;

    info!("Updating gfortran link to your Apple SDK");
    run(
        "/opt/R/arm64/gfortran/bin/gfortran-update-sdk".into(),
        vec![],
        "gfortran-update-sdk"
    )?;

    Ok(())
}

#[cfg(target_os = "macos")]
fn macos_install_gfortran_intel() -> Result<(), Box<dyn Error>> {
    let url = "https://github.com/fxcoudert/gfortran-for-macOS/releases/download/8.2/gfortran-8.2-Mojave.dmg";
    let filename = basename(url).unwrap_or("gfortran-arm64");
    let target = download_file_sync(url, filename, true)?;

    // umount currently mounted gfortran images first, ignore errors
    info!("Trying to unmount leftover gfortran disk images");
    match run("umount".into(), vec!["/Volumes/gfortran-8.2-Mojave".into()], "umount") {
        _ => {}
    };

    run("hdiutil".into(), vec!["attach".into(), target], "hdiutil")?;

    run("installer".into(),
        vec![
            "-allowUntrusted".into(),
            "-package".into(),
            "/Volumes/gfortran-8.2-Mojave/gfortran-8.2-Mojave/gfortran.pkg".into(),
            "-target".into(),
            "/".into()
        ],
        "installer"
    )?;

    // Ignore failure of unmount, it is not a tragedy...
    match run("umount".into(), vec!["/Volumes/gfortran-8.2-Mojave".into()], "umount") {
        Err(x) => warn!("Failed to unmount gfortran installer: {}", x.to_string()),
        _ => {}
    };

    Ok(())
}
