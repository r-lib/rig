//! Development tasks for a package or project: `rig proj test` and
//! `rig proj check`.
//!
//! A task runs in the root of the package or project at or above the current
//! directory, with the project's environment if there is an `rproj.toml`, or
//! with the selected (or default) R version for a plain package that only has
//! a `DESCRIPTION` file. A project can replace a task with its own script, by
//! declaring a `[[bin]]` with the same name in `rproj.toml`.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use clap::ArgMatches;
use log::{error, info, trace};

use crate::common::*;
use crate::output::OUTPUT;
use crate::proj::{description_is_rig_generated, parse_description_paragraph};
use crate::rproj::{Rproj, RPROJ_MANIFEST_FILE};
use crate::rvenv::{
    rscript_of, rvenv_wrapper_target, RPROJ_LOCK_FILE, RVENV_DIR, RVENV_RENVIRON_FILE,
    RVENV_SHIM_DIR,
};
use crate::scrun::{
    ignore_sigint, project_bins, project_env_r_binary, sc_run_project_script, user_renviron, RunEnv,
};

#[cfg(target_os = "macos")]
use crate::macos::*;

#[cfg(target_os = "windows")]
use crate::windows::*;

#[cfg(target_os = "linux")]
use crate::linux::*;

/// The package or project a task runs on.
struct TaskRoot {
    dir: PathBuf,
    /// `rproj.toml`, if there is one. A plain package has only a
    /// `DESCRIPTION`.
    manifest: Option<Rproj>,
    /// Whether this is an R package, as opposed to a `type = "project"`
    /// project.
    is_package: bool,
}

/// The nearest directory at or above `start` that has an `rproj.toml` or a
/// `DESCRIPTION` file. In a directory with both, `rproj.toml` wins.
fn find_task_root(start: &Path) -> Result<Option<TaskRoot>, Box<dyn Error>> {
    let mut dir = start;
    loop {
        if dir.join(RPROJ_MANIFEST_FILE).is_file() {
            let manifest = crate::proj::proj_read_manifest_opt(dir)?
                .ok_or_else(|| format!("Cannot read {}", RPROJ_MANIFEST_FILE))?;
            let is_package = manifest.project.is_package();
            return Ok(Some(TaskRoot {
                dir: dir.to_path_buf(),
                manifest: Some(manifest),
                is_package,
            }));
        }
        let desc = dir.join("DESCRIPTION");
        if desc.is_file() {
            let paragraph = parse_description_paragraph(fs::File::open(&desc)?)?;
            let is_package = paragraph
                .get("Type")
                .map(|t| t.trim().eq_ignore_ascii_case("package"))
                .unwrap_or(true);
            return Ok(Some(TaskRoot {
                dir: dir.to_path_buf(),
                manifest: None,
                is_package,
            }));
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => return Ok(None),
        }
    }
}

fn task_root() -> Result<TaskRoot, Box<dyn Error>> {
    match find_task_root(&std::env::current_dir()?)? {
        Some(root) => Ok(root),
        None => {
            let msg = format!(
                "No package or project here: found no {} or DESCRIPTION file in the \
                 current directory or above it",
                RPROJ_MANIFEST_FILE
            );
            OUTPUT.error(&msg);
            error!("{}", msg);
            bail!("{}", msg);
        }
    }
}

/// The R (or `Rscript`) to run a task with, in the task's root directory.
/// That is the project environment for a project, unless `--r-version` asks
/// for a specific R version, and the selected or default R otherwise.
fn task_env(
    root: &TaskRoot,
    args: &ArgMatches,
    rscript: bool,
    dry_run: bool,
) -> Result<RunEnv, Box<dyn Error>> {
    let rver = args.get_one::<String>("r-version");
    let use_project = root.manifest.is_some() && rver.is_none();
    let rbin = if use_project {
        project_env_r_binary(&root.dir, args, dry_run)?
    } else {
        let rver = match rver {
            Some(x) => check_installed(x)?,
            None => sc_get_default_or_fail()?,
        };
        get_r_binary(&rver)?.to_string_lossy().into_owned()
    };
    let rbin = if rscript {
        rscript_of(Path::new(&rbin)).to_string_lossy().into_owned()
    } else {
        rbin
    };
    let direct = if dry_run {
        None
    } else {
        rvenv_wrapper_target(Path::new(&rbin))?
    };
    // A project's own `.Renviron` sets up its environment, which does not
    // fit another R version.
    let renviron_user = if root.manifest.is_some() && !use_project {
        user_renviron()
    } else {
        None
    };
    Ok(RunEnv {
        rbin,
        direct,
        renviron_user,
        path_prepend: None,
        cwd: Some(root.dir.clone()),
    })
}

/// The positional arguments of a task, i.e. everything after `--`, too.
fn task_args(args: &ArgMatches) -> Vec<String> {
    args.get_many::<String>("args")
        .map(|v| v.cloned().collect())
        .unwrap_or_default()
}

/// If the project declares a `[[bin]]` called `task`, and `--builtin` was not
/// given, run that script instead of the built-in task, and return its exit
/// status.
fn run_override(
    task: &str,
    root: &TaskRoot,
    args: &ArgMatches,
    dry_run: bool,
) -> Result<Option<i32>, Box<dyn Error>> {
    if root.manifest.is_none() || args.get_flag("builtin") {
        return Ok(None);
    }
    let Some(bin) = project_bins(&root.dir)?
        .into_iter()
        .find(|b| b.name == task)
    else {
        return Ok(None);
    };
    let msg = format!(
        "Running the `{}` script from {} (use --builtin for rig's own {} task)",
        task, RPROJ_MANIFEST_FILE, task
    );
    OUTPUT.info(&msg);
    info!("{}", msg);
    let env = task_env(root, args, false, dry_run)?;
    let rargs = vec!["-q".to_string(), "--slave".to_string()];
    let code = sc_run_project_script(env, rargs, &root.dir, &bin, task_args(args), false, dry_run)?;
    Ok(Some(code))
}

/// Runs `env`'s R (or `Rscript`) with `rargs`, or prints the command for
/// `--dry-run`. Call `ignore_sigint()` once before this.
fn run_r(env: &RunEnv, rargs: &[String], dry_run: bool) -> Result<i32, Box<dyn Error>> {
    if dry_run {
        println!("\"{}\" {:?}", env.rbin, rargs);
        return Ok(0);
    }
    trace!("Running {} with arguments {:?}", env.rbin, rargs);
    let status = env.command().args(rargs).status()?;
    Ok(status.code().unwrap_or(-1))
}

// ------------------------------------------------------------------------
// rig proj test

#[derive(Debug, PartialEq)]
enum TestFramework {
    Testthat,
    Tinytest,
    /// Plain `tests/*.R` (and `*.Rin`) files, run the way `R CMD check`
    /// runs them.
    Base,
}

fn detect_test_framework(root: &Path) -> Option<TestFramework> {
    let tests = root.join("tests");
    if tests.join("testthat").is_dir() {
        Some(TestFramework::Testthat)
    } else if root.join("inst").join("tinytest").is_dir() {
        Some(TestFramework::Tinytest)
    } else if has_base_tests(&tests) {
        Some(TestFramework::Base)
    } else {
        None
    }
}

/// Whether `dir` has `.R`, `.r` or `.Rin` files, the files
/// `tools:::.runPackageTests()` runs.
fn has_base_tests(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|e| {
        let name = e.file_name();
        let name = name.to_string_lossy();
        e.path().is_file()
            && (name.ends_with(".R") || name.ends_with(".r") || name.ends_with(".Rin"))
    })
}

/// R code that runs plain `tests/*.R` files with `tools:::.runPackageTests()`,
/// the function `R CMD check` uses. It writes its output files into the
/// current directory, so it runs in a copy of `tests`. The test files load
/// the installed package, so for a package it installs the package into a
/// temporary library first. The copy is kept if a test fails, to look at
/// the `.Rout.fail` files.
fn base_test_code(is_package: bool) -> String {
    let mut code = String::from(
        "if (!exists(\".runPackageTests\", asNamespace(\"tools\"))) { \
         cat(\"This R version cannot run plain tests/*.R files\\n\", file = stderr()); \
         quit(status = 1) }\n\
         tmp <- tempfile(\"rig-tests-\", tmpdir = dirname(tempdir()))\n\
         dir.create(tmp)\n",
    );
    if is_package {
        code.push_str(
            "lib <- file.path(tmp, \"lib\")\n\
             dir.create(lib)\n\
             out <- suppressWarnings(system2(file.path(R.home(\"bin\"), \"R\"), \
             c(\"CMD\", \"INSTALL\", \"--no-docs\", \"--no-multiarch\", \"--no-test-load\", \
             paste0(\"--library=\", shQuote(lib)), shQuote(getwd())), stdout = TRUE, stderr = TRUE))\n\
             if (!is.null(attr(out, \"status\"))) { writeLines(out, con = stderr()); \
             cat(\"Could not install the package\\n\", file = stderr()); quit(status = 1) }\n\
             Sys.setenv(R_LIBS = paste(c(lib, .libPaths()), collapse = .Platform$path.sep))\n",
        );
    }
    code.push_str(
        "invisible(file.copy(\"tests\", tmp, recursive = TRUE))\n\
         wd <- setwd(file.path(tmp, \"tests\"))\n\
         nfail <- tools:::.runPackageTests(stop_on_error = FALSE)\n\
         if (nfail > 0) { cat(\"\\n\", nfail, \" test file(s) failed, see the .Rout.fail \
         files in \", getwd(), \"\\n\", sep = \"\", file = stderr()); quit(status = 1) }\n\
         setwd(wd)\n\
         unlink(tmp, recursive = TRUE)\n",
    );
    code
}

/// `x` as an R string literal.
fn r_string(x: &str) -> String {
    let mut out = String::from("\"");
    for c in x.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// R code that fails with `hint` if package `pkg` is not installed.
fn r_require(pkg: &str, hint: &str) -> String {
    format!(
        "if (!requireNamespace({}, quietly = TRUE)) {{ \
         cat({}, file = stderr()); quit(status = 1) }}\n",
        r_string(pkg),
        r_string(&format!("The {} package is not installed. {}\n", pkg, hint))
    )
}

/// The R code that runs the tests.
fn test_code(
    framework: &TestFramework,
    is_package: bool,
    has_manifest: bool,
    filter: Option<&str>,
) -> String {
    let pkg = match framework {
        TestFramework::Testthat => "testthat",
        TestFramework::Tinytest => "tinytest",
        TestFramework::Base => return base_test_code(is_package),
    };
    let hint = if has_manifest {
        format!(
            "Add it with `rig proj add --group dev {}`, or run `rig proj sync` \
             if you synced the project with --no-dev.",
            pkg
        )
    } else {
        format!("Install it with `rig pkg install {}`.", pkg)
    };
    let mut code = r_require(pkg, &hint);
    let filter = match filter {
        Some(f) => format!("filter = {}, ", r_string(f)),
        None => String::new(),
    };
    match (framework, is_package) {
        (TestFramework::Testthat, true) => code.push_str(&format!(
            "testthat::test_local(\".\", {}stop_on_failure = TRUE)\n",
            filter
        )),
        (TestFramework::Testthat, false) => code.push_str(&format!(
            "testthat::test_dir(\"tests/testthat\", load_package = \"none\", \
             {}stop_on_failure = TRUE)\n",
            filter
        )),
        (TestFramework::Tinytest, true) => code.push_str(
            "res <- tinytest::build_install_test(\".\")\n\
             print(res)\n\
             if (!tinytest::all_pass(res)) quit(status = 1)\n",
        ),
        (TestFramework::Tinytest, false) => code.push_str(
            "res <- tinytest::run_test_dir(\"inst/tinytest\")\n\
             print(res)\n\
             if (!tinytest::all_pass(res)) quit(status = 1)\n",
        ),
        (TestFramework::Base, _) => unreachable!(),
    }
    code
}

/// `rig proj test`
pub fn sc_proj_test(args: &ArgMatches) -> Result<i32, Box<dyn Error>> {
    let dry_run = args.get_flag("dry-run");
    let root = task_root()?;
    if let Some(code) = run_override("test", &root, args, dry_run)? {
        return Ok(code);
    }

    let filter = task_args(args);
    if filter.len() > 1 {
        let msg = format!(
            "`rig proj test` takes at most one test filter, got {}: {}",
            filter.len(),
            filter.join(" ")
        );
        OUTPUT.error(&msg);
        error!("{}", msg);
        bail!("{}", msg);
    }
    let filter = filter.first().map(|f| f.as_str());

    let Some(framework) = detect_test_framework(&root.dir) else {
        let msg = format!(
            "Found no tests in {}: looked for tests/testthat, inst/tinytest and \
             .R files in tests",
            root.dir.display()
        );
        OUTPUT.error(&msg);
        error!("{}", msg);
        bail!("{}", msg);
    };
    if framework != TestFramework::Testthat && filter.is_some() {
        let msg = "A test filter only works with testthat tests";
        OUTPUT.error(msg);
        error!("{}", msg);
        bail!("{}", msg);
    }

    let code = test_code(&framework, root.is_package, root.manifest.is_some(), filter);
    let env = task_env(&root, args, true, dry_run)?;
    if !dry_run {
        ignore_sigint();
    }
    run_r(&env, &["-e".to_string(), code], dry_run)
}

// ------------------------------------------------------------------------
// rig proj check

/// Files and directories in a project's root that belong to rig, not to the
/// package, so `rig proj check` leaves them out of the package it builds.
const RIG_PROJECT_FILES: [&str; 4] = [
    RVENV_DIR,
    RVENV_SHIM_DIR,
    RPROJ_MANIFEST_FILE,
    RPROJ_LOCK_FILE,
];

/// Whether `rig proj check` leaves the top level file or directory `name`
/// of the project at `root` out of the package it builds.
fn skip_in_build(root: &Path, name: &str) -> bool {
    if name == ".git" || RIG_PROJECT_FILES.contains(&name) {
        return true;
    }
    // The `.Renviron` that activates the project environment.
    if name == RVENV_RENVIRON_FILE {
        if let Ok(text) = fs::read_to_string(root.join(name)) {
            return text.contains("# rig rvenv start");
        }
    }
    false
}

/// Copy the directory `from` to `to`, recursively.
fn copy_dir(from: &Path, to: &Path) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if src.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

/// Copy the package source at `root` into `to`, without rig's own files and
/// without `skip_dir` (the check output directory, if it is inside the
/// package). For a project with an `rproj.toml`, write the `DESCRIPTION`
/// from `rproj.toml`, unless the package has its own `DESCRIPTION`.
fn copy_package_source(root: &TaskRoot, to: &Path, skip_dir: &Path) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(to)?;
    let skip_dir = skip_dir.canonicalize().ok();
    for entry in fs::read_dir(&root.dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if skip_in_build(&root.dir, &name) {
            continue;
        }
        let src = entry.path();
        if skip_dir.is_some() && src.canonicalize().ok() == skip_dir {
            continue;
        }
        let dst = to.join(entry.file_name());
        if src.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            fs::copy(&src, &dst)?;
        }
    }

    if let Some(manifest) = &root.manifest {
        let desc = root.dir.join("DESCRIPTION");
        if !desc.exists() || description_is_rig_generated(&desc) {
            let (text, _dropped) = manifest.to_description()?;
            fs::write(to.join("DESCRIPTION"), text)?;
        }
    }
    Ok(())
}

/// The package name and version from the `DESCRIPTION` file in `dir`.
fn package_name_version(dir: &Path) -> Result<(String, String), Box<dyn Error>> {
    let paragraph = parse_description_paragraph(fs::File::open(dir.join("DESCRIPTION"))?)?;
    let name = paragraph
        .get("Package")
        .ok_or("No Package field in DESCRIPTION")?
        .trim()
        .to_string();
    let version = paragraph
        .get("Version")
        .ok_or("No Version field in DESCRIPTION")?
        .trim()
        .to_string();
    Ok((name, version))
}

/// The arguments of `R CMD build` and `R CMD check`.
fn check_args(args: &ArgMatches, source: &str, tarball: &str) -> (Vec<String>, Vec<String>) {
    let no_vignettes = args.get_flag("no-vignettes");
    let mut build = vec!["CMD".to_string(), "build".to_string()];
    if no_vignettes {
        build.push("--no-build-vignettes".to_string());
    }
    build.push(source.to_string());

    let mut check = vec!["CMD".to_string(), "check".to_string()];
    if args.get_flag("as-cran") {
        check.push("--as-cran".to_string());
    }
    if !args.get_flag("manual") {
        check.push("--no-manual".to_string());
    }
    if no_vignettes {
        check.push("--ignore-vignettes".to_string());
    }
    check.push(tarball.to_string());
    check.extend(task_args(args));
    (build, check)
}

/// Where `rig proj check` puts the tarball and the check results: `--output`,
/// or `.rvenv/check` in a project. `None` means a new temporary directory.
fn check_output_dir(root: &TaskRoot, args: &ArgMatches) -> Result<Option<PathBuf>, Box<dyn Error>> {
    let dir = match args.get_one::<String>("output") {
        Some(dir) => PathBuf::from(dir),
        None if root.manifest.is_some() => root.dir.join(RVENV_DIR).join("check"),
        None => return Ok(None),
    };
    if dir.is_absolute() {
        Ok(Some(dir))
    } else {
        Ok(Some(std::env::current_dir()?.join(dir)))
    }
}

/// `rig proj check`
pub fn sc_proj_check(args: &ArgMatches) -> Result<i32, Box<dyn Error>> {
    let dry_run = args.get_flag("dry-run");
    let root = task_root()?;
    if let Some(code) = run_override("check", &root, args, dry_run)? {
        return Ok(code);
    }

    if !root.is_package {
        let msg = format!(
            "{} is not an R package, `rig proj check` only works for packages",
            root.dir.display()
        );
        OUTPUT.error(&msg);
        error!("{}", msg);
        bail!("{}", msg);
    }

    // Get the R binary first, this syncs the project, which writes its
    // `DESCRIPTION`.
    let mut env = task_env(&root, args, false, dry_run)?;

    if dry_run {
        // A dry run does not create or copy anything, so name the tarball
        // from the package itself.
        let (name, version) = match &root.manifest {
            Some(m) => (m.project.name.clone(), m.project.version.clone()),
            None => package_name_version(&root.dir)?,
        };
        let tarball = format!("{}_{}.tar.gz", name, version);
        let (build, check) = check_args(args, "<copy of the package>", &tarball);
        match check_output_dir(&root, args)? {
            Some(dir) => println!("cd \"{}\"", dir.display()),
            None => println!("cd <new temporary directory>"),
        }
        run_r(&env, &build, true)?;
        return run_r(&env, &check, true);
    }

    let out_dir = match check_output_dir(&root, args)? {
        Some(dir) => dir,
        None => tempfile::Builder::new()
            .prefix("rig-check-")
            .tempdir()?
            .keep(),
    };
    env.cwd = Some(out_dir.clone());
    let build_dir = tempfile::Builder::new().prefix("rig-build-").tempdir()?;
    let source = build_dir.path().join("pkg");

    fs::create_dir_all(&out_dir)?;
    copy_package_source(&root, &source, &out_dir)?;
    let (name, version) = package_name_version(&source)?;
    let tarball = format!("{}_{}.tar.gz", name, version);
    let (build, check) = check_args(args, &source.to_string_lossy(), &tarball);

    let msg = format!("Checking {} {} in {}", name, version, out_dir.display());
    OUTPUT.status(&msg);
    info!("{}", msg);

    ignore_sigint();
    let code = run_r(&env, &build, false)?;
    if code != 0 {
        let msg = format!("`R CMD build` failed with exit status {}", code);
        OUTPUT.error(&msg);
        error!("{}", msg);
        return Ok(code);
    }
    let code = run_r(&env, &check, false)?;
    let msg = format!(
        "Check results are in {}",
        out_dir.join(format!("{}.Rcheck", name)).display()
    );
    OUTPUT.info(&msg);
    info!("{}", msg);
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_task_root_prefers_the_nearest_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("DESCRIPTION"), "Package: foo\nVersion: 1.0.0\n").unwrap();
        let sub = root.join("tests").join("testthat");
        fs::create_dir_all(&sub).unwrap();

        let found = find_task_root(&sub).unwrap().unwrap();
        assert_eq!(found.dir, root);
        assert!(found.manifest.is_none());
        assert!(found.is_package);
    }

    #[test]
    fn find_task_root_reads_the_type_field() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("DESCRIPTION"),
            "Package: foo\nVersion: 1.0.0\nType: Project\n",
        )
        .unwrap();
        let found = find_task_root(tmp.path()).unwrap().unwrap();
        assert!(!found.is_package);
    }

    #[test]
    fn find_task_root_prefers_the_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join(RPROJ_MANIFEST_FILE),
            "[project]\nname = \"foo\"\nversion = \"1.0.0\"\ntype = \"project\"\n",
        )
        .unwrap();
        fs::write(
            tmp.path().join("DESCRIPTION"),
            "Package: foo\nVersion: 1.0.0\n",
        )
        .unwrap();
        let found = find_task_root(tmp.path()).unwrap().unwrap();
        assert!(found.manifest.is_some());
        assert!(!found.is_package);
    }

    #[test]
    fn find_task_root_finds_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        // The temp dir's parents might have a DESCRIPTION file, but that is
        // very unlikely.
        let found = find_task_root(tmp.path()).unwrap();
        assert!(found.is_none());
    }

    #[test]
    fn detect_test_framework_finds_testthat_and_tinytest() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(detect_test_framework(tmp.path()), None);
        fs::create_dir_all(tmp.path().join("inst/tinytest")).unwrap();
        assert_eq!(
            detect_test_framework(tmp.path()),
            Some(TestFramework::Tinytest)
        );
        fs::create_dir_all(tmp.path().join("tests/testthat")).unwrap();
        assert_eq!(
            detect_test_framework(tmp.path()),
            Some(TestFramework::Testthat)
        );
    }

    #[test]
    fn detect_test_framework_finds_plain_test_files() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("tests")).unwrap();
        fs::write(tmp.path().join("tests/README.md"), "").unwrap();
        assert_eq!(detect_test_framework(tmp.path()), None);
        fs::write(tmp.path().join("tests/basic.R"), "").unwrap();
        assert_eq!(detect_test_framework(tmp.path()), Some(TestFramework::Base));
    }

    #[test]
    fn test_code_for_plain_test_files() {
        let code = test_code(&TestFramework::Base, true, false, None);
        assert!(code.contains("\"CMD\", \"INSTALL\""));
        assert!(code.contains("tools:::.runPackageTests(stop_on_error = FALSE)"));
        let code = test_code(&TestFramework::Base, false, true, None);
        assert!(!code.contains("INSTALL"));
        assert!(code.contains("tools:::.runPackageTests(stop_on_error = FALSE)"));
    }

    #[test]
    fn r_string_escapes() {
        assert_eq!(r_string("abc"), "\"abc\"");
        assert_eq!(r_string("a\"b\\c\nd"), "\"a\\\"b\\\\c\\nd\"");
    }

    #[test]
    fn test_code_for_each_framework() {
        let code = test_code(&TestFramework::Testthat, true, true, Some("utils"));
        assert!(code.contains("requireNamespace(\"testthat\""));
        assert!(code.contains("rig proj add --group dev testthat"));
        assert!(code
            .contains("testthat::test_local(\".\", filter = \"utils\", stop_on_failure = TRUE)"));

        let code = test_code(&TestFramework::Testthat, false, true, None);
        assert!(code.contains(
            "testthat::test_dir(\"tests/testthat\", load_package = \"none\", stop_on_failure = TRUE)"
        ));

        let code = test_code(&TestFramework::Tinytest, true, false, None);
        assert!(code.contains("rig pkg install tinytest"));
        assert!(code.contains("tinytest::build_install_test(\".\")"));
        assert!(code.contains("tinytest::all_pass(res)"));

        let code = test_code(&TestFramework::Tinytest, false, false, None);
        assert!(code.contains("tinytest::run_test_dir(\"inst/tinytest\")"));
    }

    #[test]
    fn copy_package_source_skips_rig_files() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(src.join("R")).unwrap();
        fs::create_dir_all(src.join(".rvenv/lib")).unwrap();
        fs::create_dir_all(src.join(".rvenvlib/rvenv")).unwrap();
        fs::create_dir_all(src.join("out")).unwrap();
        fs::write(src.join("R/foo.R"), "f <- function() 1\n").unwrap();
        fs::write(src.join("DESCRIPTION"), "Package: foo\nVersion: 1.0.0\n").unwrap();
        fs::write(src.join(RPROJ_LOCK_FILE), "").unwrap();
        fs::write(
            src.join(".Renviron"),
            "# rig rvenv start\nX=1\n# rig rvenv end\n",
        )
        .unwrap();
        fs::write(src.join(".Rbuildignore"), "^foo$\n").unwrap();

        let root = TaskRoot {
            dir: src.clone(),
            manifest: None,
            is_package: true,
        };
        let dst = tmp.path().join("dst");
        copy_package_source(&root, &dst, &src.join("out")).unwrap();

        assert!(dst.join("R/foo.R").exists());
        assert!(dst.join("DESCRIPTION").exists());
        assert!(dst.join(".Rbuildignore").exists());
        assert!(!dst.join(".rvenv").exists());
        assert!(!dst.join(".rvenvlib").exists());
        assert!(!dst.join(RPROJ_LOCK_FILE).exists());
        assert!(!dst.join(".Renviron").exists());
        assert!(!dst.join("out").exists());
        assert_eq!(
            package_name_version(&dst).unwrap(),
            ("foo".to_string(), "1.0.0".to_string())
        );
    }

    #[test]
    fn copy_package_source_keeps_a_users_renviron() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join(".Renviron"), "X=1\n").unwrap();
        let root = TaskRoot {
            dir: src.clone(),
            manifest: None,
            is_package: true,
        };
        let dst = tmp.path().join("dst");
        copy_package_source(&root, &dst, &tmp.path().join("nope")).unwrap();
        assert!(dst.join(".Renviron").exists());
    }
}
