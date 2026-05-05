#[cfg(target_os = "linux")]
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs::File;
use std::io::{prelude::*, BufReader};
use std::path::{Path, PathBuf};

use regex::Regex;
use sha2::{Digest, Sha256};

use log::{debug, error};
use simple_error::*;
use std::error::Error;

#[cfg(any(target_os = "macos", target_os = "linux"))]
use crate::rversion::*;

pub fn os(x: &str) -> OsString {
    let mut ostr = OsString::new();
    ostr.push(x);
    ostr
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn osjoin(x: Vec<OsString>, sep: &str) -> String {
    let mut buffer = String::new();

    for (i, item) in x.iter().enumerate() {
        if i > 0 {
            buffer.push_str(sep);
        }
        let sitem = item.to_owned().into_string().unwrap_or("???".to_string());
        buffer.push_str(&sitem);
    }

    buffer
}

pub fn basename(path: &str) -> Option<&str> {
    path.rsplitn(2, '/').next()
}

pub fn read_file_string(path: &Path) -> Result<String, Box<dyn Error>> {
    let data = std::fs::read_to_string(path)?;
    Ok(data)
}

pub fn read_lines(path: &Path) -> Result<Vec<String>, Box<dyn Error>> {
    let file = File::open(path)?;
    let mut result: Vec<String> = vec![];
    let lines = BufReader::new(file).lines();
    for line in lines {
        result.push(line?);
    }
    Ok(result)
}

pub fn grep_lines(re: &Regex, lines: &Vec<String>) -> Vec<usize> {
    lines
        .iter()
        .enumerate()
        .filter_map(|record| {
            let (no, line) = record;
            if re.is_match(line) {
                Some(no)
            } else {
                None
            }
        })
        .collect()
}

pub fn bak_file(path: &Path) -> PathBuf {
    let mut path2 = path.to_owned();
    let ext = path.extension().unwrap_or_else(|| std::ffi::OsStr::new(""));
    let mut new_ext = OsString::new();
    new_ext.push(ext);
    new_ext.push(".bak");
    path2.set_extension(new_ext);
    path2
}

#[cfg(any(target_os = "macos"))]
pub fn replace_in_file(path: &Path, re: &Regex, sub: &str) -> Result<(), Box<dyn Error>> {
    let mut lines = read_lines(path)?;
    let mch = grep_lines(re, &lines);
    if mch.len() > 0 {
        debug!("Updating {:?}", path);
        for m in mch {
            lines[m] = re.replace(&lines[m], sub).to_string();
        }
        let path2 = bak_file(path);
        let mut f = File::create(&path2)?;
        for line in &lines {
            write!(f, "{}\n", line)?;
        }

        let perms = std::fs::metadata(path)?.permissions();
        std::fs::set_permissions(&path2, perms)?;
        std::fs::rename(path2, path)?;
    }

    Ok(())
}

pub fn append_to_file(path: &Path, extra: Vec<String>) -> Result<(), Box<dyn Error>> {
    debug!("Updating {:?}", path);
    let lines = read_lines(path)?;
    let path2 = bak_file(path);
    let mut f = File::create(&path2)?;
    for line in &lines {
        write!(f, "{}\n", line)?;
    }
    for line in &extra {
        write!(f, "{}\n", line)?;
    }
    let perms = std::fs::metadata(path)?.permissions();
    std::fs::set_permissions(&path2, perms)?;
    std::fs::rename(path2, path)?;

    Ok(())
}

pub fn calculate_hash(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s);
    let hash = hasher.finalize();
    let string = format!("{:x}", hash);
    string
}

pub fn unquote(s: &str) -> String {
    let l = s.len();
    if l <= 2 {
        return s.to_string();
    }
    let first = &s[0..1];
    let last = &s[l - 1..l];
    if first == last && (first == "'" || first == "\"") {
        s[1..l - 1].to_string()
    } else {
        s.to_string()
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn read_version_link(path: &str) -> Result<Option<String>, Box<dyn Error>> {
    let linkpath = Path::new(path);
    if !linkpath.exists() {
        return Ok(None);
    }

    let tgt = std::fs::read_link(path)?;

    // file_name() might be None if tgt ends with ".."
    let fname = match tgt.file_name() {
        None => {
            error!("Symlink for default version is invalid: {}", path);
            bail!("Symlink for default version is invalid")
        }
        Some(f) => f,
    };

    let fname = match fname.to_os_string().into_string() {
        Ok(x) => x,
        Err(x) => {
            let fpath = Path::new(&x);
            error!(
                "Default version is not a Unicode string: {}",
                fpath.display()
            );
            bail!(
                "Default version is not a Unicode string: {}",
                fpath.display()
            );
        }
    };

    Ok(Some(fname))
}

pub fn not_too_old(path: &std::path::PathBuf) -> bool {
    let meta = std::fs::metadata(path);
    match meta {
        Err(_) => return false,
        Ok(meta) => {
            let mtime = match meta.modified() {
                Err(_) => return false,
                Ok(mtime) => mtime,
            };
            let now = std::time::SystemTime::now();
            let age = match now.duration_since(mtime) {
                Err(_) => return false,
                Ok(age) => age,
            };
            let day = std::time::Duration::from_secs(60 * 60 * 24);
            age < day
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[allow(deprecated)]
// home_dir is no longer deprecated, actually, its behaviour was
// fixed in Rust 1.85 and the deprecation will be removed in 1.87.
pub fn get_user() -> Result<User, Box<dyn Error>> {
    let uid: u32;
    let gid: u32;
    let user;
    let sudo;

    fn parse_uid(str: Option<std::ffi::OsString>) -> Option<u32> {
        str.and_then(|x| x.into_string().ok())
            .and_then(|x| x.parse::<u32>().ok())
    }

    let euid = nix::unistd::geteuid();
    let sudo_uid: Option<u32> = parse_uid(std::env::var_os("SUDO_UID"));
    let sudo_gid: Option<u32> = parse_uid(std::env::var_os("SUDO_GID"));
    let sudo_user = std::env::var_os("SUDO_USER").and_then(|x| x.into_string().ok());
    if euid.is_root() && sudo_uid.is_some() && sudo_gid.is_some() && sudo_user.is_some() {
        sudo = true;
        uid = sudo_uid.unwrap_or_else(|| unreachable!());
        gid = sudo_gid.unwrap_or_else(|| unreachable!());
        user = sudo_user.unwrap_or_else(|| unreachable!());
    } else {
        sudo = false;
        uid = nix::unistd::getuid().as_raw();
        gid = nix::unistd::getgid().as_raw();
        user = std::env::var_os("USER")
            .and_then(|x: OsString| x.into_string().ok())
            .unwrap_or_else(|| "Current user".to_string());
    }

    let ouid = nix::unistd::Uid::from_raw(uid);
    let user_record = nix::unistd::User::from_uid(ouid);
    let dir;
    if let Ok(Some(d)) = user_record {
        dir = d.dir.into_os_string();
    } else {
        // home_dir is no longer deprecated, actually, its behaviour was
        // fixed in Rust 1.85 and the deprecation will be removed in 1.87.
        dir = std::env::home_dir()
            .map(|x| x.into_os_string())
            .ok_or(SimpleError::new("Failed to find user HOME"))?;
    }

    Ok(User {
        user,
        uid,
        gid,
        dir,
        sudo,
    })
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn get_binary_dir() -> Result<String, Box<dyn Error>> {
    if let Ok(val) = std::env::var("RIG_BINARY_DIR") {
        return Ok(val.trim_end_matches('/').to_string());
    }

    if let Some(val) = crate::config::get_global_config_value("binary-dir")? {
        return Ok(val.trim_end_matches('/').to_string());
    }

    let mode = if let Ok(val) = std::env::var("RIG_MODE") {
        Some(val)
    } else {
        crate::config::get_global_config_value("mode")?
    };

    if mode.as_deref() == Some("user") {
        let home = std::env::var("HOME")?;
        return Ok(format!("{}/.local/bin", home));
    }

    Ok("/usr/local/bin".to_string())
}

pub fn unset_r_envvars() {
    let evs = vec![
        "R_ARCH",
        "R_BROWSER",
        "R_BZIPCMD",
        "R_COMPILED_BY",
        "R_DOC_DIR",
        "R_GZIPCMD",
        "R_HOME",
        "R_INCLUDE_DIR",
        "R_LIBS_SITE",
        "R_LIBS_USER",
        "R_PAPERSIZE",
        "R_PDFVIEWER",
        "R_PLATFORM",
        "R_PRINTCMD",
        "R_RD4PDF",
        "R_SESSION_TMPDIR",
        "R_SHARE_DIR",
        "R_STRIP_SHARED_LIB",
        "R_STRIP_STATIC_LIB",
        "R_SYSTEM_ABI",
        "R_TEXI2DVICMD",
        "R_UNZIPCMD",
        "R_USER",
        "R_ZIPCMD",
    ];
    for ev in evs {
        std::env::remove_var(ev);
    }
}

#[cfg(target_os = "linux")]
pub fn format_cmd_args(x: Vec<String>, val: &OsStr) -> Vec<OsString> {
    let x2 = x.iter().map(|e| format_cmd_arg(e, val)).collect();
    x2
}

#[cfg(target_os = "linux")]
fn format_cmd_arg(x: &str, val: &OsStr) -> OsString {
    let parts: Vec<_> = x.split("{}").collect();
    let mut ox = OsString::new();
    let n = parts.len();
    for (idx, part) in parts.iter().enumerate() {
        ox.push(part);
        if idx != n - 1 {
            ox.push(val);
        }
    }

    ox
}

pub fn create_parent_dir_if_needed(path: &PathBuf) -> Result<(), Box<dyn Error>> {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)?;
        }
    }
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn add_local_bin_to_path() -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::PermissionsExt;

    let home = match std::env::var("HOME") {
        Ok(x) => x,
        Err(_) => bail!("HOME environment variable is not set"),
    };
    let home_path = Path::new(&home);

    let local_bin = home_path.join(".local/bin");
    std::fs::create_dir_all(&local_bin)?;

    let env_file = local_bin.join("env");
    if !env_file.exists() {
        let content = "\
#!/bin/sh
# add binaries to PATH if they aren't added yet
# affix colons on either side of $PATH to simplify matching
case \":${PATH}:\" in
    *:\"$HOME/.local/bin\":*)
        ;;
    *)
        # Prepending path in case a system-installed binary needs to be overridden
        export PATH=\"$HOME/.local/bin:$PATH\"
        ;;
esac
";
        std::fs::write(&env_file, content)?;
        let mut perms = std::fs::metadata(&env_file)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&env_file, perms)?;
    }

    let source_line = ". \"$HOME/.local/bin/env\"";

    let candidates = [
        home_path.join(".profile"),
        home_path.join(".bash_profile"),
        home_path.join(".zprofile"),
    ];

    let mut any_found = false;
    for profile in &candidates {
        if profile.exists() {
            any_found = true;
            let content = std::fs::read_to_string(profile)?;
            if !content.contains(source_line) {
                let mut file = std::fs::OpenOptions::new().append(true).open(profile)?;
                writeln!(file, "\n{}", source_line)?;
            }
        }
    }

    if !any_found {
        let profile = home_path.join(".profile");
        std::fs::write(&profile, format!("{}\n", source_line))?;
    }

    Ok(())
}

#[cfg(target_os = "windows")]
pub fn add_local_bin_to_path() -> Result<(), Box<dyn Error>> {
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn check_local_bin_path() -> Result<(), Box<dyn Error>> {
    use crate::output::OUTPUT;

    let binary_dir = get_binary_dir()?;

    let home = match std::env::var("HOME") {
        Ok(x) => x,
        Err(_) => bail!("HOME environment variable is not set"),
    };
    let local_bin = Path::new(&home).join(".local/bin");
    if Path::new(&binary_dir) == local_bin {
        add_local_bin_to_path()?;
    }

    let path_var = std::env::var("PATH").unwrap_or_default();
    let on_path = std::env::split_paths(&path_var)
        .any(|p| p == Path::new(&binary_dir));

    if !on_path {
        let is_local_bin = Path::new(&binary_dir) == local_bin;
        if is_local_bin {
            let shell = std::env::var("SHELL").unwrap_or_default();
            let is_fish = shell.contains("fish");
            OUTPUT.warn(&format!("{} is not on the PATH.", binary_dir));
            eprintln!("  To add it to the current session, run:");
            if is_fish {
                eprintln!("    fish_add_path \"$HOME/.local/bin\"          # fish");
            } else {
                eprintln!("    . \"$HOME/.local/bin/env\"                   # bash/zsh/sh");
                eprintln!("    fish_add_path \"$HOME/.local/bin\"           # fish");
            }
            eprintln!("  New shell sessions will pick it up automatically.");
        } else {
            OUTPUT.warn(&format!("{} is not on the PATH.", binary_dir));
        }
    }

    Ok(())
}

#[cfg(target_os = "windows")]
pub fn check_local_bin_path() -> Result<(), Box<dyn Error>> {
    Ok(())
}
