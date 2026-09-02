//! `cargo xtask gen-rvenv-shim [--check]`
//!
//! Builds the pre-built copies of the `rig` shim R package that `rig proj
//! init` seeds into a project's `.rvenv/lib/rig/`. The package source is
//! `src/data/rvenv-pkg/`; the built artifacts are committed to
//! `src/data/rvenv-shim/` and embedded into the `rig` binary with
//! `include_bytes!`.
//!
//! Why pre-built and not built on the user's machine: the whole point of the
//! shim is to work on a *fresh clone*, before anything has been installed,
//! and on an R installation rig does not manage. So it has to be committed to
//! the project, which means rig has to be able to write it without running R.
//!
//! R's installed-package format has version boundaries, so there is one
//! artifact per R version bracket (see `BRACKETS`):
//!
//! - serialization format 3 became the default in R 3.6.0 and is unreadable
//!   by R < 3.5.0, hence `R_DEFAULT_SERIALIZE_VERSION=2` for the two older
//!   brackets;
//! - a package installed by R < 4.0.0 refuses to load under R >= 4.0.0
//!   ("package 'rig' was installed before R 4.0.0: please re-install it").
//!
//! Each bracket is built with the *oldest* R in it. This task needs those R
//! versions installed, and installs them with `rig add` if they are missing,
//! so it is a maintainer-only task; CI only ever runs `--check`, which needs
//! no R.
//!
//! `--check` cannot simply diff the tarballs, because `R CMD INSTALL` output
//! is not reproducible (`DESCRIPTION`'s `Built:` field carries a timestamp).
//! Instead the committed `src/data/rvenv-shim/SOURCE-HASH` manifest records a
//! hash of the *package source*, plus the hash and build R version of each
//! artifact, and `--check` verifies all of those.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use flate2::write::GzEncoder;
use flate2::{Compression, GzBuilder};
use sha2::{Digest, Sha256};

/// The R version brackets, oldest first. `(artifact file name, R version to
/// build with, serialization format)`.
///
/// Measured load matrix (macOS, `library(rig, lib.loc = ...)`):
///
/// | built with        | 3.4.4 | 3.5.3 | 3.6.3 | 4.0.5 | 4.1.3 | 4.6.1 |
/// |-------------------|-------|-------|-------|-------|-------|-------|
/// | 3.4.4, serialize 2| ok    | ok    | ok    | no    | no    | no    |
/// | 3.5.3, serialize 2| ok    | ok    | ok    | no    | no    | no    |
/// | 4.0.5, serialize 3| no    | ok    | ok    | ok    | ok    | ok    |
///
/// So the R 4.0.0 boundary is one-directional (a package built by an older R
/// is rejected by R >= 4.0.0, not the other way round), and serialization
/// format 3 is the only thing that keeps the 4.0 artifact off R < 3.5. That
/// makes the middle bracket redundant in practice — its artifact covers
/// exactly the same R versions as the oldest one. It is kept for now because
/// it costs ~4 KB and guards against the two older R series diverging.
const BRACKETS: [(&str, &str, u32); 3] = [
    ("shim-lt-3.5.tar.gz", "3.4.4", 2),
    ("shim-3.5.tar.gz", "3.5.3", 2),
    ("shim-4.0.tar.gz", "4.0.5", 3),
];

const MANIFEST_FILE: &str = "SOURCE-HASH";

/// Files every artifact must contain. `R/rig` is the lazy-load loader stub,
/// `R/rig.rdb`/`R/rig.rdx` the lazy-load database itself.
const REQUIRED_ENTRIES: [&str; 6] = [
    "rig/DESCRIPTION",
    "rig/NAMESPACE",
    "rig/Meta/package.rds",
    "rig/R/rig",
    "rig/R/rig.rdb",
    "rig/R/rig.rdx",
];

fn pkg_dir(root: &Path) -> PathBuf {
    root.join("src/data/rvenv-pkg")
}

fn shim_dir(root: &Path) -> PathBuf {
    root.join("src/data/rvenv-shim")
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
/// source changed without the artifacts being rebuilt.
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

// -------------------------------------------------------------------- tar --

/// Tar + gzip `tree` deterministically: sorted entries, no timestamps, no
/// ownership, fixed modes. Two runs over the same input give the same bytes,
/// so a rebuild that changes nothing shows up as no diff.
pub fn deterministic_targz(tree: &BTreeMap<String, Vec<u8>>) -> Result<Vec<u8>, String> {
    let mut tar = tar::Builder::new(Vec::new());
    tar.mode(tar::HeaderMode::Deterministic);
    for (path, bytes) in tree {
        let mut header = tar::Header::new_ustar();
        header
            .set_path(path)
            .map_err(|e| format!("tar path {}: {}", path, e))?;
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        header
            .set_username("")
            .map_err(|e| format!("tar user: {}", e))?;
        header
            .set_groupname("")
            .map_err(|e| format!("tar group: {}", e))?;
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        tar.append(&header, &bytes[..])
            .map_err(|e| format!("tar append {}: {}", path, e))?;
    }
    let tarred = tar.into_inner().map_err(|e| e.to_string())?;
    let gz = GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), Compression::default());
    let mut gz: GzEncoder<Vec<u8>> = gz;
    use std::io::Write;
    gz.write_all(&tarred).map_err(|e| e.to_string())?;
    gz.finish().map_err(|e| e.to_string())
}

fn untargz(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(std::io::Cursor::new(bytes)));
    let mut out = BTreeMap::new();
    for entry in ar.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry
            .path()
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        out.insert(path, buf);
    }
    Ok(out)
}

// ------------------------------------------------------------------ rig(1) --

#[derive(serde::Deserialize)]
struct RigListEntry {
    version: Option<String>,
    binary: Option<String>,
}

fn rig_binary() -> String {
    std::env::var("RIG").unwrap_or_else(|_| "rig".to_string())
}

fn rig_list() -> Result<Vec<RigListEntry>, String> {
    let out = Command::new(rig_binary())
        .args(["--json", "list"])
        .output()
        .map_err(|e| format!("cannot run `{} --json list`: {}", rig_binary(), e))?;
    if !out.status.success() {
        return Err(format!(
            "`{} --json list` failed: {}",
            rig_binary(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("cannot parse `rig list` output: {}", e))
}

/// The R binary for `version`, installing it with `rig add` if it is missing.
fn r_binary_for(version: &str) -> Result<PathBuf, String> {
    if let Some(bin) = find_r_binary(version)? {
        return Ok(bin);
    }
    eprintln!(
        "R {} is not installed, running `rig add {}`",
        version, version
    );
    // `--without-pak`: pak needs R >= 3.5.0, and the shim build does not use
    // it. We do not check the exit status, only whether R ended up installed:
    // some post-install steps can fail without that mattering here.
    rig_add(version, &["--without-pak"])?;
    // The R versions in the older brackets predate arm64 macOS, so on an
    // Apple silicon machine they only exist as x86_64 builds (run under
    // Rosetta). The installed package is pure R either way.
    if cfg!(target_os = "macos") && find_r_binary(version)?.is_none() {
        eprintln!("retrying with `rig add {} --arch x86_64`", version);
        rig_add(version, &["--without-pak", "--arch", "x86_64"])?;
    }
    find_r_binary(version)?.ok_or_else(|| {
        format!(
            "`{} add {}` did not install R {}; install it manually and re-run",
            rig_binary(),
            version,
            version
        )
    })
}

fn rig_add(version: &str, extra: &[&str]) -> Result<bool, String> {
    let status = Command::new(rig_binary())
        .args(["add", version])
        .args(extra)
        .status()
        .map_err(|e| format!("cannot run `{} add {}`: {}", rig_binary(), version, e))?;
    Ok(status.success())
}

fn find_r_binary(version: &str) -> Result<Option<PathBuf>, String> {
    Ok(rig_list()?
        .into_iter()
        .find(|e| e.version.as_deref() == Some(version))
        .and_then(|e| e.binary)
        .map(PathBuf::from))
}

// ------------------------------------------------------------------ build --

/// `R CMD INSTALL` the package source into a throwaway library and return the
/// installed `rig/` tree, ready to be tarred.
fn build_one(
    root: &Path,
    r_binary: &Path,
    serialize_version: u32,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let lib = root.join("target/rvenv-shim/lib");
    if lib.exists() {
        fs::remove_dir_all(&lib).map_err(|e| format!("cannot clean {}: {}", lib.display(), e))?;
    }
    fs::create_dir_all(&lib).map_err(|e| format!("cannot create {}: {}", lib.display(), e))?;

    let status = Command::new(r_binary)
        .args([
            "CMD",
            "INSTALL",
            // Bytecode carries the compiler's own version tag, which is one
            // more cross-version hazard for an artifact we ship. The shim is
            // a few lines of code run once per session; it does not need it.
            "--no-byte-compile",
            "--no-help",
            "--no-multiarch",
            // Loading the package during install would fire .onLoad() and
            // its side effects.
            "--no-test-load",
            "-l",
        ])
        .arg(&lib)
        .arg(pkg_dir(root))
        .env("R_DEFAULT_SERIALIZE_VERSION", serialize_version.to_string())
        .status()
        .map_err(|e| format!("cannot run `{} CMD INSTALL`: {}", r_binary.display(), e))?;
    if !status.success() {
        return Err(format!("`{} CMD INSTALL` failed", r_binary.display()));
    }

    let installed = read_tree(&lib.join("rig"))?;
    let mut tree = BTreeMap::new();
    for (path, bytes) in installed {
        tree.insert(format!("rig/{}", path), bytes);
    }
    for required in REQUIRED_ENTRIES {
        if !tree.contains_key(required) {
            return Err(format!("the installed package has no {}", required));
        }
    }
    Ok(tree)
}

/// The R version out of `DESCRIPTION`'s `Built:` field, e.g. `Built: R 4.0.5;
/// ; 2026-09-02 10:11:12 UTC; unix` -> `4.0.5`.
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
    pub source: String,
    /// `(file name, R version, serialization format, sha256)`
    pub artifacts: Vec<(String, String, u32, String)>,
}

pub fn render_manifest(m: &Manifest) -> String {
    let mut out = String::from(
        "# Generated by `cargo xtask gen-rvenv-shim` (run `make rvenv-shim`).\n\
         # Do not edit by hand. `cargo xtask gen-rvenv-shim --check` verifies that\n\
         # the committed shim packages still match src/data/rvenv-pkg.\n",
    );
    out.push_str(&format!("source = {}\n", m.source));
    for (file, rver, serialize, hash) in &m.artifacts {
        out.push_str(&format!(
            "{} = r {}, serialize {}, sha256 {}\n",
            file, rver, serialize, hash
        ));
    }
    out
}

pub fn parse_manifest(text: &str) -> Result<Manifest, String> {
    let mut source = None;
    let mut artifacts = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("malformed manifest line: {}", line))?;
        let (key, value) = (key.trim(), value.trim());
        if key == "source" {
            source = Some(value.to_string());
            continue;
        }
        let mut rver = None;
        let mut serialize = None;
        let mut hash = None;
        for field in value.split(',') {
            let field = field.trim();
            if let Some(v) = field.strip_prefix("r ") {
                rver = Some(v.trim().to_string());
            } else if let Some(v) = field.strip_prefix("serialize ") {
                serialize = v.trim().parse::<u32>().ok();
            } else if let Some(v) = field.strip_prefix("sha256 ") {
                hash = Some(v.trim().to_string());
            }
        }
        match (rver, serialize, hash) {
            (Some(r), Some(s), Some(h)) => artifacts.push((key.to_string(), r, s, h)),
            _ => return Err(format!("malformed manifest entry for {}", key)),
        }
    }
    Ok(Manifest {
        source: source.ok_or_else(|| "manifest has no `source` line".to_string())?,
        artifacts,
    })
}

// ------------------------------------------------------------------ tasks --

fn gen(root: &Path) -> Result<(), String> {
    let src = read_tree(&pkg_dir(root))?;
    fs::create_dir_all(shim_dir(root)).map_err(|e| e.to_string())?;

    let mut artifacts = Vec::new();
    let mut payloads: Vec<(String, BTreeMap<String, Vec<u8>>)> = Vec::new();
    for (file, rver, serialize) in BRACKETS {
        let r_binary = r_binary_for(rver)?;
        eprintln!("building {} with R {} ({})", file, rver, r_binary.display());
        let tree = build_one(root, &r_binary, serialize)?;
        let targz = deterministic_targz(&tree)?;
        let path = shim_dir(root).join(file);
        fs::write(&path, &targz).map_err(|e| format!("cannot write {}: {}", path.display(), e))?;
        eprintln!("wrote {} ({} bytes)", path.display(), targz.len());
        artifacts.push((
            file.to_string(),
            rver.to_string(),
            serialize,
            sha256_hex(&targz),
        ));
        payloads.push((file.to_string(), tree));
    }

    // The two pre-4.0 brackets are both serialization format 2, so they may
    // well be interchangeable in practice. If the lazy-load databases come
    // out identical, one of the two brackets is dead weight and can be
    // dropped — but say so rather than guess.
    if let (Some((a_name, a)), Some((b_name, b))) = (payloads.first(), payloads.get(1)) {
        if a.get("rig/R/rig.rdb") == b.get("rig/R/rig.rdb")
            && a.get("rig/R/rig.rdx") == b.get("rig/R/rig.rdx")
        {
            eprintln!(
                "note: {} and {} have identical lazy-load databases; \
                 these two brackets could be collapsed into one",
                a_name, b_name
            );
        }
    }

    let manifest = Manifest {
        source: source_hash(&src),
        artifacts,
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

    let src = read_tree(&pkg_dir(root))?;
    if source_hash(&src) != manifest.source {
        return Err(
            "src/data/rvenv-pkg changed but the pre-built shim packages were \
                    not rebuilt; run `make rvenv-shim`"
                .to_string(),
        );
    }

    if manifest.artifacts.len() != BRACKETS.len() {
        return Err(format!(
            "{} lists {} artifacts, expected {}",
            MANIFEST_FILE,
            manifest.artifacts.len(),
            BRACKETS.len()
        ));
    }

    for (file, rver, serialize) in BRACKETS {
        let entry = manifest
            .artifacts
            .iter()
            .find(|(f, _, _, _)| f == file)
            .ok_or_else(|| format!("{} has no entry for {}", MANIFEST_FILE, file))?;
        if entry.1 != rver || entry.2 != serialize {
            return Err(format!(
                "{} says {} was built with R {} (serialize {}), expected R {} \
                 (serialize {}); run `make rvenv-shim`",
                MANIFEST_FILE, file, entry.1, entry.2, rver, serialize
            ));
        }
        let path = shim_dir(root).join(file);
        let bytes =
            fs::read(&path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
        if sha256_hex(&bytes) != entry.3 {
            return Err(format!(
                "{} does not match its hash in {}; run `make rvenv-shim`",
                path.display(),
                MANIFEST_FILE
            ));
        }

        let tree = untargz(&bytes)?;
        for required in REQUIRED_ENTRIES {
            if !tree.contains_key(required) {
                return Err(format!("{} has no {}", file, required));
            }
        }
        // This data is unpacked into the user's project, so make sure it
        // cannot write anywhere else.
        for path in tree.keys() {
            if !path.starts_with("rig/") || path.contains("..") {
                return Err(format!("{} contains an unexpected entry: {}", file, path));
            }
        }
        // The source hash cannot catch an artifact rebuilt from unchanged
        // source with the wrong R, but `Built:` records it.
        let description = tree
            .get("rig/DESCRIPTION")
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default();
        match built_r_version(&description) {
            Some(built) if built == rver => {}
            Some(built) => {
                return Err(format!(
                    "{} was built with R {}, but the {} bracket needs R {}",
                    file, built, file, rver
                ))
            }
            None => return Err(format!("{} has no `Built:` field in DESCRIPTION", file)),
        }
    }

    eprintln!("{} shim packages are up to date", BRACKETS.len());
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
    fn targz_is_deterministic() {
        let t = tree(&[("rig/DESCRIPTION", b"Package: rig\n"), ("rig/R/rig", b"x")]);
        assert_eq!(
            deterministic_targz(&t).unwrap(),
            deterministic_targz(&t).unwrap()
        );
    }

    #[test]
    fn targz_round_trips() {
        let t = tree(&[("rig/DESCRIPTION", b"Package: rig\n"), ("rig/R/rig", b"x")]);
        assert_eq!(untargz(&deterministic_targz(&t).unwrap()).unwrap(), t);
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
            artifacts: vec![
                (
                    "shim-lt-3.5.tar.gz".to_string(),
                    "3.4.4".to_string(),
                    2,
                    "deadbeef".to_string(),
                ),
                (
                    "shim-4.0.tar.gz".to_string(),
                    "4.0.5".to_string(),
                    3,
                    "cafe".to_string(),
                ),
            ],
        };
        assert_eq!(parse_manifest(&render_manifest(&m)).unwrap(), m);
    }

    #[test]
    fn manifest_rejects_garbage() {
        assert!(parse_manifest("no source line here = x").is_err());
        assert!(parse_manifest("source = abc\nshim.tar.gz = r 4.0.5").is_err());
    }

    #[test]
    fn built_r_version_parses_the_built_field() {
        assert_eq!(
            built_r_version("Package: rig\nBuilt: R 4.0.5; ; 2026-09-02 10:00:00 UTC; unix\n")
                .as_deref(),
            Some("4.0.5")
        );
        assert_eq!(built_r_version("Package: rig\n"), None);
    }
}
