//! Per-package binary index from Posit Package Manager.
//!
//! P3M serves one compressed TSV per package at
//! `https://ppm.r-pkg.org/binaries/<package>.tsv.zst`, listing every source and
//! binary artifact ever published for that package, with a snapshot-pinned
//! download URL for each. It is the R analogue of the Python simple-repo API
//! index that uv consumes.
//!
//! Columns: `package version platform arch r_version sha256 url linkingto`.
//!
//! ```text
//! pak 0.9.5 source  *      *   f5f899… https://p3m.dev/cran/2026-04-27/src/contrib/pak_0.9.5.tar.gz
//! pak 0.9.5 macos   arm64  4.5 f5f899… https://p3m.dev/cran/2026-04-27/bin/macosx/big-sur-arm64/contrib/4.5/pak_0.9.5.tgz
//! pak 0.9.5 windows x86_64 4.5 f5f899… https://p3m.dev/cran/2026-04-27/bin/windows/contrib/4.5/pak_0.9.5.zip
//! pak 0.9.5 jammy   x86_64 4.5 f5f899… https://p3m.dev/cran/2026-04-27/bin/linux/jammy-x86_64/4.5/src/contrib/pak_0.9.5.tar.gz
//! ```
//!
//! Things about this data that are easy to get wrong:
//!
//! * **`sha256` verifies nothing that P3M serves.** It is the hash of the
//!   *original CRAN* source tarball, repeated byte-identically on every platform
//!   row of a version. So it is not the hash of the binary on that row, and it
//!   is not even the hash of the source tarball at that row's own `p3m.dev`
//!   URL, because P3M rewrites `Repository: CRAN` to `Repository: RSPM` in the
//!   DESCRIPTION before serving it. Treat it strictly as an identity key for the
//!   upstream CRAN artifact; do not build download verification on it.
//! * **`linkingto` records build provenance**, and is populated on binary rows
//!   exactly when the package has a `LinkingTo:` field. It lists the dependency
//!   source versions the binary was compiled against, as comma-separated
//!   `pkg@version=sha256` (those hashes being the same upstream-CRAN identity
//!   hashes as the `sha256` column).
//! * **Several rows can share `(version, platform, arch, r_version)`, and
//!   `linkingto` is exactly what distinguishes them** — e.g. dplyr 0.7.4 on
//!   xenial/R 3.4 has seven rows, one built against `plogr 0.1-1` and another
//!   against `plogr 0.2.0`. There is therefore no correct single-row answer
//!   without knowing which LinkingTo dependency versions were resolved, which is
//!   why [`BinaryIndex::binary_rows`] returns every candidate rather than
//!   picking one.
//! * **There is no aggregate index** — no `ALLBINARIES`, no directory listing.
//!   It is one HTTP request per package, so the index is fetched on demand and
//!   cached as a file rather than being bulk-imported into `packages.db`.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use log::*;
use serde::Deserialize;
use simple_error::bail;
use zstd::stream::read::Decoder as ZstdDecoder;

use crate::cache::get_cache_dir;
use crate::dcf::RPackageVersion;
use crate::download::download_optional_if_newer_;
use crate::rversion::OsVersion;

/// Magic bytes of a zstd frame.
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// Columns we require to be present in the header.
const REQUIRED_COLUMNS: [&str; 7] = [
    "package",
    "version",
    "platform",
    "arch",
    "r_version",
    "sha256",
    "url",
];

/// P3M's generic glibc Linux build, used for any Linux without a specific
/// target. This is the one target name we have to know by name, because P3M
/// lists it against the distro it is built on rather than the ones it serves.
const MANYLINUX: &str = "manylinux_2_28";

/// One `pkg@version=sha256` entry from the `linkingto` column: a dependency
/// source version this binary was compiled against.
///
/// `sha256` is the same upstream-CRAN identity hash as [`BinaryRow::sha256`],
/// not a checksum of anything downloadable from P3M.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkingToRef {
    pub package: String,
    pub version: String,
    pub sha256: String,
}

/// One row of a package's binary index: a single downloadable artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryRow {
    /// Package version, exactly as published. Matched by string equality.
    pub version: String,
    /// `source`, `macos`, `windows`, or a Linux codename such as `jammy`.
    pub platform: String,
    /// `*` for source rows, otherwise `x86_64` or `arm64` (note: `arm64`, not
    /// the `aarch64` that `std::env::consts::ARCH` uses).
    pub arch: String,
    /// `*` for source rows, otherwise a minor R version such as `4.5`.
    pub r_version: String,
    /// Hash of the upstream CRAN source tarball, *not* a checksum for `url`.
    /// See the module docs.
    pub sha256: String,
    pub url: String,
    /// Empty on source rows and for packages without `LinkingTo:`.
    pub linkingto: Vec<LinkingToRef>,
}

impl BinaryRow {
    /// Whether this row is the source tarball rather than a built binary.
    pub fn is_source(&self) -> bool {
        self.platform == "source"
    }
}

/// A package's parsed binary index.
pub struct BinaryIndex {
    package: String,
    rows: Vec<BinaryRow>,
    /// Versions in the order they first appear in the file.
    versions: Vec<String>,
    /// Version -> indices into `rows`, in file order.
    by_version: HashMap<String, Vec<usize>>,
}

/// The cache file backing a package's index, and how we got it.
pub struct CachedIndexFile {
    pub path: PathBuf,
    /// True if this run fetched new content, false if the cached copy was
    /// reused (either younger than the TTL, or revalidated with a 304).
    pub downloaded: bool,
}

/// Base URL for the per-package indices, overridable with `RIG_BINARIES_URL`.
pub fn binaries_base_url() -> String {
    std::env::var("RIG_BINARIES_URL")
        .unwrap_or_else(|_| "https://ppm.r-pkg.org/binaries".to_string())
}

/// URL of a package's binary index.
pub fn binary_index_url(package: &str) -> String {
    format!(
        "{}/{}.tsv.zst",
        binaries_base_url().trim_end_matches('/'),
        package
    )
}

/// Reject anything that is not a plain R package name.
///
/// The name is interpolated into both a filesystem path and a URL, so this is
/// what stops `../..` traversal out of the cache directory.
pub fn validate_package_name(package: &str) -> Result<(), Box<dyn Error>> {
    let ok = !package.is_empty()
        && package.starts_with(|c: char| c.is_ascii_alphabetic())
        && package
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.');
    if !ok {
        bail!("Invalid R package name: '{}'", package);
    }
    Ok(())
}

/// Cache path of a package's index, `<cache>/binaries/<package>.tsv.zst`.
pub fn binary_index_local_file(package: &str) -> Result<PathBuf, Box<dyn Error>> {
    validate_package_name(package)?;
    Ok(get_cache_dir()?
        .join("binaries")
        .join(format!("{}.tsv.zst", package)))
}

/// Sidecar holding the ETag of the cached index.
fn etag_file(index_path: &Path) -> PathBuf {
    let mut name = index_path.as_os_str().to_os_string();
    name.push(".etag");
    PathBuf::from(name)
}

/// Download `url` into `path` if the cached copy is missing or older than
/// `ttl` (24 hours by default), keeping the ETag in a `<path>.etag` sidecar so
/// a stale copy can be revalidated with a 304 instead of re-fetched.
///
/// Returns `Ok(None)` if the server answered 404, and otherwise whether new
/// content was downloaded.
fn ensure_cached_file(
    url: &str,
    path: &Path,
    ttl: Option<Duration>,
) -> Result<Option<bool>, Box<dyn Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let etag_path = etag_file(path);
    let etag = if path.exists() {
        fs::read_to_string(&etag_path).ok()
    } else {
        // A stale ETag without its payload would suppress the download.
        let _ = fs::remove_file(&etag_path);
        None
    };

    let path_buf = path.to_path_buf();
    match download_optional_if_newer_(url, &path_buf, ttl, None, etag.as_deref())? {
        None => Ok(None),
        Some((downloaded, new_etag)) => {
            if downloaded {
                match new_etag {
                    Some(e) => fs::write(&etag_path, e)?,
                    None => {
                        let _ = fs::remove_file(&etag_path);
                    }
                }
            }
            Ok(Some(downloaded))
        }
    }
}

/// Download a package's index if the cached copy is missing or older than
/// `ttl` (24 hours by default), and return the cache file.
///
/// Returns `Ok(None)` if P3M has no index for this package (404). Note that
/// this is only ever observed when we actually make a request: while a cached
/// copy is younger than `ttl` it is served without asking.
pub fn ensure_binary_index_cached(
    package: &str,
    ttl: Option<Duration>,
) -> Result<Option<CachedIndexFile>, Box<dyn Error>> {
    let path = binary_index_local_file(package)?;
    let url = binary_index_url(package);
    match ensure_cached_file(&url, &path, ttl)? {
        None => {
            debug!("No binary index for package '{}'", package);
            Ok(None)
        }
        Some(downloaded) => Ok(Some(CachedIndexFile { path, downloaded })),
    }
}

/// Wrap `bytes` in a zstd decoder if it is a zstd frame, else read it as is.
/// Sniffing the magic bytes lets test fixtures be plain TSV.
fn decode_maybe_zstd(bytes: &[u8]) -> Result<Box<dyn Read + '_>, Box<dyn Error>> {
    if bytes.len() >= 4 && bytes[0..4] == ZSTD_MAGIC {
        Ok(Box::new(ZstdDecoder::new(bytes)?))
    } else {
        Ok(Box::new(bytes))
    }
}

/// Parse the `linkingto` column: comma-separated `pkg@version=sha256`.
fn parse_linkingto(field: &str) -> Vec<LinkingToRef> {
    let mut refs = vec![];
    for entry in field.split(',') {
        if entry.is_empty() {
            continue;
        }
        let parsed = entry
            .split_once('@')
            .and_then(|(pkg, rest)| rest.split_once('=').map(|(ver, sha)| (pkg, ver, sha)));
        match parsed {
            Some((pkg, ver, sha)) if !pkg.is_empty() && !ver.is_empty() && !sha.is_empty() => refs
                .push(LinkingToRef {
                    package: pkg.to_string(),
                    version: ver.to_string(),
                    sha256: sha.to_string(),
                }),
            _ => debug!("Skipping malformed linkingto entry '{}'", entry),
        }
    }
    refs
}

/// Parse a binary index, plain or zstd-compressed.
///
/// Columns are looked up by name from the header row, so extra or reordered
/// columns are tolerated; a missing required column is an error. Individual
/// malformed rows are skipped rather than failing the whole file.
pub fn parse_binaries_tsv(bytes: &[u8]) -> Result<Vec<BinaryRow>, Box<dyn Error>> {
    let reader = BufReader::new(decode_maybe_zstd(bytes)?);
    let mut lines = reader.lines();

    let header = match lines.next() {
        Some(line) => line?,
        None => bail!("Empty binary index, no header row"),
    };
    let columns: HashMap<&str, usize> = header
        .trim_end_matches(['\r', '\n'])
        .split('\t')
        .enumerate()
        .map(|(i, name)| (name, i))
        .collect();

    let mut required = [0usize; REQUIRED_COLUMNS.len()];
    for (slot, name) in required.iter_mut().zip(REQUIRED_COLUMNS.iter()) {
        match columns.get(name) {
            Some(i) => *slot = *i,
            None => bail!("Binary index has no '{}' column", name),
        }
    }
    let [_i_package, i_version, i_platform, i_arch, i_r_version, i_sha256, i_url] = required;
    let i_linkingto = columns.get("linkingto").copied();
    let min_fields = required.iter().max().map(|m| m + 1).unwrap_or(0);

    let mut rows = vec![];
    let mut skipped = 0usize;
    for line in lines {
        let line = line?;
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < min_fields {
            skipped += 1;
            debug!(
                "Skipping malformed binary index row with {} fields: '{}'",
                fields.len(),
                line
            );
            continue;
        }
        // `linkingto` is the last column and is empty for most rows, so tolerate
        // its absence even though P3M does send the trailing tab today.
        let linkingto = i_linkingto
            .and_then(|i| fields.get(i))
            .map(|f| parse_linkingto(f))
            .unwrap_or_default();
        rows.push(BinaryRow {
            version: fields[i_version].to_string(),
            platform: fields[i_platform].to_string(),
            arch: fields[i_arch].to_string(),
            r_version: fields[i_r_version].to_string(),
            sha256: fields[i_sha256].to_string(),
            url: fields[i_url].to_string(),
            linkingto,
        });
    }

    if skipped > 0 {
        warn!("Skipped {} malformed rows in binary index", skipped);
    }

    Ok(rows)
}

/// Compare two package versions the way R does, by numeric components.
///
/// The index files are sorted as *strings*, which puts `0.10.0` and `0.11.1`
/// between `0.1.2.1` and `0.2.0` — so the last version in a file is not the
/// newest one. Versions that do not parse sort lowest rather than winning by
/// accident.
fn compare_versions(a: &str, b: &str) -> Ordering {
    let key = |v: &str| RPackageVersion::from_str(v).ok().map(|p| p.components);
    key(a).cmp(&key(b)).then_with(|| a.cmp(b))
}

impl BinaryIndex {
    /// Build an index from already-parsed rows.
    pub fn from_rows(package: &str, rows: Vec<BinaryRow>) -> BinaryIndex {
        let mut versions: Vec<String> = vec![];
        let mut by_version: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, row) in rows.iter().enumerate() {
            let entry = by_version.entry(row.version.clone()).or_insert_with(|| {
                versions.push(row.version.clone());
                vec![]
            });
            entry.push(i);
        }
        versions.sort_by(|a, b| compare_versions(a, b));
        BinaryIndex {
            package: package.to_string(),
            rows,
            versions,
            by_version,
        }
    }

    /// Parse an index from a file on disk.
    pub fn from_file(package: &str, path: &Path) -> Result<BinaryIndex, Box<dyn Error>> {
        let bytes = fs::read(path)?;
        Ok(BinaryIndex::from_rows(package, parse_binaries_tsv(&bytes)?))
    }

    pub fn package(&self) -> &str {
        &self.package
    }

    pub fn rows(&self) -> &[BinaryRow] {
        &self.rows
    }

    /// All known versions, oldest first.
    ///
    /// Sorted numerically, *not* in the order they appear in the index: those
    /// files are sorted as strings, so `0.9.5` comes last while `0.11.1` is the
    /// newest. See [`compare_versions`].
    pub fn versions(&self) -> &[String] {
        &self.versions
    }

    /// The newest version in the index.
    pub fn latest_version(&self) -> Option<&str> {
        self.versions.last().map(|v| v.as_str())
    }

    fn rows_for_version(&self, version: &str) -> impl Iterator<Item = &BinaryRow> {
        self.by_version
            .get(version)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
            .iter()
            .map(|i| &self.rows[*i])
    }

    /// The source tarball row for a version, if any.
    pub fn source_row(&self, version: &str) -> Option<&BinaryRow> {
        self.rows_for_version(version).find(|r| r.is_source())
    }

    /// Every binary build matching a version and target.
    ///
    /// Returns *all* candidates in file order (oldest snapshot first), because
    /// several builds of the same version can exist for the same target,
    /// differing only in the LinkingTo dependency versions they were compiled
    /// against. Choosing between them requires knowing which of those versions
    /// were resolved, which this layer does not know. For packages without
    /// `LinkingTo:` there is at most one.
    pub fn binary_rows(
        &self,
        version: &str,
        platform: &str,
        arch: &str,
        r_version: &str,
    ) -> Vec<&BinaryRow> {
        self.rows_for_version(version)
            .filter(|r| r.platform == platform && r.arch == arch && r.r_version == r_version)
            .collect()
    }

    /// The newest-snapshot candidate from [`BinaryIndex::binary_rows`].
    ///
    /// Only unambiguous when there is at most one candidate; where several
    /// builds differ by their `linkingto`, picking the last one is arbitrary.
    /// Intended for `rig test binary-index`, not for resolution.
    pub fn latest_binary_row(
        &self,
        version: &str,
        platform: &str,
        arch: &str,
        r_version: &str,
    ) -> Option<&BinaryRow> {
        self.binary_rows(version, platform, arch, r_version).pop()
    }

    /// Distinct platforms a version has artifacts for, in file order.
    pub fn platforms_for(&self, version: &str) -> Vec<&str> {
        let mut seen = vec![];
        for row in self.rows_for_version(version) {
            if !seen.contains(&row.platform.as_str()) {
                seen.push(row.platform.as_str());
            }
        }
        seen
    }
}

/// URL of P3M's status document, overridable with `RIG_PPM_STATUS_URL`.
pub fn ppm_status_url() -> String {
    std::env::var("RIG_PPM_STATUS_URL")
        .unwrap_or_else(|_| "https://packagemanager.posit.co/__api__/status".to_string())
}

/// One entry of P3M's `distros` list: a target P3M builds binaries for.
#[derive(Debug, Clone, Deserialize)]
pub struct PpmDistro {
    pub name: String,
    pub os: String,
    /// P3M's own name for the target. Empty for macOS and Windows.
    #[serde(rename = "binaryURL", default)]
    pub binary_url: String,
    /// The distribution P3M *builds on*, which is not always the one it serves:
    /// `rhel9` is built on `rockylinux` and also listed under `redhat`.
    pub distribution: String,
    pub release: String,
    #[serde(default)]
    pub binaries: bool,
    /// `x86_64` and/or `arm64`. Absent on some pseudo-entries.
    #[serde(default)]
    pub arch: Vec<String>,
}

impl PpmDistro {
    /// How this target appears in the `platform` column of a binary index.
    ///
    /// That is `binaryURL` for Linux, but macOS and Windows have an empty
    /// `binaryURL` and appear under their `name`.
    pub fn platform(&self) -> &str {
        if self.binary_url.is_empty() {
            &self.name
        } else {
            &self.binary_url
        }
    }
}

/// P3M's status document: the authoritative list of what it builds binaries
/// for. Used instead of hard-coding distro-to-codename mappings, which go stale
/// every time P3M adds or retires a target.
#[derive(Debug, Clone, Deserialize)]
pub struct PpmStatus {
    pub distros: Vec<PpmDistro>,
}

/// P3M's `distribution` for an `/etc/os-release` ID.
///
/// Only the vendor names differ between the two vocabularies; the versions come
/// from the status document itself.
///
/// The RHEL rebuilds all go to `redhat`, which spans releases 7 to 10 —
/// `rockylinux` is a partial duplicate covering only 9 and 10, with the same
/// `binaryURL` for both.
fn ppm_distribution(distro: &str) -> Option<&'static str> {
    match distro {
        "ubuntu" => Some("ubuntu"),
        "debian" => Some("debian"),
        "centos" => Some("centos"),
        "rhel" | "redhat" | "rocky" | "rockylinux" | "almalinux" | "alma" => Some("redhat"),
        "opensuse" | "opensuse-leap" => Some("opensuse"),
        "sles" | "sle" => Some("sle"),
        _ => None,
    }
}

/// Undo `detect_platform()`'s dot-stripping for SUSE versions, which reports
/// openSUSE 15.6 as `156` (see the workaround in `platform.rs`). P3M spells the
/// release `15.6`.
fn suse_version_with_dot(version: &str) -> String {
    if !version.contains('.') && version.len() >= 3 && version.chars().all(|c| c.is_ascii_digit()) {
        format!("{}.{}", &version[..2], &version[2..])
    } else {
        version.to_string()
    }
}

/// The arch spelling used by P3M and by the binary index: `arm64` where rig and
/// `std::env::consts::ARCH` say `aarch64`.
fn ppm_arch(arch: &str) -> &str {
    if arch == "aarch64" {
        "arm64"
    } else {
        arch
    }
}

/// Resolve a candidate target to a `(platform, arch)` pair, if it actually
/// builds binaries for that arch.
fn usable(distro: Option<&PpmDistro>, arch: &str) -> Option<(String, String)> {
    let distro = distro?;
    if !distro.binaries || !distro.arch.iter().any(|a| a == arch) {
        return None;
    }
    Some((distro.platform().to_string(), arch.to_string()))
}

impl PpmStatus {
    pub fn parse(bytes: &[u8]) -> Result<PpmStatus, Box<dyn Error>> {
        Ok(serde_json::from_slice(bytes)?)
    }

    /// Cache path of the status document.
    pub fn local_file() -> Result<PathBuf, Box<dyn Error>> {
        Ok(get_cache_dir()?.join("p3m-status.json"))
    }

    /// Fetch (or reuse the cached) status document.
    ///
    /// P3M serves this with `cache-control: no-cache, no-store` and no ETag,
    /// because the document also reports live server state. We cache it anyway,
    /// for the same 24 hours as other repository metadata: the only part we read
    /// is the `distros` list, which changes when P3M adds or retires a build
    /// target — a few times a year. The cost of being a day stale is falling
    /// back to the generic glibc build.
    pub fn load(ttl: Option<Duration>) -> Result<PpmStatus, Box<dyn Error>> {
        let path = PpmStatus::local_file()?;
        let url = ppm_status_url();
        if ensure_cached_file(&url, &path, ttl)?.is_none() {
            bail!("No P3M status document at {}", url);
        }
        PpmStatus::parse(&fs::read(&path)?)
    }

    /// The generic glibc Linux build, used for any Linux without a specific
    /// target. It is excluded from the normal search because it is listed
    /// against the distro it is built on (CentOS 8), which would otherwise make
    /// CentOS 8 ambiguous.
    fn manylinux(&self) -> Option<&PpmDistro> {
        self.distros.iter().find(|d| d.platform() == MANYLINUX)
    }

    fn find_by_os(&self, os: &str) -> Option<&PpmDistro> {
        self.distros.iter().find(|d| d.os == os && d.binaries)
    }

    /// The P3M target for a Linux distro and version, ignoring arch.
    ///
    /// An exact release match wins over a major-version one, so openSUSE 15.6
    /// resolves to `opensuse156` rather than to the `15` release. The
    /// major-version pass is what maps RHEL 9.4 onto the `9` release, since P3M
    /// records only the release it built for.
    fn find_linux(&self, distro: Option<&str>, version: Option<&str>) -> Option<&PpmDistro> {
        let distribution = ppm_distribution(distro?)?;
        let version = version?;
        let version = if distribution == "opensuse" || distribution == "sle" {
            suse_version_with_dot(version)
        } else {
            version.to_string()
        };
        let candidate = |release: &str| {
            self.distros.iter().find(|d| {
                d.binaries
                    && d.platform() != MANYLINUX
                    && d.distribution == distribution
                    && d.release == release
            })
        };
        candidate(&version).or_else(|| {
            let major = version.split('.').next().unwrap_or(&version);
            candidate(major)
        })
    }

    /// Translate a rig platform into the `(platform, arch)` pair used by the
    /// binary index, e.g. `("macos", "arm64")` or `("jammy", "x86_64")`.
    ///
    /// Returns `None` when P3M builds nothing usable, including the case where
    /// the target exists but not for this arch (P3M builds `jammy` for x86_64
    /// only, for instance).
    pub fn ppm_platform(&self, platform: &OsVersion) -> Option<(String, String)> {
        let arch = ppm_arch(&platform.arch);

        if platform.os.starts_with("darwin") {
            return usable(self.find_by_os("macos"), arch);
        }
        if platform.os == "mingw32" {
            return usable(self.find_by_os("windows"), arch);
        }
        if !platform.os.starts_with("linux") {
            return None;
        }

        // A specific target if there is one, else P3M's generic glibc build,
        // which also covers the case of a specific target that is x86_64-only.
        let specific = self.find_linux(platform.distro.as_deref(), platform.version.as_deref());
        usable(specific, arch).or_else(|| usable(self.manylinux(), arch))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        fs::read(PathBuf::from("tests/fixtures/binaries").join(name)).unwrap()
    }

    fn index(package: &str, name: &str) -> BinaryIndex {
        BinaryIndex::from_rows(package, parse_binaries_tsv(&fixture(name)).unwrap())
    }

    #[test]
    fn parses_plain_tsv() {
        let rows = parse_binaries_tsv(&fixture("simple.tsv")).unwrap();
        assert_eq!(rows.len(), 6);

        let src = &rows[0];
        assert_eq!(src.version, "1.0.0");
        assert_eq!(src.platform, "source");
        assert_eq!(src.arch, "*");
        assert_eq!(src.r_version, "*");
        assert_eq!(src.sha256, "aaaa0000");
        assert_eq!(
            src.url,
            "https://p3m.dev/cran/2025-01-01/src/contrib/testpkg_1.0.0.tar.gz"
        );
        assert!(src.is_source());
        // Source rows carry a trailing empty `linkingto`.
        assert!(src.linkingto.is_empty());

        assert_eq!(rows[1].platform, "macos");
        assert_eq!(rows[1].arch, "arm64");
        assert_eq!(rows[1].r_version, "4.5");
        assert!(!rows[1].is_source());
    }

    #[test]
    fn parses_zstd_fixture() {
        let rows = parse_binaries_tsv(&fixture("pak.tsv.zst")).unwrap();
        assert_eq!(rows.len(), 2466);
    }

    #[test]
    fn tolerates_reordered_and_extra_columns() {
        let tsv = "url\tsomething_new\tversion\tpackage\tplatform\tarch\tr_version\tsha256\tlinkingto\n\
                   https://example.com/x.tgz\tignored\t1.0.0\tx\tmacos\tarm64\t4.5\tdeadbeef\tcli@3.6.6=aa11\n";
        let rows = parse_binaries_tsv(tsv.as_bytes()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].url, "https://example.com/x.tgz");
        assert_eq!(rows[0].version, "1.0.0");
        assert_eq!(rows[0].sha256, "deadbeef");
        assert_eq!(rows[0].linkingto.len(), 1);
    }

    #[test]
    fn missing_required_column_is_an_error() {
        // No `url` column.
        let tsv = "package\tversion\tplatform\tarch\tr_version\tsha256\n\
                   x\t1.0.0\tsource\t*\t*\tdeadbeef\n";
        let err = parse_binaries_tsv(tsv.as_bytes()).unwrap_err();
        assert!(err.to_string().contains("url"), "got: {}", err);
    }

    #[test]
    fn missing_linkingto_column_is_fine() {
        let tsv = "package\tversion\tplatform\tarch\tr_version\tsha256\turl\n\
                   x\t1.0.0\tsource\t*\t*\tdeadbeef\thttps://example.com/x.tar.gz\n";
        let rows = parse_binaries_tsv(tsv.as_bytes()).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].linkingto.is_empty());
    }

    #[test]
    fn skips_short_rows_and_keeps_the_rest() {
        let tsv = "package\tversion\tplatform\tarch\tr_version\tsha256\turl\tlinkingto\n\
                   x\t1.0.0\tsource\t*\t*\tdeadbeef\thttps://example.com/a.tar.gz\t\n\
                   x\t1.1.0\ttruncated\n\
                   x\t1.2.0\tsource\t*\t*\tdeadbeef\thttps://example.com/c.tar.gz\t\n";
        let rows = parse_binaries_tsv(tsv.as_bytes()).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].version, "1.0.0");
        assert_eq!(rows[1].version, "1.2.0");
    }

    #[test]
    fn parses_single_entry_linkingto() {
        let idx = index("zip", "zip.tsv.zst");
        let rows = idx.binary_rows("3.0.1", "macos", "arm64", "4.5");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].url,
            "https://p3m.dev/cran/2026-07-14/bin/macosx/big-sur-arm64/contrib/4.5/zip_3.0.1.tgz"
        );
        assert_eq!(
            rows[0].linkingto,
            vec![LinkingToRef {
                package: "cli".to_string(),
                version: "3.6.6".to_string(),
                sha256: "b2b58d6dd82f5798b335e39c00591686a01fd3e94399ef898e146173e36f18f9"
                    .to_string(),
            }]
        );
    }

    #[test]
    fn parses_multi_entry_linkingto() {
        let idx = index("dplyr", "dplyr.tsv.zst");
        let rows = idx.binary_rows("0.7.4", "xenial", "x86_64", "3.4");
        let lt = &rows[0].linkingto;
        assert_eq!(lt.len(), 4);
        let names: Vec<&str> = lt.iter().map(|l| l.package.as_str()).collect();
        assert_eq!(names, vec!["BH", "Rcpp", "bindrcpp", "plogr"]);
        // Versions can contain dashes.
        assert_eq!(lt[0].version, "1.65.0-1");
        assert_eq!(lt[3].version, "0.1-1");
        assert_eq!(lt[0].sha256.len(), 64);
    }

    #[test]
    fn skips_malformed_linkingto_entries_but_keeps_siblings() {
        // Missing `=sha`, and missing `@version`.
        assert_eq!(
            parse_linkingto("cli@3.6.6=aa11,broken,alsobroken@1.0,BH@1.87.0-1=bb22")
                .iter()
                .map(|l| l.package.as_str())
                .collect::<Vec<_>>(),
            vec!["cli", "BH"]
        );
        assert!(parse_linkingto("").is_empty());
    }

    #[test]
    fn finds_the_source_row() {
        let idx = index("pak", "pak.tsv.zst");
        let src = idx.source_row("0.9.5").unwrap();
        assert_eq!(
            src.url,
            "https://p3m.dev/cran/2026-04-27/src/contrib/pak_0.9.5.tar.gz"
        );
        assert_eq!(
            src.sha256,
            "f5f8997ccfaab842b67c4b708dfb34963bb13c0830741101aae9c866c979139c"
        );
        assert!(src.linkingto.is_empty());
    }

    #[test]
    fn finds_a_binary_row() {
        let idx = index("pak", "pak.tsv.zst");
        let rows = idx.binary_rows("0.9.5", "macos", "arm64", "4.5");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].url,
            "https://p3m.dev/cran/2026-04-27/bin/macosx/big-sur-arm64/contrib/4.5/pak_0.9.5.tgz"
        );
    }

    /// Several builds of one version can target the same platform, differing
    /// only in the LinkingTo versions they were compiled against. All of them
    /// must be returned: collapsing them here would bake in a wrong answer.
    #[test]
    fn returns_every_linkingto_candidate() {
        let idx = index("dplyr", "dplyr.tsv.zst");
        let rows = idx.binary_rows("0.7.4", "xenial", "x86_64", "3.4");
        assert_eq!(rows.len(), 7);

        // The candidates are distinguished by `linkingto`, not by anything else.
        let fingerprints: Vec<Vec<(&str, &str)>> = rows
            .iter()
            .map(|r| {
                r.linkingto
                    .iter()
                    .map(|l| (l.package.as_str(), l.version.as_str()))
                    .collect()
            })
            .collect();
        let mut unique = fingerprints.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), 7);

        // Every candidate has its own URL, too.
        let mut urls: Vec<&str> = rows.iter().map(|r| r.url.as_str()).collect();
        urls.sort();
        urls.dedup();
        assert_eq!(urls.len(), 7);

        assert!(fingerprints[0].contains(&("plogr", "0.1-1")));
        assert!(fingerprints[6].contains(&("plogr", "0.2.0")));

        // `latest_binary_row` picks the last of them.
        let latest = idx
            .latest_binary_row("0.7.4", "xenial", "x86_64", "3.4")
            .unwrap();
        assert_eq!(latest.url, rows[6].url);
    }

    #[test]
    fn unknown_lookups_return_nothing() {
        let idx = index("pak", "pak.tsv.zst");
        assert!(idx
            .binary_rows("0.9.5", "nosuchdistro", "x86_64", "4.5")
            .is_empty());
        assert!(idx
            .binary_rows("99.0.0", "macos", "arm64", "4.5")
            .is_empty());
        assert!(idx.source_row("99.0.0").is_none());
        assert!(idx
            .latest_binary_row("99.0.0", "macos", "arm64", "4.5")
            .is_none());
        assert!(idx.platforms_for("99.0.0").is_empty());
    }

    #[test]
    fn indexes_versions() {
        let idx = index("testpkg", "simple.tsv");
        assert_eq!(idx.package(), "testpkg");
        assert_eq!(idx.versions(), ["1.0.0", "1.1.0"]);
        assert_eq!(
            idx.platforms_for("1.0.0"),
            vec!["source", "macos", "windows", "jammy"]
        );
        let pak = index("pak", "pak.tsv.zst");
        assert_eq!(pak.versions().len(), 26);
    }

    /// The index files are sorted as strings, so `0.10.0` and `0.11.1` sit
    /// between `0.1.2.1` and `0.2.0`, and the file's last version is `0.9.5`.
    /// The newest version has to be found numerically.
    #[test]
    fn orders_versions_numerically_not_as_strings() {
        let pak = index("pak", "pak.tsv.zst");
        assert_eq!(pak.latest_version(), Some("0.11.1"));
        assert_eq!(pak.versions().first().map(|s| s.as_str()), Some("0.1.2"));

        // The raw file really is in the misleading order.
        let rows = parse_binaries_tsv(&fixture("pak.tsv.zst")).unwrap();
        assert_eq!(rows.last().unwrap().version, "0.9.5");

        // Ordering rules, including dashed and unequal-length versions.
        assert_eq!(compare_versions("0.11.1", "0.9.5"), Ordering::Greater);
        assert_eq!(compare_versions("0.9.3-1", "0.9.4"), Ordering::Less);
        assert_eq!(compare_versions("0.8.0", "0.8.0.1"), Ordering::Less);
        assert_eq!(compare_versions("1.0.0", "1.0.0"), Ordering::Equal);
        // Unparseable versions lose rather than winning by accident.
        assert_eq!(compare_versions("not-a-version", "0.0.1"), Ordering::Less);
    }

    #[test]
    fn rejects_unsafe_package_names() {
        for bad in ["../evil", "a/b", "", ".hidden", "9lives", "a b", "a_b"] {
            assert!(
                validate_package_name(bad).is_err(),
                "should have rejected '{}'",
                bad
            );
        }
        for good in ["dplyr", "R6", "data.table", "A3"] {
            assert!(validate_package_name(good).is_ok(), "rejected '{}'", good);
        }
    }

    #[test]
    fn builds_the_index_url() {
        assert_eq!(
            binary_index_url("dplyr"),
            "https://ppm.r-pkg.org/binaries/dplyr.tsv.zst"
        );
    }

    fn os(arch: &str, os: &str, distro: Option<&str>, version: Option<&str>) -> OsVersion {
        OsVersion {
            rig_platform: None,
            arch: arch.to_string(),
            vendor: "unknown".to_string(),
            os: os.to_string(),
            distro: distro.map(|s| s.to_string()),
            version: version.map(|s| s.to_string()),
        }
    }

    fn status() -> PpmStatus {
        PpmStatus::parse(&fixture("ppm-status.json")).unwrap()
    }

    fn platform_of(
        arch: &str,
        os_: &str,
        distro: Option<&str>,
        version: Option<&str>,
    ) -> Option<(String, String)> {
        status().ppm_platform(&os(arch, os_, distro, version))
    }

    fn expect(platform: &str, arch: &str) -> Option<(String, String)> {
        Some((platform.to_string(), arch.to_string()))
    }

    #[test]
    fn parses_the_status_document() {
        let s = status();
        assert!(s.distros.len() > 20);
        let jammy = s.distros.iter().find(|d| d.name == "jammy").unwrap();
        assert_eq!(jammy.distribution, "ubuntu");
        assert_eq!(jammy.release, "22.04");
        assert_eq!(jammy.platform(), "jammy");
        assert_eq!(jammy.arch, vec!["x86_64"]);
        // macOS and Windows have an empty binaryURL and appear under `name`.
        let macos = s.distros.iter().find(|d| d.os == "macos").unwrap();
        assert_eq!(macos.binary_url, "");
        assert_eq!(macos.platform(), "macos");
    }

    #[test]
    fn maps_platforms_to_the_index_vocabulary() {
        // aarch64 -> arm64 is a real rename, not a passthrough.
        assert_eq!(
            platform_of("aarch64", "darwin23", None, None),
            expect("macos", "arm64")
        );
        assert_eq!(
            platform_of("x86_64", "darwin20", None, None),
            expect("macos", "x86_64")
        );
        assert_eq!(
            platform_of("x86_64", "mingw32", None, None),
            expect("windows", "x86_64")
        );
        assert_eq!(
            platform_of("x86_64", "linux", Some("ubuntu"), Some("22.04")),
            expect("jammy", "x86_64")
        );
        assert_eq!(
            platform_of("aarch64", "linux", Some("ubuntu"), Some("24.04")),
            expect("noble", "arm64")
        );
        assert_eq!(
            platform_of("x86_64", "linux", Some("debian"), Some("12")),
            expect("bookworm", "x86_64")
        );
        // `linux-gnu` is what parse_platform_string() produces for a full triple.
        assert_eq!(
            platform_of("x86_64", "linux-gnu", Some("ubuntu"), Some("22.04")),
            expect("jammy", "x86_64")
        );
        assert_eq!(platform_of("x86_64", "freebsd", None, None), None);
    }

    /// P3M records the release it built for, which can be less precise than
    /// what the machine reports.
    #[test]
    fn matches_point_releases_to_their_major() {
        assert_eq!(
            platform_of("x86_64", "linux", Some("rhel"), Some("9.4")),
            expect("rhel9", "x86_64")
        );
        assert_eq!(
            platform_of("x86_64", "linux", Some("rhel"), Some("10.1")),
            expect("rhel10", "x86_64")
        );
        // The RHEL rebuilds all resolve through the `redhat` releases.
        assert_eq!(
            platform_of("x86_64", "linux", Some("rocky"), Some("9.5")),
            expect("rhel9", "x86_64")
        );
        assert_eq!(
            platform_of("x86_64", "linux", Some("almalinux"), Some("9.4")),
            expect("rhel9", "x86_64")
        );
        assert_eq!(
            platform_of("aarch64", "linux", Some("almalinux"), Some("10.0")),
            expect("rhel10", "arm64")
        );
        // Below 9 the `redhat` releases point at the CentOS builds.
        assert_eq!(
            platform_of("x86_64", "linux", Some("almalinux"), Some("8.10")),
            expect("centos8", "x86_64")
        );
        assert_eq!(
            platform_of("x86_64", "linux", Some("rhel"), Some("7.9")),
            expect("centos7", "x86_64")
        );
    }

    /// `detect_platform()` reports openSUSE versions with the dots stripped
    /// (`156`) while SLES keeps them (`15.6`); P3M spells both `15.6`.
    #[test]
    fn normalizes_suse_versions() {
        assert_eq!(suse_version_with_dot("156"), "15.6");
        assert_eq!(suse_version_with_dot("15.6"), "15.6");
        assert_eq!(suse_version_with_dot("15"), "15");

        assert_eq!(
            platform_of("x86_64", "linux", Some("opensuse"), Some("156")),
            expect("opensuse156", "x86_64")
        );
        assert_eq!(
            platform_of("x86_64", "linux", Some("opensuse-leap"), Some("152")),
            expect("opensuse152", "x86_64")
        );
        assert_eq!(
            platform_of("x86_64", "linux", Some("opensuse"), Some("423")),
            expect("opensuse42", "x86_64")
        );

        // SLES targets are *named* `sles156` but their `binaryURL` — and the
        // platform the indices actually use — is `opensuse156`. Going by `name`
        // here would produce a platform no index contains.
        assert_eq!(
            platform_of("x86_64", "linux", Some("sles"), Some("15.6")),
            expect("opensuse156", "x86_64")
        );
        assert_eq!(
            platform_of("x86_64", "linux", Some("sles"), Some("15")),
            expect("opensuse15", "x86_64")
        );
    }

    /// A Linux P3M has no specific target for still gets the generic glibc build.
    #[test]
    fn falls_back_to_the_generic_glibc_build() {
        assert_eq!(
            platform_of("x86_64", "linux", Some("fedora"), Some("42")),
            expect(MANYLINUX, "x86_64")
        );
        // jammy is built for x86_64 only, so arm64 falls back rather than
        // pointing at binaries that do not exist.
        assert_eq!(
            platform_of("aarch64", "linux", Some("ubuntu"), Some("22.04")),
            expect(MANYLINUX, "arm64")
        );
    }

    /// The generic build is listed against CentOS 8, the distro it is built on.
    /// CentOS 8 itself must still resolve to `centos8`.
    #[test]
    fn generic_build_does_not_shadow_the_distro_it_is_built_on() {
        assert_eq!(
            platform_of("x86_64", "linux", Some("centos"), Some("8")),
            expect("centos8", "x86_64")
        );
    }

    /// Every platform the mapping produces must be one the binary indices
    /// actually use.
    #[test]
    fn produced_platforms_exist_in_a_real_index() {
        let dplyr = index("dplyr", "dplyr.tsv.zst");
        let seen: Vec<&str> = dplyr.rows().iter().map(|r| r.platform.as_str()).collect();
        for (distro, version) in [
            ("ubuntu", "22.04"),
            ("ubuntu", "24.04"),
            ("debian", "12"),
            ("rhel", "9.4"),
            ("centos", "8"),
            ("opensuse", "156"),
            ("fedora", "42"),
        ] {
            let (platform, _) =
                platform_of("x86_64", "linux", Some(distro), Some(version)).unwrap();
            assert!(
                seen.contains(&platform.as_str()),
                "{} {} -> '{}', which no dplyr row uses",
                distro,
                version,
                platform
            );
        }
    }
}
