use std::path::{Path, PathBuf};
use std::ffi::OsString;
use std::fs::File;
use std::io::{prelude::*, BufReader};

#[cfg(any(target_os = "macos", target_os = "linux"))]
use regex::Regex;

#[cfg(target_os = "macos")]
use sha2::{Digest, Sha256};

#[cfg(any(target_os = "macos", target_os = "linux"))]
use simple_error::SimpleError;

#[cfg(any(target_os = "macos", target_os = "linux"))]
use crate::rversion::User;

#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::error::Error;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use simple_error::bail;

use simplelog::*;

pub fn basename(path: &str) -> Option<&str> {
    path.rsplitn(2, '/').next()
}

pub fn read_lines(path: &Path) -> Result<Vec<String>, std::io::Error> {
    let file = File::open(path)?;
    BufReader::new(file).lines().collect()
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
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

fn bak_file(path: &Path) -> PathBuf {
    let mut path2 = path.to_owned();
    let ext = path
        .extension()
        .unwrap_or_else(|| std::ffi::OsStr::new(""));
    let mut new_ext = OsString::new();
    new_ext.push(ext);
    new_ext.push("bak");
    path2.set_extension(new_ext);
    path2
}

#[cfg(target_os = "macos")]
pub fn replace_in_file(path: &Path, re: &Regex, sub: &str) -> Result<(), std::io::Error> {
    let mut lines = read_lines(path)?;
    let mch = grep_lines(re, &lines);
    if mch.len() > 0 {
        info!("<cyan>[INFO]</> Updating {:?}", path);
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

pub fn append_to_file(path: &Path, extra: Vec<String>) -> Result<(), std::io::Error> {
    info!("<cyan>[INFO]</> Updating {:?}", path);
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

#[cfg(target_os = "macos")]
pub fn calculate_hash(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s);
    let hash = hasher.finalize();
    let string = format!("{:x}", hash);
    string
}

pub fn unquote(s: &str) -> String {
    let l = s.len();
    if l <= 2 { return s.to_string(); }
    let first = &s[0..1];
    let last = &s[l-1..l];
    if first == last && (first == "'" || first == "\"") {
	s[1..l-1].to_string()
    } else {
	s.to_string()
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
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
    let user_record = nix::unistd::User::from_uid(ouid)?
        .ok_or(SimpleError::new("Failed to find user HOME"))?;
    let dir = user_record.dir.into_os_string();

    Ok(User { user, uid, gid, dir, sudo })
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn read_version_link(path: &str) -> Result<Option<String>,Box<dyn Error>> {
    let linkpath = Path::new(path);
    if !linkpath.exists() {
        return Ok(None);
    }

    let tgt = std::fs::read_link(path)?;

    // file_name() might be None if tgt ends with ".."
    let fname = match tgt.file_name() {
        None => bail!("Symlink for default version is invalid"),
        Some(f) => f
    };

    let fname = match fname.to_os_string().into_string() {
        Ok(x) => x,
        Err(x) => {
            let fpath = Path::new(&x);
            bail!("Default version is not a Unicode string: {}", fpath.display());
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
		Ok(mtime) => mtime
	    };
	    let now = std::time::SystemTime::now();
	    let age = match now.duration_since(mtime) {
		Err(_) => return false,
		Ok(age) => age
	    };
	    let day = std::time::Duration::from_secs(60 * 60 * 24);
	    age < day
	}
    }
}
