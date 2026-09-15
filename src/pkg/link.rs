//! `rig pkg link`: an editable install.
//!
//! Instead of copying a built package into the library, this writes a stub
//! package entry whose `R/<pkgname>` file loads the package's code live from
//! its source directory every time it is loaded, so an edit to the source
//! takes effect on the next `library(pkgname)` call, with no reinstall.
//!
//! The mechanism: `loadNamespace()` always sources `R/<pkgname>` (no
//! extension) as plain R code. A normal `R CMD INSTALL`-produced package
//! happens to make that file's content a small loader that calls
//! `lazyLoad()` against a compiled `R/<pkg>.rdb`/`.rdx` pair sitting next to
//! it; nothing forces that, so this file contains different code instead, and
//! the compiled pair is simply never referenced. Exports, `.onLoad` and S3
//! method registration are still handled normally by R's own NAMESPACE
//! processing once `R/<pkgname>` returns, so this only needs to get the
//! package's own R code into the namespace `loadNamespace()` is already
//! building — done here via pkgload's own (non-exported) `load_code()` step,
//! which resolves its target namespace via `asNamespace(package)` rather than
//! creating a new one, so it lands in the right place.
//!
//! Compiled code (a non-empty `src/` directory) needs a real `libs/<pkg>
//! <.so|.dll>` in the stub directory after all: unlike exports, `.onLoad` and
//! S3 registration, which R only processes once `R/<pkgname>` returns, R's
//! `useDynLib()` handling in `loadNamespace()` runs unconditionally against
//! the *installed* package directory -- confirmed empirically, since
//! `pkgload:::load_dll()` manually `dyn.load()`ing the source's own compiled
//! object still left R's own dynlib step failing right after (looking for
//! `libs/<pkg><.so|.dll>` in the stub, and also re-registering the same native
//! routines a second time). So the stub script instead just compiles the
//! source in place with `pkgbuild::compile_dll()` (the same step
//! `pkgload::load_all()` uses) and copies the resulting object into the
//! stub's own `libs/` directory, where R's ordinary `library.dynam()` step
//! finds it on its own; nothing R-side has to know this is a link rather than
//! a normal install. An edit to `src/` is only picked up on the next
//! `library()` call, same as an `R/` edit, but recompiling (unlike a plain R
//! source re-read) takes real time.
//!
//! A `libs/` directory also makes `loadNamespace()` demand a
//! `Meta/features.rds` holding an `internalsID` -- one specific GUID per R
//! ABI epoch (stable across patch releases, unlike `Built$R`), which
//! `R CMD INSTALL` always writes and which `library()` checks *before*
//! sourcing `R/<pkgname>` at all; without it (or with the wrong one) loading
//! fails with "installed by an R version with different internals", even
//! though the compiled object itself loads fine on its own (confirmed
//! empirically by bisecting a real install's `Meta/` files one at a time
//! against ours). There is no public R API for this value, so [`sc_pkg_link`]
//! gets it the only way there is: running the target R version once with
//! `.Internal(internalsID())` (the base `system.file()`/`utils::install
//! packages()` machinery has no exposed accessor either). This is the one
//! point where linking a compiled package needs to start R at all.

use std::error::Error;
use std::fs;
use std::path::Path;
use std::process::Command;

use clap::ArgMatches;
use rds2rust::RObject;
use simple_error::*;

#[cfg(target_os = "macos")]
use crate::macos::get_r_binary;

#[cfg(target_os = "windows")]
use crate::windows::get_r_binary;

#[cfg(target_os = "linux")]
use crate::linux::get_r_binary;

use crate::dcf::parse_dcf;
use crate::install::drop_fields;
use crate::library::{library_rver, sc_library_get_default};
use crate::output::OUTPUT;
use crate::rds::{
    character_scalar, named_character_vector, named_list, r_system_version, write_rds_file,
};
use crate::textfmt::reflow;

use super::list::{read_installed, resolve_library, ResolvedLibrary};

/// The `DESCRIPTION` field recording that a package is a `rig pkg link`
/// editable install: the absolute path of the source directory it loads
/// from. Unlike `RemoteType`/`RemoteUrl` (which pak/renv also write, for a
/// package installed from a local path), this field is rig's own, so that a
/// linked package is never mistaken for a normal, reproducible install by
/// other tooling.
pub const RIG_LINK_FIELD: &str = "RigLink";

pub fn sc_pkg_link(
    args: &ArgMatches,
    pkgargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let json = args.get_flag("json") || pkgargs.get_flag("json") || mainargs.get_flag("json");

    match sc_pkg_link_impl(args, json) {
        Ok(()) => Ok(()),
        Err(err) => {
            OUTPUT.error(&err.to_string());
            Err(err)
        }
    }
}

fn sc_pkg_link_impl(args: &ArgMatches, json: bool) -> Result<(), Box<dyn Error>> {
    let path_arg = args.get_one::<String>("path").unwrap();
    let source =
        fs::canonicalize(path_arg).map_err(|err| format!("Cannot find {}: {}", path_arg, err))?;
    if !source.is_dir() {
        bail!("{} is not a directory", source.display());
    }

    let desc_path = source.join("DESCRIPTION");
    let desc_text = fs::read_to_string(&desc_path)
        .map_err(|err| format!("Cannot read {}: {}", desc_path.display(), err))?;
    let (name, version) = package_name_and_version(&desc_text)?;

    if !source.join("NAMESPACE").is_file() {
        bail!(
            "{} has no NAMESPACE file. Run roxygen2::roxygenise() (or write \
            one by hand) before linking it.",
            source.display()
        );
    }

    let src_dir = source.join("src");
    let has_compiled_code = src_dir.is_dir() && has_entries(&src_dir)?;

    let lib = resolve_library(args)?;
    ensure_pkgload_available(&lib, has_compiled_code)?;
    // `resolve_library` only knows the R version when `--library` named a
    // library of an R installation, not when it was a plain directory path
    // (see `ResolvedLibrary`'s doc comment); `Built$R` still needs one, R
    // itself rejects an empty version there, so this falls back to
    // `--r-version`/the default R version the same way `rig pkg install` does.
    let rver = match &lib.rversion {
        Some(rver) => rver.clone(),
        None => library_rver(args)?,
    };

    let target = lib.path.join(&name);
    if target.exists() {
        let installed = read_installed(&lib.path)?;
        let already_linked = installed
            .iter()
            .any(|pkg| pkg.path == target && pkg.link_source.is_some());
        if !already_linked {
            bail!(
                "{} is already installed in {} and is not a link. Run `rig \
                pkg remove {}` first.",
                name,
                lib.path.display(),
                name
            );
        }
        fs::remove_dir_all(&target)?;
    }

    write_stub(
        &target,
        &source,
        &name,
        &desc_text,
        &rver,
        has_compiled_code,
    )?;

    if json {
        print_linked_json(&name, &version, &source, &lib)?;
    } else {
        OUTPUT.success(&format!(
            "Linked {} {} to {} {}{}",
            name,
            version,
            source.display(),
            lib.tag(),
            if has_compiled_code {
                " (has compiled code, recompiled on every load)"
            } else {
                ""
            }
        ));
    }

    Ok(())
}

/// The `Package` and `Version` fields of a DESCRIPTION, erroring if either is
/// missing: that means `source` is not an R package.
fn package_name_and_version(desc_text: &str) -> Result<(String, String), Box<dyn Error>> {
    let desc = parse_dcf(desc_text)?;
    let para = desc.iter().next().ok_or("empty DESCRIPTION file")?;
    let name = para
        .get("Package")
        .ok_or("DESCRIPTION has no Package field")?
        .to_string();
    let version = para
        .get("Version")
        .ok_or("DESCRIPTION has no Version field")?
        .to_string();
    Ok((name, version))
}

fn has_entries(dir: &Path) -> Result<bool, Box<dyn Error>> {
    Ok(fs::read_dir(dir)?.next().is_some())
}

/// Whether `pkgload` -- and, when the source has compiled code,
/// `pkgbuild` too -- is installed somewhere the target library's R session
/// would find it: the library itself, or, when it is not already the
/// default, the R version's default library too (both are on `.libPaths()`).
/// A plain filesystem check, no R invocation needed.
fn ensure_pkgload_available(
    lib: &ResolvedLibrary,
    has_compiled_code: bool,
) -> Result<(), Box<dyn Error>> {
    let mut needed = vec!["pkgload"];
    if has_compiled_code {
        needed.push("pkgbuild");
    }
    needed.retain(|pkg| !is_available(lib, pkg));
    if needed.is_empty() {
        return Ok(());
    }

    let lib_flag = match &lib.name {
        Some(name) => name.clone(),
        None => lib.path.display().to_string(),
    };
    let rver_flag = match &lib.rversion {
        Some(rver) => format!(" -r {}", rver),
        None => String::new(),
    };
    bail!(
        "The {} {} required to load a linked package{}, but {} not installed \
        in {}. Install {} with:\n  rig pkg install {} -l {}{}",
        needed.join(" and "),
        if needed.len() == 1 {
            "package is"
        } else {
            "packages are"
        },
        if has_compiled_code {
            " with compiled code"
        } else {
            ""
        },
        if needed.len() == 1 {
            "it is"
        } else {
            "they are"
        },
        lib.path.display(),
        if needed.len() == 1 { "it" } else { "them" },
        needed.join(" "),
        lib_flag,
        rver_flag
    );
}

/// Whether `pkg` is installed somewhere the target library's R session would
/// find it: the library itself, or its R version's default library.
fn is_available(lib: &ResolvedLibrary, pkg: &str) -> bool {
    if has_installed_package(&lib.path, pkg) {
        return true;
    }
    if let Some(rver) = &lib.rversion {
        if let Ok(default_lib) = sc_library_get_default(rver) {
            if default_lib.path != lib.path && has_installed_package(&default_lib.path, pkg) {
                return true;
            }
        }
    }
    false
}

fn has_installed_package(lib_path: &Path, name: &str) -> bool {
    lib_path.join(name).join("DESCRIPTION").is_file()
}

/// Write the linked package's stub directory: `NAMESPACE` copied from the
/// source, a `DESCRIPTION` copy marked with [`RIG_LINK_FIELD`], the
/// `Meta/package.rds` R itself requires to accept the directory as an
/// installed package, and the `R/<pkgname>` loader script.
fn write_stub(
    target: &Path,
    source: &Path,
    name: &str,
    desc_text: &str,
    rversion: &str,
    has_compiled_code: bool,
) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(target.join("R"))?;
    fs::create_dir_all(target.join("Meta"))?;

    fs::copy(source.join("NAMESPACE"), target.join("NAMESPACE"))?;

    let description = build_description(desc_text, source);
    fs::write(target.join("DESCRIPTION"), &description)?;

    let package_rds = build_package_rds(&description, rversion)?;
    write_rds_file(&package_rds, &target.join("Meta").join("package.rds"))?;

    if has_compiled_code {
        let features = named_list([("internalsID", character_scalar(&internals_id(rversion)?))]);
        write_rds_file(&features, &target.join("Meta").join("features.rds"))?;
    }

    fs::write(
        target.join("R").join(name),
        build_stub_script(name, source, target, has_compiled_code),
    )?;

    Ok(())
}

/// The running-R-version-specific GUID `Meta/features.rds`'s `internalsID`
/// needs -- see the module doc comment for why a compiled linked package
/// needs this at all. Fetched by actually starting `rversion`'s R once,
/// there being no other way to learn it.
fn internals_id(rversion: &str) -> Result<String, Box<dyn Error>> {
    let r_binary = get_r_binary(rversion)?;
    let output = Command::new(&r_binary)
        .args([
            "--vanilla",
            "--slave",
            "-e",
            "cat(.Internal(internalsID()))",
        ])
        .output()
        .map_err(|err| format!("Cannot run {}: {}", r_binary.display(), err))?;
    if !output.status.success() {
        bail!(
            "Cannot determine R {}'s internals ID: {}",
            rversion,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let id = String::from_utf8(output.stdout)?.trim().to_string();
    if id.is_empty() {
        bail!("R {} reported an empty internals ID", rversion);
    }
    Ok(id)
}

/// The linked package's `DESCRIPTION`: the source's own, with any previous
/// `RigLink`/`Built`/`Packaged` fields dropped (a link is not a "built"
/// install) and a fresh `RigLink` recording the source path.
fn build_description(text: &str, source: &Path) -> String {
    let mut out = drop_fields(text, &[RIG_LINK_FIELD, "Built", "Packaged"]);
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&format!("{}: {}\n", RIG_LINK_FIELD, source.display()));
    out
}

/// `Meta/package.rds`, without which `library()` refuses the directory as
/// "not a valid installed package" (confirmed empirically). A real
/// `R CMD INSTALL` writes considerably more (`Rdepends`/`Suggests`/`Imports`/
/// `LinkingTo` breakdowns), but none of that turned out to be required for
/// `library()`, `installed.packages()` or `packageVersion()` to accept the
/// package. `Built$R` does need to be [`r_system_version`]'s classed shape,
/// not a plain string, though: `loadNamespace()` checks a compiled-code
/// package's `Built$R` more strictly, and a plain string there fails with
/// "installed by an R version with different internals" (confirmed
/// empirically too, and only for a package with a `libs/` directory).
fn build_package_rds(description_text: &str, rversion: &str) -> Result<RObject, Box<dyn Error>> {
    let desc = parse_dcf(description_text)?;
    let para = desc.iter().next().ok_or("empty DESCRIPTION file")?;
    let fields: Vec<(String, String)> = para
        .iter()
        .map(|(k, v)| (k.to_string(), reflow(v)))
        .collect();

    let built = named_list([
        ("R", r_system_version(rversion)),
        // Left empty, same as a source install R itself produces (see
        // `src/pkg/list.rs`'s `source_install_has_no_platform`): a linked
        // package is not platform-specific either.
        ("Platform", character_scalar("")),
        ("Date", character_scalar(&current_timestamp())),
        (
            "OStype",
            character_scalar(if cfg!(windows) { "windows" } else { "unix" }),
        ),
    ]);

    Ok(named_list([
        ("DESCRIPTION", named_character_vector(fields)),
        ("Built", built),
    ]))
}

/// The current time, as `Built$Date` fields read: `YYYY-MM-DD HH:MM:SS UTC`.
fn current_timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (y, m, d) = civil_from_days((secs / 86400) as i64);
    let rem = secs % 86400;
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        y,
        m,
        d,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's `civil_from_days`: the Gregorian calendar date for a
/// count of days since the Unix epoch. Public-domain algorithm, chosen over a
/// date/time dependency for a field nothing actually validates.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The `R/<pkgname>` loader script: see the module doc comment for why this
/// delegates to pkgload's internal `load_code()` rather than
/// `pkgload::load_all()`, and, for a package with compiled code, to
/// `pkgbuild::compile_dll()` -- see the module doc comment for why the
/// compiled object is then copied into `target`'s own `libs/` rather than
/// loaded directly.
fn build_stub_script(name: &str, source: &Path, target: &Path, has_compiled_code: bool) -> String {
    let source = escape_r_string(&source.display().to_string());
    let target = escape_r_string(&target.display().to_string());

    let pkgbuild_check = if has_compiled_code {
        format!(
            "if (!requireNamespace(\"pkgbuild\", quietly = TRUE)) {{\n\
             \x20\x20stop(\n\
             \x20\x20\x20\x20\"Package '{name}' is linked (via `rig pkg link`) to\\n\",\n\
             \x20\x20\x20\x20\"  {source}\\n\",\n\
             \x20\x20\x20\x20\"and has compiled code, but the 'pkgbuild' package is not \
installed. Install it with:\\n\",\n\
             \x20\x20\x20\x20\"  rig pkg install pkgbuild\",\n\
             \x20\x20\x20\x20call. = FALSE\n\
             \x20\x20)\n\
             }}\n"
        )
    } else {
        String::new()
    };

    // `compile_dll()` rebuilds `src/<pkg><.so|.dll>` in place, the same as
    // `pkgload::load_all()` -- see the module doc comment. The result is
    // copied (not loaded directly) into `target/libs/`, where R's own
    // `useDynLib` processing -- which runs after `R/<pkgname>` returns, driven
    // by the NAMESPACE copy already sitting in `target`, not by anything this
    // script does -- expects to find it and loads it itself, the same as
    // after an ordinary `R CMD INSTALL`.
    let compile_and_copy_dll = if has_compiled_code {
        format!(
            "\n\x20\x20\x20\x20message(\"Compiling linked package '{name}'...\")\n\
             \x20\x20\x20\x20pkgbuild::compile_dll(\"{source}\", quiet = TRUE)\n\
             \x20\x20\x20\x20dyn_ext <- .Platform$dynlib.ext\n\
             \x20\x20\x20\x20so_file <- file.path(\"{source}\", \"src\", paste0(\"{name}\", dyn_ext))\n\
             \x20\x20\x20\x20libs_dir <- file.path(\"{target}\", \"libs\")\n\
             \x20\x20\x20\x20dir.create(libs_dir, showWarnings = FALSE, recursive = TRUE)\n\
             \x20\x20\x20\x20file.copy(so_file, file.path(libs_dir, basename(so_file)), overwrite = TRUE)"
        )
    } else {
        String::new()
    };

    format!(
        "# Generated by `rig pkg link`. Do not edit -- edit the source package,\n\
         # or run `rig pkg unlink {name}` to remove this link.\n\
         if (!requireNamespace(\"pkgload\", quietly = TRUE)) {{\n\
         \x20\x20stop(\n\
         \x20\x20\x20\x20\"Package '{name}' is linked (via `rig pkg link`) to\\n\",\n\
         \x20\x20\x20\x20\"  {source}\\n\",\n\
         \x20\x20\x20\x20\"but the 'pkgload' package is not installed. Install it with:\\n\",\n\
         \x20\x20\x20\x20\"  rig pkg install pkgload\",\n\
         \x20\x20\x20\x20call. = FALSE\n\
         \x20\x20)\n\
         }}\n\
         {pkgbuild_check}\
         pkgload_ns <- asNamespace(\"pkgload\")\n\
         tryCatch(\n\
         \x20\x20{{\n\
         \x20\x20\x20\x20message(\"Loading linked package '{name}'...\")\n\
         \x20\x20\x20\x20pkgload_ns$load_code(\"{source}\", quiet = TRUE){compile_and_copy_dll}\n\
         \x20\x20}},\n\
         \x20\x20error = function(e) {{\n\
         \x20\x20\x20\x20stop(\n\
         \x20\x20\x20\x20\x20\x20\"Failed to load linked package '{name}' from\\n\",\n\
         \x20\x20\x20\x20\x20\x20\"  {source}\\n\",\n\
         \x20\x20\x20\x20\x20\x20\"This may mean your installed pkgload version is \
incompatible with `rig pkg link`.\\n\",\n\
         \x20\x20\x20\x20\x20\x20conditionMessage(e),\n\
         \x20\x20\x20\x20\x20\x20call. = FALSE\n\
         \x20\x20\x20\x20)\n\
         \x20\x20}}\n\
         )\n",
        name = name,
        source = source,
        pkgbuild_check = pkgbuild_check,
        compile_and_copy_dll = compile_and_copy_dll,
    )
}

/// Escape a path so it can sit inside a double-quoted R string literal.
fn escape_r_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn print_linked_json(
    name: &str,
    version: &str,
    source: &Path,
    lib: &ResolvedLibrary,
) -> Result<(), Box<dyn Error>> {
    #[derive(serde::Serialize)]
    struct LinkedEntry<'a> {
        package: &'a str,
        version: &'a str,
        source: String,
        library: String,
    }

    let entry = LinkedEntry {
        package: name,
        version,
        source: source.display().to_string(),
        library: lib.path.display().to_string(),
    };
    println!("{}", serde_json::to_string_pretty(&entry)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source_package(dir: &Path, name: &str) {
        fs::create_dir_all(dir.join("R")).unwrap();
        fs::write(
            dir.join("DESCRIPTION"),
            format!("Package: {}\nVersion: 0.0.1\n", name),
        )
        .unwrap();
        fs::write(dir.join("NAMESPACE"), "export(hello)\n").unwrap();
        fs::write(dir.join("R/hello.R"), "hello <- function() \"hi\"\n").unwrap();
    }

    #[test]
    fn build_description_adds_riglink_and_drops_built() {
        let source = Path::new("/some/source/pkg");
        let text = "Package: pkg\nVersion: 1.0.0\nBuilt: R 4.4.0; ; 2024; unix\n";
        let out = build_description(text, source);
        assert!(out.contains("Package: pkg\n"));
        assert!(!out.contains("Built:"));
        assert!(out.contains(&format!("RigLink: {}\n", source.display())));
    }

    #[test]
    fn build_description_replaces_existing_riglink() {
        let source = Path::new("/new/source");
        let text = "Package: pkg\nVersion: 1.0.0\nRigLink: /old/source\n";
        let out = build_description(text, source);
        assert_eq!(out.matches("RigLink:").count(), 1);
        assert!(out.contains("RigLink: /new/source\n"));
    }

    #[test]
    fn package_rds_round_trips_through_rds2rust() {
        let text = "Package: pkg\nVersion: 1.0.0\n";
        let obj = build_package_rds(text, "4.4.0").unwrap();
        let bytes = rds2rust::write_rds(&obj).unwrap();
        let parsed = rds2rust::read_rds(&bytes).unwrap().object;
        match parsed {
            RObject::WithAttributes { object, .. } => match *object {
                RObject::List(items) => assert_eq!(items.len(), 2),
                other => panic!("expected a list, got {:?}", other),
            },
            other => panic!("expected attributes, got {:?}", other),
        }
    }

    #[test]
    fn linking_writes_a_working_stub() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("src-pkg");
        source_package(&source, "mypkg");

        let target = tmp.path().join("lib").join("mypkg");
        let desc_text = fs::read_to_string(source.join("DESCRIPTION")).unwrap();
        write_stub(&target, &source, "mypkg", &desc_text, "4.4.0", false).unwrap();

        assert!(target.join("NAMESPACE").is_file());
        assert!(target.join("Meta/package.rds").is_file());
        let stub = fs::read_to_string(target.join("R/mypkg")).unwrap();
        assert!(stub.contains("pkgload_ns$load_code"));
        assert!(stub.contains(&source.display().to_string()));
        assert!(!stub.contains("compile_dll"));

        let desc = fs::read_to_string(target.join("DESCRIPTION")).unwrap();
        assert!(desc.contains(&format!("RigLink: {}", source.display())));
    }

    // `write_stub` is not exercised end-to-end for the compiled-code case
    // here: with `has_compiled_code`, it shells out to the target R version
    // to read its `internalsID` (see [`internals_id`]), which needs a real R
    // installation the test suite cannot rely on. `build_stub_script` -- the
    // part specific to compiled code -- is still tested directly.
    #[test]
    fn compiled_stub_script_adds_a_compile_step() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("src-pkg");
        let target = tmp.path().join("lib").join("mypkg");

        let stub = build_stub_script("mypkg", &source, &target, true);
        assert!(stub.contains("pkgbuild::compile_dll"));
        assert!(stub.contains(&target.display().to_string()));
        assert!(stub.contains("requireNamespace(\"pkgbuild\""));
    }

    #[test]
    fn has_entries_distinguishes_empty_from_nonempty() {
        let tmp = tempfile::tempdir().unwrap();
        let empty = tmp.path().join("empty");
        let nonempty = tmp.path().join("nonempty");
        fs::create_dir(&empty).unwrap();
        fs::create_dir(&nonempty).unwrap();
        fs::write(nonempty.join("a.c"), "").unwrap();

        assert!(!has_entries(&empty).unwrap());
        assert!(has_entries(&nonempty).unwrap());
    }
}
