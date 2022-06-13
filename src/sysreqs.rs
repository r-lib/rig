
use std::error::Error;
use std::ffi::OsString;
use std::path::Path;

use clap::ArgMatches;
use lazy_static::lazy_static;
use simple_error::*;
use simplelog::*;

#[cfg(target_os = "macos")]
use crate::macos::*;

#[cfg(target_os = "windows")]
use crate::windows::*;

#[cfg(target_os = "linux")]
use crate::linux::*;

use crate::run::*;

lazy_static! {
    static ref SYSREQS: Vec<&'static str> = vec![
        "checkbashisms",
        "pkgconfig",
        "tidy-html5",
    ];
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
        Some(("list", s)) => sc_sysreqs_list(s, args, mainargs),
        Some(("add", s)) => sc_sysreqs_add(s, args, mainargs),
        _ => Ok(()), // unreachable
    }
}

#[cfg(target_os = "macos")]
pub fn sc_sysreqs_list(
    args: &ArgMatches,
    libargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {

    if args.is_present("json") || libargs.is_present("json") || mainargs.is_present("json") {
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

    let mut srs: Vec<String> = vec![];
    match args.values_of("name") {
        None => {
            debug!("No system package to install");
            return Ok(());
        },
        Some(x) => {
            for sr in x {
                if !SYSREQS.contains(&sr) {
                    bail!("Unknown system package: {}", sr);
                }
                srs.push(sr.to_string());
            }
        }
    };

    let rver = match sc_get_default()? {
        Some(x) => x,
        None => {
            bail!("Need to set default R version for sysreqs");
        }
    };

    brew_install(&rver, srs)?;

    Ok(())
}

#[cfg(target_os = "macos")]
fn find_brew(rver: &str) -> Result<(String, Vec<OsString>), Box<dyn Error>> {
    let arch = if rver.ends_with("-arm64") { "arm64" } else { "x86_64" };
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
fn brew_install(rver: &str, pkgs: Vec<String>)
                -> Result<(), Box<dyn Error>> {

    let brew = find_brew(&rver)?;
    let mut args: Vec<OsString> = brew.1;
    args.push("install".into());
    for p in pkgs {
        args.push(p.into());
    }

    run(brew.0.into(), args, "brew")?;

    Ok(())
}

#[cfg(target_os = "macos")]
fn is_arm64_machine() -> bool {
    let proc = std::process::Command::new("arch")
        .args(["-arm64", "true"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    if let Ok(mut proc) = proc {
        let out = proc.wait();
        if let Ok(out) = out {
            if out.success() {
                true
            } else {
                false
            }
        } else {
            false
        }
    } else {
        false
    }
}
