#![cfg(target_os = "macos")]

use rand::Rng;
use std::error::Error;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::ArgMatches;
use log::{debug, error, info, warn};
use nix::sys::stat::umask;
use nix::sys::stat::Mode;
use nix::unistd::{access, AccessFlags};
use owo_colors::OwoColorize;
use path_clean::PathClean;
use regex::Regex;
use simple_error::*;

use crate::alias::*;
use crate::common::*;
use crate::download::*;
use crate::escalate::*;
use crate::library::*;
use crate::output::OUTPUT;
use crate::repos::*;
use crate::resolve::get_resolve;
use crate::run::*;
use crate::rversion::*;
use crate::utils::*;

pub const R_ROOT_: &str = "/Library/Frameworks/R.framework/Versions";
pub const R_VERSIONDIR: &str = "{}";

macro_rules! osvec {
    // match a list of expressions separated by comma:
    ($($str:expr),*) => ({
        // create a Vec with this list of expressions,
        // calling String::from on each:
        vec![$(OsString::from($str),)*] as Vec<OsString>
    });
}

// /Library/Frameworks/R.framework/Versions
// ~/.local/share/rig/r
pub fn get_r_root() -> Result<String, Box<dyn Error>> {
    if let Some(dir) = get_r_install_dir()? {
        return Ok(dir);
    }
    Ok(R_ROOT_.to_string())
}

pub fn get_r_root_for(_name: &str) -> Result<String, Box<dyn Error>> {
    get_r_root()
}

pub fn version_dir_key(name: &str) -> String {
    name.to_string()
}

pub fn get_r_syslibpath() -> Result<String, Box<dyn Error>> {
    if get_mode()? == crate::utils::Mode::User {
        return Ok("{}/library".to_string());
    }
    Ok("{}/Resources/library".to_string())
}

pub fn get_r_binpath() -> Result<String, Box<dyn Error>> {
    if get_mode()? == crate::utils::Mode::User {
        return Ok("{}/bin/R".to_string());
    }
    Ok("{}/Resources/bin/R".to_string())
}

fn get_r_exec_binpath() -> Result<String, Box<dyn Error>> {
    if get_mode()? == crate::utils::Mode::User {
        return Ok("{}/bin/exec/R".to_string());
    }
    Ok("{}/Resources/bin/exec/R".to_string())
}

pub fn get_r_default_bindir() -> Result<String, Box<dyn Error>> {
    if get_mode()? == crate::utils::Mode::User {
        return Ok(get_r_root()? + "/Current/bin");
    }
    Ok(get_r_root()? + "/Current/Resources/bin")
}

pub fn get_r_base_profile() -> Result<String, Box<dyn Error>> {
    if get_mode()? == crate::utils::Mode::User {
        return Ok("{}/library/base/R/Rprofile".to_string());
    }
    Ok("{}/Resources/library/base/R/Rprofile".to_string())
}

pub fn get_r_etc_path() -> Result<String, Box<dyn Error>> {
    if get_mode()? == crate::utils::Mode::User {
        return Ok("{}/etc".to_string());
    }
    Ok("{}/Resources/etc".to_string())
}

pub fn get_r_current() -> Result<String, Box<dyn Error>> {
    if let Some(dir) = get_r_install_dir()? {
        return Ok(format!("{}/Current", dir));
    }
    Ok("/Library/Frameworks/R.framework/Versions/Current".to_string())
}

pub fn sc_add(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    if get_mode()? == crate::utils::Mode::Admin {
        escalate("adding new R versions")?;
    }
    let mut version = get_resolve(args)?;
    let alias = get_alias(args);
    let ver = version.version.to_owned();
    let verstr = match ver {
        Some(ref x) => x,
        None => "???",
    };
    let url: String = match &version.url {
        Some(s) => s.to_string(),
        None => {
            let archarg: &String = args.get_one("arch").unwrap();
            OUTPUT.error(&format!(
                "Cannot find a download url for R version {}, {}",
                verstr, archarg
            ));
            error!(
                "Cannot find a download url for R version {}, {}",
                verstr, archarg
            );
            bail!(
                "Cannot find a download url for R version {}, {}",
                verstr,
                archarg
            );
        }
    };
    let arch = version.arch.to_owned();
    let prefix = match arch {
        Some(ref x) => x.to_owned(),
        None => calculate_hash(&url),
    };
    let filename = prefix + "-" + basename(&url).unwrap_or("foo");
    let tmp_dir = std::env::temp_dir().join("rig");
    let target = tmp_dir.join(&filename);
    let cache = target.exists() && not_too_old(&target);
    let target_str = target.to_owned().into_os_string();
    let target_dsp = target.display();
    if cache {
        OUTPUT.success(&format!("{} is cached at {}", filename, target_dsp));
        info!("{} is cached at {}", filename, target_dsp);
    } else {
        OUTPUT.status(&format!("Downloading {} -> {}", url, target_dsp));
        info!("Downloading {} -> {}", url, target_dsp);
        let client = &reqwest::Client::new();
        download_file(client, &url, &target_str)?;
    }

    sc_system_forget()?;

    // If installed from URL, then we'll use the version in the file
    let fver = extract_pkg_version(&target_str)?;

    let mode = get_mode()?;

    match ver {
        Some(_) => {}
        None => {
            version.version = Some(fver.version.clone());
            version.arch = Some(fver.arch.clone());
        }
    };

    // Install without changing default. In user mode the installation
    // directory is named after the full version number (or `devel`/`next`
    // for development builds), so the name is determined while unpacking the
    // framework and returned by `safe_user_install`.
    let dirname = if mode == crate::utils::Mode::User {
        let install_dir = std::path::PathBuf::from(get_r_root()?);
        let dirname = safe_user_install(target, &fver, install_dir)?;
        if let Err(e) = ensure_positron_custom_root_folders() {
            OUTPUT.warn(&format!("Could not update Positron settings: {}", e));
            warn!("Could not update Positron settings: {}", e);
        }
        dirname
    } else {
        let dirname = fver.installdir.clone();
        safe_install(target, &dirname, arch)?;
        dirname
    };

    // This should not happen currently on macOS, a .pkg installer
    // sets the default, but prepare for the future
    set_default_if_none(dirname.to_string())?;

    sc_system_forget()?;
    system_no_openmp(Some(vec![dirname.to_string()]))?;
    system_fix_permissions(Some(vec![dirname.to_string()]))?;
    library_update_rprofile(&dirname.to_string())?;
    sc_system_make_links()?;
    match alias {
        // The `release`/`oldrel` aliases point at the native build. An
        // x86_64 build on an arm64 machine gets an `-x86_64` suffix instead,
        // to avoid colliding with the native alias.
        Some(alias) => {
            let alias = if fver.arch == "x86_64" && is_arm64_machine() {
                format!("{}-x86_64", alias)
            } else {
                alias
            };
            add_alias(&dirname, &alias)?
        }
        None => {}
    };

    let setup = interpret_repos_args(args, true);
    repos_setup(Some(vec![dirname.to_string()]), setup)?;

    if !args.get_flag("without-pak") {
        let pakver: &String = args.get_one("pak-version").unwrap();
        let explicit =
            args.value_source("pak-version") == Some(clap::parser::ValueSource::CommandLine);

        system_add_pak(Some(vec![dirname.to_string()]), pakver, explicit)?;
    }

    Ok(())
}

fn random_string() -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ\
                            abcdefghijklmnopqrstuvwxyz";
    const PASSWORD_LEN: usize = 10;
    let mut rng = rand::thread_rng();

    let password: String = (0..PASSWORD_LEN)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect();

    password
}

fn unpack_and_patch(
    target: &Path,
) -> Result<(PathBuf, PathBuf), Box<dyn Error>> {
    let dir = target.parent().ok_or(SimpleError::new("Internal error"))?;
    let tmp = dir.join(random_string());

    let output = Command::new("pkgutil")
        .arg("--expand")
        .arg(target)
        .arg(&tmp)
        .output()?;
    if !output.status.success() {
        OUTPUT.error(&format!(
            "Failed to expand installer with pkgutil: {}",
            output.status.to_string()
        ));
        error!(
            "Failed to expand installer with pkgutil: {}",
            output.status.to_string()
        );
        bail!("pkgutil exited with {}", output.status.to_string());
    }

    let wd1 = tmp.join("r.pkg");
    let wd2 = tmp.join("R-fw.pkg");
    let wd = if wd2.exists() {
        wd2
    } else if wd1.exists() {
        wd1
    } else {
        OUTPUT.error("Failed to patch installer, could not find framework");
        error!("Failed to patch installer, could not find framework");
        bail!("Failed to patch installer, could not find framework");
    };

    let output = Command::new("sh")
        .current_dir(&wd)
        .args(["-c", "gzip -dcf Payload | cpio -i"])
        .output()?;
    if !output.status.success() {
        let err = output.status.to_string();
        OUTPUT.error(&format!("Failed to extract installer: {}", err));
        error!("Failed to extract installer: {}", err);
        bail!("Failed to extract installer: {}", err);
    }

    Ok((tmp, wd))
}

fn run_fc_cache(fc_cache: &Path) {
    if !fc_cache.exists() {
        debug!("Skipping fc-cache; {} does not exist", fc_cache.display());
        return;
    }
    debug!("Running {}", fc_cache.display());
    match Command::new(fc_cache).output() {
        Err(err) => {
            OUTPUT.warn(&format!(
                "Failed to run {}: {}",
                fc_cache.display(),
                err.to_string()
            ));
            warn!("Failed to run {}: {}", fc_cache.display(), err.to_string());
        }
        Ok(output) if !output.status.success() => {
            OUTPUT.warn(&format!(
                "{} exited with {}",
                fc_cache.display(),
                output.status
            ));
            warn!("{} exited with {}", fc_cache.display(), output.status);
        }
        Ok(_) => {}
    }
}

fn safe_install(
    target: std::path::PathBuf,
    ver: &str,
    arch: Option<String>,
) -> Result<(), Box<dyn Error>> {
    let (tmp, wd) = unpack_and_patch(&target)?;
    let link: PathBuf = wd.join("R.framework").join("Versions").join("Current");
    make_orthogonal_(&link, ver)?;
    std::fs::remove_file(&link)?;

    let dir = tmp.parent().ok_or(SimpleError::new("Internal error"))?;

    let output = Command::new("sh")
        .current_dir(&wd)
        .args(["-c", "find R.framework | cpio -oz > Payload"])
        .output()?;
    if !output.status.success() {
        let err = output.status.to_string();
        OUTPUT.error(&format!("Failed to re-package installer (cpio): {}", err));
        error!("Failed to re-package installer (cpio): {}", err);
        bail!("Failed to re-package installer (cpio): {}", err);
    }

    let rf = wd.join("R.framework");
    std::fs::remove_dir_all(&rf)?;

    let pkgf = random_string() + ".pkg";
    let pkg = dir.join(pkgf);
    let output = Command::new("pkgutil")
        .arg("--flatten")
        .arg(&tmp)
        .arg(&pkg)
        .output()?;
    if !output.status.success() {
        let err = output.status.to_string();
        OUTPUT.error(&format!(
            "Failed to re-package installer (pkgutil): {}",
            err
        ));
        error!("Failed to re-package installer (pkgutil): {}", err);
        bail!("Failed to re-package installer (pkgutil): {}", err);
    }

    let mut cmd: OsString = os("installer");
    let mut args: Vec<OsString> = vec![];

    match arch {
        Some(arch) => {
            if arch == "arm64" {
                cmd = os("arch");
                args = vec![os("-arm64"), os("installer")];
            }
        }
        None => {}
    };

    args.push(os("-pkg"));
    args.push(pkg.to_owned().into_os_string());
    args.push(os("-target"));
    args.push(os("/"));

    OUTPUT.status("Running installer");
    info!("Running installer");
    run(cmd.into(), args, "installer")?;

    let fc_cache = Path::new(R_ROOT_).join(ver).join("Resources").join("bin").join("fc-cache");
    run_fc_cache(&fc_cache);

    if let Err(err) = std::fs::remove_file(&pkg) {
        OUTPUT.warn(&format!(
            "Failed to remove temporary installer {}: {}",
            pkg.display(),
            err.to_string()
        ));
        warn!(
            "Failed to remove temporary file {}: {}",
            pkg.display(),
            err.to_string()
        );
    }
    if let Err(err) = std::fs::remove_dir_all(&tmp) {
        OUTPUT.warn(&format!(
            "Failed to remove temporary directory {}: {}",
            tmp.display(),
            err.to_string()
        ));
        warn!(
            "Failed to remove temporary directory {}: {}",
            tmp.display(),
            err.to_string()
        );
    }

    Ok(())
}

fn patch_user_r_script(source_dir: &Path, home_dir: &Path) -> Result<(), Box<dyn Error>> {
    let rfile = source_dir.join("bin").join("R");
    let re = Regex::new(r"(?m)^R_HOME_DIR=.*$")?;
    // Escape `$` so it survives the regex-crate replacement expansion.
    let home_escaped = home_dir.display().to_string().replace('$', "$$");
    let sub = format!(
        "R_HOME_DIR={}\n\
         R_HOME_DIR=$$(cd \"$$(dirname \"$$(realpath \"$$0\")\")/..\" && pwd -P)\n\
         export DYLD_LIBRARY_PATH=\"$${{R_HOME_DIR}}/lib\"",
        home_escaped
    );
    debug!("Patching R_HOME_DIR in {}", rfile.display());
    replace_in_file(&rfile, &re, &sub)?;

    let re = Regex::new(r"/Library/Frameworks/R\.framework/Resources")?;
    debug!("Patching framework references in {}", rfile.display());
    replace_in_file(&rfile, &re, "$${R_HOME}")?;

    Ok(())
}

fn patch_user_scripts(source_dir: &Path, home_dir: &Path) -> Result<(), Box<dyn Error>> {
    // Escape `$` so it survives the regex-crate replacement expansion.
    let home_escaped = home_dir.display().to_string().replace('$', "$$");

    let makeconf = source_dir.join("etc").join("Makeconf");
    let re = Regex::new(r"(?m)^LIBR\s*=.*$")?;
    // `$$` so the regex replacement emits a literal `$` for `$(R_HOME)`.
    let sub = "LIBR = -L\"$$(R_HOME)/lib\" -lR";
    debug!("Patching LIBR in {}", makeconf.display());
    replace_in_file(&makeconf, &re, sub)?;

    let renviron = source_dir.join("etc").join("Renviron");
    let re = Regex::new(r"(?m)^R_QPDF=.*$")?;
    // R does not expand $R_HOME when reading Renviron, so bake home_dir in.
    // `$$` for the `${R_QPDF-...}` shell-style fallback expansion.
    let sub = format!("R_QPDF=$${{R_QPDF-{}/bin/qpdf}}", home_escaped);
    debug!("Patching R_QPDF in {}", renviron.display());
    replace_in_file(&renviron, &re, &sub)?;

    let fonts = source_dir.join("fontconfig").join("fonts").join("fonts.conf");
    if fonts.exists() {
        let re = Regex::new(r"/Library/Frameworks/R\.framework/Resources")?;
        debug!("Patching fontconfig in {}", fonts.display());
        replace_in_file(&fonts, &re, &home_escaped)?;
    } else {
        debug!("Skipping fonts.conf patch; {} does not exist", fonts.display());
    }

    let libpc = source_dir.join("lib").join("pkgconfig").join("libR.pc");
    if libpc.exists() {
        // Do `rincludedir` first so the next pass doesn't rewrite the framework
        // path inside it before we replace the whole line.
        let re = Regex::new(r"(?m)^rincludedir=.*$")?;
        let sub = "rincludedir=$${rhome}/include";
        debug!("Patching rincludedir in {}", libpc.display());
        replace_in_file(&libpc, &re, sub)?;
        let re = Regex::new(r"/Library/Frameworks/R\.framework/Versions/[^/]+/Resources")?;
        debug!("Patching framework path in {}", libpc.display());
        replace_in_file(&libpc, &re, &home_escaped)?;
    } else {
        debug!("Skipping libR.pc patch; {} does not exist", libpc.display());
    }

    Ok(())
}

fn replace_user_rscript(source_dir: &Path) -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::PermissionsExt;

    // (path-to-Rscript, shell expression for RHOME relative to "$0")
    let scripts: [(PathBuf, &str); 2] = [
        (
            source_dir.join("bin").join("Rscript"),
            "$(cd \"$(dirname \"$(realpath \"$0\")\")/..\" && pwd -P)",
        ),
        (
            source_dir.join("Rscript"),
            "$(cd \"$(dirname \"$(realpath \"$0\")\")\" && pwd -P)",
        ),
    ];

    for (rscript, rhome_expr) in &scripts {
        let rscript_orig = rscript.with_file_name("Rscript.orig");
        debug!("Renaming {} to {}", rscript.display(), rscript_orig.display());
        std::fs::rename(rscript, &rscript_orig)?;

        let content = format!(
            "#!/bin/sh\n\
             RHOME={}\n\
             export RHOME\n\
             exec \"$(dirname \"$(realpath \"$0\")\")/Rscript.orig\" \"$@\"\n",
            rhome_expr
        );
        debug!("Writing wrapper Rscript to {}", rscript.display());
        std::fs::write(rscript, content)?;
        let mut perms = std::fs::metadata(rscript)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(rscript, perms)?;
    }

    Ok(())
}

fn replace_user_fontconfig(source_dir: &Path) -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::PermissionsExt;

    let fc_cache = source_dir.join("bin").join("fc-cache");
    if !fc_cache.exists() {
        debug!("Skipping fc-cache wrapper; {} does not exist", fc_cache.display());
        return Ok(());
    }
    let fc_cache_orig = fc_cache.with_file_name("fc-cache.orig");
    debug!("Copying {} to {}", fc_cache.display(), fc_cache_orig.display());
    std::fs::copy(&fc_cache, &fc_cache_orig)?;

    let content = "#!/bin/sh\n\
                   RHOME=$(cd \"$(dirname \"$(realpath \"$0\")\")/..\" && pwd -P)\n\
                   FONTCONFIG_FILE=\"$RHOME/fontconfig/fonts/fonts.conf\"\n\
                   export FONTCONFIG_FILE\n\
                   exec \"$(dirname \"$(realpath \"$0\")\")/fc-cache.orig\" \"$@\"\n";
    debug!("Writing wrapper fc-cache to {}", fc_cache.display());
    std::fs::write(&fc_cache, content)?;
    let mut perms = std::fs::metadata(&fc_cache)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&fc_cache, perms)?;

    Ok(())
}

// These are included on older R and shadow system libraries and cause crashes
fn remove_user_dyld_shadows(source_dir: &Path) -> Result<(), Box<dyn Error>> {
    let lib = source_dir.join("lib");
    let shadows = ["libc++.1.dylib", "libc++abi.1.dylib", "libunwind.1.dylib"];
    for name in shadows {
        let path = lib.join(name);
        match std::fs::remove_file(&path) {
            Ok(()) => debug!("Removed system-shadowing dylib {}", path.display()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                debug!("No system-shadowing dylib at {}", path.display());
            }
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}

fn safe_user_install(
    target: std::path::PathBuf,
    fver: &RversionDir,
    install_dir: std::path::PathBuf,
) -> Result<String, Box<dyn Error>> {
    let (tmp, wd) = unpack_and_patch(&target)?;
    let source_dir = wd.join("R.framework")
        .join("Versions")
        .join("Current")
        .join("Resources");

    let dirname = user_install_dirname(&source_dir, fver)?;
    let target_dir = install_dir.join(&dirname);
    patch_user_r_script(&source_dir, &target_dir)?;
    patch_user_scripts(&source_dir, &target_dir)?;
    replace_user_rscript(&source_dir)?;
    replace_user_fontconfig(&source_dir)?;
    remove_user_dyld_shadows(&source_dir)?;

    debug!("Copying {} to {}", source_dir.display(), target_dir.display());
    let output = Command::new("ditto")
        .arg(&source_dir)
        .arg(&target_dir)
        .output()?;
    if !output.status.success() {
        let err = output.status.to_string();
        OUTPUT.error(&format!("Failed to copy R framework: {}", err));
        error!("Failed to copy R framework: {}", err);
        bail!("Failed to copy R framework: {}", err);
    }

    // Positron tries to start Resources/bin/R
    let resources_link = target_dir.join("Resources");
    if !resources_link.exists() {
        debug!("Creating Resources -> . symlink in {}", target_dir.display());
        symlink(".", &resources_link)?;
    }

    let fc_cache = target_dir.join("bin").join("fc-cache");
    run_fc_cache(&fc_cache);

    if let Err(err) = std::fs::remove_dir_all(&tmp) {
        OUTPUT.warn(&format!(
            "Failed to remove temporary directory {}: {}",
            tmp.display(),
            err.to_string()
        ));
        warn!(
            "Failed to remove temporary directory {}: {}",
            tmp.display(),
            err.to_string()
        );
    }

    Ok(dirname)
}

// Determine the user-mode installation directory name. Normal releases are
// named after their full version number. Development builds are named `devel`
// (R-devel) or `next` (R-next), as recorded by the `R_STATUS` macro in
// `include/Rversion.h`. An x86_64 build installed on an arm64 machine gets a
// `-x86_64` suffix to avoid colliding with the native build.
fn user_install_dirname(
    source_dir: &Path,
    fver: &RversionDir,
) -> Result<String, Box<dyn Error>> {
    let status = read_r_status(source_dir)?;
    let base = match status.as_str() {
        "" => fver.version.clone(),
        "Under development (unstable)" => "devel".to_string(),
        _ => "next".to_string(),
    };

    let dirname = if fver.arch == "x86_64" && is_arm64_machine() {
        format!("{}-x86_64", base)
    } else {
        base
    };

    debug!(
        "User install directory name is {} (R_STATUS = {:?})",
        dirname, status
    );

    Ok(dirname)
}

// Read the value of the `R_STATUS` macro from `include/Rversion.h`. It is an
// empty string for released versions, "Under development (unstable)" for
// R-devel, and something else (e.g. a patched/prerelease label) for R-next.
fn read_r_status(source_dir: &Path) -> Result<String, Box<dyn Error>> {
    let path = source_dir.join("include").join("Rversion.h");
    let content = std::fs::read_to_string(&path)?;
    let re = Regex::new(r#"(?m)^\s*#define\s+R_STATUS\s+"(.*)"\s*$"#)?;
    let status = re
        .captures(&content)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_default();
    Ok(status)
}

pub fn sc_rm(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    if get_mode()? == crate::utils::Mode::Admin {
        escalate("removing R versions")?;
    }
    let vers = args.get_many::<String>("version");
    if vers.is_none() {
        return Ok(());
    }
    let vers = vers.ok_or(SimpleError::new("Internal argument error"))?;
    let default = sc_get_default()?;

    for ver in vers {
        let ver = check_installed(&ver.to_string())?;

        if let Some(ref default) = default {
            if default == &ver {
                OUTPUT.warn(&format!(
                    "Removing default version {}, set new default with {}",
                    ver,
                    "rig default <version>".bold(),
                ));
                warn!(
                    "Removing default version, set new default with {}",
                    "rig default <version>".bold()
                );
            }
        }

        let rroot = get_r_root()?;
        let dir = Path::new(&rroot);
        let dir = dir.join(&ver);
        OUTPUT.status(&format!("Removing {}", dir.display()));
        info!("Removing {}", dir.display());
        sc_system_forget()?;
        match std::fs::remove_dir_all(&dir) {
            Err(err) => {
                OUTPUT.error(&format!(
                    "Cannot remove {}: {}",
                    dir.display(),
                    err.to_string()
                ));
                error!("Cannot remove {}: {}", dir.display(), err.to_string());
                bail!("Cannot remove {}: {}", dir.display(), err.to_string())
            }
            _ => {}
        };
    }

    sc_system_make_links()?;

    if get_mode()? == crate::utils::Mode::User && sc_get_list()?.is_empty() {
        if let Err(e) = remove_rstudio_which_r_plist() {
            OUTPUT.warn(&format!("Could not remove RSTUDIO_WHICH_R LaunchAgent: {}", e));
            warn!("Could not remove RSTUDIO_WHICH_R LaunchAgent: {}", e);
        }
    }

    Ok(())
}

pub fn sc_system_make_links() -> Result<(), Box<dyn Error>> {
    let binary_dir = get_binary_dir()?;
    let mode = get_mode()?;
    if mode == crate::utils::Mode::Admin &&
        access(binary_dir.as_str(), AccessFlags::W_OK).is_err()
    {
        escalate("making R-* quick links")?;
    }
    check_local_bin_path()?;
    let vers = sc_get_list()?;
    let rroot = get_r_root()?;
    let base = Path::new(&rroot);

    OUTPUT.status("Updating R-* quick links (as needed)");
    info!("Updating R-* quick links (as needed)");

    // https://github.com/r-lib/rig/issues/197
    let old_umask = umask(Mode::from_bits(0o022).unwrap());

    let binpath = if mode == crate::utils::Mode::Admin {
        "Resources/bin/R"
    } else {
        "bin/R"
    };

    // Create new links
    debug!("Creating quick links for installed versions");
    for ver in vers {
        if mode == crate::utils::Mode::Admin && !is_orthogonal(&ver)? {
            OUTPUT.warn(&format!(
                "Refusing to create quick link for non-orthogonal R version: {}.\n Call `rig system make-orthogonal` to fix this.",
                ver
            ));
            warn!(
              "Refusing to create quick link for non-orthogonal R version: {}.\n Call `rig system make-orthogonal` to fix this.",
              ver
            );
            continue;
        }
        let linkfile = Path::new(&binary_dir).join("R-".to_string() + &ver);
        let target = base.join(&ver).join(binpath);
        if !linkfile.exists() {
            debug!("Adding {} -> {}", linkfile.display(), target.display());
            match symlink(&target, &linkfile) {
                Err(err) => {
                    umask(old_umask);
                    OUTPUT.error(&format!(
                        "Cannot create symlink {}: {}",
                        linkfile.display(),
                        err.to_string()
                    ));
                    error!(
                        "Cannot create symlink {}: {}",
                        linkfile.display(),
                        err.to_string()
                    );
                    bail!(
                        "Cannot create symlink {}: {}",
                        linkfile.display(),
                        err.to_string()
                    )
                }
                _ => {}
            };
        }
    }
    umask(old_umask);

    // Remove dangling links
    debug!("Cleaning up dangling quick links");
    let paths = std::fs::read_dir(&binary_dir)?;
    let re = Regex::new("^R-[0-9]+[.][0-9]+")?;
    let re2 = re_alias();
    for file in paths {
        let path = file?.path();
        // If no path name, then path ends with ..., so we can skip
        let fnamestr = match path.file_name() {
            Some(x) => x,
            None => continue,
        };
        // If the path is not UTF-8, we'll skip it, this should not happen
        let fnamestr = match fnamestr.to_str() {
            Some(x) => x,
            None => continue,
        };
        if re.is_match(&fnamestr) || re2.is_match(&fnamestr) {
            match std::fs::read_link(&path) {
                Err(_) => debug!("{} is not a symlink", path.display()),
                Ok(target) => {
                    if !target.exists() {
                        debug!("Cleaning up {}", target.display());
                        match std::fs::remove_file(&path) {
                            Err(err) => {
                                OUTPUT.warn(&format!(
                                    "Failed to remove {}: {}",
                                    path.display(),
                                    err.to_string()
                                ));
                                warn!("Failed to remove {}: {}", path.display(), err.to_string())
                            }
                            _ => {}
                        }
                    }
                }
            };
        }
    }

    Ok(())
}

pub fn re_alias() -> Regex {
    let re = Regex::new("^R-(next|devel|release|release-x86_64|oldrel|oldrel-x86_64)$").unwrap();
    re
}

pub fn find_aliases() -> Result<Vec<Alias>, Box<dyn Error>> {
    debug!("Finding existing aliases");

    let binary_dir = get_binary_dir()?;
    let paths = std::fs::read_dir(&binary_dir)?;
    let re = re_alias();
    let mut result: Vec<Alias> = vec![];

    for file in paths {
        let path = file?.path();
        // If no path name, then path ends with ..., so we can skip
        let fnamestr = match path.file_name() {
            Some(x) => x,
            None => continue,
        };
        // If the path is not UTF-8, we'll skip it, this should not happen
        let fnamestr = match fnamestr.to_str() {
            Some(x) => x,
            None => continue,
        };
        if re.is_match(&fnamestr) {
            match std::fs::read_link(&path) {
                Err(_) => debug!("{} is not a symlink", path.display()),
                Ok(target) => {
                    if !target.exists() {
                        debug!("Target does not exist at {}", target.display());
                    } else {
                        let version = version_from_link(target);
                        match version {
                            None => continue,
                            Some(version) => {
                                let als = Alias {
                                    alias: fnamestr[2..].to_string(),
                                    version: version.to_string(),
                                };
                                result.push(als);
                            }
                        };
                    }
                }
            };
        }
    }

    Ok(result)
}

// /Library/Frameworks/R.framework/Versions/4.2-arm64/Resources/bin/R ->
// 4.2-arm64
fn version_from_link(pb: PathBuf) -> Option<String> {
    // Strip the trailing /bin/R
    let p = match pb.parent().and_then(|x| x.parent()) {
        None => {
            debug!("Path {} is too short to contain /bin/R", pb.display());
            return None;
        }
        Some(p) => p,
    };

    // Strip a trailing Resources directory, if present
    let p = if p.file_name().and_then(|s| s.to_str()) == Some("Resources") {
        match p.parent() {
            None => {
                debug!("Path {} has no parent above Resources", pb.display());
                return None;
            }
            Some(p) => p,
        }
    } else {
        p
    };

    // The last remaining directory is the version
    let ver = match p.file_name() {
        None => {
            debug!("Cannot extract version directory from {}", pb.display());
            return None;
        }
        Some(s) => s.to_os_string(),
    };

    match ver.into_string() {
        Ok(s) => Some(s),
        Err(_) => {
            debug!("Version directory in {} is not valid UTF-8", pb.display());
            None
        }
    }
}

pub fn sc_system_allow_core_dumps(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    if get_mode()? == crate::utils::Mode::Admin {
        escalate("updating code signature of R and /cores permissions")?;
    }
    sc_system_allow_debugger(args)?;
    OUTPUT.status("Updating permissions of /cores");
    info!("Updating permissions of /cores");
    Command::new("chmod").args(["1777", "/cores"]).output()?;
    Ok(())
}

pub fn sc_system_allow_debugger(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    if get_mode()? == crate::utils::Mode::Admin {
        escalate("updating code signature of R")?;
    }
    let all = args.get_flag("all");
    let vers = args.get_many::<String>("version");

    let vers: Vec<String> = if all {
        sc_get_list()?
    } else if vers.is_none() {
        vec![sc_get_default_or_fail()?]
    } else {
        vers.ok_or(SimpleError::new("Internal argument error"))?
            .map(|v| v.to_string())
            .collect()
    };

    for ver in vers {
        let ver = check_installed(&ver)?;
        let path = PathBuf::new()
            .join(get_r_root()?)
            .join(get_r_exec_binpath()?.replace("{}", &ver));
        update_entitlements(path)?;
    }

    Ok(())
}

pub fn sc_system_allow_debugger_rstudio(_args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    if !is_rstudio_installed() {
        OUTPUT.error("RStudio is not installed, at least not in /Applications/RStudio.app");
        error!("RStudio is not installed, at least not in /Applications/RStudio.app");
        bail!("RStudio is not installed, at least not in /Applications/RStudio.app");
    }

    let rsess = PathBuf::new().join("/Applications/RStudio.app/Contents/MacOS/rsession");
    update_entitlements(rsess)?;

    let rsessarm64 = PathBuf::new().join("/Applications/RStudio.app/Contents/MacOS/rsession-arm64");

    if rsessarm64.exists() {
        update_entitlements(rsessarm64)?;
    }

    Ok(())
}

pub fn update_entitlements(path: PathBuf) -> Result<(), Box<dyn Error>> {
    let tmp_dir = std::env::temp_dir().join("rig");
    match std::fs::create_dir_all(&tmp_dir) {
        Err(err) => {
            let dir = tmp_dir.to_str().unwrap_or_else(|| "???");
            OUTPUT.error(&format!(
                "Cannot create temporary directory {}: {}",
                dir,
                err.to_string()
            ));
            error!(
                "Cannot create temporary directory {}: {}",
                dir,
                err.to_string()
            );
            bail!(
                "Cannot craete temporary file in {}: {}",
                dir,
                err.to_string()
            );
        }
        _ => {}
    };

    OUTPUT.status(&format!("Updating entitlements of {}", path.display()));
    info!("Updating entitlements of {}", path.display());

    let out = Command::new("codesign")
        .args(["-d", "--entitlements", ":-"])
        .arg(&path)
        .output()?;
    if !out.status.success() {
        let stderr = match std::str::from_utf8(&out.stderr) {
            Ok(v) => v,
            Err(e) => {
                OUTPUT.error(&format!("Invalid UTF-8 output from codesign: {}", e));
                error!("Invalid UTF-8 output from codesign: {}", e);
                bail!("Invalid UTF-8 output from codesign: {}", e)
            }
        };
        if stderr.contains("is not signed") {
            OUTPUT.status(&format!("{} is not signed.", path.display()));
            info!("{} is not signed.", path.display());
        } else {
            OUTPUT.error(&format!("Cannot query entitlements:\n   {}", stderr));
            error!("Cannot query entitlements:\n   {}", stderr);
            bail!("Cannot query entitlements:\n   {}", stderr);
        }
        return Ok(());
    }

    let titles = tmp_dir.join("r.entitlements");
    std::fs::write(&titles, out.stdout)?;

    let out = Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Add :com.apple.security.get-task-allow bool true"])
        .arg(&titles)
        .output()?;

    if !out.status.success() {
        let stderr = match std::str::from_utf8(&out.stderr) {
            Ok(v) => v,
            Err(e) => {
                OUTPUT.error(&format!("Invalid UTF-8 output from codesign: {}", e));
                error!("Invalid UTF-8 output from codesign: {}", e);
                bail!("Invalid UTF-8 output from codesign: {}", e)
            }
        };
        if stderr.contains("Entry Already Exists") {
            OUTPUT.status(&format!("{} already allows debugging", path.display()));
            info!("{} already allows debugging", path.display());
            return Ok(());
        } else if stderr.contains("zero-length data") {
            OUTPUT.status(&format!("{} is not signed.", path.display()));
            info!("{} is not signed.", path.display());
            return Ok(());
        } else {
            OUTPUT.error(&format!("Cannot update entitlements: {}", stderr));
            error!("Cannot update entitlements: {}", stderr);
            bail!("Cannot update entitlements: {}", stderr);
        }
    }

    let out = Command::new("codesign")
        .args(["-s", "-", "-f", "--entitlements"])
        .arg(&titles)
        .arg(&path)
        .output()?;

    if !out.status.success() {
        OUTPUT.error(&format!("Cannot update entitlements"));
        error!("Cannot update entitlements");
        bail!("Cannot update entitlements");
    } else {
        OUTPUT.success(&format!("Updated entitlements of {}", path.display()));
        info!("Updated entitlements of {}", path.display());
    }

    Ok(())
}

pub fn sc_system_make_orthogonal(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    if get_mode()? == crate::utils::Mode::User {
        return Ok(());
    }
    escalate("updating the R installations")?;
    let vers = args.get_many::<String>("version");
    if vers.is_none() {
        system_make_orthogonal(None)
    } else {
        let vers: Vec<String> = vers
            .ok_or(SimpleError::new("Internal argument error"))?
            .map(|v| v.to_string())
            .collect();
        system_make_orthogonal(Some(vers))
    }
}

fn system_make_orthogonal(vers: Option<Vec<String>>) -> Result<(), Box<dyn Error>> {
    let vers = match vers {
        Some(x) => {
            let str = x.join(", ");
            OUTPUT.status(&format!(
                "Making R version{} {} orthogonal",
                if x.len() > 1 { "s" } else { "" },
                str
            ));
            info!(
                "Making R version{} {} orthogonal",
                if x.len() > 1 { "s" } else { "" },
                str
            );
            x
        }
        None => {
            OUTPUT.status("Making all R versions orthogonal");
            info!("Making all R versions orthogonal");
            sc_get_list()?
        }
    };

    for ver in vers {
        let ver = check_installed(&ver)?;
        debug!("Making R {} orthogonal", ver);
        let base = Path::new(&get_r_root()?).join(&ver);
        make_orthogonal_(&base, &ver)?;
    }

    Ok(())
}

fn is_orthogonal(ver: &str) -> Result<bool, Box<dyn Error>> {
    if get_mode()? == crate::utils::Mode::User {
        return Ok(true);
    }
    let base = Path::new(&get_r_root()?).join(&ver);
    let re = Regex::new("R[.]framework/Resources")?;
    let rfile = base.join("Resources/bin/R");
    let lines = read_lines(&rfile)?;
    let mch = grep_lines(&re, &lines);
    Ok(mch.len() == 0)
}

fn make_orthogonal_(base: &Path, ver: &str) -> Result<(), Box<dyn Error>> {
    let re = Regex::new("R[.]framework/Resources")?;
    let re2 = Regex::new("[-]F/Library/Frameworks/R[.]framework/[.][.]")?;

    let sub = "R.framework/Versions/".to_string() + &ver + "/Resources";

    let rfile = base.join("Resources/bin/R");
    replace_in_file(&rfile, &re, &sub).ok();

    let efile = base.join("Resources/etc/Renviron");
    replace_in_file(&efile, &re, &sub).ok();

    let ffile = base.join("Resources/fontconfig/fonts/fonts.conf");
    replace_in_file(&ffile, &re, &sub).ok();

    let mfile = base.join("Resources/etc/Makeconf");
    let sub = "-F/Library/Frameworks/R.framework/Versions/".to_string() + &ver;
    replace_in_file(&mfile, &re2, &sub).ok();

    let fake = base.join("R.framework");
    let fake = fake.as_path();
    // TODO: only ignore failure if files already exist
    std::fs::create_dir_all(&fake).ok();
    symlink("../Headers", fake.join("Headers")).ok();
    symlink("../Resources/lib", fake.join("Libraries")).ok();
    symlink("../PrivateHeaders", fake.join("PrivateHeaders")).ok();
    symlink("../R", fake.join("R")).ok();
    symlink("../Resources", fake.join("Resources")).ok();

    Ok(())
}

pub fn sc_system_fix_permissions(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    if get_mode()? == crate::utils::Mode::User {
        return Ok(());
    }
    escalate("changing system library permissions")?;
    let vers = args.get_many::<String>("version");
    if vers.is_none() {
        system_fix_permissions(None)
    } else {
        let vers: Vec<String> = vers
            .ok_or(SimpleError::new("Internal argument error"))?
            .map(|v| v.to_string())
            .collect();
        system_fix_permissions(Some(vers))
    }
}

fn system_fix_permissions(vers: Option<Vec<String>>) -> Result<(), Box<dyn Error>> {
    if get_mode()? == crate::utils::Mode::User {
        return Ok(());
    }
    let vers = match vers {
        Some(x) => x,
        None => sc_get_list()?,
    };

    OUTPUT.status("Fixing permissions");
    info!("Fixing permissions");

    for ver in vers {
        let ver = check_installed(&ver)?;
        let path = Path::new(&get_r_root()?)
            .join(ver.as_str())
            .join("Resources")
            .join("library");
        debug!("Fixing permissions in {}", path.display());
        let output = Command::new("chmod")
            .args(["-R", "g-w"])
            .arg(&path)
            .output()?;

        if !output.status.success() {
            OUTPUT.warn(&format!("Failed to update permissions for {}", ver));
            warn!("Failed to update permissions for {}", ver);
        }

        let output = Command::new("chgrp").args(["admin"]).arg(&path).output()?;

        if !output.status.success() {
            OUTPUT.warn(&format!("Failed to update group for {}", ver));
            warn!("Failed to update group for {}", ver);
        }
    }

    // also change group and permissions of the Current link, so admin users can update it
    // without sudo
    let current = PathBuf::from(get_r_current()?);

    let output = Command::new("chmod").args(["775"]).arg(&current).output()?;
    if !output.status.success() {
        OUTPUT.warn(&format!(
            "Failed to update permissions of link at {}",
            current.display()
        ));
        warn!(
            "Failed to update permissions of link at {}",
            current.display()
        );
    }

    let output = Command::new("chgrp")
        .args(["admin"])
        .arg(&current)
        .output()?;
    if !output.status.success() {
        OUTPUT.warn(&format!(
            "Failed to update group of link at {}",
            current.display()
        ));
        warn!("Failed to update group of link at {}", current.display());
    }

    Ok(())
}

pub fn sc_system_forget() -> Result<(), Box<dyn Error>> {
    if get_mode()? == crate::utils::Mode::User {
        return Ok(());
    }
    escalate("forgetting R versions")?;
    let out = Command::new("sh")
        .args(["-c", "pkgutil --pkgs | grep -i r-project | grep -v clang"])
        .output()?;

    let output = match String::from_utf8(out.stdout) {
        Ok(v) => v,
        Err(_) => {
            OUTPUT.error("Invalid UTF-8 output from pkgutil");
            error!("Invalid UTF-8 output from pkgutil");
            bail!("Invalid UTF-8 output from pkgutil")
        }
    };

    if output.lines().count() > 0 {
        OUTPUT.status("Forgetting installed versions");
        info!("Forgetting installed versions");
    }

    // TODO: this can fail, but if it fails it will still have exit
    // status 0, so we would need to check stderr to see if it failed.
    for line in output.lines() {
        debug!("Calling pkgutil --forget {}", line.trim());
        Command::new("pkgutil")
            .args(["--forget", line.trim()])
            .output()?;
    }

    Ok(())
}

pub fn sc_system_no_openmp(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    if get_mode()? == crate::utils::Mode::Admin {
        escalate("updating R compiler configuration")?;
    }
    let vers = args.get_many::<String>("version");
    if vers.is_none() {
        system_no_openmp(None)
    } else {
        let vers: Vec<String> = vers
            .ok_or(SimpleError::new("Internal argument error"))?
            .map(|v| v.to_string())
            .collect();
        system_no_openmp(Some(vers))
    }
}

fn system_no_openmp(vers: Option<Vec<String>>) -> Result<(), Box<dyn Error>> {
    let vers = match vers {
        Some(x) => x,
        None => sc_get_list()?,
    };
    let re = Regex::new("[-]fopenmp")?;

    for ver in vers {
        let ver = check_installed(&ver)?;
        let makevars = Path::new(&get_r_root()?)
            .join(get_r_etc_path()?.replace("{}", &ver))
            .join("Makeconf");
        if !makevars.exists() {
            continue;
        }

        match replace_in_file(&makevars, &re, "") {
            Ok(_) => {}
            Err(err) => {
                OUTPUT.error(&format!("Failed to update {}: {}", makevars.display(), err));
                error!("Failed to update {}: {}", makevars.display(), err);
                bail!("Failed to update {}: {}", makevars.display(), err);
            }
        };
    }

    Ok(())
}

pub fn sc_clean_registry() -> Result<(), Box<dyn Error>> {
    // Nothing to do on macOS
    Ok(())
}

pub fn sc_system_update_rtools40() -> Result<(), Box<dyn Error>> {
    Ok(())
}

pub fn sc_system_rtools(_args: &ArgMatches, _mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    Ok(())
}

pub fn sc_rstudio_(
    version: Option<&str>,
    project: Option<&str>,
    arg: Option<&OsStr>,
) -> Result<(), Box<dyn Error>> {
    let mut args = match project {
        // open -n -a RStudio
        None => osvec!["-n", "-a", "RStudio"],
        // open -n <project>
        Some(p) => osvec!["-n", p],
    };

    // open ... --env RSTUDIO_WHICH_R=...
    if let Some(ver) = version {
        let ver = check_installed(&ver.to_string())?;
        if !is_orthogonal(&ver)? {
            OUTPUT.error(&format!(
                "R {} is not orthogonal, it cannot run as a non-default. \
                   Run `rig system make-orthogonal`.",
                ver
            ));
            error!(
                "R {} is not orthogonal, it cannot run as a non-default. \
                   Run `rig system make-orthogonal`.",
                ver
            );
            bail!(
                "R {} is not orthogonal, it cannot run as a non-default. \
                   Run `rig system make-orthogonal`.",
                ver
            )
        }
        let rbin = Path::new(&get_r_root()?)
            .join(get_r_binpath()?.replace("{}", &ver));
        let path = "RSTUDIO_WHICH_R=".to_string() + &rbin.to_string_lossy();
        args.append(&mut osvec!["--env", &path]);
    }

    if let Some(a) = arg {
        let absa = absolute_path(a)?;
        args.append(&mut osvec!["--args", absa]);
    }

    let cmdline = osjoin(args.to_owned(), " ");
    OUTPUT.status(&format!("Running open {}", cmdline));
    info!("Running open {}", cmdline);

    match run(os("open"), args, "open") {
        Err(e) => {
            OUTPUT.error(&format!("RStudio failed to start: {}", e.to_string()));
            error!("RStudio failed to start: {}", e.to_string());
            bail!("RStudio failed to start: {}", e.to_string());
        }
        _ => {}
    };

    Ok(())
}

pub fn absolute_path(path: impl AsRef<Path>) -> Result<PathBuf, Box<dyn Error>> {
    let path = path.as_ref();

    let absolute_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    }
    .clean();

    Ok(absolute_path)
}

// ------------------------------------------------------------------------

pub fn sc_set_default(ver: &str) -> Result<(), Box<dyn Error>> {
    let ver = check_installed(&ver.to_string())?;
    let cur = get_r_current()?;
    // Maybe it does not exist, ignore error here
    std::fs::remove_file(&cur).ok();
    let path = Path::new(&get_r_root()?).join(&ver);
    match std::os::unix::fs::symlink(&path, &cur) {
        Ok(_) => {}
        Err(_) => {
            let msg = "Could not change the default R version. :( To be able to\n        \
                change the default R version, you need to be an admin.\n        \
                See the 'Users & Groups' section in the 'System Settings' app.";
            OUTPUT.error(msg);
            error!("{}", msg);
            bail!(msg);
        }
    };

    check_local_bin_path()?;
    let binary_dir = get_binary_dir()?;

    let r = Path::new(&binary_dir).join("R");
    if !r.exists() {
        debug!("Creating {}", r.display());
        let tgt = Path::new(&get_r_default_bindir()?).join("R");
        match std::os::unix::fs::symlink(&tgt, &r) {
            Err(e) => {
                OUTPUT.warn(&format!(
                    "Cannot create missing {}/R: {}",
                    binary_dir,
                    e.to_string()
                ));
                warn!("Cannot create missing {}/R: {}", binary_dir, e.to_string())
            }
            _ => {}
        };
    }

    let rscript = Path::new(&binary_dir).join("Rscript");
    if !rscript.exists() {
        debug!("Creating {}", rscript.display());
        let tgt = Path::new(&get_r_default_bindir()?).join("Rscript");
        match std::os::unix::fs::symlink(&tgt, &rscript) {
            Err(e) => {
                OUTPUT.warn(&format!(
                    "Cannot create missing {}/Rscript: {}",
                    binary_dir,
                    e.to_string()
                ));
                warn!(
                    "Cannot create missing {}/Rscript: {}",
                    binary_dir,
                    e.to_string()
                )
            }
            _ => {}
        };
    }

    if get_mode()? == crate::utils::Mode::User {
        if let Err(e) = ensure_rstudio_which_r_plist() {
            OUTPUT.warn(&format!("Could not register default R version in RStudio: {}", e));
            warn!("Could not install RSTUDIO_WHICH_R LaunchAgent: {}", e);
        }
        if let Err(e) = ensure_positron_custom_root_folders() {
            OUTPUT.warn(&format!("Could not register rig R versions in Positron: {}", e));
            warn!("Could not update Positron settings: {}", e);
        }
    }

    Ok(())
}

fn ensure_rstudio_which_r_plist() -> Result<(), Box<dyn Error>> {
    if !is_rstudio_installed() {
        return Ok(());
    }

    let plist_path = rstudio_which_r_plist_path()?;

    if Path::new(&plist_path).exists() {
        return Ok(());
    }

    let rbin = Path::new(&get_r_default_bindir()?).join("R");
    let rbin_str = rbin.to_string_lossy();

    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>io.r-lib.rig.rstudio-which-r</string>
    <key>ProgramArguments</key>
    <array>
        <string>/bin/launchctl</string>
        <string>setenv</string>
        <string>RSTUDIO_WHICH_R</string>
        <string>{rbin_str}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>
"#
    );

    let plist_dir = Path::new(&plist_path).parent().unwrap();
    std::fs::create_dir_all(plist_dir)?;
    std::fs::write(&plist_path, plist)?;
    info!("Installed LaunchAgent {}", plist_path);

    let out = Command::new("launchctl")
        .args(["load", &plist_path])
        .output()?;
    if !out.status.success() {
        let msg = format!(
            "Could not register default R version in RStudio: launchctl load {} failed: {}",
            plist_path,
            String::from_utf8_lossy(&out.stderr)
        );
        OUTPUT.error(&msg);
        error!("{}", msg);
        bail!(msg);
    }

    OUTPUT.success("Registered default R version in RStudio");

    Ok(())
}

fn ensure_positron_custom_root_folders() -> Result<(), Box<dyn Error>> {
    if let Some(val) = crate::config::get_global_config_value("positron-setup")? {
        if val == "false" {
            debug!("Skipping Positron setup (positron-setup=false in rig config)");
            return Ok(());
        }
    }

    let home = std::env::var("HOME")?;
    let positron_dir = format!("{}/Library/Application Support/Positron", home);
    if !Path::new(&positron_dir).exists() {
        debug!("Skipping Positron setup; Positron not found");
        return Ok(());
    }
    let settings_path_str = format!("{}/User/settings.json", positron_dir);
    let settings_path = Path::new(&settings_path_str);
    let r_root = get_r_root()?;
    const KEY: &str = "positron.r.customRootFolders";

    let mut settings: serde_json::Value = if settings_path.exists() {
        let contents = std::fs::read_to_string(settings_path)?;
        serde_json::from_str(&contents)?
    } else {
        serde_json::Value::Object(serde_json::Map::new())
    };

    let obj = settings
        .as_object_mut()
        .ok_or_else(|| SimpleError::new("Positron settings.json is not a JSON object"))?;

    match obj.get_mut(KEY) {
        Some(serde_json::Value::Array(arr)) => {
            // Already contains our path — nothing to do
            if arr.iter().any(|v| v.as_str() == Some(r_root.as_str())) {
                return Ok(());
            }
            // Append our path to the existing list
            arr.push(serde_json::Value::String(r_root.clone()));
            OUTPUT.success("Registered rig R versions in Positron");
            info!("Appended \"{}\" to Positron setting '{}'", r_root, KEY);
        }
        Some(other) => {
            // Unexpected type — leave it alone and inform
            info!(
                "Positron setting '{}' is not an array ({}); not modifying",
                KEY, other
            );
            return Ok(());
        }
        None => {
            obj.insert(KEY.to_string(), serde_json::json!([r_root]));
            OUTPUT.success("Registered rig R versions in Positron");
            info!("Set Positron setting '{}' = [\"{}\"]", KEY, r_root);
        }
    }
    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(settings_path, serde_json::to_string_pretty(&settings)?)?;

    Ok(())
}

fn is_rstudio_installed() -> bool {
    Path::new("/Applications/RStudio.app/Contents/MacOS/rsession").exists()
}

fn rstudio_which_r_plist_path() -> Result<String, Box<dyn Error>> {
    let home = std::env::var("HOME")?;
    Ok(format!(
        "{}/Library/LaunchAgents/io.r-lib.rig.rstudio-which-r.plist",
        home
    ))
}

fn remove_rstudio_which_r_plist() -> Result<(), Box<dyn Error>> {
    let plist_path = rstudio_which_r_plist_path()?;

    if !Path::new(&plist_path).exists() {
        return Ok(());
    }

    let out = Command::new("launchctl")
        .args(["unload", &plist_path])
        .output()?;
    if !out.status.success() {
        let msg = format!(
            "launchctl unload {} failed: {}",
            plist_path,
            String::from_utf8_lossy(&out.stderr)
        );
        OUTPUT.error(&msg);
        error!("{}", msg);
        bail!(msg);
    }

    std::fs::remove_file(&plist_path)?;
    info!("Removed LaunchAgent {}", plist_path);

    Ok(())
}

pub fn sc_get_default() -> Result<Option<String>, Box<dyn Error>> {
    read_version_link(&get_r_current()?)
}

pub fn sc_get_list() -> Result<Vec<String>, Box<dyn Error>> {
    let mut vers = Vec::new();
    if !Path::new(&get_r_root()?).exists() {
        return Ok(vers);
    }

    let paths = std::fs::read_dir(get_r_root()?)?;

    for de in paths {
        let path = de?.path();
        // If no path name, then path ends with ..., so we can skip
        let fnamestr = match path.file_name() {
            Some(x) => x,
            None => continue,
        };
        // If the path is not UTF-8, we'll skip it, this should not happen
        let fnamestr = match fnamestr.to_str() {
            Some(x) => x,
            None => continue,
        };
        if fnamestr == "Current" || fnamestr == ".DS_Store" {
            continue;
        }
        // If there is no Resources/bin/R, then this is not an R installation
        let rbin = path.join("Resources").join("bin").join("R");
        let rbin2 = path.join("bin").join("R");
        if !rbin.exists() && !rbin2.exists() {
            debug!("Skipping {}, no R binary found at {} or {}", path.display(), rbin.display(), rbin2.display());
            continue;
        }

        // Ok
        vers.push(fnamestr.to_string());
    }
    vers.sort();
    Ok(vers)
}

fn get_minor_version(ver: &str) -> Result<String, Box<dyn Error>> {
    let re = Regex::new("[.][^.]*$")?;
    Ok(re.replace(ver, "").to_string())
}

fn extract_pkg_version(filename: &OsStr) -> Result<RversionDir, Box<dyn Error>> {
    let out = Command::new("installer")
        .args(["-pkginfo", "-pkg"])
        .arg(filename)
        .output()?;
    let std = match String::from_utf8(out.stdout) {
        Ok(v) => v,
        Err(err) => {
            OUTPUT.error(&format!(
                "Cannot extract version from .pkg file {}: {}",
                filename.to_string_lossy(),
                err.to_string()
            ));
            error!(
                "Cannot extract version from .pkg file {}: {}",
                filename.to_string_lossy(),
                err.to_string()
            );
            bail!(
                "Cannot extract version from .pkg file {}: {}",
                filename.to_string_lossy(),
                err.to_string()
            )
        }
    };

    let lines = std.lines();
    let re = Regex::new("^R ([0-9]+[.][0-9]+[.][0-9])+.*$")?;
    let lines: Vec<&str> = lines.filter(|l| re.is_match(l)).collect();

    if lines.len() == 0 {
        OUTPUT.error(&format!(
            "Cannot extract version from .pkg file {}: no line with R version found",
            filename.to_string_lossy()
        ));
        error!(
            "Cannot extract version from .pkg file {}: no line with R version found",
            filename.to_string_lossy()
        );
        bail!(
            "Cannot extract version from .pkg file {}: no line with R version found",
            filename.to_string_lossy()
        );
    }

    let arm64 = Regex::new("ARM64")?;
    let ver = re.replace(lines[0], "${1}");
    let arch = if arm64.is_match(lines[0]) {
        "arm64"
    } else {
        "x86_64"
    };

    // Right now there are two installers for arm64 R 4.6.0, one writes to '4.6-arm64', and the
    // newer one to '4.6'. So there is no way to determine the install dir name from the
    // version. Let's extract it from the pkg file for R 4.6.0.

    let installdir: String;
    if ver == "4.6.0" {
        let out = Command::new("pkgutil")
            .args(["--payload-files"])
            .arg(filename)
            .output()?;
        let std = match String::from_utf8(out.stdout) {
            Ok(v) => v,
            Err(err) => {
                OUTPUT.error(&format!(
                    "Cannot extract version from .pkg file {}: {}",
                    filename.to_string_lossy(),
                    err.to_string()
                ));
                error!(
                    "Cannot extract version from .pkg file {}: {}",
                    filename.to_string_lossy(),
                    err.to_string()
                );
                bail!(
                    "Cannot extract version from .pkg file {}: {}",
                    filename.to_string_lossy(),
                    err.to_string()
                )
            }
        };

        let mut lines = std.lines();
        let re = Regex::new(r"\./R\.framework/Versions/([0-9][^/]+)$")?;
        installdir = match lines.find_map(|line| {
            re.captures(line)
                .and_then(|caps| caps.get(1))
                .map(|m| m.as_str().to_string())
        }) {
            Some(dir) => dir,
            None => bail!(
                "Cannot extract version from .pkg file {}",
                filename.display()
            ),
        }
    } else {
        let minor = get_minor_version(&ver)?;
        let x86_64 = Regex::new("X86_64")?;
        let ver_semver = semver::Version::parse(&ver).ok();
        let cutoff = semver::Version::new(4, 6, 0);
        let arm64_no_suffix = ver_semver.map_or(false, |v| v >= cutoff);
        installdir = if arch == "arm64" && !arm64_no_suffix {
            minor + "-arm64"
        } else if x86_64.is_match(lines[0]) {
            minor + "-x86_64"
        } else {
            minor
        };
    }

    OUTPUT.success(&format!("This is R {} for {}.", ver, arch));
    info!("This is R {} for {}.", ver, arch);

    let res = RversionDir {
        version: ver.to_string(),
        arch: arch.to_string(),
        installdir: installdir.to_string(),
    };

    Ok(res)
}

pub fn get_r_binary(rver: &str) -> Result<PathBuf, Box<dyn Error>> {
    debug!("Finding R binary for R {}", rver);
    let bin = Path::new(&get_r_root()?)
        .join(get_r_binpath()?.replace("{}", rver));
    debug!("R {} binary is at {}", rver, bin.display());
    Ok(bin)
}

#[allow(dead_code)]
pub fn get_system_renviron(rver: &str) -> Result<PathBuf, Box<dyn Error>> {
    let renviron = Path::new(&get_r_root()?)
        .join(rver)
        .join("Resources/etc/Renviron");
    Ok(renviron)
}

pub fn get_system_profile(rver: &str) -> Result<PathBuf, Box<dyn Error>> {
    let profile = get_r_base_profile()?.replace("{}",rver);
    Ok(PathBuf::from(&get_r_root()?).join(profile))
}

pub fn is_arm64_machine() -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_fake_r(source_dir: &Path, content: &str) {
        let bindir = source_dir.join("bin");
        fs::create_dir_all(&bindir).unwrap();
        fs::write(bindir.join("R"), content).unwrap();
    }

    #[test]
    fn patch_user_r_script_replaces_path_and_inserts_self_locating_lines() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_r(
            dir.path(),
            "#!/bin/sh\n\
             R_HOME_DIR=/Library/Frameworks/R.framework/Resources\n\
             exec \"${R_HOME_DIR}/bin/exec/R\" \"$@\"\n",
        );
        let home_dir = Path::new("/opt/r/4.6-arm64");

        patch_user_r_script(dir.path(), home_dir).unwrap();
        let content = fs::read_to_string(dir.path().join("bin/R")).unwrap();

        // Original path is replaced with home_dir.
        assert!(!content.contains("R_HOME_DIR=/Library/Frameworks/R.framework/Resources"));
        assert!(content.contains("R_HOME_DIR=/opt/r/4.6-arm64"));
        // Self-locating override added with $-escapes correctly applied.
        assert!(content.contains(
            "R_HOME_DIR=$(cd \"$(dirname \"$(realpath \"$0\")\")/..\" && pwd -P)"
        ));
        // DYLD line added; ${R_HOME_DIR} stays literal (not eaten as a regex group).
        assert!(content.contains("export DYLD_LIBRARY_PATH=\"${R_HOME_DIR}/lib\""));
        // Trailing content survives.
        assert!(content.contains("exec \"${R_HOME_DIR}/bin/exec/R\" \"$@\""));
    }

    #[test]
    fn patch_user_r_script_preserves_order() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_r(dir.path(), "before\nR_HOME_DIR=/orig\nafter\n");
        let home_dir = Path::new("/home/x");

        patch_user_r_script(dir.path(), home_dir).unwrap();
        let content = fs::read_to_string(dir.path().join("bin/R")).unwrap();
        let lines: Vec<&str> = content.lines().collect();

        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0], "before");
        assert_eq!(lines[1], "R_HOME_DIR=/home/x");
        assert!(lines[2].starts_with("R_HOME_DIR=$(cd "));
        assert!(lines[3].starts_with("export DYLD_LIBRARY_PATH="));
        assert_eq!(lines[4], "after");
    }

    #[test]
    fn patch_user_r_script_only_matches_assignment_lines() {
        // References to $R_HOME_DIR that aren't assignments must not be touched.
        let dir = tempfile::tempdir().unwrap();
        write_fake_r(
            dir.path(),
            "echo \"$R_HOME_DIR\"\nR_HOME_DIR=/orig\nuse $R_HOME_DIR here\n",
        );
        let home_dir = Path::new("/opt/r");

        patch_user_r_script(dir.path(), home_dir).unwrap();
        let content = fs::read_to_string(dir.path().join("bin/R")).unwrap();

        assert_eq!(content.matches("echo \"$R_HOME_DIR\"").count(), 1);
        assert_eq!(content.matches("use $R_HOME_DIR here").count(), 1);

        // Original /orig is gone; replaced with the new home_dir.
        assert!(!content.contains("R_HOME_DIR=/orig"));
        assert!(content.contains("R_HOME_DIR=/opt/r"));

        let assignment_count = content
            .lines()
            .filter(|l| l.starts_with("R_HOME_DIR="))
            .count();
        assert_eq!(assignment_count, 2);
    }

    #[test]
    fn patch_user_r_script_escapes_dollar_in_home_dir() {
        // A `$` in the install path must not be interpreted as a regex backref.
        let dir = tempfile::tempdir().unwrap();
        write_fake_r(dir.path(), "R_HOME_DIR=/orig\n");
        let home_dir = Path::new("/weird/$path");

        patch_user_r_script(dir.path(), home_dir).unwrap();
        let content = fs::read_to_string(dir.path().join("bin/R")).unwrap();

        assert!(content.contains("R_HOME_DIR=/weird/$path"));
    }

    #[test]
    fn patch_user_r_script_errors_when_script_missing() {
        let dir = tempfile::tempdir().unwrap();
        let home_dir = Path::new("/opt/r");
        assert!(patch_user_r_script(dir.path(), home_dir).is_err());
    }

    fn write_fake_makeconf(source_dir: &Path, content: &str) {
        let etcdir = source_dir.join("etc");
        fs::create_dir_all(&etcdir).unwrap();
        fs::write(etcdir.join("Makeconf"), content).unwrap();
    }

    fn write_fake_renviron(source_dir: &Path, content: &str) {
        let etcdir = source_dir.join("etc");
        fs::create_dir_all(&etcdir).unwrap();
        fs::write(etcdir.join("Renviron"), content).unwrap();
    }

    fn write_fake_fonts_conf(source_dir: &Path, content: &str) {
        let fontsdir = source_dir.join("fontconfig").join("fonts");
        fs::create_dir_all(&fontsdir).unwrap();
        fs::write(fontsdir.join("fonts.conf"), content).unwrap();
    }

    fn write_fake_libpc(source_dir: &Path, content: &str) {
        let pcdir = source_dir.join("lib").join("pkgconfig");
        fs::create_dir_all(&pcdir).unwrap();
        fs::write(pcdir.join("libR.pc"), content).unwrap();
    }

    const STUB_LIBPC: &str = "rincludedir=/x\n";

    #[test]
    fn patch_user_scripts_replaces_libr_line() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_makeconf(
            dir.path(),
            "CC = clang\n\
             LIBR = -F/Library/Frameworks/R.framework/.. -framework R\n\
             LDFLAGS = -L/usr/local/lib\n",
        );
        write_fake_renviron(dir.path(), "R_QPDF=qpdf\n");
        write_fake_fonts_conf(dir.path(), "<fontconfig></fontconfig>\n");
        write_fake_libpc(dir.path(), STUB_LIBPC);
        let home_dir = Path::new("/opt/r");

        patch_user_scripts(dir.path(), home_dir).unwrap();
        let content = fs::read_to_string(dir.path().join("etc/Makeconf")).unwrap();

        // Old LIBR is gone; new one in place with literal $(R_HOME).
        assert!(!content.contains("-framework R"));
        assert!(content.contains("LIBR = -L\"$(R_HOME)/lib\" -lR"));
        // Surrounding lines untouched.
        assert!(content.contains("CC = clang"));
        assert!(content.contains("LDFLAGS = -L/usr/local/lib"));
    }

    #[test]
    fn patch_user_scripts_matches_various_libr_whitespace() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_makeconf(dir.path(), "LIBR=-framework R\n");
        write_fake_renviron(dir.path(), "R_QPDF=qpdf\n");
        write_fake_fonts_conf(dir.path(), "<fontconfig></fontconfig>\n");
        write_fake_libpc(dir.path(), STUB_LIBPC);
        let home_dir = Path::new("/opt/r");

        patch_user_scripts(dir.path(), home_dir).unwrap();
        let content = fs::read_to_string(dir.path().join("etc/Makeconf")).unwrap();
        assert!(content.contains("LIBR = -L\"$(R_HOME)/lib\" -lR"));
    }

    #[test]
    fn patch_user_scripts_does_not_touch_similar_names() {
        // Variables that merely *start* with LIBR (e.g. LIBRARY) must not match.
        let dir = tempfile::tempdir().unwrap();
        write_fake_makeconf(
            dir.path(),
            "LIBRARY = something\n\
             LIBR_FOO = other\n\
             LIBR = -framework R\n",
        );
        write_fake_renviron(
            dir.path(),
            "R_QPDFEXT=foo\n\
             R_QPDF_X=bar\n\
             R_QPDF=qpdf\n",
        );
        write_fake_fonts_conf(dir.path(), "<fontconfig></fontconfig>\n");
        write_fake_libpc(dir.path(), STUB_LIBPC);
        let home_dir = Path::new("/opt/r");

        patch_user_scripts(dir.path(), home_dir).unwrap();
        let mk = fs::read_to_string(dir.path().join("etc/Makeconf")).unwrap();
        let rv = fs::read_to_string(dir.path().join("etc/Renviron")).unwrap();

        assert!(mk.contains("LIBRARY = something"));
        assert!(mk.contains("LIBR_FOO = other"));
        assert_eq!(mk.lines().filter(|l| l.starts_with("LIBR ")).count(), 1);

        assert!(rv.contains("R_QPDFEXT=foo"));
        assert!(rv.contains("R_QPDF_X=bar"));
        assert_eq!(
            rv.lines().filter(|l| l.starts_with("R_QPDF=")).count(),
            1
        );
    }

    #[test]
    fn patch_user_scripts_replaces_r_qpdf_line() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_makeconf(dir.path(), "LIBR = -framework R\n");
        write_fake_renviron(
            dir.path(),
            "R_PAPERSIZE=letter\n\
             R_QPDF=/usr/local/bin/qpdf\n\
             R_BROWSER=open\n",
        );
        write_fake_fonts_conf(dir.path(), "<fontconfig></fontconfig>\n");
        write_fake_libpc(dir.path(), STUB_LIBPC);
        let home_dir = Path::new("/opt/r");

        patch_user_scripts(dir.path(), home_dir).unwrap();
        let content = fs::read_to_string(dir.path().join("etc/Renviron")).unwrap();

        // Old value is gone; new one in place with home_dir baked in
        // (R does not expand $R_HOME when reading Renviron).
        assert!(!content.contains("R_QPDF=/usr/local/bin/qpdf"));
        assert!(content.contains("R_QPDF=${R_QPDF-/opt/r/bin/qpdf}"));
        assert!(!content.contains("${R_HOME}"));
        // Surrounding lines untouched.
        assert!(content.contains("R_PAPERSIZE=letter"));
        assert!(content.contains("R_BROWSER=open"));
    }

    #[test]
    fn patch_user_scripts_replaces_fontconfig_paths() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_makeconf(dir.path(), "LIBR = -framework R\n");
        write_fake_renviron(dir.path(), "R_QPDF=qpdf\n");
        write_fake_fonts_conf(
            dir.path(),
            "<dir>/Library/Frameworks/R.framework/Resources/share/fonts</dir>\n\
             <cachedir>/Library/Frameworks/R.framework/Resources/var/cache</cachedir>\n\
             <other>untouched</other>\n",
        );
        write_fake_libpc(dir.path(), STUB_LIBPC);
        let home_dir = Path::new("/opt/r/4.6-arm64");

        patch_user_scripts(dir.path(), home_dir).unwrap();
        let content = fs::read_to_string(dir.path().join("fontconfig/fonts/fonts.conf")).unwrap();

        // Original framework path is gone everywhere it appeared.
        assert!(!content.contains("/Library/Frameworks/R.framework/Resources"));
        // Replaced with home_dir at every occurrence.
        assert!(content.contains("<dir>/opt/r/4.6-arm64/share/fonts</dir>"));
        assert!(content.contains("<cachedir>/opt/r/4.6-arm64/var/cache</cachedir>"));
        // Unrelated content unchanged.
        assert!(content.contains("<other>untouched</other>"));
    }

    #[test]
    fn patch_user_scripts_escapes_dollar_in_home_dir_for_fontconfig() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_makeconf(dir.path(), "LIBR = -framework R\n");
        write_fake_renviron(dir.path(), "R_QPDF=qpdf\n");
        write_fake_fonts_conf(
            dir.path(),
            "<dir>/Library/Frameworks/R.framework/Resources/share/fonts</dir>\n",
        );
        write_fake_libpc(dir.path(), STUB_LIBPC);
        let home_dir = Path::new("/weird/$path");

        patch_user_scripts(dir.path(), home_dir).unwrap();
        let content = fs::read_to_string(dir.path().join("fontconfig/fonts/fonts.conf")).unwrap();
        assert!(content.contains("<dir>/weird/$path/share/fonts</dir>"));
    }

    #[test]
    fn patch_user_scripts_errors_when_makeconf_missing() {
        let dir = tempfile::tempdir().unwrap();
        // Renviron and fonts.conf present, Makeconf missing — should fail on Makeconf.
        write_fake_renviron(dir.path(), "R_QPDF=qpdf\n");
        write_fake_fonts_conf(dir.path(), "<fontconfig></fontconfig>\n");
        let home_dir = Path::new("/opt/r");
        assert!(patch_user_scripts(dir.path(), home_dir).is_err());
    }

    #[test]
    fn patch_user_scripts_errors_when_renviron_missing() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_makeconf(dir.path(), "LIBR = -framework R\n");
        write_fake_fonts_conf(dir.path(), "<fontconfig></fontconfig>\n");
        let home_dir = Path::new("/opt/r");
        assert!(patch_user_scripts(dir.path(), home_dir).is_err());
    }

    #[test]
    fn patch_user_scripts_ok_when_fonts_conf_missing() {
        // fonts.conf is optional; older R versions don't ship it.
        let dir = tempfile::tempdir().unwrap();
        write_fake_makeconf(dir.path(), "LIBR = -framework R\n");
        write_fake_renviron(dir.path(), "R_QPDF=qpdf\n");
        write_fake_libpc(dir.path(), STUB_LIBPC);
        let home_dir = Path::new("/opt/r");
        assert!(patch_user_scripts(dir.path(), home_dir).is_ok());
    }

    #[test]
    fn patch_user_scripts_replaces_libpc_paths_and_rincludedir() {
        // Realistic libR.pc body: framework paths in `rhome=` should become
        // home_dir, and `rincludedir=` should become a pkg-config var ref.
        let dir = tempfile::tempdir().unwrap();
        write_fake_makeconf(dir.path(), "LIBR = -framework R\n");
        write_fake_renviron(dir.path(), "R_QPDF=qpdf\n");
        write_fake_fonts_conf(dir.path(), "<fontconfig></fontconfig>\n");
        write_fake_libpc(
            dir.path(),
            "prefix=/Library/Frameworks/R.framework/Resources\n\
             exec_prefix=${prefix}\n\
             libdir=${exec_prefix}/lib\n\
             rhome=/Library/Frameworks/R.framework/Versions/4.5-arm64/Resources\n\
             rincludedir=/Library/Frameworks/R.framework/Versions/4.5-arm64/Resources/include\n",
        );
        let home_dir = Path::new("/opt/r/4.5-arm64");

        patch_user_scripts(dir.path(), home_dir).unwrap();
        let content = fs::read_to_string(dir.path().join("lib/pkgconfig/libR.pc")).unwrap();

        // rhome's framework path is rewritten to home_dir.
        assert!(content.contains("rhome=/opt/r/4.5-arm64\n"));
        // rincludedir line becomes the pkg-config var form (literal `${rhome}`).
        assert!(content.contains("rincludedir=${rhome}/include"));
        assert!(!content
            .contains("rincludedir=/Library/Frameworks/R.framework/Versions/4.5-arm64/Resources/include"));
        // Lines without the versioned framework path are left alone.
        assert!(content.contains("prefix=/Library/Frameworks/R.framework/Resources"));
        assert!(content.contains("exec_prefix=${prefix}"));
        assert!(content.contains("libdir=${exec_prefix}/lib"));
        // No leftover versioned framework path anywhere.
        assert!(!content.contains("/Library/Frameworks/R.framework/Versions/"));
    }

    #[test]
    fn patch_user_scripts_libpc_handles_various_versions() {
        // The version segment is matched as `[^/]+`, so any value works.
        let dir = tempfile::tempdir().unwrap();
        write_fake_makeconf(dir.path(), "LIBR = -framework R\n");
        write_fake_renviron(dir.path(), "R_QPDF=qpdf\n");
        write_fake_fonts_conf(dir.path(), "<fontconfig></fontconfig>\n");
        write_fake_libpc(
            dir.path(),
            "rhome=/Library/Frameworks/R.framework/Versions/4.6/Resources\n\
             rincludedir=/Library/Frameworks/R.framework/Versions/4.6/Resources/include\n",
        );
        let home_dir = Path::new("/opt/r/4.6");

        patch_user_scripts(dir.path(), home_dir).unwrap();
        let content = fs::read_to_string(dir.path().join("lib/pkgconfig/libR.pc")).unwrap();
        assert!(content.contains("rhome=/opt/r/4.6\n"));
        assert!(content.contains("rincludedir=${rhome}/include"));
    }

    #[test]
    fn patch_user_scripts_ok_when_libpc_missing() {
        // libR.pc is optional; older R versions don't ship it.
        let dir = tempfile::tempdir().unwrap();
        write_fake_makeconf(dir.path(), "LIBR = -framework R\n");
        write_fake_renviron(dir.path(), "R_QPDF=qpdf\n");
        write_fake_fonts_conf(dir.path(), "<fontconfig></fontconfig>\n");
        let home_dir = Path::new("/opt/r");
        assert!(patch_user_scripts(dir.path(), home_dir).is_ok());
    }

    #[test]
    fn remove_user_dyld_shadows_drops_shadowing_dylibs_and_keeps_others() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("lib");
        fs::create_dir_all(&lib).unwrap();
        let shadows = ["libc++.1.dylib", "libc++abi.1.dylib", "libunwind.1.dylib"];
        for name in shadows {
            fs::write(lib.join(name), b"shadow").unwrap();
        }
        let keep = ["libR.dylib", "libomp.dylib", "libgfortran.3.dylib"];
        for name in keep {
            fs::write(lib.join(name), b"keep").unwrap();
        }

        remove_user_dyld_shadows(dir.path()).unwrap();

        for name in shadows {
            assert!(!lib.join(name).exists(), "expected {} to be removed", name);
        }
        for name in keep {
            assert!(lib.join(name).exists(), "expected {} to be preserved", name);
        }
    }

    #[test]
    fn remove_user_dyld_shadows_is_ok_when_dylibs_are_absent() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("lib")).unwrap();
        // No shadow dylibs present (e.g. R 4.0+); call must succeed.
        remove_user_dyld_shadows(dir.path()).unwrap();
    }

    fn write_rversion_h(source_dir: &Path, status: &str) {
        let incdir = source_dir.join("include");
        fs::create_dir_all(&incdir).unwrap();
        let content = format!(
            "#define R_VERSION_STRING \"4.6.0\"\n\
             #define R_STATUS \"{}\"\n\
             #define R_YEAR 2026\n",
            status
        );
        fs::write(incdir.join("Rversion.h"), content).unwrap();
    }

    #[test]
    fn read_r_status_extracts_released_devel_and_next() {
        let dir = tempfile::tempdir().unwrap();

        write_rversion_h(dir.path(), "");
        assert_eq!(read_r_status(dir.path()).unwrap(), "");

        write_rversion_h(dir.path(), "Under development (unstable)");
        assert_eq!(
            read_r_status(dir.path()).unwrap(),
            "Under development (unstable)"
        );

        write_rversion_h(dir.path(), "Prerelease");
        assert_eq!(read_r_status(dir.path()).unwrap(), "Prerelease");
    }
}
