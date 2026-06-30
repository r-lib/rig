#![cfg(target_os = "windows")]

mod registry;
pub use registry::sc_clean_registry;
use registry::{
    add_user_bin_to_path, admin_rtools_paths, clean_admin_registry, clean_admin_rtools_registry,
    get_latest_install_path, list_admin_rtools, maybe_update_registry_default, sc_rtools_ls,
    unset_registry_default, update_registry_default,
};

use regex::Regex;
use std::error::Error;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Write;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::ArgMatches;
use directories::BaseDirs;
use log::{debug, error, info, trace, warn};
use owo_colors::OwoColorize;
use remove_dir_all::remove_dir_all;
use semver;
use simple_error::{bail, SimpleError};
use whoami::{fallible::hostname, username};

use crate::alias::*;
use crate::common::*;
use crate::download::*;
use crate::escalate::*;
use crate::library::*;
use crate::output::OUTPUT;
use crate::repos::*;
use crate::resolve::*;
use crate::run::*;
use crate::rversion::*;
use crate::utils::*;
use crate::windows_arch::*;

// get_r_root(): returns the directory where the R versions are installed
// get_links_dir(): returns the directory where the quick links are created
// get_r_versiondir(): name template for a single R version directory inside R_ROOT()
// get_r_syslibpath(): path to the system library of an R version from R_ROOT()
// get_r_binpath(): path of the R executable from R_ROOT()

pub fn get_links_dir() -> Result<String, Box<dyn Error>> {
    get_binary_dir()
}

macro_rules! osvec {
    // match a list of expressions separated by comma:
    ($($str:expr),*) => ({
        // create a Vec with this list of expressions,
        // calling String::from on each:
        vec![$(OsString::from($str),)*] as Vec<OsString>
    });
}

pub fn get_r_root() -> Result<String, Box<dyn Error>> {
    get_r_root_arch(get_native_arch())
}

pub fn get_r_root_arch(arch: &str) -> Result<String, Box<dyn Error>> {
    if get_mode()? == Mode::User {
        let base = match get_r_install_dir()? {
            Some(dir) => dir,
            None => {
                let appdata = std::env::var("APPDATA")?;
                format!("{}\\rig\\data\\r", appdata)
            }
        };
        // User mode: all arches share the same root; arch is encoded in the directory name
        return Ok(base);
    }
    Ok(admin_r_root_arch(arch))
}

// The fixed admin-mode (system-wide) R installation root for a given
// architecture. On an aarch64 host the native arm64 build lives in
// `C:\Program Files\R-aarch64` and the cross-architecture x86_64 build in
// `C:\Program Files (x86)\R`; everywhere else R lives in `C:\Program Files\R`.
// This is the single source of truth for the admin roots, used both by
// get_r_root_arch (in admin mode) and by the user-mode migration and
// clean-admin-r (which act on the admin locations regardless of the
// configured mode).
pub(super) fn admin_r_root_arch(arch: &str) -> String {
    const R_ROOT_: &str = "C:\\Program Files\\R";
    const R_X86_ROOT_: &str = "C:\\Program Files (x86)\\R";
    const R_AARCH64_ROOT_: &str = "C:\\Program Files\\R-aarch64";
    match arch {
        "aarch64" | "arm64" => R_AARCH64_ROOT_.to_string(),
        "x86_64" if get_native_arch() == "aarch64" => R_X86_ROOT_.to_string(),
        _ => R_ROOT_.to_string(),
    }
}

// Strip a trailing -x86_64, -aarch64, or -arm64 arch suffix from a rig name.
pub fn base_version(name: &str) -> String {
    for suffix in &["-x86_64", "-aarch64", "-arm64"] {
        if let Some(base) = name.strip_suffix(suffix) {
            return base.to_string();
        }
    }
    name.to_string()
}

// Determine the architecture implied by a rig version name.
// Names with a -x86_64 suffix are x86_64; -aarch64/-arm64 are aarch64.
// An unsuffixed name is the native architecture.
pub fn arch_of_name(name: &str) -> &'static str {
    if name.ends_with("-x86_64") {
        "x86_64"
    } else if name.ends_with("-aarch64") || name.ends_with("-arm64") {
        "aarch64"
    } else {
        get_native_arch()
    }
}

// Return the R root directory for a given rig version name.
pub fn get_r_root_for(name: &str) -> Result<String, Box<dyn Error>> {
    get_r_root_arch(arch_of_name(name))
}

// Return the directory key used inside the root.
// In user mode, all arches share one root and the arch suffix stays in the key.
// In admin mode, arches use different roots so the suffix is stripped here.
pub fn version_dir_key(name: &str) -> String {
    if get_mode().unwrap_or(Mode::Admin) == Mode::User {
        name.to_string()
    } else {
        base_version(name)
    }
}

// Build the rig name from a bare version string and an architecture.
// On x86_64 hosts or for the native arch on aarch64, no suffix is added.
pub(super) fn rig_name_for_arch(base: &str, arch: &str) -> String {
    let native = get_native_arch();
    if native == "aarch64" && arch == "x86_64" {
        base.to_string() + "-x86_64"
    } else {
        base.to_string()
    }
}

pub fn get_r_syslibpath() -> Result<String, Box<dyn Error>> {
    if get_mode()? == Mode::User {
        Ok("{}\\library".to_string())
    } else {
        Ok("R-{}\\library".to_string())
    }
}

pub fn get_r_binpath() -> Result<String, Box<dyn Error>> {
    if get_mode()? == Mode::User {
        Ok("{}\\bin\\R.exe".to_string())
    } else {
        Ok("R-{}\\bin\\R.exe".to_string())
    }
}

pub fn get_r_base_profile() -> Result<String, Box<dyn Error>> {
    if get_mode()? == Mode::User {
        Ok("{}\\library\\base\\R\\Rprofile".to_string())
    } else {
        Ok("R-{}\\library\\base\\R\\Rprofile".to_string())
    }
}

pub fn get_r_etc_path() -> Result<String, Box<dyn Error>> {
    if get_mode()? == Mode::User {
        Ok("{}\\etc".to_string())
    } else {
        Ok("R-{}\\etc".to_string())
    }
}

pub fn get_r_versiondir() -> Result<String, Box<dyn Error>> {
    if get_mode()? == Mode::User {
        Ok("{}".to_string())
    } else {
        Ok("R-{}".to_string())
    }
}

// Build the version's installation directory name: "<base>" in user mode, "R-<base>" in admin.
pub(super) fn r_dirname(key: &str) -> Result<String, Box<dyn Error>> {
    if get_mode()? == Mode::User {
        Ok(key.to_string())
    } else {
        Ok(format!("R-{}", key))
    }
}

fn read_rversion_h(install_dir: &Path) -> Result<(String, String), Box<dyn Error>> {
    let path = install_dir.join("include").join("Rversion.h");
    let content = std::fs::read_to_string(&path)?;
    let major_re = Regex::new(r#"(?m)^\s*#define\s+R_MAJOR\s+"(.*)"\s*$"#)?;
    let minor_re = Regex::new(r#"(?m)^\s*#define\s+R_MINOR\s+"(.*)"\s*$"#)?;
    let status_re = Regex::new(r#"(?m)^\s*#define\s+R_STATUS\s+"(.*)"\s*$"#)?;
    let grab = |re: &Regex| {
        re.captures(&content)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
    };
    let major =
        grab(&major_re).ok_or_else(|| SimpleError::new("Cannot find R_MAJOR in Rversion.h"))?;
    let minor =
        grab(&minor_re).ok_or_else(|| SimpleError::new("Cannot find R_MINOR in Rversion.h"))?;
    let status = grab(&status_re).unwrap_or_default();
    Ok((format!("{}.{}", major, minor), status))
}

fn user_install_name(install_dir: &Path, arch: &str) -> Result<String, Box<dyn Error>> {
    let (version, status) = read_rversion_h(install_dir)?;
    let base = match status.as_str() {
        "" => version,
        "Under development (unstable)" => "devel".to_string(),
        _ => "next".to_string(),
    };
    let name = rig_name_for_arch(&base, arch);
    debug!(
        "User install directory name is {} (R_STATUS = {:?})",
        name, status
    );
    Ok(name)
}

#[warn(unused_variables)]
pub fn sc_add(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    escalate("adding new R version")?;
    let alias = get_alias(args);
    sc_clean_registry()?;
    let str = args.get_one::<String>("str").unwrap();
    if str.len() >= 6 && &str[0..6] == "rtools" {
        // For bare "rtools" (install all needed), only honour --arch when the user
        // explicitly passed it; the flag's native-arch default should not filter out
        // cross-arch installations.
        let explicit_arch =
            args.value_source("arch") == Some(clap::parser::ValueSource::CommandLine);
        let arch = if explicit_arch {
            args.get_one::<String>("arch").map(|s| normalize_arch(s))
        } else {
            None
        };
        return add_rtools(str.to_string(), arch);
    }
    let (version_info, target) = download_r(&args)?;
    let installed_arch = version_info
        .arch
        .clone()
        .unwrap_or_else(|| get_native_arch().to_string());
    let target_path = Path::new(&target);

    OUTPUT.status(&format!("Installing {}", target_path.display()));
    info!("Installing {}", target_path.display());

    let mut cmd_args = vec![os("/VERYSILENT"), os("/SUPPRESSMSGBOXES")];
    if args.get_flag("without-translations") {
        cmd_args.push(os("/components=main,x64,i386"));
    }
    if args.get_flag("with-desktop-icon") {
        cmd_args.push(os("/mergetasks=desktopicon"));
    } else {
        cmd_args.push(os("/mergetasks=!desktopicon"));
    }

    let dirname = if get_mode()? == Mode::User {
        let r_root = get_r_root_arch(&installed_arch)?;
        std::fs::create_dir_all(&r_root)?;
        let temp_dir = format!("{}\\.rig-install-{}", r_root, std::process::id());
        // Clean up a leftover temp dir from a previous interrupted install.
        if Path::new(&temp_dir).exists() {
            remove_dir_all(&temp_dir)?;
        }
        std::fs::create_dir_all(&temp_dir)?;
        cmd_args.push(os("/CURRENTUSER"));
        cmd_args.push(OsString::from(format!("/DIR={}", temp_dir)));
        run(target, cmd_args, "installer")?;

        // The installer must have populated the temp dir. If it did not (e.g.
        // it failed silently or exited before finishing), fail clearly here
        // rather than with a confusing "Rversion.h not found" error.
        let rversion_h = Path::new(&temp_dir).join("include").join("Rversion.h");
        if !rversion_h.exists() {
            let _ = remove_dir_all(&temp_dir);
            let msg = format!(
                "Installation failed: the R installer did not populate {} (expected {} to exist)",
                temp_dir,
                rversion_h.display()
            );
            OUTPUT.error(&msg);
            error!("{}", msg);
            bail!("{}", msg);
        }

        let rig_name = match user_install_name(Path::new(&temp_dir), &installed_arch) {
            Ok(name) => name,
            Err(err) => {
                let _ = remove_dir_all(&temp_dir);
                OUTPUT.error(&format!(
                    "Cannot determine installed R version: {}",
                    err.to_string()
                ));
                error!("Cannot determine installed R version: {}", err.to_string());
                return Err(err);
            }
        };
        let final_dir = format!("{}\\{}", r_root, &rig_name);
        if Path::new(&final_dir).exists() {
            if let Err(err) = remove_dir_all(&final_dir) {
                let _ = remove_dir_all(&temp_dir);
                let msg = format!(
                    "Cannot replace existing R installation at {}: {}",
                    final_dir,
                    err.to_string()
                );
                OUTPUT.error(&msg);
                error!("{}", msg);
                bail!("{}", msg);
            }
        }
        if let Err(err) = std::fs::rename(&temp_dir, &final_dir) {
            let _ = remove_dir_all(&temp_dir);
            OUTPUT.error(&format!(
                "Cannot move R installation into {}: {}",
                final_dir,
                err.to_string()
            ));
            error!(
                "Cannot move R installation into {}: {}",
                final_dir,
                err.to_string()
            );
            return Err(err.into());
        }
        OUTPUT.status(&format!("Installed R as '{}'", rig_name));
        info!("Installed R as '{}' in {}", rig_name, final_dir);
        Some(rig_name)
    } else {
        run(target, cmd_args, "installer")?;
        get_latest_install_path(&installed_arch)?
    };

    match dirname {
        None => {
            let vers = sc_get_list()?;
            for ver in vers {
                library_update_rprofile(&ver)?;
            }
        }
        Some(ref dirname) => {
            set_default_if_none(dirname.to_string())?;
            library_update_rprofile(&dirname.to_string())?;
        }
    };
    sc_system_make_links()?;
    match dirname {
        None => {}
        Some(ref dirname) => match alias {
            // The `release`/`oldrel` aliases point at the native build. An
            // x86_64 build on an aarch64 machine gets an `-x86_64` suffix
            // instead, to avoid colliding with the native alias.
            Some(alias) => {
                let alias = if installed_arch == "x86_64" && get_native_arch() == "aarch64" {
                    format!("{}-x86_64", alias)
                } else {
                    alias
                };
                add_alias(&dirname, &alias)?
            }
            None => {}
        },
    };
    patch_for_rtools()?;
    maybe_update_registry_default()?;

    match dirname {
        None => {
            OUTPUT.warn("Cannot set up repositories, cannot determine installation directory");
            warn!("Cannot set up repositories, cannot determine installation directory");
        }
        Some(ref dirname) => {
            let setup = interpret_repos_args(args, true);
            repos_setup(Some(vec![dirname.to_string()]), setup)?;
        }
    };

    if !args.get_flag("without-pak") {
        match dirname {
            None => {
                OUTPUT.warn("Cannot install pak, cannot determine installation directory");
                warn!("Cannot install pak, cannot determine installation directory");
            }
            Some(ref dirname) => {
                let explicit = args.value_source("pak-version")
                    == Some(clap::parser::ValueSource::CommandLine);
                system_add_pak(
                    Some(vec![dirname.to_string()]),
                    args.get_one::<String>("pak-version").unwrap(),
                    // If this is specified then we always re-install
                    explicit,
                )?;
            }
        }
    }

    Ok(())
}

fn normalize_arch(arch: &str) -> String {
    match arch {
        "arm64" => "aarch64".to_string(),
        other => other.to_string(),
    }
}

struct NeededRtools {
    version: String,
    arch: String,
}

// Folder basename for an Rtools version+arch, mirroring the historical C:\Rtools layout:
// "Rtools44", "Rtools44-aarch64", "Rtools40", and "Rtools" for 3.x (or an empty version).
fn rtools_dir_name(version: &str, arch: &str) -> String {
    let versuffix = if version.is_empty() || version.starts_with('3') {
        ""
    } else {
        version
    };
    let archsuffix = if arch == "aarch64" { "-aarch64" } else { "" };
    format!("Rtools{}{}", versuffix, archsuffix)
}

// Full install path for an Rtools version+arch. In User mode this lives under the per-user
// rtools root (default %APPDATA%\rig\data\rtools); in Admin mode it stays at C:\<name>.
fn rtools_install_path(version: &str, arch: &str) -> Result<PathBuf, Box<dyn Error>> {
    let name = rtools_dir_name(version, arch);
    match get_rtools_install_dir()? {
        Some(root) => Ok(Path::new(&root).join(name)),
        None => Ok(Path::new("C:\\").join(name)),
    }
}

// The environment variable R uses to locate Rtools, e.g. RTOOLS44_HOME on x86_64 and
// RTOOLS44_AARCH64_HOME on aarch64. R (4.2+) derives the toolchain PATH, include and library
// paths from this single variable (via Makeconf, Rcmd_environ and Rprofile.windows).
fn rtools_home_var(version: &str, arch: &str) -> String {
    let archinfix = if arch == "aarch64" { "_AARCH64" } else { "" };
    format!("RTOOLS{}{}_HOME", version, archinfix)
}

// The 8.3 short path of an existing path (e.g. C:\Users\GABORC~1\...), or None if it cannot
// be determined (path missing, or 8.3 name generation disabled on the volume).
fn short_path_name(path: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    extern "system" {
        fn GetShortPathNameW(
            lpsz_long_path: *const u16,
            lpsz_short_path: *mut u16,
            cch_buffer: u32,
        ) -> u32;
    }
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // First call (null buffer) returns the required size, including the null terminator.
    let len = unsafe { GetShortPathNameW(wide.as_ptr(), std::ptr::null_mut(), 0) };
    if len == 0 {
        return None;
    }
    let mut buf = vec![0u16; len as usize];
    let written = unsafe { GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), len) };
    if written == 0 {
        return None;
    }
    buf.truncate(written as usize);
    Some(String::from_utf16_lossy(&buf))
}

// Render a path for use in Renviron.site. The make/clang toolchain breaks on spaces in
// paths (and a user's %APPDATA% almost always contains one), so prefer the 8.3 short name;
// also use forward slashes, which R and make handle without backslash-escaping surprises.
fn renviron_path(path: &Path) -> String {
    short_path_name(path)
        .unwrap_or_else(|| path.display().to_string())
        .replace('\\', "/")
}

// The lines to add to an R version's Renviron.site so it can find the given Rtools.
// In user mode rig sets RTOOLS<NN>_HOME itself (the system installer would otherwise do so);
// in admin mode it is set by the installer, so only the legacy 3.5/4.0 PATH lines are kept.
// Values use forward slashes and are quoted so spaces in the path do not break parsing.
fn rtools_renviron_lines(version: &str, arch: &str, rtools_path: &Path, user_mode: bool) -> String {
    let path = renviron_path(rtools_path);
    if version == "35" {
        // Rtools 3.5: prepend its bin/ to PATH (no RTOOLS<NN>_HOME mechanism).
        format!("PATH=\"{}/bin;${{PATH}}\"", path)
    } else if version == "40" {
        // Rtools 4.0: R does not auto-derive PATH, so keep the explicit PATH line that
        // references RTOOLS40_HOME (set here in user mode, by the installer in admin mode).
        let var = rtools_home_var(version, arch);
        let mut s = String::new();
        if user_mode {
            s += &format!("{}=\"{}\"\n", var, path);
        }
        s += &format!(
            "PATH=\"${{{var}}}/ucrt64/bin;${{{var}}}/usr/bin;${{PATH}}\"",
            var = var
        );
        s
    } else {
        // Rtools 4.2+: R derives everything it needs from RTOOLS<NN>[_AARCH64]_HOME.
        format!("{}=\"{}\"", rtools_home_var(version, arch), path)
    }
}

fn add_rtools(version: String, arch: Option<String>) -> Result<(), Box<dyn Error>> {
    let needed: Vec<NeededRtools>;
    if version == "rtools" {
        needed = get_rtools_needed(None, arch.as_deref())?;
    } else {
        let ver = version.replace("rtools", "");
        let a = arch.unwrap_or_else(|| get_native_arch().to_string());
        needed = vec![NeededRtools {
            version: ver,
            arch: a,
        }];
    }
    let client = &reqwest::Client::new();
    for item in needed {
        let instdirpath = rtools_install_path(&item.version, &item.arch)?;
        if instdirpath.exists() {
            OUTPUT.success(&format!(
                "Rtools{} ({}) is already installed",
                &item.version, &item.arch
            ));
            info!(
                "Rtools{} ({}) is already installed",
                &item.version, &item.arch
            );
            continue;
        }
        let rtver = get_rtools_version(&item.version, &item.arch)?;
        let url = rtver.url;
        let filename = "rtools-".to_string() + &item.version + "-" + &item.arch + ".exe";

        let tmp_dir = std::env::temp_dir().join("rig");
        let target = tmp_dir.join(&filename);
        OUTPUT.status(&format!("Downloading {} -> {}", url, target.display()));
        info!("Downloading {} -> {}", url, target.display());
        download_file(client, &url, &target.as_os_str())?;
        OUTPUT.status(&format!("Installing {}", target.display()));
        info!("Installing {}", target.display());

        // In User mode install per-user (/CURRENTUSER) into the rig-managed rtools root,
        // which writes the Rtools registry key to HKCU. Admin mode keeps the system install.
        let mut cmd_args = vec![os("/VERYSILENT"), os("/SUPPRESSMSGBOXES")];
        if get_mode()? == Mode::User {
            if let Some(parent) = instdirpath.parent() {
                std::fs::create_dir_all(parent)?;
            }
            cmd_args.push(os("/CURRENTUSER"));
            cmd_args.push(OsString::from(format!("/DIR={}", instdirpath.display())));
        }
        run(target.into_os_string(), cmd_args, "installer")?;
    }

    // In user mode R finds Rtools via RTOOLS<NN>_HOME, which rig writes into each R
    // version's Renviron.site. Re-run the patch now so an Rtools installed after R is
    // picked up (patch_for_rtools is idempotent and only acts on installed Rtools).
    if get_mode()? == Mode::User {
        patch_for_rtools()?;
    }

    Ok(())
}

fn patch_for_rtools() -> Result<(), Box<dyn Error>> {
    let user_mode = get_mode()? == Mode::User;
    let vers = sc_get_list()?;

    for ver in vers {
        let vver = vec![ver.to_owned()];
        let needed = get_rtools_needed(Some(vver), None)?;
        if needed.is_empty() {
            continue;
        }
        let nn = &needed[0].version;
        let arch = &needed[0].arch;

        // Admin mode keeps the historical behaviour: only Rtools 3.5/4.0 are patched, since
        // the system Rtools installer sets RTOOLS<NN>_HOME for 4.2+ itself. In user mode rig
        // must set RTOOLS<NN>_HOME itself, for every Rtools version R can find it through.
        if !user_mode && nn != "35" && nn != "40" {
            continue;
        }

        // In user mode only patch once the matching Rtools is actually installed in the
        // rig-managed location; otherwise defer until `rtools add` runs (which re-runs this).
        let rtools_path = rtools_install_path(nn, arch)?;
        if user_mode && !rtools_path.exists() {
            continue;
        }

        let ver_rroot = get_r_root_for(&ver)?;
        let ver_base = version_dir_key(&ver);
        let envfile = Path::new(&ver_rroot)
            .join(r_dirname(&ver_base)?)
            .join("etc")
            .join("Renviron.site");

        // Skip if this Renviron.site has already been patched by rig.
        if envfile.exists() {
            let file = File::open(&envfile)?;
            let reader = BufReader::new(file);
            let mut patched = false;
            for line in reader.lines() {
                if line?.starts_with("# added by rig") {
                    patched = true;
                    break;
                }
            }
            if patched {
                continue;
            }
        }

        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .append(true)
            .open(&envfile)?;

        let head = "\n# added by rig, do not update by hand-----\n";
        let tail = "\n# ----------------------------------------\n";
        let body = rtools_renviron_lines(nn, arch, &rtools_path, user_mode);

        if let Err(e) = writeln!(file, "{}{}{}", head, body, tail) {
            OUTPUT.warn(&format!("Couldn't write to Renviron.site file: {}", e));
            warn!("Couldn't write to Renviron.site file: {}", e);
        }
    }

    Ok(())
}

fn get_rtools_needed(
    version: Option<Vec<String>>,
    arch_filter: Option<&str>,
) -> Result<Vec<NeededRtools>, Box<dyn Error>> {
    let vers = match version {
        None => sc_get_list()?,
        Some(x) => x,
    };
    let mut res: Vec<NeededRtools> = vec![];
    let errmsg = "Cannot parse list of Rtools versions.";

    for ver in vers {
        let r_arch = normalize_arch(arch_of_name(&ver));
        if let Some(filter) = arch_filter {
            if r_arch != filter {
                continue;
            }
        }
        let r = get_r_binary(&ver)?;
        let out = Command::new(r)
            .args(["--vanilla", "-s", "-e", "cat(as.character(getRversion()))"])
            .output()?;
        let rver_str: String = String::from_utf8(out.stdout)?;
        let sver = semver::Version::parse(&rver_str)?;
        debug!("Get Rtools version for R {}.", rver_str);

        let rtversval = get_available_rtools_versions(&r_arch);
        let rtvers = match rtversval.as_array() {
            Some(x) => x,
            None => {
                OUTPUT.error(errmsg);
                error!("{}", errmsg);
                bail!("{}", errmsg)
            }
        };

        for rtver in rtvers {
            let first = rtver["first"].as_str().ok_or(errmsg)?;
            let last = rtver["last"].as_str().ok_or(errmsg)?;
            let first = semver::Version::parse(first)?;
            let last = semver::Version::parse(last)?;
            if first <= sver && sver <= last {
                let rtverver = rtver["version"].as_str().ok_or(errmsg)?.to_string();
                debug!("R {} needs Rtools {} ({}).", rver_str, rtverver, r_arch);
                if !res
                    .iter()
                    .any(|x| x.version == rtverver && x.arch == r_arch)
                {
                    res.push(NeededRtools {
                        version: rtverver,
                        arch: r_arch.clone(),
                    });
                }
            }
        }
    }
    Ok(res)
}

pub fn sc_rm(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    escalate("removing R versions")?;
    let vers = args.get_many::<String>("version");
    if vers.is_none() {
        return Ok(());
    }
    let vers = vers.ok_or(SimpleError::new("Internal argument error"))?;
    let default = sc_get_default()?;

    for ver in vers {
        let verstr = ver.to_string();
        if verstr.len() >= 6 && &verstr[0..6] == "rtools" {
            rm_rtools(verstr, None)?;
            continue;
        }
        let ver = check_installed(&verstr)?;

        if let Some(ref default) = default {
            if default == &ver {
                OUTPUT.warn(&format!(
                    "Removing default version, set new default with {}",
                    "rig default <version>".bold(),
                ));
                warn!(
                    "Removing default version, set new default with {}",
                    "rig default <version>".bold()
                );
                match unset_default() {
                    Err(e) => warn!("Failed to unset default version: {}", e.to_string()),
                    _ => {}
                };
            }
        }

        let rroot = get_r_root_for(&ver)?;
        let base = version_dir_key(&ver);
        let dir = Path::new(&rroot).join(r_dirname(&base)?);
        OUTPUT.status(&format!("Removing {}", dir.display()));
        info!("Removing {}", dir.display());
        remove_dir_all(&dir)?;
    }

    sc_clean_registry()?;
    sc_system_make_links()?;

    Ok(())
}

fn rm_rtools(ver: String, arch: Option<String>) -> Result<(), Box<dyn Error>> {
    let arch = arch.unwrap_or_else(|| get_native_arch().to_string());
    let version = ver.trim_start_matches("rtools").to_string();
    let dir = rtools_install_path(&version, &arch)?;
    OUTPUT.status(&format!("Removing {}", dir.display()));
    info!("Removing {}", dir.display());
    match remove_dir_all(&dir) {
        Err(_err) => {
            let cmd = format!(
                "Remove-Item -Recurse -Force -LiteralPath '{}'",
                dir.display().to_string().replace('\'', "''")
            );
            let out = Command::new("powershell")
                .args(["-command", &cmd])
                .output()?;
            let stderr = match std::str::from_utf8(&out.stderr) {
                Ok(v) => v,
                Err(_v) => "cannot parse stderr",
            };
            if !out.status.success() {
                OUTPUT.error(&format!("Failed to remove {}: {}", dir.display(), stderr));
                error!("Failed to remove {}: {}", dir.display(), stderr);
                bail!("Cannot remove {}: {}", dir.display(), stderr);
            }
        }
        _ => {}
    }

    sc_clean_registry()?;

    Ok(())
}

pub fn sc_system_make_links() -> Result<(), Box<dyn Error>> {
    escalate("making R-* quick shortcuts")?;
    let vers = sc_get_list()?;
    let links_dir = get_links_dir()?;
    let linkdir = Path::new(&links_dir);
    let mut new_links: Vec<String> = vec![
        "RS.bat".to_string(),
        "R.bat".to_string(),
        "Rscript.bat".to_string(),
    ];
    std::fs::create_dir_all(linkdir)?;

    for ver in vers {
        let filename = "R-".to_string() + &ver + ".bat";
        let linkfile = linkdir.join(&filename);
        new_links.push(filename);
        let ver_rroot = get_r_root_for(&ver)?;
        let ver_base = version_dir_key(&ver);
        let ver_dir = r_dirname(&ver_base)?;
        let target = Path::new(&ver_rroot).join(&ver_dir);

        let cnt = format!("@\"{}\\{}\\bin\\R\" %*\n", ver_rroot, ver_dir);
        let op;
        if linkfile.exists() {
            op = "Updating";
            let orig = std::fs::read_to_string(&linkfile)?;
            if orig == cnt {
                continue;
            }
        } else {
            op = "Adding";
        };
        OUTPUT.status(&format!("{} R-{} -> {}", op, ver, target.display()));
        info!("{} R-{} -> {}", op, ver, target.display());
        let mut file = File::create(&linkfile)?;
        file.write_all(cnt.as_bytes())?;
    }

    // Delete the ones we don't need
    let re_als = re_alias();
    let old_links = std::fs::read_dir(linkdir)?;
    for path in old_links {
        let path = path?;
        match path.file_name().into_string() {
            Err(_) => continue,
            Ok(filename) => {
                if !filename.ends_with(".bat") {
                    continue;
                }
                if !filename.starts_with("R-") {
                    continue;
                }
                if re_als.is_match(&filename) {
                    let rver = find_r_version_in_link(&path.path())?;
                    let realname = "R-".to_string() + &rver + ".bat";
                    if new_links.contains(&realname) {
                        continue;
                    }
                }
                if !new_links.contains(&filename) {
                    OUTPUT.status(&format!("Deleting unused {}", filename));
                    info!("Deleting unused {}", filename);
                    match std::fs::remove_file(path.path()) {
                        Ok(_) => {}
                        Err(e) => {
                            OUTPUT.warn(&format!(
                                "Failed to remove {}: {}",
                                filename,
                                e.to_string()
                            ));
                            warn!("Failed to remove {}: {}", filename, e.to_string());
                        }
                    }
                }
            }
        };
    }

    if get_mode()? == Mode::User {
        add_user_bin_to_path()?;
    }

    Ok(())
}

fn re_alias() -> Regex {
    let re = Regex::new("^R-(oldrel|release|next)(-x86_64)?[.]bat$").unwrap();
    re
}

pub fn find_aliases() -> Result<Vec<Alias>, Box<dyn Error>> {
    let mut result: Vec<Alias> = vec![];
    let links_dir = get_links_dir()?;
    let bin = Path::new(&links_dir);
    debug!("Finding existing aliases in {}", bin.display());

    if !bin.exists() {
        return Ok(result);
    }

    let paths = std::fs::read_dir(bin)?;
    let re = re_alias();

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
        debug!("Alias candidate: {}", fnamestr);
        if re.is_match(&fnamestr) {
            trace!("Checking {}", path.display());
            let rver = find_r_version_in_link(&path)?;
            let als = Alias {
                alias: fnamestr[2..fnamestr.len() - 4].to_string(),
                version: rver,
            };
            result.push(als);
        }
    }

    Ok(result)
}

fn find_r_version_in_link(path: &PathBuf) -> Result<String, Box<dyn Error>> {
    let lines = read_lines(path)?;
    if lines.len() == 0 {
        OUTPUT.error(&format!("Invalid R link file: {}", path.display()));
        error!("Invalid R link file: {}", path.display());
        bail!("Invalid R link file: {}", path.display());
    }
    let line = &lines[0];

    if get_mode().unwrap_or(Mode::Admin) == Mode::User {
        // User mode: @"<root>\<version>\bin\R" %* — extract the component after the root
        let user_root = get_r_root()?;
        let prefix = format!("@\"{}\\", user_root);
        let line_lower = line.to_lowercase();
        let prefix_lower = prefix.to_lowercase();
        if line_lower.starts_with(&prefix_lower) {
            let rest = &line[prefix.len()..];
            if let Some(slash_pos) = rest.find('\\') {
                return Ok(rest[..slash_pos].to_string());
            }
        }
        OUTPUT.error(&format!(
            "Cannot extract R version from {}, invalid R link file?",
            path.display(),
        ));
        error!(
            "Cannot extract R version from {}, invalid R link file?",
            path.display()
        );
        bail!(
            "Cannot extract R version from {}, invalid R link file?",
            path.display()
        );
    }

    // Admin mode: @"<root>\R-<base>\bin\R" %*
    // Determine which root this link points into, so we can re-attach the right suffix.
    let x86_root = get_r_root_arch("x86_64")?;
    let is_x86 = get_native_arch() == "aarch64"
        && line
            .to_lowercase()
            .starts_with(&("@\"".to_string() + &x86_root.to_lowercase()));

    let split = line.split("\\").collect::<Vec<&str>>();
    for s in split {
        if s == "R-devel" {
            let base = "devel".to_string();
            return Ok(if is_x86 { base + "-x86_64" } else { base });
        }
        // Skip the R-aarch64 root directory name itself
        if s != "R-aarch64" && s.starts_with("R-") {
            let base = s[2..].to_string();
            return Ok(if is_x86 { base + "-x86_64" } else { base });
        }
    }
    OUTPUT.error(&format!(
        "Cannot extract R version from {}, invalid R link file?",
        path.display(),
    ));
    error!(
        "Cannot extract R version from {}, invalid R link file?",
        path.display()
    );
    bail!(
        "Cannot extract R version from {}, invalid R link file?",
        path.display()
    );
}

pub fn sc_system_allow_core_dumps(_args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    // Nothing to do on Windows
    Ok(())
}

pub fn sc_system_allow_debugger(_args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    // Nothing to do on Windows
    Ok(())
}

pub fn sc_system_allow_debugger_rstudio(_args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    // Nothing to do on Windows
    Ok(())
}

pub fn sc_system_make_orthogonal(_args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    // Nothing to do on Windows
    Ok(())
}

// ------------------------------------------------------------------------
// `rig system user-mode` (Windows): switch rig to user mode and clean up an
// existing admin-mode setup. Mirrors the macOS/Linux implementation:
//
//   1. Captures the admin-mode versions, default and aliases from the system
//      locations (`C:\Program Files\R` and `C:\Program Files\R\bin`), before
//      switching the mode.
//   2. Switches the configured mode to `user`.
//   3. Reinstalls the admin-mode versions in user mode (unless --no-reinstall)
//      and restores the previous default version. Aliases are recreated by
//      reinstalling via the alias name.
//   4. Removes the admin-mode installations (the `C:\Program Files\R`
//      directories and their registry entries) and the quick links. This needs
//      administrator rights, so it is delegated to `rig system clean-admin-r`,
//      which elevates on its own.

// The admin-mode R installation roots to scan, derived from admin_r_root_arch()
// so they stay in sync with get_r_root_arch(). Each entry is (root directory,
// name suffix): the suffix is re-attached to recovered rig names, matching
// sc_get_list() (the cross-arch x86_64 root on aarch64 gets `-x86_64`).
fn admin_r_roots() -> Vec<(String, &'static str)> {
    if get_native_arch() == "aarch64" {
        vec![
            (admin_r_root_arch("aarch64"), ""),
            (admin_r_root_arch("x86_64"), "-x86_64"),
        ]
    } else {
        vec![(admin_r_root_arch(get_native_arch()), "")]
    }
}

// The admin-mode quick-link directory, regardless of the configured mode or any
// `binary-dir` override. This mirrors get_binary_dir()'s admin default and is
// always `<C:\Program Files\R>\bin`, even on aarch64 where R itself installs
// elsewhere.
const ADMIN_LINKS_DIR: &str = "C:\\Program Files\\R\\bin";

pub fn sc_system_user_mode(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let no_reinstall = args.get_flag("no-reinstall");
    let keep_install = args.get_flag("keep-install");
    let keep_links = args.get_flag("keep-links");

    // 1. Capture the admin-mode setup from the system locations, before we
    //    switch the configured mode.
    let versions = list_admin_versions()?;
    let default = find_admin_default()?;
    let aliases = find_admin_aliases()?;
    let rtools = list_admin_rtools()?;

    // 2. Switch to user mode. We write the config and prime the in-process mode
    //    (via the RIG_MODE env var, which child processes inherit) so that the
    //    reinstallation below targets the user location. Read the persisted mode
    //    first so we can report whether we actually switched.
    let already_user = crate::config::get_global_config_value("mode")?.as_deref() == Some("user");
    std::env::set_var("RIG_MODE", "user");
    let _ = set_mode(Mode::User);
    crate::config::set_global_config_value("mode", "user")?;
    if already_user {
        OUTPUT.success("rig already in user mode");
        info!("rig already in user mode");
    } else {
        OUTPUT.success("Switched rig to user mode");
        info!("Switched rig to user mode");
    }

    // 3. Reinstall the admin-mode versions in user mode and restore the
    //    previous default version. Aliases are recreated automatically by
    //    reinstalling via the alias name (see user_mode_install_spec()).
    if no_reinstall {
        if !versions.is_empty() {
            OUTPUT.status("Not reinstalling R versions in user mode (--no-reinstall)");
        }
    } else if !versions.is_empty() {
        let map = reinstall_in_user_mode(&versions, &aliases)?;
        if let Some(adef) = &default {
            match map.iter().find(|(a, _)| a == adef) {
                Some((_, udef)) => {
                    OUTPUT.status(&format!("Restoring default R version ({})", udef));
                    if let Err(e) = sc_set_default(udef) {
                        OUTPUT.warn(&format!("Could not restore default R version: {}", e));
                        warn!("Could not restore default R version: {}", e);
                    }
                }
                None => {
                    OUTPUT.warn(&format!(
                        "Could not restore the previous default R version ({})",
                        adef
                    ));
                    warn!("Could not restore default R version {}", adef);
                }
            }
        }
    }

    // 3b. Reinstall the admin-mode Rtools in user mode.
    if no_reinstall {
        if !rtools.is_empty() {
            OUTPUT.status("Not reinstalling Rtools in user mode (--no-reinstall)");
        }
    } else {
        reinstall_rtools_in_user_mode(&rtools)?;
    }

    // 4. Remove the admin-mode installations (unless `--keep-install`) and the
    //    quick links (unless `--keep-links`). This needs administrator rights,
    //    so it runs as a separate, self-elevating child process to avoid
    //    re-running the user-side work above elevated.
    clean_admin_installations(keep_install, keep_links)?;

    Ok(())
}

// List the admin-mode R version directories (named `R-<base>`) under the system
// roots, returning rig names (with `-x86_64` suffix for the x86_64 root on
// aarch64). Mirrors sc_get_list(), but works on the admin locations regardless
// of the configured mode.
fn list_admin_versions() -> Result<Vec<String>, Box<dyn Error>> {
    let mut vers = Vec::new();
    for (root, suffix) in admin_r_roots() {
        let rootp = Path::new(&root);
        if !rootp.exists() {
            continue;
        }
        for de in std::fs::read_dir(rootp)? {
            let path = de?.path();
            if !path.is_dir() {
                continue;
            }
            let name = match path.file_name().and_then(|s| s.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            // Version directories are named `R-<base>`; the `bin` links
            // directory and `.rig-install-*` temporaries are not.
            let base = match name.strip_prefix("R-") {
                Some(b) => b,
                None => continue,
            };
            if !path.join("bin").join("R.exe").exists() {
                continue;
            }
            vers.push(format!("{}{}", base, suffix));
        }
    }
    vers.sort();
    Ok(vers)
}

// Read the admin-mode default R version from `C:\Program Files\R\bin\R.bat`,
// whose first line is a `::<rig-name>` marker written by sc_set_default().
fn find_admin_default() -> Result<Option<String>, Box<dyn Error>> {
    let linkfile = Path::new(ADMIN_LINKS_DIR).join("R.bat");
    if !linkfile.exists() {
        return Ok(None);
    }
    let file = File::open(&linkfile)?;
    match BufReader::new(file).lines().next() {
        Some(line) => Ok(line?.strip_prefix("::").map(|rest| rest.trim().to_string())),
        None => Ok(None),
    }
}

// Recover the admin rig name a quick-link `.bat` file points at, from its
// `@"<root>\R-<base>\bin\R" %*` line. On aarch64 a link into the x86_64 root
// (`C:\Program Files (x86)\R`) yields a `-x86_64`-suffixed name.
fn admin_name_from_link_line(line: &str) -> Option<String> {
    let is_x86 = get_native_arch() == "aarch64"
        && line.to_lowercase().contains("\\program files (x86)\\r\\");
    for part in line.split('\\') {
        if let Some(base) = part.strip_prefix("R-") {
            // Skip the `R-aarch64` root directory component.
            if base == "aarch64" {
                continue;
            }
            return Some(if is_x86 {
                format!("{}-x86_64", base)
            } else {
                base.to_string()
            });
        }
    }
    None
}

// Find the alias quick links (R-release, R-oldrel, ...) in the admin-mode links
// directory and the rig name each points at.
fn find_admin_aliases() -> Result<Vec<Alias>, Box<dyn Error>> {
    let mut result: Vec<Alias> = vec![];
    let bindir = Path::new(ADMIN_LINKS_DIR);
    if !bindir.exists() {
        return Ok(result);
    }
    let re = re_alias();
    for de in std::fs::read_dir(bindir)? {
        let path = de?.path();
        let fname = match path.file_name().and_then(|s| s.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if !re.is_match(&fname) {
            continue;
        }
        let lines = read_lines(&path)?;
        if lines.is_empty() {
            continue;
        }
        if let Some(version) = admin_name_from_link_line(&lines[0]) {
            // Strip the leading `R-` and trailing `.bat` to get the alias name.
            result.push(Alias {
                alias: fname[2..fname.len() - 4].to_string(),
                version,
            });
        }
    }
    Ok(result)
}

// Determine how to reinstall an admin-mode version in user mode: the `rig add`
// spec (an alias name when one points at the version, otherwise the bare
// version) and the architecture to install.
fn user_mode_install_spec(admin_name: &str, aliases: &[Alias]) -> (String, String) {
    let arch = arch_of_name(admin_name).to_string();
    for al in aliases {
        if al.version == admin_name {
            let spec = al
                .alias
                .strip_suffix("-x86_64")
                .unwrap_or(&al.alias)
                .to_string();
            return (spec, arch);
        }
    }
    (base_version(admin_name), arch)
}

// Reinstall the given admin-mode versions in user mode by spawning `rig add`
// for each (so the full install path, including alias creation, is reused).
// Returns a mapping from each admin-mode rig name to the resulting user-mode
// rig name, used to restore the default version.
fn reinstall_in_user_mode(
    versions: &[String],
    aliases: &[Alias],
) -> Result<Vec<(String, String)>, Box<dyn Error>> {
    let exe = std::env::current_exe()?;
    let mut map: Vec<(String, String)> = vec![];
    for admin_name in versions {
        // A user-mode install of the same R release has the same rig name (the
        // base version plus arch suffix are mode-independent), so if it is
        // already present we skip the download/reinstall. We still (re)create
        // any aliases that pointed at it, so the migrated setup is complete.
        if sc_get_list()?.iter().any(|v| v == admin_name) {
            OUTPUT.success(&format!(
                "R '{}' is already installed in user mode",
                admin_name
            ));
            info!(
                "R '{}' already installed in user mode, not reinstalling",
                admin_name
            );
            recreate_aliases_for(admin_name, aliases);
            map.push((admin_name.clone(), admin_name.clone()));
            continue;
        }
        let (spec, arch) = user_mode_install_spec(admin_name, aliases);
        OUTPUT.status(&format!(
            "Reinstalling R '{}' ({}) in user mode",
            spec, arch
        ));
        info!("Reinstalling R '{}' ({}) in user mode", spec, arch);
        let before = sc_get_list()?;
        let status = Command::new(&exe)
            .args(["add", &spec, "--arch", &arch, "--without-pak"])
            .status()?;
        if !status.success() {
            OUTPUT.warn(&format!("Failed to reinstall R '{}' in user mode", spec));
            warn!("Failed to reinstall R '{}' in user mode", spec);
            continue;
        }
        let after = sc_get_list()?;
        match after.iter().find(|v| !before.contains(v)) {
            Some(udir) => map.push((admin_name.clone(), udir.clone())),
            None => debug!(
                "No new user-mode version directory after reinstalling '{}'",
                spec
            ),
        }
    }
    Ok(map)
}

// (Re)create, in user mode, the aliases that pointed at the given admin-mode
// version. Used when the version is already installed in user mode and so was
// not reinstalled (the install path would otherwise have created the alias).
fn recreate_aliases_for(admin_name: &str, aliases: &[Alias]) {
    for al in aliases {
        if al.version != admin_name {
            continue;
        }
        if let Err(e) = add_alias(admin_name, &al.alias) {
            OUTPUT.warn(&format!("Could not create R-{} alias: {}", al.alias, e));
            warn!("Could not create R-{} alias: {}", al.alias, e);
        }
    }
}

// Reinstall the given admin-mode Rtools (version-name, arch) in user mode by
// spawning `rig rtools add <version> --arch <arch>` for each, so the full
// install path (download, registry, Renviron.site wiring) is reused. The
// `RIG_MODE=user` env var we set earlier makes each child install into the
// per-user location.
fn reinstall_rtools_in_user_mode(rtools: &[(String, String)]) -> Result<(), Box<dyn Error>> {
    if rtools.is_empty() {
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    for (version, arch) in rtools {
        // We are already in user mode here, so rtools_install_path() resolves to
        // the per-user location; if that Rtools is already there, do not reinstall.
        if rtools_install_path(version, arch)?.exists() {
            OUTPUT.success(&format!(
                "Rtools{} ({}) is already installed in user mode",
                version, arch
            ));
            info!(
                "Rtools{} ({}) already installed in user mode, not reinstalling",
                version, arch
            );
            continue;
        }
        OUTPUT.status(&format!(
            "Reinstalling Rtools{} ({}) in user mode",
            version, arch
        ));
        info!("Reinstalling Rtools{} ({}) in user mode", version, arch);
        let status = Command::new(&exe)
            .args(["system", "rtools", "add", version, "--arch", arch])
            .status()?;
        if !status.success() {
            OUTPUT.warn(&format!(
                "Failed to reinstall Rtools{} ({}) in user mode",
                version, arch
            ));
            warn!(
                "Failed to reinstall Rtools{} ({}) in user mode",
                version, arch
            );
        }
    }
    Ok(())
}

// Spawn `rig system clean-admin-r` to remove the admin-mode installations,
// unless there is nothing to remove. The child elevates on its own.
fn clean_admin_installations(keep_install: bool, keep_links: bool) -> Result<(), Box<dyn Error>> {
    if !admin_cleanup_needed(keep_install, keep_links)? {
        debug!("No admin-mode installations or links to clean up");
        return Ok(());
    }
    if keep_install {
        OUTPUT.status("Removing admin-mode links, keeping installations (this needs admin rights)");
        info!("Removing admin-mode links, keeping installations");
    } else {
        OUTPUT.status("Removing admin-mode R and Rtools installations (this needs admin rights)");
        info!("Removing admin-mode R and Rtools installations");
    }
    // Run the cleanup in admin mode (`--admin`), overriding the `RIG_MODE=user`
    // we just set, so that `escalate()` in the child actually elevates — in user
    // mode it is a no-op, and removing `C:\Program Files\R` needs admin rights.
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(&exe);
    cmd.args(["--admin", "system", "clean-admin-r"]);
    if keep_install {
        cmd.arg("--keep-install");
    }
    if keep_links {
        cmd.arg("--keep-links");
    }
    let status = cmd.status()?;
    if !status.success() {
        bail!("Failed to remove admin-mode R installations");
    }
    Ok(())
}

// Whether there are any admin-mode installations or quick links left to remove,
// to avoid asking for elevation when there is nothing to do.
fn admin_cleanup_needed(keep_install: bool, keep_links: bool) -> Result<bool, Box<dyn Error>> {
    if !keep_install && (!list_admin_versions()?.is_empty() || !admin_rtools_paths()?.is_empty()) {
        return Ok(true);
    }
    if keep_links {
        return Ok(false);
    }
    let bindir = Path::new(ADMIN_LINKS_DIR);
    let paths = match std::fs::read_dir(bindir) {
        Ok(p) => p,
        Err(_) => return Ok(false),
    };
    let re = re_alias();
    for de in paths.flatten() {
        if let Some(name) = de.path().file_name().and_then(|s| s.to_str()) {
            if name == "R.bat"
                || name == "Rscript.bat"
                || name == "RS.bat"
                || (name.starts_with("R-") && name.ends_with(".bat"))
                || re.is_match(name)
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

// Remove all admin-mode R installations: delete the `C:\Program Files\R`
// version directories, the quick links, and the stale registry entries. This
// operates on the system locations directly, regardless of the configured mode,
// and elevates to administrator. It is invoked by `rig system user-mode`.
pub fn sc_system_clean_admin_r(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let keep_install = args.get_flag("keep-install");
    let keep_links = args.get_flag("keep-links");

    escalate("removing admin-mode R installations")?;

    if !keep_install {
        remove_admin_installations()?;
        remove_admin_rtools()?;
    }

    if !keep_links {
        remove_admin_links()?;
    }

    // Prune the HKLM registry entries that now point at removed directories.
    if !keep_install {
        clean_admin_registry()?;
    }

    Ok(())
}

// Remove all admin-mode Rtools installations: delete the `C:\Rtools*`
// directories recorded under HKLM and prune their registry entries. Operates on
// the system locations directly, regardless of the configured mode. Invoked by
// `rig system clean-admin-r` unless `--keep-install` was given.
fn remove_admin_rtools() -> Result<(), Box<dyn Error>> {
    for path in admin_rtools_paths()? {
        OUTPUT.status(&format!("Removing {}", path));
        info!("Removing {}", path);
        if let Err(e) = remove_dir_all(Path::new(&path)) {
            OUTPUT.warn(&format!("Cannot remove {}: {}", path, e));
            warn!("Cannot remove {}: {}", path, e);
        }
    }
    clean_admin_rtools_registry()?;
    Ok(())
}

fn remove_admin_installations() -> Result<(), Box<dyn Error>> {
    for (root, _suffix) in admin_r_roots() {
        let rootp = Path::new(&root);
        if !rootp.exists() {
            continue;
        }
        for de in std::fs::read_dir(rootp)? {
            let path = de?.path();
            if !path.is_dir() {
                continue;
            }
            let name = match path.file_name().and_then(|s| s.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            if !name.starts_with("R-") {
                continue;
            }
            OUTPUT.status(&format!("Removing {}", path.display()));
            info!("Removing {}", path.display());
            if let Err(e) = remove_dir_all(&path) {
                OUTPUT.warn(&format!("Cannot remove {}: {}", path.display(), e));
                warn!("Cannot remove {}: {}", path.display(), e);
            }
        }
    }
    Ok(())
}

fn remove_admin_links() -> Result<(), Box<dyn Error>> {
    let bindir = Path::new(ADMIN_LINKS_DIR);
    let paths = match std::fs::read_dir(bindir) {
        Ok(p) => p,
        Err(_) => return Ok(()),
    };
    let re = re_alias();
    for de in paths.flatten() {
        let path = de.path();
        let name = match path.file_name().and_then(|s| s.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let is_link = name == "R.bat"
            || name == "Rscript.bat"
            || name == "RS.bat"
            || re.is_match(&name)
            || (name.starts_with("R-") && name.ends_with(".bat"));
        if !is_link {
            continue;
        }
        OUTPUT.status(&format!("Removing {}", path.display()));
        info!("Removing {}", path.display());
        if let Err(e) = std::fs::remove_file(&path) {
            OUTPUT.warn(&format!("Cannot remove {}: {}", path.display(), e));
            warn!("Cannot remove {}: {}", path.display(), e);
        }
    }

    // Remove the now-empty `bin` links directory and admin roots, ignoring
    // errors (they are non-empty if anything else still lives there).
    let _ = std::fs::remove_dir(bindir);
    for (root, _suffix) in admin_r_roots() {
        let _ = std::fs::remove_dir(Path::new(&root));
    }
    Ok(())
}

pub fn sc_system_update_certs() -> Result<(), Box<dyn Error>> {
    // Not supported on Windows
    Ok(())
}

pub fn sc_system_fix_permissions(_args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    // Nothing to do on Windows
    Ok(())
}

pub fn sc_system_forget() -> Result<(), Box<dyn Error>> {
    // Nothing to do on Windows
    Ok(())
}

pub fn sc_system_no_openmp(_args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    // Nothing to do on Windows
    Ok(())
}

// ------------------------------------------------------------------------

fn list_r_in_root(root: &str, suffix: &str, vers: &mut Vec<String>) -> Result<(), Box<dyn Error>> {
    if !Path::new(root).exists() {
        return Ok(());
    }
    let user_mode = get_mode().unwrap_or(Mode::Admin) == Mode::User;
    for de in std::fs::read_dir(root)? {
        let path = de?.path();
        if !path.is_dir() {
            continue;
        }
        match path.file_name() {
            None => continue,
            Some(fname) => match fname.to_str() {
                None => continue,
                Some(fname) => {
                    // Skip dot-directories (e.g. `.rig-install-*` temporary
                    // install directories left behind by an interrupted add).
                    if fname.starts_with('.') {
                        continue;
                    }
                    if user_mode {
                        // User mode: no R- prefix; directory name is the version
                        vers.push(fname.to_string() + suffix);
                    } else if fname.len() > 2 && &fname[0..2] == "R-" {
                        let v = fname[2..].to_string() + suffix;
                        vers.push(v);
                    }
                }
            },
        }
    }
    Ok(())
}

pub fn sc_get_list() -> Result<Vec<String>, Box<dyn Error>> {
    let mut vers = Vec::new();

    if get_mode()? == Mode::User {
        // User mode: all arches share one root; version names already include arch suffix
        list_r_in_root(&get_r_root()?, "", &mut vers)?;
    } else {
        // Admin mode: native root, and on aarch64 also a separate x86_64 root
        list_r_in_root(&get_r_root()?, "", &mut vers)?;
        if get_native_arch() == "aarch64" {
            let x86_root = get_r_root_arch("x86_64")?;
            list_r_in_root(&x86_root, "-x86_64", &mut vers)?;
        }
    }

    vers.sort();
    Ok(vers)
}

pub fn sc_set_default(ver: &str) -> Result<(), Box<dyn Error>> {
    let ver = check_installed(&ver.to_string())?;
    escalate("setting the default R version")?;
    let rroot = get_r_root_for(&ver)?;
    let base = version_dir_key(&ver);
    let links_dir = get_links_dir()?;
    let linkdir = Path::new(&links_dir);
    std::fs::create_dir_all(&linkdir)?;

    let linkfile = linkdir.join("R.bat");
    let base_dir = r_dirname(&base)?;
    let cnt =
        "::".to_string() + &ver + "\n" + "@\"" + &rroot + "\\" + &base_dir + "\\bin\\R\" %*\n";
    let mut file = File::create(linkfile)?;
    file.write_all(cnt.as_bytes())?;

    let linkfile2 = linkdir.join("RS.bat");
    let mut file2 = File::create(linkfile2)?;
    file2.write_all(cnt.as_bytes())?;

    let linkfile3 = linkdir.join("Rscript.bat");
    let mut file3 = File::create(linkfile3)?;
    let cnt3 = "::".to_string()
        + &ver
        + "\n"
        + "@\""
        + &rroot
        + "\\"
        + &base_dir
        + "\\bin\\Rscript\" %*\n";
    file3.write_all(cnt3.as_bytes())?;

    update_registry_default()?;

    Ok(())
}

pub fn unset_default() -> Result<(), Box<dyn Error>> {
    escalate("unsetting the default R version")?;

    let links_dir = get_links_dir()?;
    let linkdir = Path::new(&links_dir);

    let try_rm = |x: &str| {
        let f = linkdir.join(x);
        if f.exists() {
            match std::fs::remove_file(&f) {
                Err(e) => {
                    OUTPUT.warn(&format!(
                        "Failed to remove {}: {}",
                        f.display(),
                        e.to_string()
                    ));
                    warn!("Failed to remove {}: {}", f.display(), e.to_string())
                }
                _ => {}
            };
        }
    };

    try_rm("R.bat");
    try_rm("RS.bat");
    try_rm("Rscript.bat");

    unset_registry_default()?;

    Ok(())
}

pub fn sc_get_default() -> Result<Option<String>, Box<dyn Error>> {
    let links_dir = get_links_dir()?;
    let linkdir = Path::new(&links_dir);
    let linkfile = linkdir.join("R.bat");
    if !linkfile.exists() {
        return Ok(None);
    }
    let file = File::open(linkfile)?;
    let reader = BufReader::new(file);

    let mut first = "".to_string();
    for line in reader.lines() {
        first = line?.replace("::", "");
        break;
    }

    Ok(Some(first.to_string()))
}

pub fn sc_system_update_rtools40() -> Result<(), Box<dyn Error>> {
    run(
        "c:\\rtools40\\usr\\bin\\bash.exe".into(),
        vec![
            "--login".into(),
            "-c".into(),
            "pacman -Syu --noconfirm".into(),
        ],
        "Rtools40 update",
    )
}

pub fn sc_system_rtools(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        Some(("add", s)) => sc_rtools_add(s, mainargs),
        Some(("list", s)) => sc_rtools_ls(s, mainargs),
        Some(("rm", s)) => sc_rtools_rm(s, mainargs),
        _ => Ok(()), // unreachable
    }
}

fn sc_rtools_add(args: &ArgMatches, _mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    escalate("adding Rtools")?;
    let ver = args.get_one::<String>("version").unwrap();
    let arch = args.get_one::<String>("arch").map(|s| normalize_arch(s));
    if ver == "all" {
        add_rtools("rtools".to_string(), arch)
    } else if ver.starts_with("rtools") {
        add_rtools(ver.to_string(), arch)
    } else {
        add_rtools("rtools".to_string() + ver, arch)
    }
}

fn sc_rtools_rm(args: &ArgMatches, _mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    escalate("removing Rtools")?;
    let vers = args.get_many::<String>("version");
    if vers.is_none() {
        return Ok(());
    }
    let vers = vers.ok_or(SimpleError::new("Internal argument error"))?;
    let arch = args.get_one::<String>("arch").map(|s| normalize_arch(s));

    for ver in vers {
        if ver == "all" {
            rm_rtools("rtools".to_string(), arch.clone())?;
        } else if ver.starts_with("rtools") {
            rm_rtools(ver.to_string(), arch.clone())?;
        } else {
            rm_rtools("rtools".to_string() + ver, arch.clone())?;
        }
    }

    Ok(())
}

// All this is from https://github.com/rstudio/rstudio/blob/44f09c50d469a14d5a9c3840c7a239f3bf21ace9/src/cpp/core/system/Xdg.cpp#L85
//
// 1. RSTUDIO_CONFIG_HOME might point to the final path.
// 2. XDG_CONFIG_HOME might point to the user config path (append RStudio).
// 3. Otherwise query FOLDERID_RoamingAppData for the user config path.
// 4. Or fall back to `~/.config` if that fails (it really should not).
// 5. Set USER, HOME and HOSTNAME env vars for expansion.
// 6. Expand env vars, both $ENV and ${ENV} types.
// 7. Expand ~ to home dir
//
// 5-6 are only needed if the path has a '$' or '~' character.
// 7 is only needed if the path has a '~' character or if it is empty.

pub fn get_rstudio_config_path() -> Result<std::path::PathBuf, Box<dyn Error>> {
    let mut final_path = true;
    let mut xdg: Option<std::path::PathBuf> = None;

    // RSTUDIO_CONFIG_HOME may point to the final path
    match std::env::var("RSTUDIO_CONFIG_HOME") {
        Ok(x) => xdg = Some(std::path::PathBuf::from(x)),
        Err(_) => {}
    };

    // XDG_CONFIG_HOME may point to the user config path
    if xdg.is_none() {
        final_path = false;
        match std::env::var("XDG_CONFIG_HOME") {
            Ok(x) => {
                let mut xdg2 = std::path::PathBuf::new();
                xdg2.push(x);
                xdg = Some(xdg2);
            }
            Err(_) => {}
        };
    }

    // Get user config path from system
    if xdg.is_none() {
        if let Some(bd) = BaseDirs::new() {
            xdg = Some(std::path::PathBuf::from(bd.config_dir()));
        }
    }

    // Fall back to `~/.config`
    if xdg.is_none() {
        xdg = Some(std::path::PathBuf::from("~/.config"));
    }

    // We have a path at this point
    let xdg = xdg.unwrap();

    // Set USER, HOME, HOSTNAME, for expansion, if needed
    let mut path = match xdg.to_str() {
        Some(x) => String::from(x),
        None => {
            OUTPUT.error("RStudio config path cannot be represented as Unicode. :(");
            error!("RStudio config path cannot be represented as Unicode. :(");
            bail!("RStudio config path cannot be represented as Unicode. :(")
        }
    };
    let has_dollar = path.contains("$");
    let empty = path.len() == 0;
    let has_tilde = path.contains("~");

    if has_dollar || empty || has_tilde {
        match std::env::var("USER") {
            Ok(_) => {}
            Err(_) => {
                let username = username();
                std::env::set_var("USER", username);
            }
        };
    }

    if has_dollar {
        match std::env::var("HOME") {
            Ok(_) => {}
            Err(_) => {
                let bd = BaseDirs::new();
                match bd {
                    Some(x) => {
                        std::env::set_var("HOME", x.home_dir().as_os_str());
                    }
                    None => {
                        OUTPUT.warn("Cannot determine HOME directory");
                        warn!("Cannot determine HOME directory");
                    }
                };
            }
        };
    }

    if has_dollar {
        match std::env::var("HOSTNAME") {
            Ok(_) => {}
            Err(_) => {
                let hostname = match hostname() {
                    Ok(x) => x,
                    Err(_) => "localhost".to_string(),
                };
                std::env::set_var("HOSTNAME", hostname);
            }
        };
    }

    // Expand env vars
    if has_dollar {
        path = match shellexpand::env(&path) {
            Ok(x) => x.to_string(),
            Err(e) => {
                OUTPUT.error(&format!(
                    "RStudio config path contains unknown environment variable: {}",
                    e.var_name
                ));
                error!(
                    "RStudio config path contains unknown environment variable: {}",
                    e.var_name
                );
                bail!(
                    "RStudio config path contains unknown environment variable: {}",
                    e.var_name
                );
            }
        };
    }

    // Expand empty path and ~
    if empty || has_tilde {
        path = shellexpand::tilde(&path).to_string();
    }

    let mut xdg = std::path::PathBuf::from(path);
    if !final_path {
        xdg.push("RStudio");
    }

    Ok(xdg)
}

pub fn sc_rstudio_(
    version: Option<&str>,
    project: Option<&str>,
    arg: Option<&OsStr>,
) -> Result<(), Box<dyn Error>> {
    debug!("Looking into starting RStudio");

    let def = sc_get_default()?;

    // def is None if there is no default R version set
    let version = match version {
        Some(x) => Some(x.to_string()),
        None => def,
    };

    let mut args = match project {
        None => osvec!["/c", "start", "/b", "rstudio"],
        Some(p) => osvec!["/c", "start", "/b", p],
    };

    if let Some(arg) = arg {
        args.push(arg.to_os_string());
    }

    // set version env var if needed
    let old = std::env::var("RSTUDIO_WHICH_R");
    if let Some(ref v) = version {
        let ver = v.to_string();
        let ver = check_installed(&ver)?;
        // TODO: this does not work aarch64 windows
        let bin = get_r_binary_x64(&ver)?;
        OUTPUT.status(&format!("Setting RSTUDIO_WHICH_R=\"{}\"", bin.display()));
        info!("Setting RSTUDIO_WHICH_R=\"{}\"", bin.display());
        std::env::set_var("RSTUDIO_WHICH_R", bin);
    }

    let cmdline = osjoin(args.to_owned(), " ");
    OUTPUT.status(&format!("Running cmd.exe {}", cmdline));
    info!("Running cmd.exe {}", cmdline);
    let status = run("cmd.exe".into(), args, "start");

    // restore version env var
    if let Some(_) = version {
        match old {
            Ok(v) => std::env::set_var("RSTUDIO_WHICH_R", v),
            Err(_) => std::env::remove_var("RSTUDIO_WHICH_R"),
        };
    }

    match status {
        Err(e) => {
            OUTPUT.error(&format!("`start` failed: {}", e.to_string()));
            error!("`start` failed: {}", e.to_string());
            bail!("`start` failed: {}", e.to_string());
        }
        _ => {}
    };

    Ok(())
}

pub fn get_system_profile(rver: &str) -> Result<PathBuf, Box<dyn Error>> {
    let rroot = get_r_root_for(rver)?;
    let base = version_dir_key(rver);
    let path = Path::new(&rroot).join(r_dirname(&base)?);
    let profile = path.join("library/base/R/Rprofile");
    Ok(profile)
}

pub fn get_r_binary(rver: &str) -> Result<PathBuf, Box<dyn Error>> {
    debug!("Finding R {} binary", rver);
    let rroot = get_r_root_for(rver)?;
    let base = version_dir_key(rver);
    let bin = Path::new(&rroot)
        .join(r_dirname(&base)?)
        .join("bin")
        .join("R.exe");
    debug!("R {} binary: {}", rver, bin.display());
    Ok(bin)
}

pub fn get_r_binary_x64(rver: &str) -> Result<PathBuf, Box<dyn Error>> {
    debug!("Finding R {} binary", rver);
    let rroot = get_r_root_for(rver)?;
    let base = version_dir_key(rver);
    let bin = Path::new(&rroot)
        .join(r_dirname(&base)?)
        .join("bin")
        .join("x64")
        .join("R.exe");
    debug!("R {} binary: {}", rver, bin.display());
    Ok(bin)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Drop guard: removes the temp install tree when the test ends.
    struct TempDir(PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    // Write a fake installed R tree with an `include/Rversion.h` holding the
    // given version and status, and return its root (with a cleanup guard).
    fn fake_install(version: &str, status: &str) -> (PathBuf, TempDir) {
        let root = std::env::temp_dir().join(format!(
            "rig-test-{}-{}",
            std::process::id(),
            // Unique per call within a process.
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let inc = root.join("include");
        std::fs::create_dir_all(&inc).expect("create include dir");
        // Split "4.6.0" into R_MAJOR "4" and R_MINOR "6.0", as R's header does.
        let (major, minor) = version.split_once('.').expect("version has a dot");
        let content = format!(
            "#define R_VERSION 263680\n\
             #define R_NICK \"Some Nickname\"\n\
             #define R_MAJOR  \"{}\"\n\
             #define R_MINOR  \"{}\"\n\
             #define R_STATUS \"{}\"\n\
             #define R_YEAR  \"2026\"\n",
            major, minor, status
        );
        std::fs::write(inc.join("Rversion.h"), content).expect("write Rversion.h");
        (root.clone(), TempDir(root))
    }

    #[test]
    fn read_rversion_h_parses_version_and_status() {
        let (root, _g) = fake_install("4.6.0", "");
        let (v, s) = read_rversion_h(&root).unwrap();
        assert_eq!(v, "4.6.0");
        assert_eq!(s, "");

        let (root, _g) = fake_install("4.7.0", "Under development (unstable)");
        let (v, s) = read_rversion_h(&root).unwrap();
        assert_eq!(v, "4.7.0");
        assert_eq!(s, "Under development (unstable)");

        let (root, _g) = fake_install("4.6.1", "Patched");
        let (v, s) = read_rversion_h(&root).unwrap();
        assert_eq!(v, "4.6.1");
        assert_eq!(s, "Patched");
    }

    #[test]
    fn read_rversion_h_errors_without_version() {
        let (root, _g) = fake_install("4.6.0", "");
        std::fs::write(
            root.join("include").join("Rversion.h"),
            "#define R_STATUS \"\"\n",
        )
        .unwrap();
        assert!(read_rversion_h(&root).is_err());
    }

    #[test]
    fn user_install_name_maps_status_to_name() {
        // Use the native arch so no `-x86_64` suffix is added, keeping the
        // assertions host-independent.
        let arch = get_native_arch();

        let (root, _g) = fake_install("4.6.0", "");
        assert_eq!(user_install_name(&root, arch).unwrap(), "4.6.0");

        let (root, _g) = fake_install("4.7.0", "Under development (unstable)");
        assert_eq!(user_install_name(&root, arch).unwrap(), "devel");

        let (root, _g) = fake_install("4.6.1", "Patched");
        assert_eq!(user_install_name(&root, arch).unwrap(), "next");
    }

    #[test]
    fn re_alias_matches_plain_and_x86_64_suffixed_names() {
        let re = re_alias();
        // Plain alias names.
        assert!(re.is_match("R-oldrel.bat"));
        assert!(re.is_match("R-release.bat"));
        assert!(re.is_match("R-next.bat"));
        // x86_64 build on aarch64 gets an `-x86_64` suffix; these must be
        // recognized too, otherwise sc_system_make_links deletes them and
        // find_aliases does not list them.
        assert!(re.is_match("R-oldrel-x86_64.bat"));
        assert!(re.is_match("R-release-x86_64.bat"));
        assert!(re.is_match("R-next-x86_64.bat"));
        // Version links and other arch suffixes are not aliases.
        assert!(!re.is_match("R-4.6.0.bat"));
        assert!(!re.is_match("R-4.6.0-x86_64.bat"));
        assert!(!re.is_match("R-oldrel-aarch64.bat"));
    }

    #[test]
    fn rtools_dir_name_encodes_version_and_arch() {
        assert_eq!(rtools_dir_name("44", "x86_64"), "Rtools44");
        assert_eq!(rtools_dir_name("44", "aarch64"), "Rtools44-aarch64");
        assert_eq!(rtools_dir_name("40", "x86_64"), "Rtools40");
        // Rtools 3.x has no version suffix; ditto an empty (unspecified) version.
        assert_eq!(rtools_dir_name("35", "x86_64"), "Rtools");
        assert_eq!(rtools_dir_name("", "x86_64"), "Rtools");
        assert_eq!(rtools_dir_name("", "aarch64"), "Rtools-aarch64");
    }

    #[test]
    fn rtools_home_var_is_arch_qualified() {
        assert_eq!(rtools_home_var("44", "x86_64"), "RTOOLS44_HOME");
        assert_eq!(rtools_home_var("45", "x86_64"), "RTOOLS45_HOME");
        assert_eq!(rtools_home_var("44", "aarch64"), "RTOOLS44_AARCH64_HOME");
    }

    #[test]
    fn rtools_renviron_lines_per_version() {
        // Use non-existent paths so short_path_name falls back to the long path; the
        // value must use forward slashes and be quoted so spaces would not break it.
        let p = Path::new("C:\\rt\\Rtools44");
        // 4.2+ sets only the home var; R derives PATH/flags from it.
        assert_eq!(
            rtools_renviron_lines("44", "x86_64", p, true),
            "RTOOLS44_HOME=\"C:/rt/Rtools44\""
        );
        // 4.0 in user mode sets the home var and the explicit PATH line.
        let l40 = rtools_renviron_lines("40", "x86_64", Path::new("C:\\rt\\Rtools40"), true);
        assert!(l40.starts_with("RTOOLS40_HOME=\"C:/rt/Rtools40\"\n"));
        assert!(l40.contains("${RTOOLS40_HOME}/ucrt64/bin"));
        // 4.0 in admin mode keeps only the PATH line (installer sets the var).
        let l40a = rtools_renviron_lines("40", "x86_64", Path::new("C:\\Rtools40"), false);
        assert!(!l40a.contains("RTOOLS40_HOME=\""));
        assert!(l40a.contains("${RTOOLS40_HOME}/usr/bin"));
        // 3.5 prepends <path>/bin to PATH.
        assert_eq!(
            rtools_renviron_lines("35", "x86_64", Path::new("C:\\Rtools"), false),
            "PATH=\"C:/Rtools/bin;${PATH}\""
        );
    }

    #[test]
    fn user_mode_install_spec_uses_alias_when_available() {
        let aliases = vec![
            Alias {
                alias: "release".to_string(),
                version: "4.6.0".to_string(),
            },
            Alias {
                alias: "oldrel".to_string(),
                version: "4.5.1".to_string(),
            },
        ];
        // A version an alias points at is reinstalled via the alias name.
        assert_eq!(
            user_mode_install_spec("4.6.0", &aliases),
            ("release".to_string(), get_native_arch().to_string())
        );
        assert_eq!(
            user_mode_install_spec("4.5.1", &aliases),
            ("oldrel".to_string(), get_native_arch().to_string())
        );
    }

    #[test]
    fn user_mode_install_spec_uses_version_without_alias() {
        let aliases = vec![Alias {
            alias: "release".to_string(),
            version: "4.6.0".to_string(),
        }];
        assert_eq!(
            user_mode_install_spec("4.4.3", &aliases),
            ("4.4.3".to_string(), get_native_arch().to_string())
        );
        assert_eq!(
            user_mode_install_spec("devel", &[]),
            ("devel".to_string(), get_native_arch().to_string())
        );
    }

    #[test]
    fn admin_name_from_link_line_extracts_version() {
        // Plain admin link into C:\Program Files\R.
        assert_eq!(
            admin_name_from_link_line("@\"C:\\Program Files\\R\\R-4.6.0\\bin\\R\" %*"),
            Some("4.6.0".to_string())
        );
        assert_eq!(
            admin_name_from_link_line("@\"C:\\Program Files\\R\\R-devel\\bin\\R\" %*"),
            Some("devel".to_string())
        );
        // Not an R link.
        assert_eq!(admin_name_from_link_line("@echo off"), None);
    }
}
