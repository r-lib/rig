
use std::error::Error;
use std::ffi::OsString;
use std::io::BufRead;
use std::io::BufReader;
use regex::Regex;

use simplelog::*;

#[cfg(any(target_os = "macos", target_os = "linux"))]
use crate::rversion::*;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use crate::utils::*;

#[cfg(target_os = "macos")]
use crate::macos::*;

#[cfg(target_os = "windows")]
use crate::windows::*;

#[cfg(target_os = "linux")]
use crate::linux::*;

pub fn run(cmd: OsString, args: Vec<OsString>, _what: &str)
       -> Result<(), Box<dyn Error>> {

    debug!("Running {:?} with args {:?}", cmd, args);
    let reader = duct::cmd(cmd, args)
        .stderr_to_stdout()
        .reader()?;
    let lines = BufReader::new(reader).lines();
    for line in lines {
        info!("<cyan>></> {}", line?);
    }

    Ok(())
}

#[cfg(target_os = "macos")]
pub fn run_as_user(cmd: String, args: Vec<String>, what: &str)
                   -> Result<(), Box<dyn Error>> {
    let user = get_user()?;

    if user.sudo {
        debug!("sudo detected, su needed for running {}", cmd);
        let cmdline = cmd.to_owned() + " " + &args.join(" ");
        let cmdline = cmdline.replace("\"", "\\\"").replace("$", "\\$");
        let mut args2: Vec<OsString> = vec![user.user.into(), "-c".into()];
        args2.push(cmdline.into());
        run(
            "su".into(),
            args2,
            &cmd
        )?;

    } else {
        debug!("no su needed for running {}", cmd);
        let mut args2: Vec<OsString> = vec![];
        for arg in args.iter() {
            args2.push(arg.into());
        }
        run(cmd.into(), args2, what)?;
    }

    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn r(version: &str, command: &str)
      -> Result<(), Box<dyn Error>> {

    let user = get_user()?;
    let cmdline = Regex::new("[\n\r]")?.replace_all(&command, "").to_string();

    if user.sudo {
	debug!("Sudo detected, using 'su' to act as user {:?}", user.user);
        r_sudo(&version, &cmdline, &user)
    } else {
	debug!("No sudo detected, can call R directly");
        r_nosudo(&version, &cmdline)
    }
}

#[cfg(target_os = "macos")]
fn r_sudo(version: &str, command: &str, user: &User)
          -> Result<(), Box<dyn Error>> {

    let rbin = R_ROOT.to_string() + "/" + &R_BINPATH.replace("{}", version);
    let escaped_command =
        rbin + " --vanilla -s -e \"" +
        &command.replace("\"", "\\\"").replace("$", "\\$") +
        "\"";

    let username = user.user.to_string();

    run(
        "su".into(),
        vec![username.into(), "-c".into(), escaped_command.into()],
        &("R ".to_string() + version)
    )
}

#[cfg(target_os = "linux")]
fn r_sudo(version: &str, command: &str, user: &User)
          -> Result<(), Box<dyn Error>> {

    let rbin = R_ROOT.to_string() + "/" + &R_BINPATH.replace("{}", version);
    let username = user.user.to_string();

    run(
        "su".into(),
        vec![username.into(), "--".into(), rbin.into(), "-s".into(),
             "-e".into(), command.into()],
        &("R ".to_string() + version)
    )
}

fn r_nosudo(version: &str, command: &str)
            -> Result<(), Box<dyn Error>> {

    let rbin = R_ROOT.to_string() + "/" + &R_BINPATH.replace("{}", version);

    run(
        rbin.into(),
        vec!["--vanilla".into(), "-s".into(), "-e".into(), command.into()],
        &("R ".to_string() + version)
    )
}

#[cfg(target_os = "windows")]
pub fn r(version: &str, command: &str)
      -> Result<(), Box<dyn Error>> {

    let cmdline = Regex::new("[\n\r]")?.replace_all(&command, "").to_string();
    r_nosudo(&version, &cmdline)
}
