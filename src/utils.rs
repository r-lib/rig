#[cfg(any(target_os = "macos", target_os = "linux"))]
use regex::Regex;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::fs::File;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::io::{prelude::*, BufReader};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::path::Path;

#[cfg(target_os = "macos")]
use sha2::{Digest, Sha256};

#[cfg(any(target_os = "macos", target_os = "linux"))]
use crate::rversion::User;

#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::error::Error;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use simple_error::bail;

pub fn basename(path: &str) -> Option<&str> {
    path.rsplitn(2, '/').next()
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn read_lines(path: &Path) -> Result<Vec<String>, std::io::Error> {
    let file = File::open(path)?;
    let buf = BufReader::new(file);
    let lines = buf
        .lines()
        .map(|l| l.expect("Could not parse line"))
        .collect();
    Ok(lines)
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

#[cfg(target_os = "macos")]
pub fn replace_in_file(path: &Path, re: &Regex, sub: &str) -> Result<(), std::io::Error> {
    let mut lines = read_lines(path)?;
    let mch = grep_lines(re, &lines);
    if mch.len() > 0 {
        println!("Updating {:?}", path);
        for m in mch {
            lines[m] = re.replace(&lines[m], sub).to_string();
        }
        let mut path2 = path.to_owned();
        let ext = path
            .extension()
            .unwrap_or_else(|| std::ffi::OsStr::new(""))
            .to_str()
            .unwrap();
        path2.set_extension(ext.to_owned() + "bak");
        let mut f = File::create(&path2).expect("Unable to create file");
        for line in &lines {
            write!(f, "{}\n", line)?;
        }

        let perms = std::fs::metadata(path)?.permissions();
        std::fs::set_permissions(&path2, perms)?;
        std::fs::rename(path2, path)?;
    }

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
pub fn get_user() -> User {
    let uid;
    let gid;
    let user;
    let sudo;

    let euid = nix::unistd::geteuid();
    let sudo_uid = std::env::var_os("SUDO_UID");
    let sudo_gid = std::env::var_os("SUDO_GID");
    let sudo_user = std::env::var_os("SUDO_USER");
    if euid.is_root() && sudo_uid.is_some() && sudo_gid.is_some() && sudo_user.is_some() {
	sudo = true;
        uid = match sudo_uid {
            Some(x) => x.to_str().unwrap().parse::<u32>().unwrap(),
            _ => {
                unreachable!();
            }
        };
        gid = match sudo_gid {
            Some(x) => x.to_str().unwrap().parse::<u32>().unwrap(),
            _ => {
                unreachable!();
            }
        };
        user = match sudo_user {
            Some(x) => x.to_str().unwrap().to_string(),
            _ => {
                unreachable!();
            }
        };
    } else {
	sudo = false;
        uid = nix::unistd::getuid().as_raw();
        gid = nix::unistd::getgid().as_raw();
        user = match std::env::var_os("USER") {
            Some(x) => x.to_str().unwrap().to_string(),
            None => "Current user".to_string(),
        };
    }

    let user_record = nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(uid)).unwrap().unwrap();
    let dir = user_record.dir.into_os_string().into_string().unwrap();

    User { user, uid, gid, dir, sudo }
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
        // to_str() fails if file name is invalid in Unicode, cannot happen?
        Some(f) => f.to_str().unwrap().to_string()
    };

    Ok(Some(fname))
}
