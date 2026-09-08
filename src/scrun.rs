use std::error::Error;
use std::ffi::OsString;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::ArgMatches;
use log::{error, info, trace, warn};
use regex::Regex;
use simple_error::*;

use crate::common::*;
use crate::output::OUTPUT;
use crate::proj::{proj_read_manifest_opt, proj_sync, ProjSyncOptions};
use crate::rproj::Bin;
use crate::rvenv::{find_project_root, project_r_wrapper, project_shim_package, rvenv_sync_needed};

#[cfg(target_os = "macos")]
use crate::macos::*;

#[cfg(target_os = "windows")]
use crate::windows::*;

#[cfg(target_os = "linux")]
use crate::linux::*;

pub fn sc_run(args: &ArgMatches, _mainargs: &ArgMatches) -> Result<i32, Box<dyn Error>> {
    let cmdargs = args.get_many::<String>("command");
    let cmdargs: Vec<String> = match cmdargs {
        None => vec![],
        Some(x) => x.map(|v| v.to_string()).collect(),
    };

    let dry_run = args.get_flag("dry-run");

    // Listing the project's scripts needs neither an R version nor an
    // up to date project environment, so it comes before both.
    if args.get_flag("list") {
        return sc_run_list(args);
    }

    let rbin = run_r_binary(args, dry_run)?;

    // R CMD must be before other arguments.
    if args.get_flag("cmd") {
        return sc_run_cmd(rbin, cmdargs, dry_run);
    }

    let eval = args.get_one::<String>("eval");
    let script = args.get_one::<String>("script");

    let startup = args.get_flag("startup");
    let echo = args.get_flag("echo");
    let mut rargs: Vec<String> = vec![];
    if !startup {
        rargs.push("-q".to_string());
    }
    if !echo {
        rargs.push("--slave".to_string())
    }

    if let Some(eval) = eval {
        sc_run_eval(rbin, rargs, eval.to_string(), cmdargs, dry_run)
    } else if let Some(script) = script {
        sc_run_script(rbin, rargs, script.to_string(), cmdargs, dry_run)
    } else if !cmdargs.is_empty() {
        let app_type: Option<&String> = args.get_one("app-type");
        if cmdargs[0].contains("::") {
            if app_type.is_some() {
                OUTPUT.warn("'--app-type' argument ignored for package scripts");
                warn!("'--app-type' argument ignored for package scripts");
            }
            sc_run_package_script(rbin, rargs, cmdargs, dry_run)
        } else if let Some((root, bin)) = project_bin(args, &cmdargs[0])? {
            if app_type.is_some() {
                OUTPUT.warn("'--app-type' argument ignored for project scripts");
                warn!("'--app-type' argument ignored for project scripts");
            }
            sc_run_project_script(rbin, rargs, &root, &bin, cmdargs[1..].to_vec(), dry_run)
        } else {
            // Not a declared script and not an existing path. If the project
            // declares scripts at all, then the user most likely meant one of
            // them, so name them instead of complaining about a directory.
            if is_bin_name(&cmdargs[0]) && !Path::new(&cmdargs[0]).exists() {
                if let Some(msg) = unknown_bin_error(args, &cmdargs[0])? {
                    OUTPUT.error(&msg);
                    error!("{}", msg);
                    bail!("{}", msg);
                }
            }
            sc_run_app(rbin, rargs, cmdargs, app_type, dry_run)
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
        sc_run_rver(rbin, rargs, cmdargs, dry_run)
    }
}

/// The R binary `rig run` runs: the project environment's R wrapper if the
/// current directory is inside a project, and the requested or default R
/// version otherwise.
fn run_r_binary(args: &ArgMatches, dry_run: bool) -> Result<String, Box<dyn Error>> {
    if let Some(rbin) = project_r_binary(args, dry_run)? {
        return Ok(rbin);
    }

    let rver = match args.get_one::<String>("r-version") {
        Some(x) => check_installed(x)?,
        None => sc_get_default_or_fail()?,
    };
    Ok(get_r_binary(&rver)?.to_string_lossy().into_owned())
}

/// `.rvenv/bin/R` of the project at or above the current directory, syncing
/// the environment first if it is missing or out of date, or `None` if there
/// is no project to use.
///
/// This is the `uv run` equivalent: no `PATH` entry, no activation script and
/// no shell state, so it also works from a Makefile or in CI. The wrapper,
/// rather than the real R binary, because the wrapper is what sets
/// `R_LIBS_USER` and the rest of the activation environment.
fn project_r_binary(args: &ArgMatches, dry_run: bool) -> Result<Option<String>, Box<dyn Error>> {
    if args.get_flag("no-project") {
        trace!("--no-project, not looking for a project environment");
        return Ok(None);
    }

    // A project environment is tied to the R version its lock file names, so
    // it cannot also honor an explicitly requested R version. An explicit
    // `--r-version` is the more specific request, so it wins.
    if let Some(rver) = args.get_one::<String>("r-version") {
        trace!(
            "--r-version {} given, not looking for a project environment",
            rver
        );
        return Ok(None);
    }

    let cwd = std::env::current_dir()?;
    let root = match find_project_root(&cwd) {
        None => return Ok(None),
        Some(root) => root,
    };

    // A project that has never been initialized has no environment to use.
    // `rig proj init` writes the committed part of `.rvenv`, the shim package
    // in `.rvenv/sys/lib`, and neither `rig proj sync` nor `rig run` writes
    // it: writing tracked project files is always an explicit request. Fail
    // rather than fall back to the default R, the same way `rig proj sync`
    // does -- inside a project, `rig run` running a non-project R would be
    // the more surprising outcome.
    if !project_shim_package(&root).exists() {
        let msg = format!(
            "No project environment in {}, run `rig proj init` first \
             (or `rig run --no-project`)",
            root.display()
        );
        OUTPUT.error(&msg);
        error!("{}", msg);
        bail!("{}", msg);
    }

    match rvenv_sync_needed(&root)? {
        None => {}
        Some(why) if dry_run => {
            // A dry run prints a command, it does not install anything.
            let msg = format!("Would run `rig proj sync` first, because {}", why);
            OUTPUT.info(&msg);
            info!("{}", msg);
        }
        Some(why) => {
            let msg = format!("Syncing the project in {}, because {}", root.display(), why);
            OUTPUT.info(&msg);
            info!("{}", msg);
            proj_sync(&root, &ProjSyncOptions::default(), args)?;
        }
    }

    let wrapper = project_r_wrapper(&root);
    if !wrapper.exists() && !dry_run {
        let msg = format!(
            "No R wrapper at {} after syncing the project, run `rig proj sync`",
            wrapper.display()
        );
        OUTPUT.error(&msg);
        error!("{}", msg);
        bail!("{}", msg);
    }

    trace!("Using the project environment at {}", wrapper.display());
    Ok(Some(
        wrapper
            .to_str()
            .ok_or("The project path is not valid Unicode")?
            .to_string(),
    ))
}

// Extensions that make an argument a file rather than a name. A `[[bin]]`
// name could technically end in one of these, but `rig run report.R` meaning
// anything other than the file `report.R` would be a bad surprise.
const NOT_BIN_NAME_EXTENSIONS: [&str; 4] = [".R", ".r", ".Rmd", ".qmd"];

/// Whether `arg` can name a `[[bin]]` in `rproj.toml`. Anything that looks
/// like a path or like a package script is not a name, so a declared script
/// can never shadow the `rig run <path-to-app>` and `rig run <pkg>::<script>`
/// forms. What is left -- a bare word -- would otherwise be a directory name,
/// and there a declared script wins, because it is the more explicit request.
fn is_bin_name(arg: &str) -> bool {
    if arg.is_empty() || arg == "." || arg == ".." {
        return false;
    }
    if arg.contains("::") || arg.contains('/') || arg.contains('\\') {
        return false;
    }
    !NOT_BIN_NAME_EXTENSIONS
        .iter()
        .any(|ext| arg.ends_with(ext) || arg.ends_with(&ext.to_lowercase()))
}

/// The scripts declared in the `[[bin]]` tables of the project at `root`.
fn project_bins(root: &Path) -> Result<Vec<Bin>, Box<dyn Error>> {
    match proj_read_manifest_opt(root)? {
        None => Ok(vec![]),
        Some(manifest) => Ok(manifest.bin),
    }
}

/// The project at or above the current directory, if any. Unlike
/// `project_r_binary()` this ignores `--r-version`: which R runs a declared
/// script and whether a name refers to one are separate questions, and
/// `rig run -r 4.4.1 <name>` should still find the script.
fn project_root(args: &ArgMatches) -> Result<Option<PathBuf>, Box<dyn Error>> {
    if args.get_flag("no-project") {
        return Ok(None);
    }
    Ok(find_project_root(&std::env::current_dir()?))
}

/// The project root and the `[[bin]]` called `name`, if `name` can be a
/// script name at all and the project declares one with that name.
fn project_bin(args: &ArgMatches, name: &str) -> Result<Option<(PathBuf, Bin)>, Box<dyn Error>> {
    if !is_bin_name(name) {
        return Ok(None);
    }
    let root = match project_root(args)? {
        None => return Ok(None),
        Some(root) => root,
    };
    for bin in project_bins(&root)? {
        if bin.name == name {
            trace!("'{}' is a script declared in {}", name, root.display());
            return Ok(Some((root, bin)));
        }
    }
    Ok(None)
}

/// The error message for a name that matches neither a declared script nor a
/// path, or `None` if the project declares no scripts and so has nothing
/// better to say than the usual "no such directory".
fn unknown_bin_error(args: &ArgMatches, name: &str) -> Result<Option<String>, Box<dyn Error>> {
    let root = match project_root(args)? {
        None => return Ok(None),
        Some(root) => root,
    };
    let bins = project_bins(&root)?;
    if bins.is_empty() {
        return Ok(None);
    }
    let names: Vec<String> = bins.iter().map(|b| b.name.to_string()).collect();
    Ok(Some(format!(
        "'{}' is not a script of the project in {}, and it is not a path \
         either. Declared scripts are: {}.",
        name,
        root.display(),
        names.join(", ")
    )))
}

/// Runs the script of a `[[bin]]`, with the remaining arguments passed on to
/// it, i.e. `rig run <name> [args...]`.
fn sc_run_project_script(
    rbin: String,
    args: Vec<String>,
    root: &Path,
    bin: &Bin,
    cmdargs: Vec<String>,
    dry_run: bool,
) -> Result<i32, Box<dyn Error>> {
    // `path` is relative to the project, so that a declared script works the
    // same from any directory within it.
    let script = root.join(&bin.path);
    if !script.exists() {
        let msg = format!(
            "The script of '{}' is missing: no file at {} (`path = \"{}\"` in {})",
            bin.name,
            script.display(),
            bin.path,
            root.join(crate::rproj::RPROJ_MANIFEST_FILE).display()
        );
        OUTPUT.error(&msg);
        error!("{}", msg);
        bail!("{}", msg);
    }

    let script = script
        .to_str()
        .ok_or("The script path is not valid Unicode")?
        .to_string();
    sc_run_script(rbin, args, script, cmdargs, dry_run)
}

/// `rig run --list`: the scripts the project declares.
fn sc_run_list(args: &ArgMatches) -> Result<i32, Box<dyn Error>> {
    let json = args.get_flag("json");

    let root = match project_root(args)? {
        None => {
            let msg = "`rig run --list` lists the scripts of a project, but \
                       the current directory is not inside one"
                .to_string();
            OUTPUT.error(&msg);
            error!("{}", msg);
            bail!("{}", msg);
        }
        Some(root) => root,
    };

    let bins = project_bins(&root)?;

    if json {
        let rows: Vec<serde_json::Value> = bins
            .iter()
            .map(|b| {
                serde_json::json!({
                    "name": b.name,
                    "path": b.path,
                    "description": b.description,
                })
            })
            .collect();
        OUTPUT.println(&serde_json::to_string_pretty(&rows)?);
        return Ok(0);
    }

    if bins.is_empty() {
        OUTPUT.info(&format!(
            "The project in {} declares no scripts. Add a `[[bin]]` table to {} \
             to declare one.",
            root.display(),
            crate::rproj::RPROJ_MANIFEST_FILE
        ));
        return Ok(0);
    }

    let mut table = tabular::Table::new("{:<}  {:<}  {:<}");
    table.add_row(
        tabular::Row::new()
            .with_cell("NAME")
            .with_cell("PATH")
            .with_cell("DESCRIPTION"),
    );
    for bin in &bins {
        table.add_row(
            tabular::Row::new()
                .with_cell(&bin.name)
                .with_cell(&bin.path)
                .with_cell(bin.description.as_deref().unwrap_or("")),
        );
    }
    OUTPUT.println(&table.to_string());

    Ok(0)
}

fn ignore_sigint() {
    // Ignore CTRL+C for Rust, the R process will still get it
    let sigint = ctrlc::set_handler(|| {});
    if let Err(e) = sigint {
        OUTPUT.warn(&format!(
            "Could not set up signal handler for SIGINT (CTRL+C): {}",
            e
        ));
        warn!("Could not set up signal handler for SIGINT (CTRL+C): {}", e);
    }
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

// Flags that R only accepts _between_ `R` and `CMD`, where they apply to the
// R processes that the `R CMD` command runs. `--arch` belongs here as well,
// but it takes a value (`--arch=<name>` or `--arch <name>`), so it does not
// fit this simple list and `split_r_cmd_args()` handles it separately.
const R_CMD_R_FLAGS: [&str; 4] = [
    "--no-environ",
    "--no-init-file",
    "--no-site-file",
    "--vanilla",
];

// Splits the arguments of `rig run --cmd` into R options that need to go
// before `CMD`, and the `R CMD` command and its own arguments. R only accepts
// these options before `CMD`, but users should not need to care about where
// exactly they put them, so we pick them out and move them into place.
// No `R CMD` command has an option with these names, so this is unambiguous.
fn split_r_cmd_args(cmdargs: Vec<String>) -> (Vec<String>, Vec<String>) {
    let mut ropts: Vec<String> = vec![];
    let mut rest: Vec<String> = vec![];
    // A `--` separator is not needed, but drop it if the user typed one anyway.
    let mut sep = false;
    let mut cmdargs = cmdargs.into_iter();
    while let Some(arg) = cmdargs.next() {
        if !sep && arg == "--" {
            sep = true;
        } else if R_CMD_R_FLAGS.contains(&arg.as_str()) || arg.starts_with("--arch=") {
            ropts.push(arg);
        } else if arg == "--arch" {
            // `--arch <name>`, the sub-architecture is the next argument
            ropts.push(arg);
            if let Some(arch) = cmdargs.next() {
                ropts.push(arch);
            }
        } else {
            rest.push(arg);
        }
    }

    (ropts, rest)
}

// Runs `<R binary> [R options] CMD <command> [args...]`, i.e. `rig run --cmd <command>
// [args...]`. `cmdargs[0]` is the `R CMD` command (e.g. `check`), the rest are
// its arguments, and they are passed on verbatim.
fn sc_run_cmd(rbin: String, cmdargs: Vec<String>, dry_run: bool) -> Result<i32, Box<dyn Error>> {
    let (ropts, cmdargs) = split_r_cmd_args(cmdargs);

    if cmdargs.is_empty() {
        OUTPUT.error("'--cmd' needs an R CMD command, e.g. `rig run --cmd check .`");
        error!("'--cmd' needs an R CMD command");
        bail!("'--cmd' needs an R CMD command");
    }

    let mut args2: Vec<String> = ropts;
    args2.push("CMD".to_string());
    args2.extend(cmdargs);

    if dry_run {
        println!("\"{}\" {:?}", rbin, args2);
        return Ok(0);
    }

    ignore_sigint();
    trace!("Running {} with arguments {:?}", rbin, args2);
    let status = Command::new(rbin).args(args2).status()?;
    match status.code() {
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
        OUTPUT.error(&format!("R project directory at '{}' does not exist", proj));
        error!("R project directory at '{}' does not exist", proj);
        bail!("R project directory at '{}' does not exist", proj);
    }
    let files: Vec<String> = match std::fs::read_dir(&proj) {
        Ok(x) => x.map(utf8_file_name).collect(),
        Err(e) => {
            OUTPUT.error(&format!(
                "Could no access files in R project at '{}': {}",
                proj, e
            ));
            error!("Could no access files in R project at '{}': {}", proj, e);
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
        &_ => {
            OUTPUT.error(&format!("Unknown app type: {}", app_type));
            error!("Unknown app type: {}", app_type);
            bail!("Unknown app type: {}", app_type)
        }
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
    files: &[String],
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

    if idxs.is_empty() {
        let re_idx = if app_type == "static" {
            Regex::new("\\.html?$")?
        } else {
            Regex::new("\\.[Rq]md$")?
        };
        let idxs = files
            .iter()
            .filter(|x| re_idx.is_match(x))
            .collect::<Vec<_>>();
        if idxs.is_empty() {
            OUTPUT.error(&format!(
                "Could not find a primary document (index.html, index.Rmd, or index.qmd) in project at '{}'.",
                project
            ));
            error!(
                "Could not find a primary document (index.html, index.Rmd, or index.qmd) in project at '{}'.",
                project
            );
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
fn detect_app_type(project: &str, files: &[String]) -> Result<String, Box<dyn Error>> {
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
    let uses_quarto = !qmds.is_empty() || (quartoyml && !rmds.is_empty());

    let mut has_shiny_rmd: bool = false;
    for rmd in &rmds {
        if is_shiny_rmd(project, rmd)? {
            has_shiny_rmd = true;
            break;
        }
    }
    let mut has_shiny_qmd = false;
    for qmd in &qmds {
        if is_shiny_rmd(project, qmd)? {
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
    if !rmds.is_empty() || !qmds.is_empty() {
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
            OUTPUT.error(&format!(
                "Cannot read YAML header from {}: {}",
                file.display(),
                e
            ));
            error!("Cannot read YAML header from {}: {}", file.display(), e);
            bail!("Cannot read YAML header from {}: {}", file.display(), e);
        }
    };
    let mut runtime: Option<String> = None;
    let mut server: Option<String> = None;

    if yaml.is_mapping() {
        let yaml = yaml.as_mapping().unwrap();
        if let Some(rt2) = yaml.get("runtime") {
            if rt2.is_string() {
                runtime = Some(rt2.as_str().unwrap().to_string());
            }
        }
        if let Some(sv2) = yaml.get("server") {
            if sv2.is_string() {
                server = Some(sv2.as_str().unwrap().to_string());
            } else if sv2.is_mapping() {
                if let Some(sv4) = sv2.get("type") {
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
    Ok(runtime.is_some_and(|r| r.starts_with("shiny")) || server.as_deref() == Some("Shiny"))
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
                OUTPUT.error(&format!(
                    "Failed to read YAML header from file at {}: {}",
                    file.display(),
                    e
                ));
                error!(
                    "Failed to read YAML header from file at {}: {}",
                    file.display(),
                    e
                );
                bail!(
                    "Failed to read YAML header from file at {}: {}",
                    file.display(),
                    e
                );
            }
        };
        trace!("Got line: {}", line);

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
                OUTPUT.error(&format!(
                    "Failed to read YAML header from file at {}: {}",
                    file.display(),
                    e
                ));
                error!(
                    "Failed to read YAML header from file at {}: {}",
                    file.display(),
                    e
                );
                bail!(
                    "Failed to read YAML header from file at {}: {}",
                    file.display(),
                    e
                );
            }
        };
        if (start_lines && re_line.is_match(&line)) || (!start_lines && re_dots.is_match(&line)) {
            trace!("End of YAML header");
            break;
        } else {
            trace!("YAML header: {}", line);
            header.push_str(&line);
            header.push('\n');
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
        OUTPUT.error(&format!(
            "Could not find script '{}' in package '{}'.",
            fun, pkg
        ));
        error!("Could not find script '{}' in package '{}'.", fun, pkg);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_bin_name() {
        for name in ["report", "build-site", "check_all", "r2d2"] {
            assert!(is_bin_name(name), "{} should be a script name", name);
        }
        for name in [
            "",
            ".",
            "..",
            "pkg::script",
            "app/dir",
            "./app",
            "app\\dir",
            "report.R",
            "report.r",
            "paper.Rmd",
            "paper.qmd",
        ] {
            assert!(!is_bin_name(name), "{} should not be a script name", name);
        }
    }

    fn split(args: &[&str]) -> (Vec<String>, Vec<String>) {
        split_r_cmd_args(args.iter().map(|x| x.to_string()).collect())
    }

    #[test]
    fn test_split_r_cmd_args_no_r_options() {
        let (ropts, rest) = split(&["check", "pkg.tar.gz", "--no-manual"]);
        assert!(ropts.is_empty());
        assert_eq!(rest, ["check", "pkg.tar.gz", "--no-manual"]);
    }

    #[test]
    fn test_split_r_cmd_args_r_options_are_moved() {
        // before and after the command, and mixed with the command's own options
        for args in [
            ["--vanilla", "check", "pkg.tar.gz"],
            ["check", "--vanilla", "pkg.tar.gz"],
            ["check", "pkg.tar.gz", "--vanilla"],
        ] {
            let (ropts, rest) = split(&args);
            assert_eq!(ropts, ["--vanilla"]);
            assert_eq!(rest, ["check", "pkg.tar.gz"]);
        }

        let (ropts, rest) = split(&[
            "INSTALL",
            "--no-environ",
            "--no-multiarch",
            "--no-init-file",
            "--no-site-file",
            "pkg.tar.gz",
        ]);
        assert_eq!(ropts, ["--no-environ", "--no-init-file", "--no-site-file"]);
        assert_eq!(rest, ["INSTALL", "--no-multiarch", "pkg.tar.gz"]);
    }

    #[test]
    fn test_split_r_cmd_args_arch() {
        let (ropts, rest) = split(&["check", "--arch", "x86_64", "."]);
        assert_eq!(ropts, ["--arch", "x86_64"]);
        assert_eq!(rest, ["check", "."]);

        let (ropts, rest) = split(&["--arch=x86_64", "check", "."]);
        assert_eq!(ropts, ["--arch=x86_64"]);
        assert_eq!(rest, ["check", "."]);

        // a missing --arch value is R's problem to report
        let (ropts, rest) = split(&["check", "--arch"]);
        assert_eq!(ropts, ["--arch"]);
        assert_eq!(rest, ["check"]);
    }

    #[test]
    fn test_split_r_cmd_args_separator() {
        let (ropts, rest) = split(&["--", "check", "."]);
        assert!(ropts.is_empty());
        assert_eq!(rest, ["check", "."]);

        // only the first one, the rest are the command's arguments
        let (ropts, rest) = split(&["--", "check", "--", "."]);
        assert!(ropts.is_empty());
        assert_eq!(rest, ["check", "--", "."]);
    }

    #[test]
    fn test_split_r_cmd_args_only_r_options() {
        let (ropts, rest) = split(&["--vanilla"]);
        assert_eq!(ropts, ["--vanilla"]);
        assert!(rest.is_empty());
    }
}
