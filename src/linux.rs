#![cfg(target_os = "linux")]

use regex::Regex;
use std::error::Error;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use clap::ArgMatches;
use log::{debug, error, info, trace, warn};
use simple_error::*;

use crate::rversion::*;

use crate::alias::*;
use crate::common::*;
use crate::download::*;
use crate::escalate::*;
use crate::library::*;
use crate::output::OUTPUT;
use crate::platform::*;
use crate::repos::*;
use crate::resolve::get_resolve;
use crate::run::*;
use crate::utils::*;

pub const R_ROOT_: &str = "/opt/R";
pub const R_VERSIONDIR: &str = "{}";

// Portable R builds used for user-mode installation. These "distros" resolve to
// relocatable `.tar.gz` files on the R-hub version server, picked by libc type.
pub const USER_DISTRO_GLIBC: &str = "linux-manylinux-2.34";
pub const USER_DISTRO_MUSL: &str = "linux-musllinux-1.2";

// Minimum libc versions the portable user-mode R builds support. These match
// the manylinux/musllinux baselines the `.tar.gz` files are built against.
pub const MIN_GLIBC_VERSION: &str = "2.34";
pub const MIN_MUSL_VERSION: &str = "1.2";

macro_rules! strvec {
    // match a list of expressions separated by comma:
    ($($str:expr),*) => ({
        // create a Vec with this list of expressions,
        // calling String::from on each:
        vec![$(String::from($str),)*] as Vec<String>
    });
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
    Ok("{}/lib/R/library".to_string())
}

pub fn get_r_binpath() -> Result<String, Box<dyn Error>> {
    Ok("{}/bin/R".to_string())
}

pub fn get_r_base_profile() -> Result<String, Box<dyn Error>> {
    Ok("{}/lib/R/library/base/R/Rprofile".to_string())
}

pub fn get_r_etc_path() -> Result<String, Box<dyn Error>> {
    Ok("{}/lib/R/etc".to_string())
}

pub fn get_r_versiondir() -> Result<String, Box<dyn Error>> {
    Ok(R_VERSIONDIR.to_string())
}

pub fn get_r_current() -> Result<String, Box<dyn Error>> {
    if let Some(dir) = get_r_install_dir()? {
        return Ok(format!("{}/current", dir));
    }
    Ok("/opt/R/current".to_string())
}

/// The C library implementation a Linux system is built against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibcType {
    Glibc,
    Musl,
}

impl std::fmt::Display for LibcType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LibcType::Glibc => write!(f, "glibc"),
            LibcType::Musl => write!(f, "musl"),
        }
    }
}

/// The detected C library type and version of the running system.
#[derive(Debug, Clone)]
pub struct Libc {
    pub kind: LibcType,
    pub version: String,
}

/// Detect the system C library type (glibc or musl) and its version.
///
/// This works by running `ldd --version`. glibc's `ldd` writes its banner to
/// stdout, while musl's `ldd` writes it to stderr (and may exit non-zero), so
/// we inspect both streams.
pub fn detect_libc() -> Result<Libc, Box<dyn Error>> {
    let out = Command::new("ldd").arg("--version").output()?;

    // Combine both streams: glibc reports on stdout, musl on stderr.
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    trace!("ldd --version output: {:?}", text);

    let libc = parse_libc(&text)?;
    debug!("Detected libc: {} {}", libc.kind, libc.version);
    Ok(libc)
}

/// Parse the combined stdout+stderr output of `ldd --version` into a [`Libc`].
fn parse_libc(text: &str) -> Result<Libc, Box<dyn Error>> {
    let lower = text.to_lowercase();
    let kind = if lower.contains("musl") {
        LibcType::Musl
    } else if lower.contains("glibc") || lower.contains("gnu libc") {
        LibcType::Glibc
    } else {
        bail!("Could not determine libc type from `ldd --version` output");
    };

    // Grab the first version-looking token, e.g. "2.35" or "1.2.4".
    let re = Regex::new(r"[0-9]+\.[0-9]+(\.[0-9]+)?")?;
    let version = match re.find(text) {
        Some(m) => m.as_str().to_string(),
        None => bail!(
            "Could not determine {} version from `ldd --version` output",
            kind
        ),
    };

    Ok(Libc { kind, version })
}

/// Compare two dotted numeric version strings (e.g. "2.35" and "2.34").
/// Returns `true` if `version` is greater than or equal to `minimum`.
/// Non-numeric or missing components are treated as `0`.
fn version_at_least(version: &str, minimum: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.split('.')
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect()
    };
    let v = parse(version);
    let m = parse(minimum);
    for i in 0..m.len() {
        let vi = v.get(i).copied().unwrap_or(0);
        if vi != m[i] {
            return vi > m[i];
        }
    }
    true
}

/// Ensure the detected libc is one we have portable user-mode R builds for, and
/// that it is recent enough. glibc must be >= 2.34, musl must be >= 1.2.
fn check_libc_supported(libc: &Libc) -> Result<(), Box<dyn Error>> {
    let minimum = match libc.kind {
        LibcType::Glibc => MIN_GLIBC_VERSION,
        LibcType::Musl => MIN_MUSL_VERSION,
    };
    if !version_at_least(&libc.version, minimum) {
        bail!(
            "Unsupported {} version {}: user-mode R installation requires \
             {} {} or later",
            libc.kind,
            libc.version,
            libc.kind,
            minimum
        );
    }
    Ok(())
}

/// The platform string used to resolve a user-mode (portable) R build, chosen
/// from the system libc: glibc systems get the manylinux build, musl systems the
/// musllinux build. Both resolve to a relocatable `.tar.gz`.
///
/// Errors if the libc type is unknown, or if it is too old for the portable
/// builds (glibc < 2.34, musl < 1.2).
pub fn user_mode_platform() -> Result<String, Box<dyn Error>> {
    let libc = detect_libc()?;
    check_libc_supported(&libc)?;
    let platform = match libc.kind {
        LibcType::Glibc => USER_DISTRO_GLIBC,
        LibcType::Musl => USER_DISTRO_MUSL,
    };
    Ok(platform.to_string())
}

pub fn sc_add(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    if args.value_source("arch") == Some(clap::parser::ValueSource::CommandLine) {
        OUTPUT.error("`--arch` is not supported on Linux.");
        error!("`--arch` is not supported on Linux");
        bail!("`--arch` is not supported on Linux");
    }

    let mode = get_mode()?;
    if mode == Mode::Admin {
        escalate("adding new R versions")?;
    }

    // This is needed to fix static linking on Arm Linux :(
    let uid = nix::unistd::getuid().as_raw();
    if false {
        println!("{}", uid);
    }

    let version = get_resolve(args)?;
    let alias = get_alias(args);
    let ver = version.version.to_owned();
    let verstr = match ver {
        Some(ref x) => x,
        None => "???",
    };

    let url: String = match &version.url {
        Some(s) => s.to_string(),
        None => {
            OUTPUT.error(&format!(
                "Cannot find a download url for R version {}",
                verstr
            ));
            error!("Cannot find a download url for R version {}", verstr);
            bail!("Cannot find a download url for R version {}", verstr)
        }
    };

    let filename = basename(&url).unwrap_or_else(|| "foo");
    let tmp_dir = std::env::temp_dir().join("rig");
    let target = tmp_dir.join(&filename);
    if target.exists() && not_too_old(&target) {
        OUTPUT.success(&format!("{} is cached at {}", filename, target.display()));
        info!("{} is cached at {}", filename, target.display());
    } else {
        OUTPUT.status(&format!("Downloading {} -> {}", url, target.display()));
        info!("Downloading {} -> {}", url, target.display());
        let client = &reqwest::Client::new();
        download_file(client, &url, &target.as_os_str())?;
    }

    let dirname = if mode == Mode::User {
        safe_user_install(&target, &version)?
    } else {
        let platform = detect_platform()?;
        add_package(target.as_os_str(), &platform)?
    };

    set_default_if_none(dirname.to_string())?;

    // In user mode, make sure the `R`/`Rscript` aliases in the binary directory
    // exist and point at the current default. `set_default_if_none` only creates
    // them on the very first install; refresh them here so they are present even
    // when a default was already set.
    if mode == Mode::User {
        make_current_r_links()?;
        // Download the CA certificate bundle (if not already present) and point
        // this installation at it. A failure here (e.g. no network) should not
        // abort the install, so only warn.
        if let Err(e) = setup_user_cert(&dirname.to_string(), false) {
            OUTPUT.warn(&format!("Could not set up CA certificate bundle: {}", e));
            warn!("Could not set up CA certificate bundle: {}", e);
        }
    }

    library_update_rprofile(&dirname.to_string())?;
    if mode == Mode::Admin {
        check_usr_bin_sed(&dirname.to_string())?;
    }
    sc_system_make_links()?;
    match alias {
        Some(alias) => add_alias(&dirname, &alias)?,
        None => {}
    };

    let setup = interpret_repos_args(args, true);
    repos_setup(Some(vec![dirname.to_string()]), setup)?;

    if args.get_flag("without-sysreqs") {
        set_sysreqs_false(Some(vec![dirname.to_string()]))?;
    }

    if !args.get_flag("without-pak") {
        let explicit =
            args.value_source("pak-version") == Some(clap::parser::ValueSource::CommandLine);
        system_add_pak(
            Some(vec![dirname.to_string()]),
            args.get_one::<String>("pak-version").unwrap(),
            // If this is specified then we always re-install
            explicit,
        )?;
    }

    Ok(())
}

fn select_linux_tools(platform: &OsVersion) -> Result<LinuxTools, Box<dyn Error>> {
    if platform.distro.as_deref() == Some("debian") || platform.distro.as_deref() == Some("ubuntu")
    {
        Ok(LinuxTools {
            package_name: "r-{}".to_string(),
            install: vec![
                strvec!["apt-get", "update"],
                strvec![
                    "apt",
                    "install",
                    "--reinstall",
                    "-y",
                    "-o=Dpkg::Use-Pty=0",
                    "-o=Apt::Cmd::Disable-Script-Warning=1",
                    "{}"
                ],
            ],
            get_package_name: strvec!["dpkg", "--field", "{}", "Package"],
            delete: strvec![
                "apt-get",
                "remove",
                "-y",
                "-o=Dpkg::Use-Pty=0",
                "--purge",
                "{}"
            ],
            is_installed: strvec!["dpkg", "-s", "{}"],
        })
    } else if platform.distro.as_deref() == Some("opensuse")
        || platform.distro.as_deref() == Some("sles")
    {
        Ok(LinuxTools {
            package_name: "R-{}".to_string(),
            install: vec![strvec!["zypper", "--no-gpg-checks", "install", "-y", "{}"]],
            get_package_name: strvec!["rpm", "-q", "--qf", "%{NAME}", "-p", "{}"],
            is_installed: strvec!["rpm", "-q", "{}"],
            delete: strvec!["zypper", "remove", "-y", "{}"],
        })
    } else if platform.distro.as_deref() == Some("fedora") {
        Ok(LinuxTools {
            package_name: "R-{}".to_string(),
            install: vec![strvec!["dnf", "install", "-y", "{}"]],
            get_package_name: strvec!["rpm", "-q", "--qf", "%{NAME}", "-p", "{}"],
            is_installed: strvec!["rpm", "-q", "{}"],
            delete: strvec!["dnf", "remove", "-y", "{}"],
        })
    } else if platform.distro.as_deref() == Some("centos")
        && (platform.version.as_deref() == Some("7")
            || platform
                .version
                .as_ref()
                .map_or(false, |v| v.starts_with("7.")))
    {
        Ok(LinuxTools {
            package_name: "R-{}".to_string(),
            install: vec![
                strvec!["yum", "install", "-y", "epel-release"],
                strvec!["yum", "install", "-y", "{}"],
            ],
            get_package_name: strvec!["rpm", "-q", "--qf", "%{NAME}", "-p", "{}"],
            is_installed: strvec!["rpm", "-q", "{}"],
            delete: strvec!["yum", "remove", "-y", "{}"],
        })
    } else if platform.distro.as_deref() == Some("rhel")
        && (platform.version.as_deref() == Some("7")
            || platform
                .version
                .as_ref()
                .map_or(false, |v| v.starts_with("7.")))
    {
        Ok(LinuxTools{
            package_name: "R-{}".to_string(),
            install: vec![
                strvec!["bash", "-c", "rpm -q epel-release || yum install -y https://dl.fedoraproject.org/pub/epel/epel-release-latest-7.noarch.rpm"],
                strvec!["yum", "--enablerepo", "rhel-7-server-optional-rpms", "install", "-y", "{}"]
            ],
            get_package_name:
                strvec!["rpm", "-q", "--qf", "%{NAME}", "-p", "{}"],
            is_installed:
                strvec!["rpm", "-q", "{}"],
            delete:
                strvec!["yum", "remove", "-y", "{}"]
        })
    } else if (platform.distro.as_deref() == Some("rhel")
        || platform.distro.as_deref() == Some("almalinux")
        || platform.distro.as_deref() == Some("rocky"))
        && (platform.version.as_deref() == Some("8")
            || platform
                .version
                .as_ref()
                .map_or(false, |v| v.starts_with("8.")))
    {
        Ok(LinuxTools {
            package_name: "R-{}".to_string(),
            install: vec![strvec!["dnf", "install", "-y", "{}"]],
            get_package_name: strvec!["rpm", "-q", "--qf", "%{NAME}", "-p", "{}"],
            is_installed: strvec!["rpm", "-q", "{}"],
            delete: strvec!["dnf", "remove", "-y", "{}"],
        })
    } else if (platform.distro.as_deref() == Some("almalinux")
        || platform.distro.as_deref() == Some("rocky"))
        && (platform.version.as_deref() == Some("9")
            || platform
                .version
                .as_ref()
                .map_or(false, |v| v.starts_with("9.")))
    {
        Ok(LinuxTools {
            package_name: "R-{}".to_string(),
            install: vec![
                strvec!["dnf", "install", "-y", "dnf-plugins-core"],
                strvec!["dnf", "config-manager", "--set-enabled", "crb"],
                strvec!["dnf", "install", "-y", "epel-release"],
                strvec!["dnf", "install", "-y", "{}"],
            ],
            get_package_name: strvec!["rpm", "-q", "--qf", "%{NAME}", "-p", "{}"],
            is_installed: strvec!["rpm", "-q", "{}"],
            delete: strvec!["dnf", "remove", "-y", "{}"],
        })
    } else if platform.distro.as_deref() == Some("rhel")
        && (platform.version.as_deref() == Some("9")
            || platform
                .version
                .as_ref()
                .map_or(false, |v| v.starts_with("9.")))
    {
        let crb = "codeready-builder-for-rhel-9-".to_string() + &platform.arch + "-rpms";
        Ok(LinuxTools{
                package_name: "R-{}".to_string(),
                install: vec![
            strvec!["bash", "-c", "rpm -q epel-release || dnf install -y https://dl.fedoraproject.org/pub/epel/epel-release-latest-9.noarch.rpm"],
                    strvec!["dnf", "install", "--enablerepo", crb, "-y", "{}"]
                ],
                get_package_name:
                    strvec!["rpm", "-q", "--qf", "%{NAME}", "-p", "{}"],
                is_installed:
                    strvec!["rpm", "-q", "{}"],
                delete:
                    strvec!["dnf", "remove", "-y", "{}"]
            })
    } else if platform.distro.as_deref() == Some("almalinux")
        || platform.distro.as_deref() == Some("rocky")
        || platform.distro.as_deref() == Some("rhel")
    {
        Ok(LinuxTools {
            package_name: "R-{}".to_string(),
            install: vec![strvec!["dnf", "install", "-y", "{}"]],
            get_package_name: strvec!["rpm", "-q", "--qf", "%{NAME}", "-p", "{}"],
            is_installed: strvec!["rpm", "-q", "{}"],
            delete: strvec!["yum", "remove", "-y", "{}"],
        })
    } else {
        let distro = platform.distro.as_deref().unwrap_or("<unknown>");
        let version = platform.version.as_deref().unwrap_or("<unknown>");
        OUTPUT.error(&format!(
            "I don't know how to install a system package on {} {}",
            distro, version
        ));
        error!(
            "I don't know how to install a system package on {} {}",
            distro, version
        );
        bail!(
            "I don't know how to install a system package on {} {}",
            distro,
            version
        );
    }
}

fn add_package(path: &OsStr, platform: &OsVersion) -> Result<String, Box<dyn Error>> {
    let tools = select_linux_tools(platform)?;

    for cmd in tools.install.iter() {
        let cmd = format_cmd_args(cmd.to_vec(), path);
        let cmdline = cmd.join(&OsString::from(" "));
        OUTPUT.status(&format!("Running {:?}", cmdline));
        info!("Running {:?}", cmdline);
        let cmd0 = cmd[0].to_owned();
        run(cmd0, cmd[1..].to_vec(), "installation")?;
    }

    let get_package_name = format_cmd_args(tools.get_package_name, path);
    let cmd0 = get_package_name[0].to_owned();
    let out = Command::new(cmd0)
        .args(get_package_name[1..].to_vec())
        .output()?;

    let std = String::from_utf8(out.stdout)?;

    let re = Regex::new("^[rR]-(.*)\\s*$")?;
    let ver = re.replace(&std, "${1}");

    Ok(ver.to_string())
}

// User-mode installation: unpack a portable R build (a `.tar.gz`) into the
// per-user R root (`~/.local/share/rig/r/<version>`), the same place macOS uses.
// No privilege escalation and no system package manager.
fn safe_user_install(target: &Path, version: &Rversion) -> Result<String, Box<dyn Error>> {
    let root = PathBuf::from(get_r_root()?);
    std::fs::create_dir_all(&root)?;

    // Unpack into a staging directory next to the destination, so the final
    // rename stays on the same filesystem.
    let staging = root.join(format!(".rig-extract-{}", std::process::id()));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir_all(&staging)?;

    OUTPUT.status(&format!("Unpacking {}", target.display()));
    info!("Unpacking {} into {}", target.display(), staging.display());
    if let Err(e) = unpack_tar_gz(target, &staging) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }

    // The build is wrapped in a single top-level directory (named after the
    // version); descend into it so the R tree (`bin/`, `lib/`, `share/`) sits at
    // the top of the install directory.
    let content = match single_subdir(&staging)? {
        Some(sub) => sub,
        None => staging.clone(),
    };

    let dirname = user_install_dirname(&content, version)?;
    let dest = root.join(&dirname);
    if dest.exists() {
        OUTPUT.status(&format!("Removing existing {}", dest.display()));
        info!("Removing existing {}", dest.display());
        std::fs::remove_dir_all(&dest)?;
    }

    debug!("Moving {} to {}", content.display(), dest.display());
    std::fs::rename(&content, &dest)?;

    // Clean up the staging wrapper if we descended into a subdirectory.
    if staging.exists() {
        let _ = std::fs::remove_dir_all(&staging);
    }

    write_install_metadata(&dest)?;

    OUTPUT.success(&format!("Installed R to {}", dest.display()));
    info!("Installed R to {}", dest.display());

    Ok(dirname)
}

// Write a `metadata.json` file at the top level of a user-mode R installation,
// recording the platform of the portable build (the manylinux/musllinux distro
// string, selected by libc).
fn write_install_metadata(dest: &Path) -> Result<(), Box<dyn Error>> {
    let platform = user_mode_platform()?;
    let metadata = serde_json::json!({ "platform": platform });
    let path = dest.join("metadata.json");
    debug!("Writing installation metadata to {}", path.display());
    std::fs::write(&path, serde_json::to_string_pretty(&metadata)?)?;
    Ok(())
}

pub fn read_install_platform(dir: &Path) -> Option<String> {
    let path = dir.join("metadata.json");
    let text = std::fs::read_to_string(&path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value.get("platform")?.as_str().map(|s| s.to_string())
}

// Extract a gzip-compressed tarball into `dest`, in-process (no external `tar`).
fn unpack_tar_gz(archive: &Path, dest: &Path) -> Result<(), Box<dyn Error>> {
    let file = std::fs::File::open(archive)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut ar = tar::Archive::new(decoder);
    ar.set_preserve_permissions(true);
    ar.set_overwrite(true);
    ar.unpack(dest)?;
    Ok(())
}

// If `dir` contains exactly one entry and it is a directory, return it. Used to
// strip an optional top-level directory inside the build tarball.
fn single_subdir(dir: &Path) -> Result<Option<PathBuf>, Box<dyn Error>> {
    let mut entries = std::fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    if entries.len() != 1 {
        return Ok(None);
    }
    let entry = entries.remove(0);
    if entry.file_type()?.is_dir() {
        Ok(Some(entry.path()))
    } else {
        Ok(None)
    }
}

// Determine the user-mode installation directory name. Released versions are
// named after their version number; development builds are named `devel`
// (R-devel) or `next` (R-next), as recorded by the `R_STATUS` macro in
// `include/Rversion.h`.
fn user_install_dirname(content: &Path, version: &Rversion) -> Result<String, Box<dyn Error>> {
    let status = read_r_status(content);
    let base = match crate::common::user_mode_dev_dirname(status.as_deref()) {
        Some(name) => name,
        None => version
            .version
            .clone()
            .ok_or_else(|| SimpleError::new("Resolved R build has no version"))?,
    };
    debug!(
        "User install directory name is {} (R_STATUS = {:?})",
        base, status
    );
    Ok(base)
}

// Read the `R_STATUS` macro from `Rversion.h` in an unpacked R build. Returns
// None if the header cannot be located, an empty string for released versions,
// "Under development (unstable)" for R-devel, and another label for R-next.
fn read_r_status(content: &Path) -> Option<String> {
    let candidates = ["include/Rversion.h", "lib/R/include/Rversion.h"];
    let re = Regex::new(r#"(?m)^\s*#define\s+R_STATUS\s+"(.*)"\s*$"#).ok()?;
    for cand in candidates {
        if let Ok(text) = std::fs::read_to_string(content.join(cand)) {
            let status = re
                .captures(&text)
                .and_then(|caps| caps.get(1))
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            return Some(status);
        }
    }
    None
}

pub fn sc_rm(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let mode = get_mode()?;
    if mode == Mode::Admin {
        escalate("removing R versions")?;
    }
    let vers = args.get_many::<String>("version");
    if vers.is_none() {
        return Ok(());
    }
    let vers = vers.unwrap();

    // In admin mode R is a system package, so we ask the package manager to
    // remove it. In user mode it is just an unpacked directory tree.
    let tools = if mode == Mode::Admin {
        Some(select_linux_tools(&detect_platform()?)?)
    } else {
        None
    };
    for ver in vers {
        let ver = check_installed(ver)?;

        if let Some(ref tools) = tools {
            let pkgname = tools.package_name.replace("{}", &ver);
            let opkgname = OsStr::new(&pkgname);
            let cmd = format_cmd_args(tools.is_installed.clone(), opkgname);
            let cmd0 = cmd[0].to_owned();
            let out = Command::new(cmd0).args(cmd[1..].to_vec()).output()?;

            if out.status.success() {
                OUTPUT.status(&format!("Removing {} package", pkgname));
                info!("Removing {} package", pkgname);
                let cmd = format_cmd_args(tools.delete.clone(), opkgname);
                let cmd0 = cmd[0].to_owned();
                run(cmd0, cmd[1..].to_vec(), "deleting system package")?;
            } else {
                OUTPUT.success(&format!("{} package is not installed", pkgname));
                info!("{} package is not installed", pkgname);
            }
        }

        let rroot = get_r_root()?;
        let dir = Path::new(&rroot);
        let dir = dir.join(&ver);
        if dir.exists() {
            OUTPUT.status(&format!("Removing {}", dir.display()));
            info!("Removing {}", dir.display());
            std::fs::remove_dir_all(&dir)?;
        }
    }

    sc_system_make_links()?;

    Ok(())
}

pub fn sc_system_make_links() -> Result<(), Box<dyn Error>> {
    let mode = get_mode()?;
    let binary_dir = get_binary_dir()?;
    if mode == Mode::Admin {
        escalate("making R-* quick links")?;
    } else {
        std::fs::create_dir_all(&binary_dir)?;
        check_local_bin_path()?;
    }
    let vers = sc_get_list()?;
    let rroot = get_r_root()?;
    let base = Path::new(&rroot);
    let binpath = get_r_binpath()?;

    // Create new links
    for ver in vers {
        let linkfile = Path::new(&binary_dir).join("R-".to_string() + &ver);
        let target = base.join(binpath.replace("{}", &ver));
        if !linkfile.exists() {
            OUTPUT.status(&format!(
                "Adding {} -> {}",
                linkfile.display(),
                target.display()
            ));
            info!("Adding {} -> {}", linkfile.display(), target.display());
            symlink(&target, &linkfile)?;
        }
    }

    // Remove dangling links
    let paths = std::fs::read_dir(&binary_dir)?;
    let re = Regex::new("^R-([0-9]+[.][0-9]+[.][0-9]+|oldrel|next|release|devel)$")?;
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
                Err(_) => {
                    OUTPUT.warn(&format!("{} is not a symlink", path.display()));
                    warn!("{} is not a symlink", path.display())
                }
                Ok(target) => {
                    if !target.exists() {
                        OUTPUT.status(&format!("Cleaning up {}", target.display()));
                        info!("Cleaning up {}", target.display());
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
    let re = Regex::new("^R-(release|oldrel)$").unwrap();
    re
}

pub fn find_aliases() -> Result<Vec<Alias>, Box<dyn Error>> {
    debug!("Finding existing aliases");

    let binary_dir = get_binary_dir()?;
    // The binary directory might not exist yet in user mode (e.g. before the
    // first install has created `~/.local/bin`), in which case there are no
    // aliases to find.
    if !Path::new(&binary_dir).exists() {
        return Ok(vec![]);
    }
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
            trace!("Checking {}", path.display());
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
                                trace!("{} -> {}", fnamestr, version);
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

fn version_from_link(pb: PathBuf) -> Option<String> {
    let osver = match pb
        .parent()
        .and_then(|x| x.parent())
        .and_then(|x| x.file_name())
    {
        None => None,
        Some(s) => Some(s.to_os_string()),
    };

    let s = match osver {
        None => None,
        Some(os) => os.into_string().ok(),
    };

    s
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
        let fname = match path.file_name() {
            Some(x) => x,
            None => continue,
        };
        // If the path is not UTF-8, we'll skip it, this should not happen
        let fname = match fname.to_str() {
            Some(x) => x,
            None => continue,
        };
        if fname == "current" {
            continue;
        }
        // Skip the staging directory used while extracting an installation
        if fname.starts_with(".rig-extract-") {
            continue;
        }
        // If there is no bin/R, then this is not an R installation
        let rbin = path.join("bin").join("R");
        if !rbin.exists() {
            continue;
        }

        vers.push(fname.to_string());
    }
    vers.sort();
    Ok(vers)
}

// Create (or refresh) the `R` and `Rscript` symlinks in the binary directory.
// They point at the current default R through the `current` symlink (e.g.
// `~/.local/share/rig/r/current/bin/R` in user mode), so they keep working as
// the default version changes. In user mode the binary directory is
// `~/.local/bin`, which is how R ends up on the user's PATH.
pub fn make_current_r_links() -> Result<(), Box<dyn Error>> {
    let binary_dir = get_binary_dir()?;
    if get_mode()? == Mode::User {
        std::fs::create_dir_all(&binary_dir)?;
        check_local_bin_path()?;
    }

    let cur = get_r_current()?;
    let curdir = Path::new(&cur);

    for prog in ["R", "Rscript"] {
        let link = Path::new(&binary_dir).join(prog);
        trace!("Removing link at {}", link.display());
        std::fs::remove_file(&link).ok();
        let target = curdir.join("bin").join(prog);
        trace!("Adding {} -> {} link", link.display(), target.display());
        symlink(&target, &link)?;
    }

    Ok(())
}

pub fn sc_set_default(ver: &str) -> Result<(), Box<dyn Error>> {
    let mode = get_mode()?;
    if mode == Mode::Admin {
        escalate("setting the default R version")?;
    }
    let ver = check_installed(&ver.to_string())?;
    trace!("Setting default version to {}", ver);

    let cur = get_r_current()?;
    // Remove current link
    // We do not check if it exists, because that follows the symlink
    trace!("Removing current at {}", cur);
    std::fs::remove_file(&cur).ok();

    // Add current link
    let path = Path::new(&get_r_root()?).join(ver);
    trace!("Adding symlink at {}", cur);
    std::os::unix::fs::symlink(&path, &cur)?;

    make_current_r_links()?;

    Ok(())
}

pub fn sc_get_default() -> Result<Option<String>, Box<dyn Error>> {
    read_version_link(&get_r_current()?)
}

fn set_sysreqs_false(vers: Option<Vec<String>>) -> Result<(), Box<dyn Error>> {
    OUTPUT.status("Setting up automatic system requirements installation.");
    info!("Setting up automatic system requirements installation.");

    let vers = match vers {
        Some(x) => x,
        None => sc_get_list()?,
    };

    let rcode = r#"
if (Sys.getenv("PKG_SYSREQS") == "") Sys.setenv(PKG_SYSREQS = "false")
"#;

    for ver in vers {
        let ver = check_installed(&ver)?;
        let path = Path::new(&get_r_root()?).join(ver.as_str());
        let profile = path.join("lib/R/library/base/R/Rprofile".to_string());
        if !profile.exists() {
            continue;
        }

        append_to_file(&profile, vec![rcode.to_string()])?;
    }
    Ok(())
}

pub fn sc_system_allow_core_dumps(_args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    // Nothing to do on Linux
    Ok(())
}

pub fn sc_system_allow_debugger(_args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    // Nothing to do on Linux
    Ok(())
}

pub fn sc_system_allow_debugger_rstudio(_args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    // Nothing to do on Linux
    Ok(())
}

pub fn sc_system_make_orthogonal(_args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    // Nothing to do on Linux
    Ok(())
}

pub fn sc_system_fix_permissions(_args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    // Nothing to do on Linux
    Ok(())
}

pub fn sc_system_forget() -> Result<(), Box<dyn Error>> {
    // Nothing to do on Linux
    Ok(())
}

pub fn sc_system_no_openmp(_args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    // Nothing to do on Linux
    Ok(())
}

pub fn sc_clean_registry() -> Result<(), Box<dyn Error>> {
    // Nothing to do on Linux
    Ok(())
}

// `rig system user-mode` (Linux): switch rig to user mode and clean up an
// existing admin-mode setup. This:
//
//   1. Captures the admin-mode versions, default and aliases from the system
//      locations (`/opt/R` and `/usr/local/bin`), before switching the mode.
//   2. Switches the configured mode to `user`.
//   3. Reinstalls the admin-mode versions in user mode (unless --no-reinstall)
//      and restores the previous default version. Aliases are recreated by
//      reinstalling via the alias name.
//   4. Removes the admin-mode installations (the `/opt/R` directories and the
//      corresponding system packages) and the `/usr/local/bin` links.
//
// Like the macOS version, this is meant to be run as a normal user: the
// reinstallation writes into the user's home directory, while the cleanup of
// the system locations is delegated to `rig system clean-admin-r`, which
// escalates on its own.
pub fn sc_system_user_mode(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let no_reinstall = args.get_flag("no-reinstall");
    let keep_install = args.get_flag("keep-install");
    let keep_links = args.get_flag("keep-links");

    if sudo::check() == sudo::RunningAs::Root {
        OUTPUT.warn("`rig system user-mode` is meant to be run as a normal user, not with `sudo`.");
        warn!("rig system user-mode is running as root");
    }

    // 1. Capture the admin-mode setup from the system locations, before we
    //    switch the configured mode. These paths are world-readable.
    let admin_root = Path::new(R_ROOT_);
    let versions = list_admin_versions(admin_root)?;
    let default = read_version_link(&admin_root.join("current").to_string_lossy())?;
    let aliases = find_admin_aliases()?;

    // If we are going to reinstall, make sure the system can actually run the
    // portable user-mode builds before we remove the admin-mode R. Otherwise we
    // would leave the user with no working R installation.
    if !no_reinstall && !versions.is_empty() {
        if let Err(e) = user_mode_platform() {
            OUTPUT.error(&format!(
                "Cannot reinstall R in user mode: {}. Re-run with `--no-reinstall` \
                 to switch to user mode and clean up without reinstalling.",
                e
            ));
            error!("Cannot switch to user mode: {}", e);
            return Err(e);
        }
    }

    // 2. Switch to user mode so the reinstallation below targets the user
    //    location.
    crate::common::switch_to_user_mode()?;

    // 3. Reinstall the admin-mode versions in user mode and restore the
    //    previous default version. Aliases are recreated automatically by
    //    reinstalling via the alias name (see user_mode_install_spec()).
    if no_reinstall {
        if !versions.is_empty() {
            OUTPUT.status("Not reinstalling R versions in user mode (--no-reinstall)");
        }
    } else if !versions.is_empty() {
        let map = reinstall_in_user_mode(&versions, &aliases)?;
        crate::common::restore_user_mode_default(&map, &default);
    }

    // 4. Remove the admin-mode installations (unless `--keep-install`) and the
    //    `/usr/local/bin` links (unless `--keep-links`). This needs root, so it
    //    runs as a separate, self-escalating child process to avoid re-running
    //    the user-side work above under `sudo`.
    clean_admin_installations(keep_install, keep_links)?;

    Ok(())
}

fn list_admin_versions(root: &Path) -> Result<Vec<String>, Box<dyn Error>> {
    let mut vers = Vec::new();
    if !root.exists() {
        return Ok(vers);
    }
    for de in std::fs::read_dir(root)? {
        let path = de?.path();
        let name = match path.file_name().and_then(|s| s.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if name == "current" || name.starts_with(".rig-extract-") {
            continue;
        }
        if !path.join("bin").join("R").exists() {
            continue;
        }
        vers.push(name);
    }
    vers.sort();
    Ok(vers)
}

fn find_admin_aliases() -> Result<Vec<Alias>, Box<dyn Error>> {
    let bindir = Path::new("/usr/local/bin");
    let mut result: Vec<Alias> = vec![];
    if !bindir.exists() {
        return Ok(result);
    }
    let re = re_alias();
    for de in std::fs::read_dir(bindir)? {
        let path = match de {
            Ok(d) => d.path(),
            Err(_) => continue,
        };
        let name = match path.file_name().and_then(|s| s.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if !re.is_match(&name) {
            continue;
        }
        if let Ok(tgt) = std::fs::read_link(&path) {
            if tgt.exists() {
                if let Some(version) = version_from_link(tgt) {
                    result.push(Alias {
                        alias: name[2..].to_string(),
                        version,
                    });
                }
            }
        }
    }
    Ok(result)
}

fn user_mode_install_spec(admin_dir: &str, aliases: &[Alias]) -> String {
    for al in aliases {
        if al.version == admin_dir {
            return al.alias.clone();
        }
    }
    admin_dir.to_string()
}

fn expected_user_dirname(admin_root: &Path, admin_dir: &str) -> String {
    crate::common::user_mode_dev_dirname(read_r_status(&admin_root.join(admin_dir)).as_deref())
        .unwrap_or_else(|| admin_dir.to_string())
}

fn reinstall_in_user_mode(
    versions: &[String],
    aliases: &[Alias],
) -> Result<Vec<(String, String)>, Box<dyn Error>> {
    let exe = std::env::current_exe()?;
    let admin_root = Path::new(R_ROOT_);
    let mut map: Vec<(String, String)> = vec![];
    for admin_dir in versions {
        // Skip versions that are already installed in user mode.
        let udir = expected_user_dirname(admin_root, admin_dir);
        if sc_get_list()?.contains(&udir) {
            OUTPUT.status(&format!(
                "R '{}' is already installed in user mode, not reinstalling",
                udir
            ));
            info!("R '{}' already installed in user mode, skipping", udir);
            map.push((admin_dir.clone(), udir));
            continue;
        }
        let spec = user_mode_install_spec(admin_dir, aliases);
        OUTPUT.status(&format!("Reinstalling R '{}' in user mode", spec));
        info!("Reinstalling R '{}' in user mode", spec);
        let before = sc_get_list()?;
        let status = Command::new(&exe)
            .args(["add", &spec, "--without-pak"])
            .status()?;
        if !status.success() {
            OUTPUT.warn(&format!("Failed to reinstall R '{}' in user mode", spec));
            warn!("Failed to reinstall R '{}' in user mode", spec);
            continue;
        }
        let after = sc_get_list()?;
        match after.iter().find(|v| !before.contains(v)) {
            Some(udir) => map.push((admin_dir.clone(), udir.clone())),
            None => debug!(
                "No new user-mode version directory after reinstalling '{}'",
                spec
            ),
        }
    }
    Ok(map)
}

fn clean_admin_installations(keep_install: bool, keep_links: bool) -> Result<(), Box<dyn Error>> {
    if !admin_cleanup_needed(keep_install, keep_links)? {
        debug!("No admin-mode installations or links to clean up");
        return Ok(());
    }
    if keep_install {
        OUTPUT.status("Removing admin-mode links, keeping installations (this needs `sudo`)");
        info!("Removing admin-mode links, keeping installations");
    } else {
        OUTPUT.status("Removing admin-mode R installations (this needs `sudo`)");
        info!("Removing admin-mode R installations");
    }
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(&exe);
    cmd.args(["system", "clean-admin-r"]);
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

fn re_admin_link() -> Result<Regex, Box<dyn Error>> {
    Ok(Regex::new(
        "^R-([0-9]+[.][0-9]+[.][0-9]+|oldrel|next|release|devel)$",
    )?)
}

fn link_points_into_admin_root(target: &Path) -> bool {
    target.starts_with(R_ROOT_)
}

fn admin_cleanup_needed(keep_install: bool, keep_links: bool) -> Result<bool, Box<dyn Error>> {
    if !keep_install && !list_admin_versions(Path::new(R_ROOT_))?.is_empty() {
        return Ok(true);
    }
    if keep_links {
        return Ok(false);
    }
    let bindir = Path::new("/usr/local/bin");
    let paths = match std::fs::read_dir(bindir) {
        Ok(p) => p,
        Err(_) => return Ok(false),
    };
    let re = re_admin_link()?;
    for de in paths.flatten() {
        let path = de.path();
        let name = match path.file_name().and_then(|s| s.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if name != "R" && name != "Rscript" && !re.is_match(&name) {
            continue;
        }
        if let Ok(tgt) = std::fs::read_link(&path) {
            if link_points_into_admin_root(&tgt) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub fn sc_system_clean_admin_r(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let keep_install = args.get_flag("keep-install");
    let keep_links = args.get_flag("keep-links");

    escalate("removing admin-mode R installations")?;

    if !keep_install {
        remove_admin_installations()?;
    }

    if !keep_links {
        remove_admin_links()?;
    }

    Ok(())
}

fn remove_admin_installations() -> Result<(), Box<dyn Error>> {
    let admin_root = Path::new(R_ROOT_);
    let versions = list_admin_versions(admin_root)?;
    if versions.is_empty() {
        return Ok(());
    }

    let tools = match detect_platform() {
        Ok(platform) => match select_linux_tools(&platform) {
            Ok(t) => Some(t),
            Err(e) => {
                debug!("Cannot determine package tools to remove R packages: {}", e);
                None
            }
        },
        Err(e) => {
            debug!("Cannot detect platform to remove R packages: {}", e);
            None
        }
    };

    for ver in &versions {
        if let Some(ref tools) = tools {
            let pkgname = tools.package_name.replace("{}", ver);
            let opkgname = OsStr::new(&pkgname);
            let cmd = format_cmd_args(tools.is_installed.clone(), opkgname);
            let cmd0 = cmd[0].to_owned();
            match Command::new(cmd0).args(cmd[1..].to_vec()).output() {
                Ok(out) if out.status.success() => {
                    OUTPUT.status(&format!("Removing {} package", pkgname));
                    info!("Removing {} package", pkgname);
                    let cmd = format_cmd_args(tools.delete.clone(), opkgname);
                    let cmd0 = cmd[0].to_owned();
                    if let Err(e) = run(cmd0, cmd[1..].to_vec(), "deleting system package") {
                        OUTPUT.warn(&format!("Could not remove {} package: {}", pkgname, e));
                        warn!("Could not remove {} package: {}", pkgname, e);
                    }
                }
                _ => debug!("{} package is not installed", pkgname),
            }
        }

        let dir = admin_root.join(ver);
        if dir.exists() {
            OUTPUT.status(&format!("Removing {}", dir.display()));
            info!("Removing {}", dir.display());
            if let Err(e) = std::fs::remove_dir_all(&dir) {
                OUTPUT.warn(&format!("Cannot remove {}: {}", dir.display(), e));
                warn!("Cannot remove {}: {}", dir.display(), e);
            }
        }
    }

    let current = admin_root.join("current");
    if current.symlink_metadata().is_ok() {
        std::fs::remove_file(&current).ok();
    }

    Ok(())
}

fn remove_admin_links() -> Result<(), Box<dyn Error>> {
    let bindir = Path::new("/usr/local/bin");
    let paths = match std::fs::read_dir(bindir) {
        Ok(p) => p,
        Err(_) => return Ok(()),
    };
    let re = re_admin_link()?;
    for de in paths.flatten() {
        let path = de.path();
        let name = match path.file_name().and_then(|s| s.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if name != "R" && name != "Rscript" && !re.is_match(&name) {
            continue;
        }
        // Only remove symlinks that point into the admin-mode R root.
        if let Ok(tgt) = std::fs::read_link(&path) {
            if link_points_into_admin_root(&tgt) {
                OUTPUT.status(&format!("Removing {}", path.display()));
                info!("Removing {}", path.display());
                if let Err(e) = std::fs::remove_file(&path) {
                    OUTPUT.warn(&format!("Cannot remove {}: {}", path.display(), e));
                    warn!("Cannot remove {}: {}", path.display(), e);
                }
            }
        }
    }
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
    let cwd = std::env::current_dir();
    let (cmd, mut args) = match project {
        Some(p) => ("xdg-open", osvec![p]),
        None => match cwd {
            Ok(x) => ("rstudio", osvec![x]),
            Err(_) => ("rstudio", osvec![]),
        },
    };

    let mut envname = "dummy";
    let mut path = "".to_string();
    if let Some(ver) = version {
        let ver = check_installed(&ver.to_string())?;
        envname = "RSTUDIO_WHICH_R";
        path = get_r_root()? + "/" + &ver + "/bin/R"
    };

    if let Some(arg) = arg {
        if project.is_none() {
            args.push(arg.into());
        }
    }

    OUTPUT.status(&format!("Running {} {:?}", cmd, args.join(&os(" "))));
    info!("Running {} {:?}", cmd, args.join(&os(" ")));

    Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env(envname, &path)
        .spawn()?;

    Ok(())
}

pub fn get_r_binary(rver: &str) -> Result<PathBuf, Box<dyn Error>> {
    debug!("Finding R binary for R {}", rver);
    let bin = Path::new(&get_r_root()?).join(rver).join("bin/R");
    debug!("R {} binary is at {}", rver, bin.display());
    Ok(bin)
}

#[allow(dead_code)]
pub fn get_system_renviron(rver: &str) -> Result<PathBuf, Box<dyn Error>> {
    let renviron = Path::new(&get_r_root()?)
        .join(rver)
        .join("lib/R/etc/Renviron");
    Ok(renviron)
}

pub fn get_system_profile(rver: &str) -> Result<PathBuf, Box<dyn Error>> {
    let profile = Path::new(&get_r_root()?)
        .join(rver)
        .join("lib/R/library/base/R/Rprofile");
    Ok(profile)
}

// /usr/bin/sed might not be available, and R will need it (issue 119#)

fn check_usr_bin_sed(rver: &str) -> Result<(), Box<dyn Error>> {
    let usrbinsed = Path::new("/usr/bin/sed");
    let binsed = Path::new("/bin/sed");

    debug!("Checking if SED = /usr/bin/sed if OK");

    // in these cases we don't need to or cannot do anything
    if usrbinsed.exists() {
        debug!("/usr/bin/sed exists giving up");
        return Ok(());
    }
    if !binsed.exists() {
        debug!("/bin/sed missing, giving up");
        return Ok(());
    }

    let makeconf = Path::new(&get_r_root()?)
        .join(rver)
        .join("lib/R/etc/Makeconf");
    let lines: Vec<String> = match read_lines(&makeconf) {
        Ok(x) => x,
        Err(_) => {
            // this should not happen, but if it does, then
            // something weird is going on and we'll bail out
            // silently
            debug!("Cannot read Makeconf, giving up");
            return Ok(());
        }
    };
    let re_sed = Regex::new("^SED = /usr/bin/sed$")?;
    let idx_sed: Vec<usize> = grep_lines(&re_sed, &lines);
    if idx_sed.len() == 0 {
        debug!("SED is not set in Makeconf, bailing.");
        return Ok(());
    }

    let msg = "This version of R was compiled using sed at /usr/bin/sed\n        \
           but it is missing on your system.\n        \
           Run `ln -s /bin/sed /usr/bin/sed` as the root user to fix this,\n        \
           and then run rig again.";
    OUTPUT.error(msg);
    error!("{}", msg);
    bail!(msg);
}

// URL of the CA certificate bundle (the Mozilla CA list extracted by the curl
// project) that user-mode installations download and point R at.
pub const CACERT_URL: &str = "https://curl.se/ca/cacert.pem";

// Path to the user-mode CA certificate bundle. It lives in an `etc` directory
// next to the R installations under the rig data directory, e.g.
// `~/.local/share/rig/etc/cacert.pem`.
fn get_user_cert_path() -> Result<PathBuf, Box<dyn Error>> {
    let rdir = get_r_install_dir()?
        .ok_or_else(|| SimpleError::new("No user-mode R installation directory"))?;
    // `r-install-dir` is `<data>/r`; keep the bundle in a sibling `etc`.
    let data = Path::new(&rdir)
        .parent()
        .ok_or_else(|| SimpleError::new("Cannot determine rig data directory"))?;
    Ok(data.join("etc").join("cacert.pem"))
}

// `Renviron.site` of an installed R version.
fn get_renviron_site(rver: &str) -> Result<PathBuf, Box<dyn Error>> {
    Ok(Path::new(&get_r_root()?)
        .join(rver)
        .join("lib/R/etc/Renviron.site"))
}

// Download the CA certificate bundle into the user-mode data directory. When
// `force` is false an existing bundle is reused; `rig system update-certs`
// passes `force = true` to always fetch a fresh copy. rig's own HTTP client uses
// bundled roots, so this download does not depend on the system certificates.
fn download_cacert(force: bool) -> Result<PathBuf, Box<dyn Error>> {
    let cert = get_user_cert_path()?;
    if cert.exists() && !force {
        debug!("CA bundle already present at {}", cert.display());
        return Ok(cert);
    }
    if let Some(parent) = cert.parent() {
        std::fs::create_dir_all(parent)?;
    }
    OUTPUT.status(&format!(
        "Downloading CA bundle {} -> {}",
        CACERT_URL,
        cert.display()
    ));
    info!("Downloading CA bundle {} -> {}", CACERT_URL, cert.display());
    let client = &reqwest::Client::new();
    download_file(client, CACERT_URL, cert.as_os_str())?;
    Ok(cert)
}

// Point an installed R version at the rig CA bundle by setting `SSL_CERT_FILE`
// and `CURL_CA_BUNDLE` in its `Renviron.site`, inside a fenced rig block so the
// change is idempotent and easy to remove. These cover R's libcurl downloads and
// the curl / openssl packages.
fn configure_cert(rver: &str, cert: &Path) -> Result<(), Box<dyn Error>> {
    let renviron = get_renviron_site(rver)?;
    let existing = std::fs::read_to_string(&renviron).unwrap_or_default();
    let out = render_renviron_cert(&existing, &cert.to_string_lossy());

    if let Some(parent) = renviron.parent() {
        std::fs::create_dir_all(parent)?;
    }
    debug!("Configuring CA bundle in {}", renviron.display());
    std::fs::write(&renviron, out)?;
    Ok(())
}

const CERT_BLOCK_START: &str = "## rig SSL_CERT_FILE start";
const CERT_BLOCK_END: &str = "## rig SSL_CERT_FILE end";

// Produce the contents of `Renviron.site` with the rig CA-bundle block set to
// `cert`. Any previous rig block in `existing` is replaced (not duplicated), so
// re-running with a new path is idempotent and leaves the rest of the file
// untouched.
fn render_renviron_cert(existing: &str, cert: &str) -> String {
    // Keep everything outside a previous rig block, dropping the old block.
    let mut kept: Vec<String> = Vec::new();
    let mut in_block = false;
    for line in existing.lines() {
        match line.trim() {
            CERT_BLOCK_START => in_block = true,
            CERT_BLOCK_END => in_block = false,
            _ if !in_block => kept.push(line.to_string()),
            _ => {}
        }
    }
    // Trim trailing blank lines so they do not accumulate across re-runs.
    while matches!(kept.last(), Some(l) if l.trim().is_empty()) {
        kept.pop();
    }

    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(&format!(
        "{}\nSSL_CERT_FILE={}\nCURL_CA_BUNDLE={}\n{}\n",
        CERT_BLOCK_START, cert, cert, CERT_BLOCK_END
    ));
    out
}

// Download the CA bundle (if needed) and point the given user-mode R version at
// it. Called automatically after a user-mode install.
fn setup_user_cert(rver: &str, force: bool) -> Result<(), Box<dyn Error>> {
    let cert = download_cacert(force)?;
    configure_cert(rver, &cert)?;
    OUTPUT.success(&format!("Configured CA certificate bundle for R {}", rver));
    info!("Configured CA certificate bundle for R {}", rver);
    Ok(())
}

// `rig system update-certs`: fetch a fresh CA bundle and re-point every
// user-mode R installation at it. User mode only.
pub fn sc_system_update_certs() -> Result<(), Box<dyn Error>> {
    if get_mode()? != Mode::User {
        OUTPUT.warn("`rig system update-certs` only applies in user mode.");
        warn!("`rig system update-certs` called in admin mode, ignoring");
        return Ok(());
    }
    let cert = download_cacert(true)?;
    let vers = sc_get_list()?;
    if vers.is_empty() {
        OUTPUT.warn("No R installations to configure.");
        return Ok(());
    }
    for ver in vers {
        configure_cert(&ver, &cert)?;
        OUTPUT.success(&format!("Configured CA certificate bundle for R {}", ver));
        info!("Configured CA certificate bundle for R {}", ver);
    }
    Ok(())
}

pub fn set_cert_envvar() {
    match std::env::var("SSL_CERT_FILE") {
        Ok(_) => {
            debug!("SSL_CERT_FILE is already set, keeping it.");
            return;
        }
        Err(_) => {
            let scertpath = "/usr/local/share/rig/cacert.pem";
            let certpath = std::path::Path::new(scertpath);
            if certpath.exists() {
                debug!("Using embedded SSL certificates via SSL_CERT_FILE");
                std::env::set_var("SSL_CERT_FILE", scertpath);
            } else {
                debug!(
                    "{} does not exist, using system SSL certificates",
                    scertpath
                );
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real `ldd --version` banner from a glibc system (Ubuntu 22.04).
    const GLIBC_OUTPUT: &str = "ldd (Ubuntu GLIBC 2.35-0ubuntu3.8) 2.35\n\
Copyright (C) 2022 Free Software Foundation, Inc.\n\
This is free software; see the source for copying conditions.  There is NO\n\
warranty; not even for MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.\n\
Written by Roland McGrath and Ulrich Drepper.\n";

    // Real `ldd --version` banner from a musl system (Alpine). musl prints to
    // stderr and uses "Version" rather than the word "glibc".
    const MUSL_OUTPUT: &str = "musl libc (x86_64)\n\
Version 1.2.4\n\
Dynamic Program Loader\n\
Usage: /lib/ld-musl-x86_64.so.1 [options] [--] pathname\n";

    #[test]
    fn parse_libc_glibc() {
        let libc = parse_libc(GLIBC_OUTPUT).unwrap();
        assert_eq!(libc.kind, LibcType::Glibc);
        assert_eq!(libc.version, "2.35");
    }

    #[test]
    fn parse_libc_musl() {
        let libc = parse_libc(MUSL_OUTPUT).unwrap();
        assert_eq!(libc.kind, LibcType::Musl);
        assert_eq!(libc.version, "1.2.4");
    }

    #[test]
    fn parse_libc_gnu_libc_wording() {
        // Some distributions phrase the banner as "GNU libc".
        let text = "ldd (GNU libc) 2.31\n";
        let libc = parse_libc(text).unwrap();
        assert_eq!(libc.kind, LibcType::Glibc);
        assert_eq!(libc.version, "2.31");
    }

    #[test]
    fn parse_libc_unknown_type_errors() {
        let err = parse_libc("some completely unrelated output 1.2.3\n");
        assert!(err.is_err());
    }

    #[test]
    fn parse_libc_missing_version_errors() {
        // Recognizable as glibc, but no version-looking token present.
        let err = parse_libc("this is glibc, but without a version number\n");
        assert!(err.is_err());
    }

    #[test]
    fn libc_type_display() {
        assert_eq!(LibcType::Glibc.to_string(), "glibc");
        assert_eq!(LibcType::Musl.to_string(), "musl");
    }

    #[test]
    fn version_at_least_works() {
        assert!(version_at_least("2.35", "2.34"));
        assert!(version_at_least("2.34", "2.34"));
        assert!(!version_at_least("2.31", "2.34"));
        assert!(version_at_least("1.2.4", "1.2"));
        assert!(version_at_least("1.2", "1.2"));
        assert!(!version_at_least("1.1.24", "1.2"));
        assert!(version_at_least("3.0", "2.34"));
    }

    #[test]
    fn check_libc_supported_accepts_recent() {
        let glibc = Libc {
            kind: LibcType::Glibc,
            version: "2.35".to_string(),
        };
        assert!(check_libc_supported(&glibc).is_ok());

        let musl = Libc {
            kind: LibcType::Musl,
            version: "1.2.4".to_string(),
        };
        assert!(check_libc_supported(&musl).is_ok());
    }

    #[test]
    fn check_libc_supported_rejects_old_glibc() {
        let glibc = Libc {
            kind: LibcType::Glibc,
            version: "2.31".to_string(),
        };
        assert!(check_libc_supported(&glibc).is_err());
    }

    #[test]
    fn check_libc_supported_rejects_old_musl() {
        let musl = Libc {
            kind: LibcType::Musl,
            version: "1.1.24".to_string(),
        };
        assert!(check_libc_supported(&musl).is_err());
    }

    #[test]
    fn render_renviron_cert_into_empty_file() {
        let out = render_renviron_cert("", "/home/u/.local/share/rig/etc/cacert.pem");
        assert_eq!(
            out,
            "## rig SSL_CERT_FILE start\n\
             SSL_CERT_FILE=/home/u/.local/share/rig/etc/cacert.pem\n\
             CURL_CA_BUNDLE=/home/u/.local/share/rig/etc/cacert.pem\n\
             ## rig SSL_CERT_FILE end\n"
        );
    }

    #[test]
    fn render_renviron_cert_preserves_existing_content() {
        let existing = "FOO=bar\nBAZ=qux\n";
        let out = render_renviron_cert(existing, "/etc/cacert.pem");
        assert!(out.starts_with("FOO=bar\nBAZ=qux\n"));
        assert!(out.contains("SSL_CERT_FILE=/etc/cacert.pem"));
        assert!(out.contains("CURL_CA_BUNDLE=/etc/cacert.pem"));
    }

    #[test]
    fn render_renviron_cert_is_idempotent() {
        let first = render_renviron_cert("FOO=bar\n", "/old/cacert.pem");
        // Re-running with a new path replaces the block rather than appending.
        let second = render_renviron_cert(&first, "/new/cacert.pem");
        assert_eq!(second.matches(CERT_BLOCK_START).count(), 1);
        assert_eq!(second.matches(CERT_BLOCK_END).count(), 1);
        assert!(!second.contains("/old/cacert.pem"));
        assert!(second.contains("SSL_CERT_FILE=/new/cacert.pem"));
        assert!(second.starts_with("FOO=bar\n"));

        // A second run with the same path is a no-op on the contents.
        let third = render_renviron_cert(&second, "/new/cacert.pem");
        assert_eq!(third, second);
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
        assert_eq!(user_mode_install_spec("4.6.0", &aliases), "release");
        assert_eq!(user_mode_install_spec("4.5.1", &aliases), "oldrel");
    }

    #[test]
    fn user_mode_install_spec_uses_version_without_alias() {
        // No alias points at this version, so reinstall by version number.
        let aliases = vec![Alias {
            alias: "release".to_string(),
            version: "4.6.0".to_string(),
        }];
        assert_eq!(user_mode_install_spec("4.4.3", &aliases), "4.4.3");
        // With no aliases at all the directory name is used verbatim, including
        // symbolic names like `devel`.
        assert_eq!(user_mode_install_spec("devel", &[]), "devel");
    }

    // Write an R installation's `lib/R/include/Rversion.h` declaring the given
    // R_STATUS (empty for a released build).
    fn write_rversion_h(root: &Path, dir: &str, status: &str) {
        let incdir = root.join(dir).join("lib").join("R").join("include");
        std::fs::create_dir_all(&incdir).unwrap();
        std::fs::write(
            incdir.join("Rversion.h"),
            format!("#define R_STATUS \"{}\"\n", status),
        )
        .unwrap();
    }

    #[test]
    fn expected_user_dirname_uses_version_devel_and_next() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // A released build keeps its version-number directory name.
        write_rversion_h(root, "4.6.0", "");
        assert_eq!(expected_user_dirname(root, "4.6.0"), "4.6.0");

        // R-devel and R-next get their symbolic directory names.
        write_rversion_h(root, "rdevel", "Under development (unstable)");
        assert_eq!(expected_user_dirname(root, "rdevel"), "devel");
        write_rversion_h(root, "rnext", "Prerelease");
        assert_eq!(expected_user_dirname(root, "rnext"), "next");

        // With no header to inspect, fall back to the directory name.
        assert_eq!(expected_user_dirname(root, "4.4.3"), "4.4.3");
    }

    #[test]
    fn link_points_into_admin_root_matches_opt_r() {
        assert!(link_points_into_admin_root(Path::new("/opt/R/4.6.0/bin/R")));
        assert!(link_points_into_admin_root(Path::new(
            "/opt/R/current/bin/Rscript"
        )));
        // Links pointing elsewhere (e.g. a user-mode install) are left alone.
        assert!(!link_points_into_admin_root(Path::new(
            "/home/u/.local/share/rig/r/4.6.0/bin/R"
        )));
        assert!(!link_points_into_admin_root(Path::new("/usr/bin/R")));
    }

    #[test]
    fn re_admin_link_matches_version_and_alias_links() {
        let re = re_admin_link().unwrap();
        assert!(re.is_match("R-4.6.0"));
        assert!(re.is_match("R-release"));
        assert!(re.is_match("R-oldrel"));
        assert!(re.is_match("R-devel"));
        assert!(re.is_match("R-next"));
        // Not version/alias links.
        assert!(!re.is_match("R"));
        assert!(!re.is_match("Rscript"));
        assert!(!re.is_match("R-4.6")); // incomplete version
        assert!(!re.is_match("Rfoo"));
    }
}
