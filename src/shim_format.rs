// The Windows `.exe` shims: the quick links (`R.exe`, `R-4.5.2.exe`, ...)
// and the `.rvenv\bin\R.exe` / `Rscript.exe` project wrappers.
//
// A shim is an exact copy of the `rig-shim.exe` template that ships with
// rig. Next to it is a small text file, the shim's config, with the same
// name and the `.shim` extension: `R-4.5.2.exe` reads `R-4.5.2.shim`. The
// config says which real executable to run and which environment
// variables to set first.
//
// The shim itself is never changed, so every shim on every machine has the
// same bytes as the template. That keeps the template's Authenticode
// signature valid, and gives security software a single hash to know,
// instead of a new unsigned binary for every link and every project.
//
// Config format: UTF-8 text, one `key=value` per line, split at the first
// `=`. Nothing is trimmed, except a trailing `\r`. Empty lines and lines
// starting with `#` are ignored. Keys:
//
//   target=<absolute path of the real R.exe / Rscript.exe>
//   marker=<rig version or alias name, only for the default-version links>
//   env:<NAME>=<value>     (any number of these, set in this order)
//
// `marker` is only non-empty for the default-version links (`R.exe`,
// `RS.exe`, `Rscript.exe`); it records the rig version/alias name that is
// currently the default, the same thing the old `::<ver>` first line in
// `R.bat` used to record.
//
// rig 0.11.1 and earlier appended this information to the end of the shim
// `.exe` instead, as a binary footer. That breaks the template's signature,
// see `read_legacy_footer` below, which is kept so that rig can still
// recognize those older links and replace them.

use std::error::Error;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub const CONFIG_EXT: &str = "shim";

#[derive(Debug, PartialEq)]
pub struct ShimConfig {
    pub target: String,
    #[allow(dead_code)] // only read by the `rig` binary, not the shim itself
    pub marker: String,
    pub env: Vec<(String, String)>,
}

/// The config file of the shim at `exe`: `R-4.5.2.exe` -> `R-4.5.2.shim`.
pub fn config_path(exe: &Path) -> PathBuf {
    exe.with_extension(CONFIG_EXT)
}

#[allow(dead_code)] // only used by the `rig` binary, not the shim itself
pub fn format_config(cfg: &ShimConfig) -> Result<String, Box<dyn Error>> {
    let check = |what: &str, s: &str| -> Result<(), Box<dyn Error>> {
        if s.contains('\n') || s.contains('\r') {
            return Err(format!("Shim {} cannot contain a newline: {:?}", what, s).into());
        }
        Ok(())
    };
    check("target", &cfg.target)?;
    check("marker", &cfg.marker)?;
    let mut out = String::from("# Written by rig, do not edit.\n");
    out.push_str(&format!("target={}\n", cfg.target));
    out.push_str(&format!("marker={}\n", cfg.marker));
    for (k, v) in &cfg.env {
        check("variable name", k)?;
        check("variable value", v)?;
        if k.is_empty() || k.contains('=') {
            return Err(format!("Invalid environment variable name for shim: {:?}", k).into());
        }
        out.push_str(&format!("env:{}={}\n", k, v));
    }
    Ok(out)
}

pub fn parse_config(text: &str) -> Result<ShimConfig, String> {
    let mut target: Option<String> = None;
    let mut marker = String::new();
    let mut env = Vec::new();
    for line in text.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = match line.split_once('=') {
            Some(kv) => kv,
            None => return Err(format!("invalid line: {:?}", line)),
        };
        if key == "target" {
            target = Some(value.to_string());
        } else if key == "marker" {
            marker = value.to_string();
        } else if let Some(name) = key.strip_prefix("env:") {
            env.push((name.to_string(), value.to_string()));
        } else {
            return Err(format!("unknown key: {:?}", key));
        }
    }
    match target {
        Some(target) if !target.is_empty() => Ok(ShimConfig {
            target,
            marker,
            env,
        }),
        _ => Err("no target".to_string()),
    }
}

/// Read the config of the shim at `exe`. `Ok(None)` if there is no config
/// file next to it.
pub fn read_config(exe: &Path) -> Result<Option<ShimConfig>, Box<dyn Error>> {
    let path = config_path(exe);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    match parse_config(&text) {
        Ok(cfg) => Ok(Some(cfg)),
        Err(e) => Err(format!("Invalid shim config {}: {}", path.display(), e).into()),
    }
}

// ---------------------------------------------------------------------------
// The binary footer of rig 0.11.1 and earlier. Only read, never written.
//
// V1 (`RIGSHIM1`, no env vars — the ordinary quick links):
//   [ template bytes ][ target path, UTF-8 ][ marker, UTF-8 (may be empty) ]
//   [ target_len: u32 LE ][ marker_len: u32 LE ][ MAGIC_V1: 8 bytes ]
//
// V2 (`RIGSHIM2`, adds an env-var block — the `.rvenv\bin` shims):
//   [ template bytes ][ target path, UTF-8 ][ marker, UTF-8 (may be empty) ]
//   [ env block ]
//   [ target_len: u32 LE ][ marker_len: u32 LE ][ env_len: u32 LE ]
//   [ MAGIC_V2: 8 bytes ]
//
// Env block: entries back-to-back, each
//   [ key_len: u16 LE ][ key, UTF-8 ][ val_len: u16 LE ][ val, UTF-8 ]

#[allow(dead_code)]
const MAGIC_V1: &[u8; 8] = b"RIGSHIM1";
#[allow(dead_code)]
const MAGIC_V2: &[u8; 8] = b"RIGSHIM2";
#[allow(dead_code)]
const TRAILER_LEN_V1: u64 = 4 + 4 + 8;
#[allow(dead_code)]
const TRAILER_LEN_V2: u64 = 4 + 4 + 4 + 8;

#[allow(dead_code)]
fn parse_env_block(mut data: &[u8]) -> Option<Vec<(String, String)>> {
    let mut envs = Vec::new();
    while !data.is_empty() {
        if data.len() < 2 {
            return None;
        }
        let klen = u16::from_le_bytes(data[0..2].try_into().unwrap()) as usize;
        data = &data[2..];
        if data.len() < klen + 2 {
            return None;
        }
        let key = String::from_utf8(data[..klen].to_vec()).ok()?;
        data = &data[klen..];
        let vlen = u16::from_le_bytes(data[0..2].try_into().unwrap()) as usize;
        data = &data[2..];
        if data.len() < vlen {
            return None;
        }
        let val = String::from_utf8(data[..vlen].to_vec()).ok()?;
        data = &data[vlen..];
        envs.push((key, val));
    }
    Some(envs)
}

#[allow(dead_code)] // only used by the `rig` binary, not the shim itself
pub fn read_legacy_footer(path: &Path) -> Result<Option<ShimConfig>, Box<dyn Error>> {
    let mut f = File::open(path)?;
    let file_len = f.metadata()?.len();
    if file_len < 8 {
        return Ok(None);
    }

    let mut magic = [0u8; 8];
    f.seek(SeekFrom::End(-8))?;
    f.read_exact(&mut magic)?;

    let v2 = if magic == *MAGIC_V2 {
        true
    } else if magic == *MAGIC_V1 {
        false
    } else {
        return Ok(None);
    };
    let trailer_len = if v2 { TRAILER_LEN_V2 } else { TRAILER_LEN_V1 };
    if file_len < trailer_len {
        return Ok(None);
    }

    let mut trailer = vec![0u8; trailer_len as usize];
    f.seek(SeekFrom::End(-(trailer_len as i64)))?;
    f.read_exact(&mut trailer)?;

    let target_len = u32::from_le_bytes(trailer[0..4].try_into().unwrap()) as u64;
    let marker_len = u32::from_le_bytes(trailer[4..8].try_into().unwrap()) as u64;
    let env_len = if v2 {
        u32::from_le_bytes(trailer[8..12].try_into().unwrap()) as u64
    } else {
        0
    };
    let data_len = target_len + marker_len + env_len;
    if file_len < trailer_len + data_len {
        return Ok(None);
    }

    let mut data = vec![0u8; data_len as usize];
    f.seek(SeekFrom::Start(file_len - trailer_len - data_len))?;
    f.read_exact(&mut data)?;

    let (target_bytes, rest) = data.split_at(target_len as usize);
    let (marker_bytes, env_bytes) = rest.split_at(marker_len as usize);
    let target = match String::from_utf8(target_bytes.to_vec()) {
        Ok(s) => s,
        Err(_) => return Ok(None),
    };
    let marker = match String::from_utf8(marker_bytes.to_vec()) {
        Ok(s) => s,
        Err(_) => return Ok(None),
    };
    let env = match parse_env_block(env_bytes) {
        Some(e) => e,
        None => return Ok(None),
    };
    Ok(Some(ShimConfig {
        target,
        marker,
        env,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy_bytes(target: &str, marker: &str, envs: &[(&str, &str)], v2: bool) -> Vec<u8> {
        let mut env_block = Vec::new();
        for (k, v) in envs {
            env_block.extend_from_slice(&(k.len() as u16).to_le_bytes());
            env_block.extend_from_slice(k.as_bytes());
            env_block.extend_from_slice(&(v.len() as u16).to_le_bytes());
            env_block.extend_from_slice(v.as_bytes());
        }
        let mut out = b"FAKE_TEMPLATE_BYTES".to_vec();
        out.extend_from_slice(target.as_bytes());
        out.extend_from_slice(marker.as_bytes());
        if v2 {
            out.extend_from_slice(&env_block);
        }
        out.extend_from_slice(&(target.len() as u32).to_le_bytes());
        out.extend_from_slice(&(marker.len() as u32).to_le_bytes());
        if v2 {
            out.extend_from_slice(&(env_block.len() as u32).to_le_bytes());
            out.extend_from_slice(MAGIC_V2);
        } else {
            out.extend_from_slice(MAGIC_V1);
        }
        out
    }

    #[test]
    fn config_path_replaces_the_extension() {
        assert_eq!(
            config_path(Path::new("C:\\R\\bin\\R-4.5.2.exe")),
            PathBuf::from("C:\\R\\bin\\R-4.5.2.shim")
        );
        assert_eq!(
            config_path(Path::new("bin/Rscript.exe")),
            PathBuf::from("bin/Rscript.shim")
        );
    }

    #[test]
    fn config_roundtrip() {
        let cfg = ShimConfig {
            target: "C:\\Program Files\\R\\R-4.5.2\\bin\\R.exe".to_string(),
            marker: "4.5.2".to_string(),
            env: vec![
                (
                    "R_LIBS_USER".to_string(),
                    "C:\\a=b\\.rvenv\\lib".to_string(),
                ),
                ("R_LIBS_SITE".to_string(), "".to_string()),
                ("RVENV".to_string(), " C:\\proj\\.rvenv ".to_string()),
            ],
        };
        let text = format_config(&cfg).unwrap();
        assert_eq!(parse_config(&text).unwrap(), cfg);
        // CRLF line endings, e.g. after an editor touched the file.
        assert_eq!(parse_config(&text.replace('\n', "\r\n")).unwrap(), cfg);
    }

    #[test]
    fn config_rejects_bad_input() {
        let cfg = ShimConfig {
            target: "C:\\R\nx".to_string(),
            marker: String::new(),
            env: vec![],
        };
        assert!(format_config(&cfg).is_err());
        assert!(parse_config("marker=4.5\n").is_err());
        assert!(parse_config("target=\n").is_err());
        assert!(parse_config("target=C:\\R.exe\nbogus\n").is_err());
        assert!(parse_config("target=C:\\R.exe\nother=1\n").is_err());
    }

    #[test]
    fn read_config_missing_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_config(&dir.path().join("R.exe")).unwrap().is_none());
    }

    #[test]
    fn legacy_v1_footer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("R.exe");
        std::fs::write(&path, legacy_bytes("C:\\R\\bin\\R.exe", "4.6", &[], false)).unwrap();
        let footer = read_legacy_footer(&path).unwrap().unwrap();
        assert_eq!(footer.target, "C:\\R\\bin\\R.exe");
        assert_eq!(footer.marker, "4.6");
        assert!(footer.env.is_empty());
    }

    #[test]
    fn legacy_v2_footer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("R.exe");
        let envs = [
            ("R_LIBS_USER", "C:\\proj\\.rvenv\\lib"),
            ("RVENV", "C:\\proj\\.rvenv"),
            ("R_LIBS_SITE", ""),
        ];
        std::fs::write(&path, legacy_bytes("C:\\R\\bin\\R.exe", "", &envs, true)).unwrap();
        let footer = read_legacy_footer(&path).unwrap().unwrap();
        assert_eq!(footer.target, "C:\\R\\bin\\R.exe");
        assert_eq!(footer.marker, "");
        let expected: Vec<(String, String)> = envs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(footer.env, expected);
    }

    #[test]
    fn legacy_rejects_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.exe");
        std::fs::write(&path, b"not a shim").unwrap();
        assert!(read_legacy_footer(&path).unwrap().is_none());
    }
}
