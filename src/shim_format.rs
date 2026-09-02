// Binary format for the Windows `.exe` quick-link shims. A shim file is the
// bundled `rig-shim.exe` template, byte-for-byte, followed by a footer that
// tells the shim which real executable to forward to:
//
//   [ template bytes ][ target path, UTF-8 ][ marker, UTF-8 (may be empty) ]
//   [ target_len: u32 LE ][ marker_len: u32 LE ][ MAGIC: 8 bytes ]
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

pub const MAGIC: &[u8; 8] = b"RIGSHIM1";
const TRAILER_LEN: u64 = 4 + 4 + 8;

pub struct ShimFooter {
    pub target: String,
    #[allow(dead_code)] // only read by the `rig` binary, not the shim itself
    pub marker: String,
}

#[allow(dead_code)] // only used by the `rig` binary, not the shim itself
pub fn build_shim_bytes(template: &[u8], target: &str, marker: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(template.len() + target.len() + marker.len() + 16);
    out.extend_from_slice(template);
    out.extend_from_slice(target.as_bytes());
    out.extend_from_slice(marker.as_bytes());
    out.extend_from_slice(&(target.len() as u32).to_le_bytes());
    out.extend_from_slice(&(marker.len() as u32).to_le_bytes());
    out.extend_from_slice(MAGIC);
    out
}

pub fn read_shim_footer(path: &Path) -> Result<Option<ShimFooter>, Box<dyn Error>> {
    let mut f = File::open(path)?;
    let file_len = f.metadata()?.len();
    if file_len < TRAILER_LEN {
        return Ok(None);
    }

    let mut trailer = [0u8; TRAILER_LEN as usize];
    f.seek(SeekFrom::End(-(TRAILER_LEN as i64)))?;
    f.read_exact(&mut trailer)?;

    if &trailer[8..16] != MAGIC.as_slice() {
        return Ok(None);
    }
    let target_len = u32::from_le_bytes(trailer[0..4].try_into().unwrap()) as u64;
    let marker_len = u32::from_le_bytes(trailer[4..8].try_into().unwrap()) as u64;
    let data_len = target_len + marker_len;
    if file_len < TRAILER_LEN + data_len {
        return Ok(None);
    }

    let mut data = vec![0u8; data_len as usize];
    f.seek(SeekFrom::Start(file_len - TRAILER_LEN - data_len))?;
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
    Ok(Some(ShimFooter { target, marker }))
}
