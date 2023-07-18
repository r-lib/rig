
use std::error::Error;
use std::process::Command;

use clap::ArgMatches;

use crate::common::*;

#[cfg(target_os = "macos")]
use crate::macos::*;

#[cfg(target_os = "windows")]
use crate::windows::*;

#[cfg(target_os = "linux")]
use crate::linux::*;

pub fn sc_run(args: &ArgMatches, _mainargs: &ArgMatches)
              -> Result<(), Box<dyn Error>> {
    let rver = args
        .get_one::<String>("r-version");
    let rver = match rver {
        Some(x) => x.to_string(),
        None => sc_get_default_or_fail()?,
    };

    let eval = args
        .get_one::<String>("eval");
    let script = args
        .get_one::<String>("script");

    let cmdargs = args.get_many::<String>("command");
    let cmdargs: Vec<String> = match cmdargs {
        None => vec![],
        Some(x) => x.map(|v| v.to_string()).collect(),
    };

    println!("{:?} {:?} {:?} {:?}", rver, eval, script, cmdargs, );

    // just run R?
    if eval.is_none() && script.is_none() && cmdargs.len() == 0 {
        return sc_run_rver(rver);
    }

    Ok(())
}

fn sc_run_rver(rver: String)
               -> Result<(), Box<dyn Error>> {
    let rver = check_installed(&rver)?;
    let rbin = R_ROOT.to_string() + "/" + &R_BINPATH.replace("{}", &rver);

    let _status = Command::new(rbin)
        .status();

    Ok(())
}
