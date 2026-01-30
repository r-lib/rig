use std::error::Error;
use std::ffi::OsString;
use std::io::BufRead;
use std::path::PathBuf;
use std::process::Command;

use clap::ArgMatches;
use regex::Regex;
use serde_yaml;
use simple_error::*;
use simplelog::{trace, warn};

use crate::common::*;

#[cfg(target_os = "macos")]
use crate::macos::*;

#[cfg(target_os = "windows")]
use crate::windows::*;

#[cfg(target_os = "linux")]
use crate::linux::*;

pub fn sc_run(args: &ArgMatches, _mainargs: &ArgMatches) -> Result<i32, Box<dyn Error>> {
    let rver = args.get_one::<String>("r-version");
    let rver = match rver {
        Some(x) => check_installed(x)?,
        None => sc_get_default_or_fail()?,
    };
    let rbin = get_r_root().to_string() + "/" + &R_BINPATH.replace("{}", &rver);

    let eval = args.get_one::<String>("eval");
    let script = args.get_one::<String>("script");

    let cmdargs = args.get_many::<String>("command");
    let cmdargs: Vec<String> = match cmdargs {
        None => vec![],
        Some(x) => x.map(|v| v.to_string()).collect(),
    };

    let dry_run = args.get_flag("dry-run");

    let startup = args.get_flag("startup");
    let echo = args.get_flag("echo");
    let mut rargs: Vec<String> = vec![];
    if !startup {
        rargs.push("-q".to_string());
    }
    if !echo {
        rargs.push("--slave".to_string())
    }

    if eval.is_some() {
        return sc_run_eval(rbin, rargs, eval.unwrap().to_string(), cmdargs, dry_run);
    } else if script.is_some() {
        return sc_run_script(rbin, rargs, script.unwrap().to_string(), cmdargs, dry_run);
    } else if cmdargs.len() > 0 {
        let app_type: Option<&String> = args.get_one("app-type");
        if cmdargs[0].contains("::") {
            if app_type.is_some() {
                warn!("'--app-type' argument ignored for package scripts");
            }
            return sc_run_package_script(rbin, rargs, cmdargs, dry_run);
        } else {
            return sc_run_app(rbin, rargs, cmdargs, app_type, dry_run);
        }
    } else {
        // just run R, default args are different in this case
        let mut rargs: Vec<String> = vec![];
        if args.get_flag("no-startup") {
            rargs.push("-q".to_string());
        }
        if args.get_flag("no-echo") {
            rargs.push("--slave".to_string())
        }
        return sc_run_rver(rbin, rargs, cmdargs, dry_run);
    }
}

fn ignore_sigint() {
    // Ignore CTRL+C for Rust, the R process will still get it
    let sigint = ctrlc::set_handler(|| {});
    if let Err(e) = sigint {
        warn!(
            "Could not set up signal handler for SIGINT (CTRL+C): {}",
            e.to_string()
        );
    }
    ()
}

fn sc_run_rver(
    rbin: String,
    args: Vec<String>,
    cmdargs: Vec<String>,
    dry_run: bool,
) -> Result<i32, Box<dyn Error>> {
    let mut args2: Vec<String> = args;
    args2.push("--args".to_string());
    for a in cmdargs {
        args2.push(a.to_string());
    }

    if dry_run {
        println!("\"{}\" {:?}", rbin, args2);
        return Ok(0);
    }

    trace!("Running {} with arguments {:?}", rbin, args2);

    ignore_sigint();
    let _status = Command::new(rbin).args(args2).status()?;
    match _status.code() {
        Some(code) => Ok(code),
        None => Ok(-1),
    }
}

fn sc_run_eval(
    rbin: String,
    args: Vec<String>,
    expr: String,
    cmdargs: Vec<String>,
    dry_run: bool,
) -> Result<i32, Box<dyn Error>> {
    let mut args2: Vec<String> = args;
    args2.push("-e".to_string());
    args2.push(expr);
    args2.push("--args".to_string());
    for a in cmdargs {
        args2.push(a.to_string());
    }

    if dry_run {
        println!("\"{}\" {:?}", rbin, args2);
        return Ok(0);
    }

    ignore_sigint();
    trace!("Running {} with arguments {:?}", rbin, args2);
    let _status = Command::new(rbin).args(args2).status()?;
    match _status.code() {
        Some(code) => Ok(code),
        None => Ok(-1),
    }
}

fn sc_run_script(
    rbin: String,
    args: Vec<String>,
    script: String,
    cmdargs: Vec<String>,
    dry_run: bool,
) -> Result<i32, Box<dyn Error>> {
    let mut args2: Vec<String> = args;
    args2.push("-f".to_string());
    args2.push(script);
    args2.push("--args".to_string());
    for a in cmdargs {
        args2.push(a.to_string());
    }

    if dry_run {
        println!("\"{}\" {:?}", rbin, args2);
        return Ok(0);
    }

    ignore_sigint();
    trace!("Running {} with arguments {:?}", rbin, args2);
    let _status = Command::new(rbin).args(args2).status()?;
    match _status.code() {
        Some(code) => Ok(code),
        None => Ok(-1),
    }
}

fn utf8_file_name(x: std::io::Result<std::fs::DirEntry>) -> String {
    let oss = match x {
        Ok(de) => de.file_name(),
        Err(_) => OsString::from(""),
    };
    match oss.into_string() {
        Ok(s) => s,
        Err(_) => "".to_string(),
    }
}

fn sc_run_app(
    rbin: String,
    args: Vec<String>,
    app: Vec<String>,
    app_type: Option<&String>,
    dry_run: bool,
) -> Result<i32, Box<dyn Error>> {
    let proj = app[0].to_string();
    let projpath = std::path::Path::new(&proj);
    if !projpath.exists() {
        bail!("R project directory at '{}' does not exist", proj);
    }
    let files: Vec<String> = match std::fs::read_dir(&proj) {
        Ok(x) => x.map(|x| utf8_file_name(x)).collect(),
        Err(e) => {
            bail!("Could no access files in R project at '{}': {}", &proj, &e);
        }
    };

    let app_type = match app_type {
        None => detect_app_type(&proj, &files)?,
        Some(t) => t.to_string(),
    };

    let mut primary_doc = "".to_string();
    if app_type == "quarto-shiny"
        || app_type == "quarto-static"
        || app_type == "rmd-shiny"
        || app_type == "rmd-static"
        || app_type == "static"
    {
        primary_doc = detect_primary_doc(&proj, &app_type, &files)?;
    }

    let cmd = match app_type.as_str() {
        "api" => "plumber::pr_run(plumber::pr('plumber.R')) ".to_string(),
        "shiny" => "shiny::runApp(launch.browser = TRUE)".to_string(),
        "quarto-shiny" | "quarto-static" => {
            "quarto::quarto_serve('".to_string() + &primary_doc + "')"
        }
        "rmd-shiny" | "rmd-static" => "rmarkdown::run('".to_string() + &primary_doc + "')",
        "static" => "utils::browseURL('".to_string() + &primary_doc + "')",
        &_ => bail!("Unknown app type: {}", app_type),
    };

    let mut args2 = args;
    args2.push("-e".to_string());
    args2.push(cmd);

    if dry_run {
        println!("{} {:?}", rbin, args2);
        return Ok(0);
    }

    ignore_sigint();
    let _status = Command::new(rbin).args(args2).current_dir(proj).status()?;
    match _status.code() {
        Some(code) => Ok(code),
        None => Ok(-1),
    }
}

fn detect_primary_doc(
    project: &str,
    app_type: &str,
    files: &Vec<String>,
) -> Result<String, Box<dyn Error>> {
    let re_idx = if app_type == "static" {
        Regex::new("^index\\.html?$")?
    } else {
        Regex::new("^index\\.[Rq]md$")?
    };

    let idxs: Vec<&String> = files
        .iter()
        .filter(|x| re_idx.is_match(x))
        .collect::<Vec<_>>();

    if idxs.len() == 0 {
        let re_idx = if app_type == "static" {
            Regex::new("\\.html?$")?
        } else {
            Regex::new("\\.[Rq]md$")?
        };
        let idxs = files
            .iter()
            .filter(|x| re_idx.is_match(x))
            .collect::<Vec<_>>();
        if idxs.len() == 0 {
            bail!(
                "Could not find the primary document in project at {}",
                project
            );
        } else {
            Ok(idxs[0].to_string())
        }
    } else {
        Ok(idxs[0].to_string())
    }
}

// port of https://github.com/rstudio/rsconnect/blob/26ec2c7ca8379cef9d139a85a2cdb62ef6db9ead/R/appMetadata.R#L120
fn detect_app_type(project: &str, files: &Vec<String>) -> Result<String, Box<dyn Error>> {
    // plumber.R or entrypoint.R -> api
    if files.contains(&"plumber.R".to_string()) || files.contains(&"entrypoint.R".to_string()) {
        return Ok("api".to_string());
    }

    // app.R -> shiny
    if files.contains(&"app.R".to_string()) {
        return Ok("shiny".to_string());
    }

    let rmds: Vec<&String> = files
        .iter()
        .filter(|x| x.ends_with(".Rmd"))
        .collect::<Vec<_>>();
    let qmds: Vec<&String> = files
        .iter()
        .filter(|x| x.ends_with(".qmd"))
        .collect::<Vec<_>>();
    let quartoyml =
        files.contains(&"_quarto.yml".to_string()) || files.contains(&"_quarto.yaml".to_string());
    let uses_quarto = qmds.len() > 0 || (quartoyml && rmds.len() > 0);

    let mut has_shiny_rmd: bool = false;
    for rmd in &rmds {
        if is_shiny_rmd(&project, rmd)? {
            has_shiny_rmd = true;
            break;
        }
    }
    let mut has_shiny_qmd = false;
    for qmd in &qmds {
        if is_shiny_rmd(&project, qmd)? {
            has_shiny_qmd = true;
            break;
        }
    }

    if has_shiny_qmd {
        return Ok("quarto-shiny".to_string());
    } else if has_shiny_rmd {
        if uses_quarto {
            return Ok("quarto-shiny".to_string());
        } else {
            return Ok("rmd-shiny".to_string());
        }
    }

    // shiny app with server.R
    if files.contains(&"server.R".to_string()) {
        return Ok("shiny".to_string());
    }

    // Any non-Shiny R Markdown or Quarto documents
    if rmds.len() > 0 || qmds.len() > 0 {
        if uses_quarto {
            return Ok("quarto-static".to_string());
        } else {
            return Ok("rmd-static".to_string());
        }
    }

    Ok("static".to_string())
}

fn is_shiny_rmd(project: &str, file: &str) -> Result<bool, Box<dyn Error>> {
    let file = std::path::Path::new(project).join(file);
    let header = read_yaml_header(&file);
    let yaml = match header {
        Ok(None) => return Ok(false),
        Ok(Some(m)) => m,
        Err(e) => {
            bail!("Cannot read YAML header from {}: {}", file.display(), e);
        }
    };
    let mut runtime: Option<String> = None;
    let mut server: Option<String> = None;

    if yaml.is_mapping() {
        let yaml = yaml.as_mapping().unwrap();
        let rt = yaml.get("runtime");
        if rt.is_some() {
            let rt2 = rt.unwrap();
            if rt2.is_string() {
                runtime = Some(rt2.as_str().unwrap().to_string());
            }
        }
        let sv = yaml.get("server");
        if sv.is_some() {
            let sv2 = sv.unwrap();
            if sv2.is_string() {
                server = Some(sv2.as_str().unwrap().to_string());
            } else if sv2.is_mapping() {
                let sv3 = sv2.get("type");
                if sv3.is_some() {
                    let sv4 = sv3.unwrap();
                    if sv4.is_string() {
                        server = Some(sv4.as_str().unwrap().to_string());
                    }
                }
            }
        }
    }

    is_shiny_preferred(runtime, server)
}

fn is_shiny_preferred(
    runtime: Option<String>,
    server: Option<String>,
) -> Result<bool, Box<dyn Error>> {
    if runtime.is_some() && runtime.unwrap().starts_with("shiny") {
        Ok(true)
    } else if server.is_some() && server.unwrap() == "Shiny" {
        Ok(true)
    } else {
        Ok(false)
    }
}

fn read_yaml_header(file: &PathBuf) -> Result<Option<serde_yaml::Value>, Box<dyn Error>> {
    let s = read_yaml_header_string(file)?;

    match s {
        None => Ok(None),
        Some(s) => Ok(serde_yaml::from_str(&s)?),
    }
}

fn read_yaml_header_string(file: &PathBuf) -> Result<Option<String>, Box<dyn Error>> {
    trace!("Reading YAML header from {}", file.display());
    let file2 = std::fs::File::open(file)?;
    let reader = std::io::BufReader::new(file2);

    let mut header: String = "".to_string();

    let re_empty = Regex::new("^\\s*$")?;
    let re_line = Regex::new("^---\\s*$")?;
    let re_dots = Regex::new("^[.][.][.]\\s*$")?;
    let mut lines = reader.lines();

    // First search for the starting delimiter
    let start_lines;
    loop {
        let line = lines.next();
        if line.is_none() {
            trace!("End of YAML file, no header");
            return Ok(None);
        }
        let line = match line.unwrap() {
            Ok(l) => l,
            Err(e) => {
                bail!(
                    "Failed to read YAML header from file at {}: {}",
                    file.display(),
                    e
                );
            }
        };
        trace!("Got line: {}", &line);

        if re_empty.is_match(&line) {
            continue;
        } else if re_line.is_match(&line) {
            trace!("Starting --- in YAML");
            start_lines = true;
            break;
        } else if re_dots.is_match(&line) {
            trace!("Starting ... in YAML");
            start_lines = false;
            break;
        }
    }

    // Now start putting stuff into 'header' until we see the same delimiter
    loop {
        let line = lines.next();
        if line.is_none() {
            // no closing delimiter, so return an empty string
            trace!("End of YAML file, no header");
            return Ok(None);
        }
        let line = match line.unwrap() {
            Ok(l) => l,
            Err(e) => {
                bail!(
                    "Failed to read YAML header from file at {}: {}",
                    file.display(),
                    e
                );
            }
        };
        if start_lines && re_line.is_match(&line) {
            trace!("End of YAML header");
            break;
        } else if !start_lines && re_dots.is_match(&line) {
            trace!("End of YAML header");
            break;
        } else {
            trace!("YAML header: {}", &line);
            header.push_str(&line);
            header.push_str("\n");
        }
    }

    Ok(Some(header))
}

fn sc_run_package_script(
    rbin: String,
    rargs: Vec<String>,
    cmdargs: Vec<String>,
    dry_run: bool,
) -> Result<i32, Box<dyn Error>> {
    let pkgfun = cmdargs[0].to_string();
    let re_pkg = Regex::new("::.*$")?;
    let re_fun = Regex::new("^.*::")?;
    let pkg = re_pkg.replace(&pkgfun, "").to_string();
    let fun = re_fun.replace(&pkgfun, "").to_string();
    let fun2 = fun.clone() + ".R";

    let stat = Command::new(&rbin)
        .env("R_DEFAULT_PACKAGES", "NULL")
        .args(["--vanilla", "-s", "-e", "writeLines(.libPaths())"])
        .output()?;
    let out = String::from_utf8(stat.stdout)?;
    let libs = out.split("\n").collect::<Vec<&str>>();

    let mut script: Option<std::path::PathBuf> = None;
    for lib in libs {
        let exec = std::path::Path::new(lib).join(&pkg).join("exec");
        let s = exec.join(&fun);
        if s.exists() {
            script = Some(s);
            break;
        }
        let s2 = exec.join(&fun2);
        if s2.exists() {
            script = Some(s2);
            break;
        }
    }

    if script.is_none() {
        bail!("Could not find script '{}' in package '{}'.", fun, pkg);
    }
    let script = script.unwrap();

    let mut allargs: Vec<OsString> = vec![];
    for a in rargs {
        allargs.push(a.into());
    }
    allargs.push("-f".into());
    allargs.push(script.into_os_string());
    allargs.push("--args".into());
    for a in &cmdargs[1..] {
        allargs.push(a.into());
    }

    if dry_run {
        println!("{} {:?}", rbin, allargs);
        return Ok(0);
    }

    ignore_sigint();
    let status = Command::new(&rbin).args(allargs).status()?;

    let code = status.code();
    match code {
        None => std::process::exit(-1),
        Some(code) => std::process::exit(code),
    };
}
