//! `cargo xtask gen-rvenv-shim [--check]`
//!
//! Generates the two committed data files that `rig proj init` needs to write
//! the `rvenv` shim R package into a project's `.rvenv/sys/lib/rvenv/`:
//! `src/data/rvenv-shim/DESCRIPTION` and `src/data/rvenv-shim/package.rds`.
//! The package source is `src/data/rvenv-pkg/`; its `NAMESPACE`, `LICENSE` and
//! `R/rvenv.R` are embedded into the `rig` binary verbatim, so only these two
//! need generating. See `src/rvenv.rs` for the writing side.
//!
//! Why generated and committed: the whole point of the shim is to be there on
//! a *fresh clone*, before anything has been installed, and on an R
//! installation rig does not manage. So rig has to be able to write it
//! without running R, and `Meta/package.rds` is a serialized R object.
//!
//! The shim is a *source-only* installed package: no lazy-load database, so
//! nothing in it is tied to an R version except two strings. It has to load on
//! every R the project may be opened with, since it is committed to version
//! control, so:
//!
//! - `Built$R` in `package.rds` is stamped **4.0.0**, because
//!   `loadNamespace()` rejects a package whose `Built$R` is below 4.0.0
//!   ("package 'rvenv' was installed before R 4.0.0: please re-install it").
//!   The only cost is that R < 4.0 warns once at startup, "package 'rvenv' was
//!   built under R version 4.0.0"; there is no stamp that satisfies both sides
//!   of that boundary.
//! - `package.rds` is saved with **serialization format 2**, because format 3
//!   (the default from R 3.6.0) cannot be read by R < 3.5.0.
//! - `Built$Platform` is empty, which is what keeps `library()`'s "package was
//!   built for <platform>" check quiet on Windows. It is empty for any package
//!   without compiled code. (`Built$OStype` is read by nothing, so it is
//!   pinned to `unix` on every platform.)
//! - `Built$Date` is pinned too, so that a rebuild that changes nothing
//!   produces no diff.
//!
//! Verified to load on R 3.4.4, 3.5.3, 3.6.3, 4.0.5 and 4.6.1.
//!
//! The generating R only has to be recent enough to install a package
//! (>= 4.0.0 is required here); the artifacts do not depend on it. CI only
//! ever runs `--check`, which needs no R at all: the committed
//! `src/data/rvenv-shim/SOURCE-HASH` manifest records a hash of the package
//! source plus a hash of each generated file, and `--check` verifies all of
//! them.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use sha2::{Digest, Sha256};

/// The R version `Meta/package.rds` claims to have been built with, and the
/// serialization format it is saved in. See the module docs for both.
const BUILT_R: &str = "4.0.0";
const SERIALIZE_VERSION: u32 = 2;

/// The pinned `Built$Date`, R 4.0.0's release date, for a reproducible
/// artifact. Nothing reads it.
const BUILT_DATE: &str = "2020-04-24 00:00:00 UTC";

/// The pinned `Built$OStype`. Read by nothing, see the module docs.
const BUILT_OSTYPE: &str = "unix";

/// The oldest R that can generate the artifacts. Not a property of the
/// artifacts themselves, just a guard against a surprising `package.rds`
/// layout from an ancient R.
const MIN_GEN_R: (u32, u32) = (4, 0);

const MANIFEST_FILE: &str = "SOURCE-HASH";
const DESCRIPTION_FILE: &str = "DESCRIPTION";
const META_FILE: &str = "package.rds";

/// The name of the package. Must match `Package:` in
/// `src/data/rvenv-pkg/DESCRIPTION` and `RVENV_SHIM_PKG` in `src/rvenv.rs`.
const PKG_NAME: &str = "rvenv";

fn pkg_dir(root: &Path) -> PathBuf {
    root.join("src/data/rvenv-pkg")
}

fn shim_dir(root: &Path) -> PathBuf {
    root.join("src/data/rvenv-shim")
}

/// The `Built:` field of the installed `DESCRIPTION`, matching what the fixup
/// script writes into `package.rds`.
fn built_field() -> String {
    format!("R {}; ; {}; {}", BUILT_R, BUILT_DATE, BUILT_OSTYPE)
}

// ---------------------------------------------------------------- hashing --

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{:02x}", b)).collect()
}

/// Every file under `dir`, keyed by its `/`-separated path relative to `dir`.
fn read_tree(dir: &Path) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut out = BTreeMap::new();
    read_tree_into(dir, dir, &mut out)?;
    Ok(out)
}

fn read_tree_into(
    base: &Path,
    dir: &Path,
    out: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("cannot read {}: {}", dir.display(), e))?;
    for entry in entries {
        let path = entry
            .map_err(|e| format!("cannot read {}: {}", dir.display(), e))?
            .path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        // Nothing of ours starts with a dot; skipping them keeps stray
        // .DS_Store files from changing the source hash.
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            read_tree_into(base, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(base)
                .map_err(|e| e.to_string())?
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            let bytes =
                fs::read(&path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
            out.insert(rel, bytes);
        }
    }
    Ok(())
}

/// A hash over the whole package source, so `--check` can tell that the
/// source changed without the artifacts being regenerated.
fn source_hash(tree: &BTreeMap<String, Vec<u8>>) -> String {
    let mut h = Sha256::new();
    for (path, bytes) in tree {
        h.update(path.as_bytes());
        h.update([0u8]);
        h.update(bytes.len().to_le_bytes());
        h.update(bytes);
    }
    h.finalize().iter().map(|b| format!("{:02x}", b)).collect()
}

// ------------------------------------------------------------------- rds ---

/// Whether `bytes` is an R serialization format 2 stream, gzipped or not.
///
/// An uncompressed RDS starts with the two ASCII bytes `X\n` (XDR format),
/// followed by the format version as a big-endian 32 bit integer. `saveRDS()`
/// gzips by default, which every R can read, so unwrap that first.
pub fn rds_serialize_version(bytes: &[u8]) -> Result<u32, String> {
    let plain: Vec<u8> = if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(bytes)
            .read_to_end(&mut out)
            .map_err(|e| format!("cannot gunzip: {}", e))?;
        out
    } else {
        bytes.to_vec()
    };
    if plain.len() < 6 {
        return Err("too short to be an RDS stream".to_string());
    }
    if &plain[..2] != b"X\n" {
        return Err(format!(
            "not an XDR RDS stream (starts with {:?})",
            &plain[..2]
        ));
    }
    Ok(u32::from_be_bytes([plain[2], plain[3], plain[4], plain[5]]))
}

// ------------------------------------------------------------------ rig(1) --

fn r_binary() -> String {
    std::env::var("R").unwrap_or_else(|_| "R".to_string())
}

/// `<major>.<minor>` of the R at `r_binary()`, and its full version string.
fn r_version(r: &str) -> Result<((u32, u32), String), String> {
    let out = Command::new(r)
        .args(["--version"])
        .output()
        .map_err(|e| format!("cannot run `{} --version`: {}", r, e))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let ver = text
        .split_whitespace()
        .find(|w| w.starts_with(|c: char| c.is_ascii_digit()) && w.contains('.'))
        .ok_or_else(|| format!("cannot parse the output of `{} --version`", r))?
        .to_string();
    let mut parts = ver.split('.');
    let major = parts.next().and_then(|p| p.parse().ok());
    let minor = parts.next().and_then(|p| p.parse().ok());
    match (major, minor) {
        (Some(a), Some(b)) => Ok(((a, b), ver)),
        _ => Err(format!("cannot parse R version: {}", ver)),
    }
}

// ------------------------------------------------------------------ build --

/// Rewrites the installed `Meta/package.rds` into the committed one: the
/// pinned `Built` stamp, and serialization format 2.
const FIXUP_R: &str = r#"
args <- commandArgs(TRUE)
info <- readRDS(args[1])
info$Built$R <- R_system_version(args[3])
info$Built$Platform <- ""
info$Built$Date <- args[4]
info$Built$OStype <- args[5]
info$DESCRIPTION[["Built"]] <- args[6]
saveRDS(info, args[2], version = as.integer(args[7]))
"#;

/// `R CMD INSTALL` the package source into a throwaway library and return the
/// installed package's directory.
fn install(root: &Path, r: &str) -> Result<PathBuf, String> {
    let lib = root.join("target/rvenv-shim/lib");
    if lib.exists() {
        fs::remove_dir_all(&lib).map_err(|e| format!("cannot clean {}: {}", lib.display(), e))?;
    }
    fs::create_dir_all(&lib).map_err(|e| format!("cannot create {}: {}", lib.display(), e))?;

    let status = Command::new(r)
        .args([
            "CMD",
            "INSTALL",
            // rig writes `R/rvenv` from the package source, so the lazy-load
            // database this would build is thrown away; only `Meta/package.rds`
            // is kept. Skipping the work also keeps the install quiet about
            // byte-compiling for a version of R that is not the stamped one.
            "--no-byte-compile",
            "--no-help",
            "--no-multiarch",
            // Loading the package during install would fire .onLoad() and its
            // side effects.
            "--no-test-load",
            "-l",
        ])
        .arg(&lib)
        .arg(pkg_dir(root))
        .status()
        .map_err(|e| format!("cannot run `{} CMD INSTALL`: {}", r, e))?;
    if !status.success() {
        return Err(format!("`{} CMD INSTALL` failed", r));
    }
    Ok(lib.join(PKG_NAME))
}

/// The installed `DESCRIPTION` with its `Built:` field replaced by the pinned
/// one, so that it cannot disagree with `package.rds`.
pub fn restamp_description(description: &str, built: &str) -> String {
    let mut out = String::new();
    let mut seen = false;
    // `Built:` is the last field `R CMD INSTALL` appends and is a single line,
    // so there is no continuation to worry about.
    for line in description.replace("\r\n", "\n").lines() {
        if line.starts_with("Built:") {
            seen = true;
            out.push_str(&format!("Built: {}\n", built));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !seen {
        out.push_str(&format!("Built: {}\n", built));
    }
    out
}

/// The R version out of `DESCRIPTION`'s `Built:` field, e.g. `Built: R 4.0.0;
/// ; 2020-04-24 00:00:00 UTC; unix` -> `4.0.0`.
pub fn built_r_version(description: &str) -> Option<String> {
    let line = description
        .lines()
        .find(|l| l.starts_with("Built:"))?
        .trim_start_matches("Built:")
        .trim();
    let field = line.split(';').next()?.trim();
    field.strip_prefix("R ").map(|v| v.trim().to_string())
}

// --------------------------------------------------------------- manifest --

#[derive(Debug, PartialEq, Eq)]
pub struct Manifest {
    /// Hash of the package source, `src/data/rvenv-pkg`.
    pub source: String,
    /// The R version the artifacts were generated with. Informational: it is
    /// not baked into them.
    pub r_version: String,
    /// `(file name, sha256)`
    pub files: Vec<(String, String)>,
}

pub fn render_manifest(m: &Manifest) -> String {
    let mut out = String::from(
        "# Generated by `cargo xtask gen-rvenv-shim` (run `make rvenv-shim`).\n\
         # Do not edit by hand. `cargo xtask gen-rvenv-shim --check` verifies that\n\
         # the committed shim data still matches src/data/rvenv-pkg.\n",
    );
    out.push_str(&format!("source = {}\n", m.source));
    out.push_str(&format!("generated-with = r {}\n", m.r_version));
    for (file, hash) in &m.files {
        out.push_str(&format!("{} = sha256 {}\n", file, hash));
    }
    out
}

pub fn parse_manifest(text: &str) -> Result<Manifest, String> {
    let mut source = None;
    let mut r_version = None;
    let mut files = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("malformed manifest line: {}", line))?;
        let (key, value) = (key.trim(), value.trim());
        match key {
            "source" => source = Some(value.to_string()),
            "generated-with" => {
                r_version = Some(
                    value
                        .strip_prefix("r ")
                        .ok_or_else(|| format!("malformed manifest line: {}", line))?
                        .trim()
                        .to_string(),
                )
            }
            _ => {
                let hash = value
                    .strip_prefix("sha256 ")
                    .ok_or_else(|| format!("malformed manifest entry for {}", key))?;
                files.push((key.to_string(), hash.trim().to_string()));
            }
        }
    }
    Ok(Manifest {
        source: source.ok_or_else(|| "manifest has no `source` line".to_string())?,
        r_version: r_version.ok_or_else(|| "manifest has no `generated-with` line".to_string())?,
        files,
    })
}

// ------------------------------------------------------------------ tasks --

fn gen(root: &Path) -> Result<(), String> {
    let r = r_binary();
    let (ver, ver_str) = r_version(&r)?;
    if ver < MIN_GEN_R {
        return Err(format!(
            "this task needs R >= {}.{} to generate the shim data, but `{}` is R {}; \
             set $R to a newer one",
            MIN_GEN_R.0, MIN_GEN_R.1, r, ver_str
        ));
    }
    eprintln!("generating the rvenv shim data with R {}", ver_str);

    let installed = install(root, &r)?;
    fs::create_dir_all(shim_dir(root)).map_err(|e| e.to_string())?;

    // DESCRIPTION: the installed one, restamped.
    let description = fs::read_to_string(installed.join("DESCRIPTION"))
        .map_err(|e| format!("cannot read the installed DESCRIPTION: {}", e))?;
    let description = restamp_description(&description, &built_field());
    let desc_path = shim_dir(root).join(DESCRIPTION_FILE);
    fs::write(&desc_path, &description)
        .map_err(|e| format!("cannot write {}: {}", desc_path.display(), e))?;
    eprintln!("wrote {}", desc_path.display());

    // Meta/package.rds: the installed one, restamped and re-serialized.
    let script = root.join("target/rvenv-shim/fixup.R");
    fs::write(&script, FIXUP_R).map_err(|e| format!("cannot write {}: {}", script.display(), e))?;
    let meta_path = shim_dir(root).join(META_FILE);
    let status = Command::new(&r)
        .arg("--vanilla")
        .args(["-q", "-s", "-f"])
        .arg(&script)
        .arg("--args")
        .arg(installed.join("Meta").join("package.rds"))
        .arg(&meta_path)
        .args([
            BUILT_R,
            BUILT_DATE,
            BUILT_OSTYPE,
            &built_field(),
            &SERIALIZE_VERSION.to_string(),
        ])
        .status()
        .map_err(|e| format!("cannot run `{}`: {}", r, e))?;
    if !status.success() {
        return Err(format!("`{}` failed on {}", r, script.display()));
    }
    let meta =
        fs::read(&meta_path).map_err(|e| format!("cannot read {}: {}", meta_path.display(), e))?;
    let got = rds_serialize_version(&meta)?;
    if got != SERIALIZE_VERSION {
        return Err(format!(
            "{} came out as RDS format {}, expected {}",
            meta_path.display(),
            got,
            SERIALIZE_VERSION
        ));
    }
    eprintln!("wrote {} ({} bytes)", meta_path.display(), meta.len());

    let manifest = Manifest {
        source: source_hash(&read_tree(&pkg_dir(root))?),
        r_version: ver_str,
        files: vec![
            (
                DESCRIPTION_FILE.to_string(),
                sha256_hex(description.as_bytes()),
            ),
            (META_FILE.to_string(), sha256_hex(&meta)),
        ],
    };
    let path = shim_dir(root).join(MANIFEST_FILE);
    fs::write(&path, render_manifest(&manifest))
        .map_err(|e| format!("cannot write {}: {}", path.display(), e))?;
    eprintln!("wrote {}", path.display());
    Ok(())
}

fn check(root: &Path) -> Result<(), String> {
    let manifest_path = shim_dir(root).join(MANIFEST_FILE);
    let text = fs::read_to_string(&manifest_path)
        .map_err(|e| format!("cannot read {}: {}", manifest_path.display(), e))?;
    let manifest = parse_manifest(&text)?;

    if source_hash(&read_tree(&pkg_dir(root))?) != manifest.source {
        return Err(
            "src/data/rvenv-pkg changed but the committed shim data was not \
             regenerated; run `make rvenv-shim`"
                .to_string(),
        );
    }

    for file in [DESCRIPTION_FILE, META_FILE] {
        let expected = manifest
            .files
            .iter()
            .find(|(f, _)| f == file)
            .ok_or_else(|| format!("{} has no entry for {}", MANIFEST_FILE, file))?;
        let path = shim_dir(root).join(file);
        let bytes =
            fs::read(&path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
        if sha256_hex(&bytes) != expected.1 {
            return Err(format!(
                "{} does not match its hash in {}; run `make rvenv-shim`",
                path.display(),
                MANIFEST_FILE
            ));
        }
    }
    if manifest.files.len() != 2 {
        return Err(format!(
            "{} lists {} files, expected 2",
            MANIFEST_FILE,
            manifest.files.len()
        ));
    }

    // The hashes above only say that the files are the ones that were
    // generated. These two properties are what makes them loadable on every R,
    // so state them as such rather than trusting the generator.
    let description = fs::read_to_string(shim_dir(root).join(DESCRIPTION_FILE))
        .map_err(|e| format!("cannot read the committed DESCRIPTION: {}", e))?;
    match built_r_version(&description).as_deref() {
        Some(BUILT_R) => {}
        Some(other) => {
            return Err(format!(
                "the committed DESCRIPTION says `Built: R {}`, expected R {}; \
                 run `make rvenv-shim`",
                other, BUILT_R
            ))
        }
        None => return Err("the committed DESCRIPTION has no `Built:` field".to_string()),
    }
    let meta = fs::read(shim_dir(root).join(META_FILE))
        .map_err(|e| format!("cannot read the committed {}: {}", META_FILE, e))?;
    let got = rds_serialize_version(&meta)?;
    if got != SERIALIZE_VERSION {
        return Err(format!(
            "the committed {} is RDS format {}, expected {} (R < 3.5.0 cannot \
             read format 3); run `make rvenv-shim`",
            META_FILE, got, SERIALIZE_VERSION
        ));
    }

    eprintln!("the committed rvenv shim data is up to date");
    Ok(())
}

pub fn gen_rvenv_shim(root: &Path, do_check: bool) -> ExitCode {
    let res = if do_check { check(root) } else { gen(root) };
    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("error: {}", msg);
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(entries: &[(&str, &[u8])]) -> BTreeMap<String, Vec<u8>> {
        entries
            .iter()
            .map(|(p, b)| (p.to_string(), b.to_vec()))
            .collect()
    }

    #[test]
    fn source_hash_tracks_content_and_paths() {
        let a = tree(&[("R/rvenv.R", b"one")]);
        let b = tree(&[("R/rvenv.R", b"two")]);
        let c = tree(&[("R/other.R", b"one")]);
        assert_eq!(source_hash(&a), source_hash(&a.clone()));
        assert_ne!(source_hash(&a), source_hash(&b));
        assert_ne!(source_hash(&a), source_hash(&c));
    }

    #[test]
    fn manifest_round_trips() {
        let m = Manifest {
            source: "abc123".to_string(),
            r_version: "4.6.1".to_string(),
            files: vec![
                ("DESCRIPTION".to_string(), "deadbeef".to_string()),
                ("package.rds".to_string(), "cafe".to_string()),
            ],
        };
        assert_eq!(parse_manifest(&render_manifest(&m)).unwrap(), m);
    }

    #[test]
    fn manifest_rejects_garbage() {
        assert!(parse_manifest("no source line here").is_err());
        assert!(parse_manifest("source = abc\npackage.rds = cafe").is_err());
        assert!(parse_manifest("source = abc\ngenerated-with = 4.6.1").is_err());
    }

    #[test]
    fn built_r_version_parses_the_built_field() {
        assert_eq!(
            built_r_version("Package: rvenv\nBuilt: R 4.0.0; ; 2020-04-24 00:00:00 UTC; unix\n")
                .as_deref(),
            Some("4.0.0")
        );
        assert_eq!(built_r_version("Package: rvenv\n"), None);
    }

    #[test]
    fn restamp_description_replaces_or_appends_built() {
        let built = "R 4.0.0; ; 2020-04-24 00:00:00 UTC; unix";
        assert_eq!(
            restamp_description("Package: rvenv\nBuilt: R 4.6.1; ; now; unix\n", built),
            format!("Package: rvenv\nBuilt: {}\n", built)
        );
        assert_eq!(
            restamp_description("Package: rvenv\n", built),
            format!("Package: rvenv\nBuilt: {}\n", built)
        );
        // A CRLF checkout of the source does not leak into the artifact.
        assert_eq!(
            restamp_description("Package: rvenv\r\nBuilt: R 4.6.1; ; now; unix\r\n", built),
            format!("Package: rvenv\nBuilt: {}\n", built)
        );
    }

    #[test]
    fn rds_serialize_version_reads_the_header() {
        // "X\n" + big-endian 2
        assert_eq!(rds_serialize_version(b"X\n\0\0\0\x02rest").unwrap(), 2);
        assert_eq!(rds_serialize_version(b"X\n\0\0\0\x03rest").unwrap(), 3);
        assert!(rds_serialize_version(b"A\n\0\0\0\x02").is_err());
        assert!(rds_serialize_version(b"X\n").is_err());
    }

    #[test]
    fn rds_serialize_version_unwraps_gzip() {
        use std::io::Write;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(b"X\n\0\0\0\x02rest").unwrap();
        let gzipped = gz.finish().unwrap();
        assert_eq!(rds_serialize_version(&gzipped).unwrap(), 2);
    }
}
