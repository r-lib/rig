// Binary format for the Windows `.exe` quick-link shims. A shim file is the
// bundled `rig-shim.exe` template, byte-for-byte, followed by a footer that
// tells the shim which real executable to forward to. Two footer versions
// exist, distinguished by their trailing 8-byte magic (always the last 8
// bytes of the file, regardless of version):
//
// V1 (`RIGSHIM1`, no env vars — used by the ordinary quick links):
//   [ template bytes ][ target path, UTF-8 ][ marker, UTF-8 (may be empty) ]
//   [ target_len: u32 LE ][ marker_len: u32 LE ][ MAGIC_V1: 8 bytes ]
//
// V2 (`RIGSHIM2`, adds a baked-in env-var block — used by `.rvenv\bin`
// shims, which need to force `R_LIBS_USER`/`R_LIBS_SITE`/`R_REPOSITORIES`/
// `RVENV` before exec-ing the real R, the same thing the Unix `.rvenv/bin/R`
// wrapper script does with `export`):
//   [ template bytes ][ target path, UTF-8 ][ marker, UTF-8 (may be empty) ]
//   [ env block, see below ]
//   [ target_len: u32 LE ][ marker_len: u32 LE ][ env_len: u32 LE ]
//   [ MAGIC_V2: 8 bytes ]
//
// Env block: zero or more entries back-to-back, each
//   [ key_len: u16 LE ][ key, UTF-8 ][ val_len: u16 LE ][ val, UTF-8 ]
// parsed by consuming entries until `env_len` bytes are used up.
//
// `target` is the absolute path of the real `R.exe`/`Rscript.exe` to run.
// `marker` is only non-empty for the default-version links (`R.exe`,
// `RS.exe`, `Rscript.exe`); it records the rig version/alias name that is
// currently the default, the same thing the old `::<ver>` first line in
// `R.bat` used to record.

use std::error::Error;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub const MAGIC_V1: &[u8; 8] = b"RIGSHIM1";
pub const MAGIC_V2: &[u8; 8] = b"RIGSHIM2";
const TRAILER_LEN_V1: u64 = 4 + 4 + 8;
const TRAILER_LEN_V2: u64 = 4 + 4 + 4 + 8;

pub struct ShimFooter {
    pub target: String,
    #[allow(dead_code)] // only read by the `rig` binary, not the shim itself
    pub marker: String,
    #[allow(dead_code)] // read by src/shim/main.rs; not yet by the `rig` binary
    pub env: Vec<(String, String)>,
}

#[allow(dead_code)] // only used by the `rig` binary, not the shim itself
pub fn build_shim_bytes(template: &[u8], target: &str, marker: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(template.len() + target.len() + marker.len() + 16);
    out.extend_from_slice(template);
    out.extend_from_slice(target.as_bytes());
    out.extend_from_slice(marker.as_bytes());
    out.extend_from_slice(&(target.len() as u32).to_le_bytes());
    out.extend_from_slice(&(marker.len() as u32).to_le_bytes());
    out.extend_from_slice(MAGIC_V1);
    out
}

// Same as `build_shim_bytes`, but also bakes in a list of environment
// variables the shim will set (via `.envs(...)`, so they override whatever
// the launching process already has) before forwarding to `target`.
#[allow(dead_code)] // only used by the `rig` binary, not the shim itself
pub fn build_shim_bytes_env(
    template: &[u8],
    target: &str,
    marker: &str,
    envs: &[(String, String)],
) -> Vec<u8> {
    let mut env_block = Vec::new();
    for (k, v) in envs {
        env_block.extend_from_slice(&(k.len() as u16).to_le_bytes());
        env_block.extend_from_slice(k.as_bytes());
        env_block.extend_from_slice(&(v.len() as u16).to_le_bytes());
        env_block.extend_from_slice(v.as_bytes());
    }
    let mut out =
        Vec::with_capacity(template.len() + target.len() + marker.len() + env_block.len() + 20);
    out.extend_from_slice(template);
    out.extend_from_slice(target.as_bytes());
    out.extend_from_slice(marker.as_bytes());
    out.extend_from_slice(&env_block);
    out.extend_from_slice(&(target.len() as u32).to_le_bytes());
    out.extend_from_slice(&(marker.len() as u32).to_le_bytes());
    out.extend_from_slice(&(env_block.len() as u32).to_le_bytes());
    out.extend_from_slice(MAGIC_V2);
    out
}

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

pub fn read_shim_footer(path: &Path) -> Result<Option<ShimFooter>, Box<dyn Error>> {
    let mut f = File::open(path)?;
    let file_len = f.metadata()?.len();
    if file_len < 8 {
        return Ok(None);
    }

    let mut magic = [0u8; 8];
    f.seek(SeekFrom::End(-8))?;
    f.read_exact(&mut magic)?;

    if magic == *MAGIC_V2 {
        if file_len < TRAILER_LEN_V2 {
            return Ok(None);
        }
        let mut trailer = [0u8; TRAILER_LEN_V2 as usize];
        f.seek(SeekFrom::End(-(TRAILER_LEN_V2 as i64)))?;
        f.read_exact(&mut trailer)?;

        let target_len = u32::from_le_bytes(trailer[0..4].try_into().unwrap()) as u64;
        let marker_len = u32::from_le_bytes(trailer[4..8].try_into().unwrap()) as u64;
        let env_len = u32::from_le_bytes(trailer[8..12].try_into().unwrap()) as u64;
        let data_len = target_len + marker_len + env_len;
        if file_len < TRAILER_LEN_V2 + data_len {
            return Ok(None);
        }

        let mut data = vec![0u8; data_len as usize];
        f.seek(SeekFrom::Start(file_len - TRAILER_LEN_V2 - data_len))?;
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
        return Ok(Some(ShimFooter {
            target,
            marker,
            env,
        }));
    }

    if magic != *MAGIC_V1 || file_len < TRAILER_LEN_V1 {
        return Ok(None);
    }

    let mut trailer = [0u8; TRAILER_LEN_V1 as usize];
    f.seek(SeekFrom::End(-(TRAILER_LEN_V1 as i64)))?;
    f.read_exact(&mut trailer)?;

    let target_len = u32::from_le_bytes(trailer[0..4].try_into().unwrap()) as u64;
    let marker_len = u32::from_le_bytes(trailer[4..8].try_into().unwrap()) as u64;
    let data_len = target_len + marker_len;
    if file_len < TRAILER_LEN_V1 + data_len {
        return Ok(None);
    }

    let mut data = vec![0u8; data_len as usize];
    f.seek(SeekFrom::Start(file_len - TRAILER_LEN_V1 - data_len))?;
    f.read_exact(&mut data)?;

    let (target_bytes, marker_bytes) = data.split_at(target_len as usize);
    let target = match String::from_utf8(target_bytes.to_vec()) {
        Ok(s) => s,
        Err(_) => return Ok(None),
    };
    let marker = match String::from_utf8(marker_bytes.to_vec()) {
        Ok(s) => s,
        Err(_) => return Ok(None),
    };
    Ok(Some(ShimFooter {
        target,
        marker,
        env: Vec::new(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn v1_roundtrip_unaffected() {
        let template = b"FAKE_TEMPLATE_BYTES";
        let bytes = build_shim_bytes(template, "C:\\R\\bin\\R.exe", "4.6");
        let dir = std::env::temp_dir();
        let path = dir.join("rig_shim_test_v1.bin");
        std::fs::File::create(&path)
            .unwrap()
            .write_all(&bytes)
            .unwrap();
        let footer = read_shim_footer(&path).unwrap().unwrap();
        assert_eq!(footer.target, "C:\\R\\bin\\R.exe");
        assert_eq!(footer.marker, "4.6");
        assert!(footer.env.is_empty());
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn v2_roundtrip_with_env() {
        let template = b"FAKE_TEMPLATE_BYTES";
        let envs = vec![
            (
                "R_LIBS_USER".to_string(),
                "C:\\proj\\.rvenv\\lib".to_string(),
            ),
            ("RVENV".to_string(), "C:\\proj\\.rvenv".to_string()),
            ("R_LIBS_SITE".to_string(), "".to_string()),
        ];
        let bytes = build_shim_bytes_env(template, "C:\\R\\bin\\R.exe", "", &envs);
        let dir = std::env::temp_dir();
        let path = dir.join("rig_shim_test_v2.bin");
        std::fs::File::create(&path)
            .unwrap()
            .write_all(&bytes)
            .unwrap();
        let footer = read_shim_footer(&path).unwrap().unwrap();
        assert_eq!(footer.target, "C:\\R\\bin\\R.exe");
        assert_eq!(footer.marker, "");
        assert_eq!(footer.env, envs);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn rejects_garbage() {
        let dir = std::env::temp_dir();
        let path = dir.join("rig_shim_test_garbage.bin");
        std::fs::File::create(&path)
            .unwrap()
            .write_all(b"not a shim")
            .unwrap();
        assert!(read_shim_footer(&path).unwrap().is_none());
        std::fs::remove_file(&path).unwrap();
    }
}
