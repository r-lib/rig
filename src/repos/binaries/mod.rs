//! Per-package binary index of an extended metadata feed (Tidyup 12, format
//! 1.x).
//!
//! A feed serves one compressed TSV per package at
//! `<base>/binaries/<package>.tsv.zst`, e.g.
//! `https://ppm.r-pkg.org/v1/binaries/pak.tsv.zst`, listing every source and
//! binary artifact ever published for that package, with a snapshot-pinned
//! download URL for each. It is the R analogue of the Python simple-repo API
//! index that uv consumes.
//!
//! The file starts with a `# Format-Version: 1.0` comment, then a header.
//! Required columns: `package version platform r_version source-id url`.
//! Optional: `linkingto sha256 md5 filesize built yanked yanked_reason`, where
//! a missing value is `NA`.
//!
//! ```text
//! pak 0.9.5 source                                  *   f5f899… ../2026-04-27/src/contrib/pak_0.9.5.tar.gz
//! pak 0.9.5 aarch64-apple-darwin                    4.5 f5f899… ../2026-04-27/bin/macosx/big-sur-arm64/contrib/4.5/pak_0.9.5.tgz
//! pak 0.9.5 x86_64-w64-mingw32                      4.5 f5f899… ../2026-04-27/bin/windows/contrib/4.5/pak_0.9.5.zip
//! pak 0.9.5 x86_64-unknown-linux-gnu-ubuntu-22.04   4.5 f5f899… ../2026-04-27/bin/linux/jammy-x86_64/4.5/src/contrib/pak_0.9.5.tar.gz
//! ```
//!
//! `platform` is `source` or a platform string, the same ones `rproj.lock`
//! uses, see [`crate::platform`]. `url` is relative to the index file.
//!
//! Things about this data that are easy to get wrong:
//!
//! * **`source-id` verifies nothing that P3M serves.** It is the `Id` of the
//!   source version in ALLPACKAGES, for P3M the hash of the *original CRAN*
//!   source tarball, repeated on every platform row of a version. So it is not
//!   the hash of the binary on that row, and not even the hash of the source
//!   tarball at that row's own URL, because P3M rewrites `Repository: CRAN` to
//!   `Repository: RSPM` in the DESCRIPTION before serving it. Treat it strictly
//!   as an identity key for the upstream artifact. The optional `sha256`
//!   column is the checksum of the file at `url`.
//! * **`linkingto` records build provenance**, and is populated on binary rows
//!   exactly when the package has a `LinkingTo:` field. It lists the dependency
//!   source versions the binary was compiled against, as comma-separated
//!   `pkg@version=id` (those ids being the same kind of identity as the
//!   `source-id` column). It is the *transitive* header closure, not
//!   a copy of the package's own `LinkingTo:` field: ragg links to `systemfonts`
//!   and `textshaping` only, but its rows also list `cpp11`, which those two
//!   link to. So an entry need not be a declared dependency of the package at
//!   all — see `crate::solver::binary_artifact_deps` for what the solver does
//!   with that.
//! * **Several rows can share `(version, platform, r_version)`, and
//!   `linkingto` is exactly what distinguishes them** — e.g. dplyr 0.7.4 on
//!   Ubuntu 16.04/R 3.4 has seven rows, one built against `plogr 0.1-1` and another
//!   against `plogr 0.2.0`. There is therefore no correct single-row answer
//!   without knowing which LinkingTo dependency versions were resolved, which is
//!   why [`BinaryIndex::binary_rows`] returns every candidate rather than
//!   picking one.
//! * **There is no aggregate index** — no `ALLBINARIES`, no directory listing.
//!   It is one HTTP request per package, so the index is fetched on demand and
//!   cached per package rather than being bulk-imported into `packages.db`.
//!
//! Parsing that TSV costs a couple of milliseconds per package, and rig is a
//! CLI, so the cost would be paid on every invocation. It is not: the TSV is
//! parsed once, out of the response body, and what gets cached is a columnar
//! blob that later runs read directly. The TSV itself is never written to
//! disk. See [`blob`] for the format and [`load_binary_index`] for the flow.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use futures::stream::StreamExt;
use log::debug;
use serde::{Deserialize, Serialize};
use zstd::stream::read::Decoder as ZstdDecoder;

use crate::cache::get_cache_dir;
use crate::download::{
    download_optional_if_newer_, fetch_optional_if_modified, fetch_optional_if_modified_,
    ConditionalFetch,
};
use crate::output::OUTPUT;
use crate::repos::feed::{resolve_download_url, MetadataFeed, RepoId};

/// How long a cached index or status document is used without asking the
/// server, matching the default in `crate::download`.
const DEFAULT_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// How many indices [`prefetch_binary_indices_in`] has in flight at once. There is
/// one request per package and they are small, so the whole batch is round-trip
/// bound; the limit is there to be a good citizen towards P3M rather than to
/// protect us.
const PREFETCH_CONCURRENCY: usize = 16;

pub mod blob;
pub mod loader;
use crate::rversion::OsVersion;
use crate::utils::http_client;
use crate::utils::is_r_package_name;
use crate::utils::write_atomically;
use blob::IndexBlob;
pub use blob::LinkingTo;

/// Magic bytes of a zstd frame.
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// Columns we require to be present in the header.
const REQUIRED_COLUMNS: [&str; 6] = [
    "package",
    "version",
    "platform",
    "r_version",
    "source-id",
    "url",
];

/// P3M's generic glibc Linux build, used for any Linux without a specific
/// target. This is the one target name we have to know by name, because P3M
/// lists it against the distro it is built on rather than the ones it serves.
pub(crate) const MANYLINUX: &str = "manylinux_2_28";

/// One `pkg@version=id` entry from the `linkingto` column: a dependency
/// source version this binary was compiled against.
///
/// `sha256` is the `Id` of that version, the same kind of identity as
/// [`BinaryRow::source_id`], not a checksum of anything downloadable.
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
    /// `source`, or a platform string such as `aarch64-apple-darwin` or
    /// `x86_64-unknown-linux-gnu-ubuntu-22.04`.
    pub platform: String,
    /// `*` for source rows, otherwise a minor R version such as `4.5`.
    pub r_version: String,
    /// The `Id` of the source version, *not* a checksum for `url`. See the
    /// module docs.
    pub source_id: String,
    pub url: String,
    /// Empty on source rows and for packages without `LinkingTo:`.
    pub linkingto: Vec<LinkingToRef>,
    /// SHA-256 checksum of the file at `url`, if the index has it.
    pub sha256: Option<String>,
    /// The `Built` field of a binary, if the index has it.
    pub built: Option<String>,
    /// The time the build was yanked, if it was.
    pub yanked: Option<String>,
    /// Why the build was yanked, if it was.
    pub yanked_reason: Option<String>,
}

/// A package's index, and how we got it.
pub struct CachedIndex {
    pub index: BinaryIndex,
    /// True if this run fetched new content, false if the cached copy was
    /// reused (either younger than the TTL, or revalidated with a 304).
    pub downloaded: bool,
}

/// URL of a package's CRAN binary index.
pub fn binary_index_url(package: &str) -> String {
    binary_index_url_in(&MetadataFeed::cran(), package)
}

/// URL of a package's binary index in `feed`.
pub fn binary_index_url_in(feed: &MetadataFeed, package: &str) -> String {
    format!("{}/{}.tsv.zst", feed.binaries_url, package)
}

/// The full download URL of `url`, from the binary index of `package` in
/// `feed`, see [`resolve_download_url`]. The package files of a P3M feed
/// come from its configured repository, see [`MetadataFeed::files`].
pub fn resolve_index_url(feed: &MetadataFeed, package: &str, url: &str) -> String {
    resolve_download_url(
        &binary_index_url_in(feed, package),
        url,
        feed.files.as_deref(),
    )
}

/// The cache directory of `feed`'s binary indices: CRAN's are in
/// `<cache>/metadata/binaries`, a Bioconductor release's in a subdirectory
/// of it, e.g. `bioc-3.22`, because the same package name may have an index
/// in both. Another CRAN feed, e.g. the own metadata of a PPM repository,
/// has a subdirectory of its own, because its indices differ from P3M's.
fn binary_index_dir(feed: &MetadataFeed) -> Result<PathBuf, Box<dyn Error>> {
    let dir = get_cache_dir()?.join("metadata").join("binaries");
    Ok(binary_index_subdir(dir, feed))
}

fn binary_index_subdir(dir: PathBuf, feed: &MetadataFeed) -> PathBuf {
    match &feed.repo {
        RepoId::Cran if feed.allpackages_url == MetadataFeed::cran().allpackages_url => dir,
        RepoId::Cran => dir.join(format!(
            "cran-{}",
            crate::utils::calculate_hash(&feed.allpackages_url)
        )),
        RepoId::Bioc(v) => dir.join(format!("bioc-{}", v)),
        // A CRAN-like repository has no per-package binary indices, its
        // binaries are in its `PACKAGES` files instead.
        RepoId::Cranlike { url, .. } => {
            dir.join(format!("cranlike-{}", crate::utils::calculate_hash(url)))
        }
    }
}

/// Reject anything that is not a plain R package name.
///
/// The name is interpolated into both a filesystem path and a URL, so this is
/// what stops `../..` traversal out of the cache directory.
pub fn validate_package_name(package: &str) -> Result<(), Box<dyn Error>> {
    if !is_r_package_name(package) {
        bail!("Invalid R package name: '{}'", package);
    }
    Ok(())
}

/// Cache path of a package's columnar blob,
/// `<cache>/metadata/binaries/<package>.v<n>.rbi`.
///
/// This is the only copy of the index we keep: the TSV the server sends is
/// parsed straight from memory and never lands on disk.
///
/// The format version is part of the *name*, so a rig that understands a newer
/// layout does not find the old file at all rather than reading one it would
/// have to reject.
pub fn binary_index_blob_file(package: &str) -> Result<PathBuf, Box<dyn Error>> {
    blob_file_in(&MetadataFeed::cran(), package)
}

fn blob_file_in(feed: &MetadataFeed, package: &str) -> Result<PathBuf, Box<dyn Error>> {
    validate_package_name(package)?;
    Ok(binary_index_dir(feed)?.join(format!("{}.v{}.rbi", package, blob::FORMAT_VERSION)))
}

/// Cache path of a blob's marker file,
/// `<cache>/metadata/binaries/<package>.v<n>.etag`.
///
/// It holds the `ETag` of the response the blob was built from, and it does
/// double duty as the blob's commit marker and its freshness clock:
///
/// * It is written *after* the blob, so its presence means the blob beside it
///   was written in full. A run interrupted between the two leaves a blob with
///   no marker, which the next run re-downloads rather than trusting.
/// * Its mtime is when we last knew the blob to be current — set when the blob
///   is built, and again whenever the server answers 304. That is what the TTL
///   is measured against, so a revalidated index does not get re-checked on
///   every invocation.
///
/// It carries the same format version as the blob it describes, because it
/// describes *that* blob and not whatever a different rig would have built.
/// An empty file means the response carried no `ETag`: still a valid marker,
/// just nothing to revalidate with.
pub fn binary_index_etag_file(package: &str) -> Result<PathBuf, Box<dyn Error>> {
    etag_file_in(&MetadataFeed::cran(), package)
}

fn etag_file_in(feed: &MetadataFeed, package: &str) -> Result<PathBuf, Box<dyn Error>> {
    validate_package_name(package)?;
    Ok(binary_index_dir(feed)?.join(format!("{}.v{}.etag", package, blob::FORMAT_VERSION)))
}

/// Cache path of the marker that says P3M has no index for a package,
/// `<cache>/metadata/binaries/<package>.missing`.
///
/// Many packages have no index in a feed, e.g. a Bioconductor package in
/// CRAN's, or a Bioconductor annotation package, which has no binaries. Without
/// this marker every solve would ask about them again. Its mtime is when the
/// server last answered 404, and it is trusted for the same TTL as a blob.
fn missing_file_in(feed: &MetadataFeed, package: &str) -> Result<PathBuf, Box<dyn Error>> {
    validate_package_name(package)?;
    Ok(binary_index_dir(feed)?.join(format!("{}.missing", package)))
}

/// Whether the server answered 404 for `package` less than `ttl` ago.
fn known_missing(feed: &MetadataFeed, package: &str, ttl: Duration) -> bool {
    missing_file_in(feed, package)
        .ok()
        .and_then(|path| file_age(&path))
        .is_some_and(|age| age < ttl)
}

/// Remember that the server has no index for `package`. Not being able to
/// write it only costs a request next time.
fn mark_missing(feed: &MetadataFeed, package: &str) {
    if let Ok(path) = missing_file_in(feed, package) {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Err(err) = fs::write(&path, b"") {
            debug!("Could not write {}: {}", path.display(), err);
        }
    }
}

/// Sidecar holding the ETag of a cached file.
fn etag_file(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
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

/// Load a package's binary index, downloading it if what we have is missing,
/// unusable, or older than `ttl` (24 hours by default).
///
/// Returns `Ok(None)` if P3M has no index for this package (404). Note that
/// this is only ever observed when we actually make a request: while the
/// cached blob is younger than `ttl` it is used without asking.
///
/// The blob is the only artifact kept; the TSV is parsed out of the response
/// body and discarded. That means the blob is authoritative rather than
/// derived, so the two ways of losing it are handled explicitly:
///
/// * **A blob that will not open** is discarded along with its marker, and the
///   refetch is unconditional. Sending `If-None-Match` here would earn a 304
///   and no body, leaving us with nothing to parse.
/// * **A marker with no readable blob beside it** — the crash window between
///   the two writes — is deleted for the same reason.
///
/// Concurrent `rig` runs are safe but not serialized. Both writes are atomic
/// renames, so a reader always sees a whole file, and neither process can
/// observe the other's partial state. Two runs that fetch at once can still
/// interleave into a marker describing the *other* run's blob; that needs the
/// server's content to have changed between the two fetches, and the worst it
/// costs is a needless refetch, or an index one snapshot stale until the TTL
/// expires. Nothing is lost, because a blob is only ever replaced by another
/// complete blob.
pub fn load_binary_index(
    package: &str,
    ttl: Option<Duration>,
) -> Result<Option<CachedIndex>, Box<dyn Error>> {
    load_binary_index_in(&MetadataFeed::cran(), package, ttl)
}

/// [`load_binary_index`] for the index of `package` in `feed`.
pub fn load_binary_index_in(
    feed: &MetadataFeed,
    package: &str,
    ttl: Option<Duration>,
) -> Result<Option<CachedIndex>, Box<dyn Error>> {
    let etag_path = etag_file_in(feed, package)?;
    let ttl = ttl.unwrap_or(DEFAULT_TTL);

    let cached = read_cached_blob(&blob_file_in(feed, package)?);
    if cached.is_none() {
        // A marker without a usable blob would suppress the download, or ask
        // for a 304 we could not use.
        let _ = fs::remove_file(&etag_path);
    }

    if cached.is_some() && file_age(&etag_path).is_some_and(|age| age < ttl) {
        debug!(
            "Binary index of '{}' is up to date, skipping download",
            package
        );
        return Ok(cached.map(|index| CachedIndex {
            index,
            downloaded: false,
        }));
    }

    if known_missing(feed, package, ttl) {
        debug!("No binary index for package '{}' (cached)", package);
        return Ok(None);
    }

    // Only ask for a 304 if we still hold the content one would refer to.
    let etag = cached
        .as_ref()
        .and_then(|_| fs::read_to_string(&etag_path).ok())
        .filter(|e| !e.is_empty());

    let url = binary_index_url_in(feed, package);
    match fetch_optional_if_modified_(&url, etag.as_deref(), None)? {
        ConditionalFetch::NotFound => {
            debug!("No binary index for package '{}'", package);
            mark_missing(feed, package);
            Ok(None)
        }

        ConditionalFetch::NotModified => match cached {
            Some(index) => {
                restart_ttl(feed, package);
                Ok(Some(CachedIndex {
                    index,
                    downloaded: false,
                }))
            }
            // We did not send `If-None-Match`, so this is the server ignoring
            // the request rather than anything we can recover from here.
            None => bail!("Server answered 304 for {} without being asked", url),
        },

        ConditionalFetch::Fetched { bytes, etag } => Ok(Some(CachedIndex {
            index: BinaryIndex::open_blob(&store_index(feed, package, &bytes, etag)?)?,
            downloaded: true,
        })),
    }
}

/// Parse a downloaded index and put it in the cache, returning the blob that
/// was built from it.
///
/// The blob is written first and its marker second: a marker means the blob
/// beside it is complete. A cache we cannot write is a slow next run, not a
/// failure of this one, so neither write is fatal — but the marker must not
/// outlive a blob that never landed.
fn store_index(
    feed: &MetadataFeed,
    package: &str,
    tsv: &[u8],
    etag: Option<String>,
) -> Result<Vec<u8>, Box<dyn Error>> {
    // The blob keeps the URLs as they are in the index, relative ones are
    // relative to the index, e.g. `../2026-11-21+.../src/contrib/cli_3.6.6.tar.gz`
    // from PPM. They are resolved when read, see [`resolve_index_url`].
    let rows = parse_binaries_tsv(tsv)?;
    let built = blob::build(package, &rows)?;
    debug!(
        "Built binary index blob for '{}' ({} rows, {} bytes)",
        package,
        rows.len(),
        built.len()
    );
    let blob_path = blob_file_in(feed, package)?;
    let etag_path = etag_file_in(feed, package)?;
    match write_atomically(&blob_path, &built) {
        Ok(()) => {
            if let Err(err) = write_atomically(&etag_path, etag.unwrap_or_default().as_bytes()) {
                debug!("Could not write {}: {}", etag_path.display(), err);
            }
            if let Ok(missing) = missing_file_in(feed, package) {
                let _ = fs::remove_file(missing);
            }
        }
        Err(err) => {
            debug!("Could not write {}: {}", blob_path.display(), err);
            let _ = fs::remove_file(&etag_path);
        }
    }
    Ok(built)
}

/// Note that a cached blob was just confirmed current, so the TTL is measured
/// from now instead of from when it was downloaded.
fn restart_ttl(feed: &MetadataFeed, package: &str) {
    if let Ok(etag_path) = etag_file_in(feed, package) {
        let _ = filetime::set_file_mtime(&etag_path, filetime::FileTime::now());
    }
}

/// What a package needs before its index can be read.
enum Prefetch {
    /// The cached blob is younger than the TTL, so there is nothing to do.
    Cached,
    /// A request is needed, sending this `ETag` if there is one to revalidate
    /// with.
    Fetch(Option<String>),
}

/// Decide whether `package` needs a request.
///
/// Unlike [`load_binary_index`] this only checks that a blob is *there*, it
/// does not open it: prefetching is a head start, and a blob that turns out to
/// be unusable is `load_binary_index`'s problem when it gets to it.
fn prefetch_plan(
    feed: &MetadataFeed,
    package: &str,
    ttl: Duration,
) -> Result<Prefetch, Box<dyn Error>> {
    let blob_path = blob_file_in(feed, package)?;
    let etag_path = etag_file_in(feed, package)?;
    if !blob_path.exists() {
        let _ = fs::remove_file(&etag_path);
        if known_missing(feed, package, ttl) {
            return Ok(Prefetch::Cached);
        }
        return Ok(Prefetch::Fetch(None));
    }
    if file_age(&etag_path).is_some_and(|age| age < ttl) {
        return Ok(Prefetch::Cached);
    }
    Ok(Prefetch::Fetch(
        fs::read_to_string(&etag_path)
            .ok()
            .filter(|e| !e.is_empty()),
    ))
}

/// Fill the cache of `feed`'s indices for many packages at once, with several
/// requests in flight.
///
/// [`load_binary_index_in`] makes one blocking request per package, so a solve
/// that walks a hundred packages pays a hundred round trips end to end. Given
/// the packages up front, this pays them concurrently instead, and leaves
/// exactly what `load_binary_index` would have written.
///
/// It is best effort and reports nothing: every package it fails on is simply
/// one that `load_binary_index` fetches itself later. Packages whose cached
/// index is still fresh cost nothing here, so calling this with more packages
/// than the solve turns out to need is cheap on a warm cache.
pub fn prefetch_binary_indices_in(feed: &MetadataFeed, packages: &[String], ttl: Option<Duration>) {
    let ttl = ttl.unwrap_or(DEFAULT_TTL);
    let mut seen: HashSet<&str> = HashSet::new();
    let mut todo: Vec<(String, Option<String>)> = vec![];
    for package in packages {
        if !seen.insert(package.as_str()) {
            continue;
        }
        match prefetch_plan(feed, package, ttl) {
            Ok(Prefetch::Cached) => {}
            Ok(Prefetch::Fetch(etag)) => todo.push((package.clone(), etag)),
            Err(err) => debug!("Not prefetching binary index of '{}': {}", package, err),
        }
    }

    if todo.is_empty() {
        debug!("All {} binary indices are up to date", seen.len());
        return;
    }
    debug!(
        "Prefetching {} of {} binary indices, {} at a time",
        todo.len(),
        seen.len(),
        PREFETCH_CONCURRENCY
    );
    if let Err(err) = prefetch_all(feed, &todo) {
        debug!("Could not prefetch binary indices: {}", err);
    }
}

/// The request half of [`prefetch_binary_indices_in`], on its own runtime.
///
/// Parsing an index and building its blob takes a couple of milliseconds, which
/// is why it happens on the blocking pool: it overlaps with the requests still
/// in flight instead of being tacked onto the end of them.
#[tokio::main]
async fn prefetch_all(
    feed: &MetadataFeed,
    todo: &[(String, Option<String>)],
) -> Result<(), Box<dyn Error>> {
    let client = http_client();
    futures::stream::iter(todo.iter().map(|(package, etag)| {
        let client = &client;
        async move {
            let url = binary_index_url_in(feed, package);
            match fetch_optional_if_modified(client, &url, etag.as_deref()).await {
                Err(err) => debug!("Could not prefetch {}: {}", url, err),
                Ok(ConditionalFetch::NotFound) => {
                    debug!("No binary index for package '{}'", package);
                    mark_missing(feed, package);
                }
                Ok(ConditionalFetch::NotModified) => restart_ttl(feed, package),
                Ok(ConditionalFetch::Fetched { bytes, etag }) => {
                    let package = package.clone();
                    let feed = feed.clone();
                    let stored = tokio::task::spawn_blocking(move || {
                        store_index(&feed, &package, &bytes, etag)
                            .map(|_| ())
                            .map_err(|e| {
                                format!("Could not store binary index of '{}': {}", package, e)
                            })
                    })
                    .await;
                    match stored {
                        Ok(Err(err)) => debug!("{}", err),
                        Err(err) => debug!("Binary index prefetch task failed: {}", err),
                        Ok(Ok(())) => {}
                    }
                }
            }
        }
    }))
    .buffer_unordered(PREFETCH_CONCURRENCY)
    .count()
    .await;
    Ok(())
}

/// Open a cached blob, treating an unusable one as absent.
///
/// It is a cache file, so a truncated or corrupt one is something to replace
/// rather than to report.
fn read_cached_blob(path: &Path) -> Option<BinaryIndex> {
    let bytes = fs::read(path).ok()?;
    match BinaryIndex::open_blob(&bytes) {
        Ok(index) => {
            debug!("Opened binary index blob at {}", path.display());
            Some(index)
        }
        Err(err) => {
            debug!("Discarding unusable blob at {}: {}", path.display(), err);
            None
        }
    }
}

/// How long ago `path` was last written, or `None` if it does not exist or the
/// clock disagrees with it.
fn file_age(path: &Path) -> Option<Duration> {
    let modified = fs::metadata(path).and_then(|m| m.modified()).ok()?;
    SystemTime::now().duration_since(modified).ok()
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
/// The leading comment lines are skipped, and the first one must give a
/// format version rig reads, see [`crate::repos::format_version`]. Columns
/// are looked up by name from the header row, so extra or reordered columns
/// are tolerated; a missing required column is an error. Individual
/// malformed rows are skipped rather than failing the whole file.
pub fn parse_binaries_tsv(bytes: &[u8]) -> Result<Vec<BinaryRow>, Box<dyn Error>> {
    let reader = BufReader::new(decode_maybe_zstd(bytes)?);
    let mut lines = reader.lines();

    let mut first = true;
    let mut found_version = None;
    let header = loop {
        let line = match lines.next() {
            Some(line) => line?,
            None => bail!("Empty binary index, no header row"),
        };
        if !line.starts_with('#') {
            break line;
        }
        if first {
            found_version = crate::repos::format_version::FormatVersion::from_comment(&line);
        }
        first = false;
    };
    crate::repos::format_version::check(found_version, "binary index")?;
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
    let [_i_package, i_version, i_platform, i_r_version, i_source_id, i_url] = required;
    let i_linkingto = columns.get("linkingto").copied();
    let i_sha256 = columns.get("sha256").copied();
    let i_built = columns.get("built").copied();
    let i_yanked = columns.get("yanked").copied();
    let i_yanked_reason = columns.get("yanked_reason").copied();
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
        // An optional column: absent, empty and `NA` all mean missing.
        let opt = |i: Option<usize>| {
            i.and_then(|i| fields.get(i))
                .filter(|f| !f.is_empty() && **f != "NA")
                .map(|f| f.to_string())
        };
        let linkingto = opt(i_linkingto)
            .map(|f| parse_linkingto(&f))
            .unwrap_or_default();
        rows.push(BinaryRow {
            version: fields[i_version].to_string(),
            platform: fields[i_platform].to_string(),
            r_version: fields[i_r_version].to_string(),
            source_id: fields[i_source_id].to_string(),
            url: fields[i_url].to_string(),
            linkingto,
            sha256: opt(i_sha256),
            built: opt(i_built),
            yanked: opt(i_yanked),
            yanked_reason: opt(i_yanked_reason),
        });
    }

    if skipped > 0 {
        OUTPUT.warn(&format!(
            "Skipped {} malformed rows in binary index",
            skipped
        ));
    }

    Ok(rows)
}

/// A package version, borrowed from the index.
///
/// The index stores the components [`RPackageVersion`] would parse out, so
/// ordering one is a slice comparison rather than a re-parse. Versions that do
/// not parse have no components, which sorts them below every version that
/// does rather than letting them win by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VersionRef<'a> {
    pub original: &'a str,
    pub components: &'a [u32],
}

impl Ord for VersionRef<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        // An empty component list means the version did not parse, and `[]` is
        // already `Less` than any non-empty list.
        self.components
            .cmp(other.components)
            .then_with(|| self.original.cmp(other.original))
    }
}

impl PartialOrd for VersionRef<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for VersionRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.original)
    }
}

/// One row of a package's binary index, borrowed from it.
///
/// Every accessor is a pair of integer reads and a slice into the index, so
/// nothing here allocates. Use [`BinaryRowRef::to_owned`] to lift a row out.
#[derive(Clone, Copy)]
pub struct BinaryRowRef<'a> {
    index: &'a BinaryIndex,
    row: usize,
}

impl<'a> BinaryRowRef<'a> {
    /// Position of this row in the whole index.
    ///
    /// This is the identity of a build: rows that share
    /// `(version, platform, r_version)` differ only in their `linkingto`,
    /// so there is nothing else to tell them apart by.
    pub fn row_index(&self) -> usize {
        self.row
    }

    pub fn version(&self) -> VersionRef<'a> {
        self.index.version(self.index.blob.row_version(self.row))
    }

    /// `source`, or a platform string such as `aarch64-apple-darwin`.
    pub fn platform(&self) -> &'a str {
        self.index.blob.row_platform(self.row)
    }

    /// `*` for source rows, otherwise a minor R version such as `4.5`.
    pub fn r_version(&self) -> &'a str {
        self.index.blob.row_r_version(self.row)
    }

    /// The `Id` of the source version, *not* a checksum for
    /// [`BinaryRowRef::url`]. See the module docs.
    pub fn source_id(&self) -> &'a str {
        self.index.blob.row_source_id(self.row)
    }

    /// SHA-256 checksum of the file at [`BinaryRowRef::url`], if known.
    pub fn sha256(&self) -> Option<&'a str> {
        self.index.blob.row_sha256(self.row)
    }

    /// The `Built` field of a binary, if known.
    pub fn built(&self) -> Option<&'a str> {
        self.index.blob.row_built(self.row)
    }

    /// The time the build was yanked, if it was.
    pub fn yanked(&self) -> Option<&'a str> {
        self.index.blob.row_yanked(self.row)
    }

    /// Why the build was yanked, if it was.
    pub fn yanked_reason(&self) -> Option<&'a str> {
        self.index.blob.row_yanked_reason(self.row)
    }

    pub fn url(&self) -> &'a str {
        self.index.blob.row_url(self.row)
    }

    /// Empty on source rows and for packages without `LinkingTo:`.
    pub fn linkingto(&self) -> impl Iterator<Item = LinkingTo<'a>> + 'a {
        self.index.blob.linkingto(self.row)
    }

    /// Whether this row is the source tarball rather than a built binary.
    pub fn is_source(&self) -> bool {
        self.platform() == "source"
    }

    /// Lift a row out of the index, copying its strings.
    pub fn to_owned(self) -> BinaryRow {
        BinaryRow {
            version: self.version().original.to_string(),
            platform: self.platform().to_string(),
            r_version: self.r_version().to_string(),
            source_id: self.source_id().to_string(),
            url: self.url().to_string(),
            sha256: self.sha256().map(String::from),
            built: self.built().map(String::from),
            yanked: self.yanked().map(String::from),
            yanked_reason: self.yanked_reason().map(String::from),
            linkingto: self
                .linkingto()
                .map(|l| LinkingToRef {
                    package: l.package.to_string(),
                    version: l.version.to_string(),
                    sha256: l.sha256.to_string(),
                })
                .collect(),
        }
    }
}

impl fmt::Debug for BinaryRowRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (*self).to_owned().fmt(f)
    }
}

/// A package's binary index, backed by the columnar blob it was read from.
pub struct BinaryIndex {
    blob: IndexBlob,
}

impl BinaryIndex {
    /// Read an index from an already-encoded blob.
    pub fn open_blob(bytes: &[u8]) -> Result<BinaryIndex, Box<dyn Error>> {
        Ok(BinaryIndex {
            blob: IndexBlob::open(bytes)?,
        })
    }

    pub fn package(&self) -> &str {
        self.blob.package()
    }

    pub fn num_rows(&self) -> usize {
        self.blob.nrows()
    }

    /// All known versions, oldest first.
    ///
    /// Sorted numerically, *not* in the order they appear in the index: those
    /// files are sorted as strings, so `0.9.5` comes last while `0.11.1` is the
    /// newest.
    pub fn versions(&self) -> &[String] {
        self.blob.versions()
    }

    /// The `i`th version, with its parsed components.
    pub fn version(&self, i: usize) -> VersionRef<'_> {
        VersionRef {
            original: self.versions().get(i).map(|v| v.as_str()).unwrap_or(""),
            components: self.blob.version_components(i),
        }
    }

    /// The newest version in the index, with its parsed components.
    pub fn latest_version(&self) -> Option<VersionRef<'_>> {
        self.versions()
            .len()
            .checked_sub(1)
            .map(|last| self.version(last))
    }

    /// Position of a version in [`BinaryIndex::versions`].
    ///
    /// A scan: the versions are ordered numerically, so they are not in string
    /// order to binary-search, and there are at most a couple of hundred of
    /// them.
    pub fn version_index(&self, version: &str) -> Option<usize> {
        self.versions().iter().position(|v| v == version)
    }

    /// The rows of one version, in file order.
    pub fn rows_for_version(&self, version: &str) -> impl Iterator<Item = BinaryRowRef<'_>> + '_ {
        let range = match self.version_index(version) {
            Some(i) => self.blob.version_rows(i),
            None => 0..0,
        };
        range.map(|row| BinaryRowRef { index: self, row })
    }

    /// The source tarball row for a version, if any.
    pub fn source_row(&self, version: &str) -> Option<BinaryRowRef<'_>> {
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
        r_version: &str,
    ) -> Vec<BinaryRowRef<'_>> {
        self.rows_for_version(version)
            .filter(|r| r.platform() == platform && r.r_version() == r_version)
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
        r_version: &str,
    ) -> Option<BinaryRowRef<'_>> {
        self.binary_rows(version, platform, r_version).pop()
    }

    /// Distinct platforms a version has artifacts for, in file order.
    pub fn platforms_for(&self, version: &str) -> Vec<&str> {
        let mut seen: Vec<&str> = vec![];
        for row in self.rows_for_version(version) {
            if !seen.contains(&row.platform()) {
                seen.push(row.platform());
            }
        }
        seen
    }
}

/// The public P3M instance, and the base of the default status URL.
const DEFAULT_PPM_URL: &str = "https://p3m.dev";

/// Host names of the public P3M instance. `packagemanager.posit.co` is its
/// older, longer name; both serve the same repositories.
const PUBLIC_PPM_HOSTS: [&str; 2] = ["p3m.dev", "packagemanager.posit.co"];

/// Base URL of the Posit Package Manager instance rig uses.
///
/// `PACKAGEMANAGER_ADDRESS` is Posit's own variable for pointing a machine at a
/// private P3M, so rig honors that rather than inventing a name for the same
/// thing. Because an environment variable is easy to lose (`sudo`, services,
/// the menu bar app), the `ppm-address` key of the rig config file works,
/// too.
///
/// Note that this does *not* affect the binary index URLs of
/// [`MetadataFeed`]: the per-package indices are rig's own derived data, and
/// no P3M instance serves them.
pub fn ppm_url() -> String {
    let setting = crate::config::get_setting("PACKAGEMANAGER_ADDRESS", "ppm-address")
        .ok()
        .flatten();
    ppm_url_from(setting.as_deref())
}

/// Whether [`ppm_url`] is the public P3M instance.
pub fn ppm_is_public_instance() -> bool {
    ppm_is_public(&ppm_url())
}

/// Whether `url` is the public P3M instance, under any of its names.
pub fn ppm_is_public(url: &str) -> bool {
    let rest = url
        .trim()
        .strip_prefix("https://")
        .or_else(|| url.trim().strip_prefix("http://"));
    let Some(rest) = rest else {
        return false;
    };
    let rest = rest.trim_end_matches('/');
    PUBLIC_PPM_HOSTS
        .iter()
        .any(|h| rest.eq_ignore_ascii_case(h))
}

/// URL of the status document of the instance [`ppm_url`] names.
pub fn ppm_status_url() -> String {
    format!("{}/__api__/status", ppm_url())
}

/// The pure part of [`ppm_url`], split out because `cargo test` runs the whole
/// suite in one multi-threaded process, where setting an env var in one test
/// races every other test that reads it.
fn ppm_url_from(env: Option<&str>) -> String {
    match env.map(str::trim).filter(|v| !v.is_empty()) {
        Some(url) => url.trim_end_matches('/').to_string(),
        None => DEFAULT_PPM_URL.to_string(),
    }
}

/// One entry of P3M's `distros` list: a target P3M builds binaries for.
///
/// The camelCase keys are `alias`es rather than `rename`s so that both
/// spellings deserialize: rig's own `--json` output is snake_case like the rest
/// of rig, and a status document cached by an older rig still parses.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PpmDistro {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub os: String,
    /// P3M's own name for the target. Empty for macOS and Windows.
    #[serde(default, alias = "binaryURL")]
    pub binary_url: String,
    /// Human-readable name of the *binary* target, e.g. `CentOS/RHEL 7` for
    /// `centos7`, which serves two distros.
    #[serde(default, alias = "binaryDisplay")]
    pub binary_display: String,
    /// Human-readable name of the distro itself, e.g. `CentOS 7`.
    #[serde(default)]
    pub display: String,
    /// The distribution P3M *builds on*, which is not always the one it serves:
    /// `rhel9` is built on `rockylinux` and also listed under `redhat`.
    #[serde(default)]
    pub distribution: String,
    #[serde(default)]
    pub release: String,
    /// Set when the binaries served for this target were built somewhere else,
    /// e.g. macOS binaries are built on `jammy`.
    #[serde(default)]
    pub build_distribution: String,
    /// Whether P3M can report system requirements for this target.
    #[serde(default, alias = "sysReqs")]
    pub sys_reqs: bool,
    #[serde(default)]
    pub binaries: bool,
    /// Set on targets P3M no longer advertises, i.e. retired distro releases.
    /// The binaries stay downloadable.
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub official_rspm: bool,
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

/// File name the status document is cached under.
///
/// It has to depend on the URL: two instances describe different build targets,
/// and a shared cache entry would have `PACKAGEMANAGER_ADDRESS` reporting — and,
/// worse, *resolving binaries against* — the wrong instance for up to a day.
/// The public instance keeps the unsuffixed name, so the common case has a
/// recognizable cache file and existing caches stay valid.
fn status_cache_name(url: &str) -> String {
    if let Some(base) = url.strip_suffix("/__api__/status") {
        if ppm_is_public(base) {
            return "p3m-status.json".to_string();
        }
    }
    format!(
        "p3m-status-{}.json",
        &crate::utils::calculate_hash(url)[..12]
    )
}

/// One entry of P3M's `bioc_versions` list: a Bioconductor release, the R
/// version it goes with, and the CRAN snapshot it is pinned to.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PpmBiocVersion {
    #[serde(default)]
    pub bioc_version: String,
    #[serde(default)]
    pub r_version: String,
    /// A date, or `latest` for the current release.
    #[serde(default)]
    pub cran_snapshot: String,
}

/// The macOS build flavors P3M serves for one R version, e.g.
/// `sonoma-arm64` / `big-sur-x86_64`. Either can be missing or empty, which
/// means P3M has no macOS binaries for that R version and arch.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PpmMacosUrls {
    #[serde(default)]
    pub arm64: String,
    #[serde(default)]
    pub x86_64: String,
}

/// How long the instance's license still covers it.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PpmSupportWindow {
    #[serde(default)]
    pub disable_expiry_banner: bool,
    #[serde(default)]
    pub days_left: i64,
    #[serde(default)]
    pub days_reminder: i64,
}

/// P3M's status document: the authoritative list of what it builds binaries
/// for. Used instead of hard-coding distro-to-codename mappings, which go stale
/// every time P3M adds or retires a target.
///
/// Every field is optional, because a private instance is under no obligation
/// to send any particular one, and a missing field should degrade the report
/// rather than fail the parse.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PpmStatus {
    /// The instance's own version, e.g. `2026.08.0`.
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub build_date: String,
    /// Name of the CRAN repository on this instance, usually `cran`.
    #[serde(default)]
    pub cran_repo: String,
    #[serde(default)]
    pub r_configured: bool,
    #[serde(default)]
    pub python_configured: bool,
    #[serde(default)]
    pub binaries_enabled: bool,
    #[serde(default)]
    pub auth_enabled: bool,
    #[serde(default)]
    pub support_window: PpmSupportWindow,
    #[serde(default)]
    pub distros: Vec<PpmDistro>,
    /// Minor R versions the instance builds binaries for, newest first.
    #[serde(default)]
    pub r_versions: Vec<String>,
    #[serde(default)]
    pub bioc_versions: Vec<PpmBiocVersion>,
    /// Keyed by minor R version, plus a `default` entry. A `BTreeMap` so
    /// `--json` output has a deterministic key order; note that the resulting
    /// order is lexicographic and so not the one to print a table in (see
    /// `crate::ppm`).
    #[serde(default)]
    pub macos_urls: BTreeMap<String, PpmMacosUrls>,
    /// Everything else the instance sent, kept verbatim so that `--json` never
    /// silently drops a field rig does not model, and so that fields whose type
    /// varies between instances cannot fail the parse. `custom_home` is the
    /// motivating case: a bool on the public instance, but documented as a
    /// home-page override, so plausibly a string elsewhere.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
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
pub(crate) fn suse_version_with_dot(version: &str) -> String {
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

/// A P3M build target for one arch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PpmTarget {
    /// P3M's name for the target, e.g. `macos`, `windows`, `jammy`.
    pub platform: String,
    /// `x86_64` or `arm64`.
    pub arch: String,
    /// The canonical rig platform string of the target, e.g.
    /// `aarch64-apple-darwin` or `x86_64-unknown-linux-gnu-ubuntu-22.04`, see
    /// [`crate::platform::parse_platform_string`].
    pub triple: String,
}

/// The rig distro name for a P3M `distribution`, the reverse of
/// [`ppm_distribution`].
fn rig_distribution(distribution: &str) -> &str {
    match distribution {
        "redhat" | "rockylinux" => "rhel",
        "sle" => "sles",
        other => other,
    }
}

/// The canonical rig platform string of a P3M target and arch, the
/// `platform` of its rows in the binary indices, see Tidyup 12.
///
/// `distro` is the target P3M builds the binaries on, see
/// [`PpmStatus::building_distro`]: a distro that uses the binaries of another
/// one has that one's platform, e.g. SLES 15.6 is `opensuse-15.6`, and
/// CentOS 8 is `rhel-8`. The version is the major version for RHEL and
/// Debian, and `major.minor` for Ubuntu and openSUSE.
fn target_triple(distro: &PpmDistro, arch: &str) -> String {
    let arch = if arch == "arm64" { "aarch64" } else { arch };
    match distro.os.as_str() {
        "macos" => format!("{}-apple-darwin", arch),
        "windows" => format!("{}-w64-mingw32", arch),
        _ if distro.platform() == MANYLINUX => {
            format!("{}-unknown-linux-gnu-{}", arch, MANYLINUX)
        }
        _ => {
            let name = match distro.distribution.as_str() {
                "centos" => "rhel",
                other => rig_distribution(other),
            };
            let release = distro.release.as_str();
            let major = release.split('.').next().unwrap_or(release);
            let version = match name {
                "rhel" | "debian" => major.to_string(),
                "opensuse" | "sles" if !release.contains('.') => format!("{}.0", release),
                _ => release.to_string(),
            };
            format!("{}-unknown-linux-gnu-{}-{}", arch, name, version)
        }
    }
}

/// Resolve a candidate target to a [`PpmTarget`], if it actually builds
/// binaries for that arch.
fn usable(status: &PpmStatus, distro: Option<&PpmDistro>, arch: &str) -> Option<PpmTarget> {
    let distro = distro?;
    if !distro.binaries || !distro.arch.iter().any(|a| a == arch) {
        return None;
    }
    Some(PpmTarget {
        platform: distro.platform().to_string(),
        arch: arch.to_string(),
        triple: target_triple(status.building_distro(distro), arch),
    })
}

impl PpmStatus {
    pub fn parse(bytes: &[u8]) -> Result<PpmStatus, Box<dyn Error>> {
        Ok(serde_json::from_slice(bytes)?)
    }

    /// Cache path of the status document.
    pub fn local_file() -> Result<PathBuf, Box<dyn Error>> {
        Ok(get_cache_dir()?
            .join("p3m")
            .join(status_cache_name(&ppm_status_url())))
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

    /// The target that builds the binaries `distro` uses: the one whose
    /// name is its `binaryURL`, e.g. `opensuse156` for `sles156`, or `distro`
    /// itself.
    fn building_distro<'a>(&'a self, distro: &'a PpmDistro) -> &'a PpmDistro {
        if distro.os != "linux" || distro.platform() == MANYLINUX {
            return distro;
        }
        self.distros
            .iter()
            .find(|d| d.os == "linux" && d.name == distro.platform())
            .unwrap_or(distro)
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
    ///
    /// A distro without a version is taken as P3M's own name for the target,
    /// e.g. `noble` from the `noble-x86_64` spelling lockfiles use.
    fn find_linux(&self, distro: Option<&str>, version: Option<&str>) -> Option<&PpmDistro> {
        let distro = distro?;
        if version.is_none() {
            return self
                .distros
                .iter()
                .find(|d| d.binaries && d.os == "linux" && d.platform() == distro);
        }
        let distribution = ppm_distribution(distro)?;
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
        self.ppm_target(platform).map(|t| (t.platform, t.arch))
    }

    /// The target of one P3M distro and arch, if it builds binaries for it.
    #[cfg(test)]
    fn ppm_target_of(&self, distro: &PpmDistro, arch: &str) -> Option<PpmTarget> {
        usable(self, Some(distro), arch)
    }

    /// Like [`PpmStatus::ppm_platform`], but also with the target's canonical
    /// rig platform string.
    pub fn ppm_target(&self, platform: &OsVersion) -> Option<PpmTarget> {
        let arch = ppm_arch(&platform.arch);

        if platform.os.starts_with("darwin") {
            return usable(self, self.find_by_os("macos"), arch);
        }
        if platform.os == "mingw32" {
            return usable(self, self.find_by_os("windows"), arch);
        }
        if !platform.os.starts_with("linux") {
            return None;
        }

        // A specific target if there is one, else P3M's generic glibc build,
        // which also covers the case of a specific target that is x86_64-only.
        let specific = self.find_linux(platform.distro.as_deref(), platform.version.as_deref());
        usable(self, specific, arch).or_else(|| usable(self, self.manylinux(), arch))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_index_urls_are_relative_to_the_index() {
        let feed = MetadataFeed::at(RepoId::Cran, "http://localhost:4242/all");
        let index = binary_index_url_in(&feed, "cli");
        assert_eq!(index, "http://localhost:4242/all/binaries/cli.tsv.zst");
        assert_eq!(
            resolve_download_url(
                &index,
                "../2026-11-21+abc/src/contrib/cli_3.6.6.tar.gz",
                None
            ),
            "http://localhost:4242/all/2026-11-21+abc/src/contrib/cli_3.6.6.tar.gz"
        );
        assert_eq!(
            resolve_download_url(&index, "https://p3m.dev/cran/x.tgz", None),
            "https://p3m.dev/cran/x.tgz"
        );
    }

    #[test]
    fn cran_feeds_have_separate_index_dirs() {
        let dir = PathBuf::from("binaries");
        let p3m = MetadataFeed::cran();
        assert_eq!(binary_index_subdir(dir.clone(), &p3m), dir);

        let all = MetadataFeed::at(RepoId::Cran, "http://localhost:4242/all");
        let cran = MetadataFeed::at(RepoId::Cran, "http://localhost:4242/cran");
        let all_dir = binary_index_subdir(dir.clone(), &all);
        let cran_dir = binary_index_subdir(dir.clone(), &cran);
        assert_ne!(all_dir, dir);
        assert_ne!(all_dir, cran_dir);
        assert!(all_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("cran-"));
    }

    fn fixture(name: &str) -> Vec<u8> {
        fs::read(PathBuf::from("tests/fixtures/binaries").join(name)).unwrap()
    }

    /// Build an index the way [`BinaryIndex::load`] does, through the blob
    /// encoder, so these tests exercise the on-disk format too.
    fn index(package: &str, name: &str) -> BinaryIndex {
        let rows = parse_binaries_tsv(&fixture(name)).unwrap();
        BinaryIndex::open_blob(&blob::build(package, &rows).unwrap()).unwrap()
    }

    #[test]
    fn parses_plain_tsv() {
        let rows = parse_binaries_tsv(&fixture("simple.tsv")).unwrap();
        assert_eq!(rows.len(), 6);

        let src = &rows[0];
        assert_eq!(src.version, "1.0.0");
        assert_eq!(src.platform, "source");
        assert_eq!(src.r_version, "*");
        assert_eq!(src.source_id, "aaaa0000");
        // URLs are kept as they are, see `store_index` for resolving them.
        assert_eq!(src.url, "../2025-01-01/src/contrib/testpkg_1.0.0.tar.gz");
        // `NA` and empty optional values are missing.
        assert!(src.linkingto.is_empty());
        assert_eq!(src.sha256, None);
        assert_eq!(src.built, None);
        assert_eq!(src.yanked, None);

        assert_eq!(rows[1].platform, "aarch64-apple-darwin");
        assert_eq!(rows[1].r_version, "4.5");
        assert_eq!(rows[1].sha256.as_deref(), Some("eeee0001"));
        assert!(rows[1].built.as_deref().unwrap().starts_with("R 4.5.0;"));

        assert_eq!(rows[2].yanked.as_deref(), Some("2025-02-01T00:00:00Z"));
        assert_eq!(rows[2].yanked_reason.as_deref(), Some("Broken build"));
    }

    #[test]
    fn parses_zstd_fixture() {
        let rows = parse_binaries_tsv(&fixture("pak.tsv.zst")).unwrap();
        assert_eq!(rows.len(), 2617);
    }

    /// Prepend the format version comment to a test index.
    fn v1(tsv: &str) -> Vec<u8> {
        format!("# Format-Version: 1.0\n{}", tsv).into_bytes()
    }

    #[test]
    fn tolerates_reordered_and_extra_columns() {
        let tsv = "url\tsomething_new\tversion\tpackage\tplatform\tr_version\tsource-id\tlinkingto\n\
                   https://example.com/x.tgz\tignored\t1.0.0\tx\taarch64-apple-darwin\t4.5\tdeadbeef\tcli@3.6.6=aa11\n";
        let rows = parse_binaries_tsv(&v1(tsv)).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].url, "https://example.com/x.tgz");
        assert_eq!(rows[0].version, "1.0.0");
        assert_eq!(rows[0].source_id, "deadbeef");
        assert_eq!(rows[0].linkingto.len(), 1);
    }

    #[test]
    fn missing_required_column_is_an_error() {
        // No `url` column.
        let tsv = "package\tversion\tplatform\tr_version\tsource-id\n\
                   x\t1.0.0\tsource\t*\tdeadbeef\n";
        let err = parse_binaries_tsv(&v1(tsv)).unwrap_err();
        assert!(err.to_string().contains("url"), "got: {}", err);
    }

    #[test]
    fn needs_a_known_format_version() {
        let tsv = "package\tversion\tplatform\tr_version\tsource-id\turl\n\
                   x\t1.0.0\tsource\t*\tdeadbeef\thttps://example.com/x.tar.gz\n";
        // No version: the format before Tidyup 12.
        assert!(parse_binaries_tsv(tsv.as_bytes()).is_err());
        let major = format!("# Format-Version: 2.0\n{}", tsv);
        assert!(parse_binaries_tsv(major.as_bytes()).is_err());
        // A newer minor version, with more comment lines.
        let minor = format!("# Format-Version: 1.3\n# more\n{}", tsv);
        assert_eq!(parse_binaries_tsv(minor.as_bytes()).unwrap().len(), 1);
    }

    #[test]
    fn missing_linkingto_column_is_fine() {
        let tsv = "package\tversion\tplatform\tr_version\tsource-id\turl\n\
                   x\t1.0.0\tsource\t*\tdeadbeef\thttps://example.com/x.tar.gz\n";
        let rows = parse_binaries_tsv(&v1(tsv)).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].linkingto.is_empty());
    }

    #[test]
    fn skips_short_rows_and_keeps_the_rest() {
        let tsv = "package\tversion\tplatform\tr_version\tsource-id\turl\tlinkingto\n\
                   x\t1.0.0\tsource\t*\tdeadbeef\thttps://example.com/a.tar.gz\t\n\
                   x\t1.1.0\ttruncated\n\
                   x\t1.2.0\tsource\t*\tdeadbeef\thttps://example.com/c.tar.gz\t\n";
        let rows = parse_binaries_tsv(&v1(tsv)).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].version, "1.0.0");
        assert_eq!(rows[1].version, "1.2.0");
    }

    #[test]
    fn parses_single_entry_linkingto() {
        let idx = index("zip", "zip.tsv.zst");
        let rows = idx.binary_rows("3.0.1", "aarch64-apple-darwin", "4.5");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].url(),
            "../2026-07-14/bin/macosx/big-sur-arm64/contrib/4.5/zip_3.0.1.tgz"
        );
        assert_eq!(
            rows[0].linkingto().collect::<Vec<_>>(),
            vec![LinkingTo {
                package: "cli",
                version: "3.6.6",
                sha256: "b2b58d6dd82f5798b335e39c00591686a01fd3e94399ef898e146173e36f18f9",
            }]
        );
    }

    #[test]
    fn parses_multi_entry_linkingto() {
        let idx = index("dplyr", "dplyr.tsv.zst");
        let rows = idx.binary_rows("0.7.4", "x86_64-unknown-linux-gnu-ubuntu-16.04", "3.4");
        let lt: Vec<LinkingTo> = rows[0].linkingto().collect();
        assert_eq!(lt.len(), 4);
        let names: Vec<&str> = lt.iter().map(|l| l.package).collect();
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
        assert!(src.is_source());
        assert_eq!(src.url(), "../2026-04-27/src/contrib/pak_0.9.5.tar.gz");
        assert_eq!(
            src.source_id(),
            "f5f8997ccfaab842b67c4b708dfb34963bb13c0830741101aae9c866c979139c"
        );
        assert_eq!(src.linkingto().count(), 0);
        assert_eq!(src.version().original, "0.9.5");
        assert_eq!(src.version().components, [0, 9, 5]);
    }

    #[test]
    fn finds_a_binary_row() {
        let idx = index("pak", "pak.tsv.zst");
        let rows = idx.binary_rows("0.9.5", "aarch64-apple-darwin", "4.5");
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].is_source());
        assert_eq!(
            rows[0].url(),
            "../2026-04-27/bin/macosx/big-sur-arm64/contrib/4.5/pak_0.9.5.tgz"
        );
    }

    /// Several builds of one version can target the same platform, differing
    /// only in the LinkingTo versions they were compiled against. All of them
    /// must be returned: collapsing them here would bake in a wrong answer.
    #[test]
    fn returns_every_linkingto_candidate() {
        let idx = index("dplyr", "dplyr.tsv.zst");
        let xenial = "x86_64-unknown-linux-gnu-ubuntu-16.04";
        let rows = idx.binary_rows("0.7.4", xenial, "3.4");
        assert_eq!(rows.len(), 7);

        // The candidates are distinguished by `linkingto`, not by anything else.
        let fingerprints: Vec<Vec<(&str, &str)>> = rows
            .iter()
            .map(|r| r.linkingto().map(|l| (l.package, l.version)).collect())
            .collect();
        let mut unique = fingerprints.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), 7);

        // Every candidate has its own URL, too.
        let mut urls: Vec<&str> = rows.iter().map(|r| r.url()).collect();
        urls.sort();
        urls.dedup();
        assert_eq!(urls.len(), 7);

        assert!(fingerprints[0].contains(&("plogr", "0.1-1")));
        assert!(fingerprints[6].contains(&("plogr", "0.2.0")));

        // `latest_binary_row` picks the last of them.
        let latest = idx.latest_binary_row("0.7.4", xenial, "3.4").unwrap();
        assert_eq!(latest.url(), rows[6].url());
    }

    #[test]
    fn unknown_lookups_return_nothing() {
        let idx = index("pak", "pak.tsv.zst");
        assert!(idx
            .binary_rows("0.9.5", "x86_64-unknown-linux-gnu-nosuchdistro-1", "4.5")
            .is_empty());
        assert!(idx
            .binary_rows("99.0.0", "aarch64-apple-darwin", "4.5")
            .is_empty());
        assert!(idx.source_row("99.0.0").is_none());
        assert!(idx
            .latest_binary_row("99.0.0", "aarch64-apple-darwin", "4.5")
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
            vec![
                "source",
                "aarch64-apple-darwin",
                "x86_64-w64-mingw32",
                "x86_64-unknown-linux-gnu-ubuntu-22.04"
            ]
        );
        let pak = index("pak", "pak.tsv.zst");
        assert_eq!(pak.versions().len(), 27);
    }

    /// The index files may be in any order, e.g. sorted as strings, which
    /// puts `0.10.0` and `0.11.1` between `0.1.2.1` and `0.2.0`. The newest
    /// version has to be found numerically.
    #[test]
    fn orders_versions_numerically_not_as_strings() {
        let pak = index("pak", "pak.tsv.zst");
        let latest = pak.latest_version().unwrap();
        assert_eq!(latest.original, "0.11.2");
        assert_eq!(latest.components, [0, 11, 2]);
        assert_eq!(pak.versions().first().map(|s| s.as_str()), Some("0.1.2"));

        // A file sorted as strings gives the same order.
        let mut rows = parse_binaries_tsv(&fixture("pak.tsv.zst")).unwrap();
        rows.sort_by(|a, b| a.version.cmp(&b.version));
        assert_eq!(rows.last().unwrap().version, "0.9.5");
        let sorted = BinaryIndex::open_blob(&blob::build("pak", &rows).unwrap()).unwrap();
        assert_eq!(sorted.versions(), pak.versions());

        // The stored list really is ascending by `VersionRef`'s ordering.
        let all: Vec<VersionRef> = (0..pak.versions().len()).map(|i| pak.version(i)).collect();
        assert!(all.windows(2).all(|w| w[0] < w[1]));
    }

    /// Ordering rules, including dashed and unequal-length versions.
    #[test]
    fn version_ordering_rules() {
        let v = |original, components: &'static [u32]| VersionRef {
            original,
            components,
        };
        assert!(v("0.11.1", &[0, 11, 1]) > v("0.9.5", &[0, 9, 5]));
        assert!(v("0.9.3-1", &[0, 9, 3, 1]) < v("0.9.4", &[0, 9, 4]));
        assert!(v("0.8.0", &[0, 8, 0]) < v("0.8.0.1", &[0, 8, 0, 1]));
        assert_eq!(
            v("1.0.0", &[1, 0, 0]).cmp(&v("1.0.0", &[1, 0, 0])),
            Ordering::Equal
        );
        // Unparseable versions have no components, and lose rather than
        // winning by accident.
        assert!(v("not-a-version", &[]) < v("0.0.1", &[0, 0, 1]));
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
            "https://ppm.r-pkg.org/v1/binaries/dplyr.tsv.zst"
        );
    }

    /// The marker carries the same format version as the blob it describes: it
    /// records the ETag *that blob* was built from, which says nothing about a
    /// blob some other rig would build.
    #[test]
    fn blob_and_marker_share_a_format_version() {
        let blob = binary_index_blob_file("dplyr").unwrap();
        let etag = binary_index_etag_file("dplyr").unwrap();
        assert_eq!(
            blob.file_name().unwrap(),
            format!("dplyr.v{}.rbi", blob::FORMAT_VERSION).as_str()
        );
        assert_eq!(
            etag.file_name().unwrap(),
            format!("dplyr.v{}.etag", blob::FORMAT_VERSION).as_str()
        );
        assert_eq!(blob.parent(), etag.parent());
        // Both go through the same name check, so neither can escape the cache.
        assert!(binary_index_blob_file("../evil").is_err());
        assert!(binary_index_etag_file("../evil").is_err());
    }

    /// A reader must see either the whole old file or the whole new one, and
    /// no temp file may be left behind.
    #[test]
    fn writes_atomically_without_leaving_litter() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("f.rbi");
        write_atomically(&path, b"first").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"first");
        write_atomically(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");

        let left: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, ["f.rbi"]);
    }

    /// The blob is the only copy of the data, so an unusable one has to be
    /// noticed here — the caller's response is to refetch unconditionally.
    #[test]
    fn treats_an_unusable_blob_as_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.v1.rbi");
        assert!(read_cached_blob(&path).is_none(), "missing");

        let rows = parse_binaries_tsv(&fixture("simple.tsv")).unwrap();
        let good = blob::build("testpkg", &rows).unwrap();
        fs::write(&path, &good).unwrap();
        let index = read_cached_blob(&path).expect("a good blob should open");
        assert_eq!(index.package(), "testpkg");
        assert_eq!(index.num_rows(), 6);

        fs::write(&path, &good[..good.len() / 2]).unwrap();
        assert!(read_cached_blob(&path).is_none(), "truncated");

        let mut corrupt = good.clone();
        corrupt[4] = 99; // format version
        fs::write(&path, &corrupt).unwrap();
        assert!(read_cached_blob(&path).is_none(), "wrong format version");

        fs::write(&path, b"").unwrap();
        assert!(read_cached_blob(&path).is_none(), "empty");
    }

    #[test]
    fn ages_a_file_from_its_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("marker");
        assert!(file_age(&path).is_none(), "a missing file has no age");

        fs::write(&path, b"").unwrap();
        assert!(file_age(&path).unwrap() < DEFAULT_TTL);

        // What the 304 path does, and what a marker older than the TTL means.
        let long_ago = SystemTime::now() - DEFAULT_TTL * 2;
        filetime::set_file_mtime(&path, filetime::FileTime::from_system_time(long_ago)).unwrap();
        assert!(file_age(&path).unwrap() > DEFAULT_TTL);
        filetime::set_file_mtime(&path, filetime::FileTime::now()).unwrap();
        assert!(file_age(&path).unwrap() < DEFAULT_TTL);
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

    // These take their input as an argument rather than setting the env vars,
    // because `cargo test` runs the whole suite in one process and a test that
    // mutates the environment races every test that reads it.
    #[test]
    fn ppm_is_public_recognizes_both_names() {
        assert!(ppm_is_public("https://p3m.dev"));
        assert!(ppm_is_public("https://p3m.dev/"));
        assert!(ppm_is_public("https://packagemanager.posit.co"));
        assert!(ppm_is_public("https://packagemanager.posit.co/"));
        assert!(ppm_is_public("http://P3M.dev"));
        assert!(!ppm_is_public("https://ppm.example.com"));
        assert!(!ppm_is_public("http://localhost:4242"));
        assert!(!ppm_is_public("https://p3m.dev.example.com"));
        assert!(!ppm_is_public("https://p3m.dev/cran"));
    }

    #[test]
    fn ppm_url_defaults_and_overrides() {
        assert_eq!(ppm_url_from(None), DEFAULT_PPM_URL);
        assert_eq!(ppm_url_from(Some("")), DEFAULT_PPM_URL);
        assert_eq!(ppm_url_from(Some("   ")), DEFAULT_PPM_URL);
        assert_eq!(
            ppm_url_from(Some("https://ppm.example.com")),
            "https://ppm.example.com"
        );
        // A trailing slash would otherwise double up in the composed URL.
        assert_eq!(
            ppm_url_from(Some("https://ppm.example.com/")),
            "https://ppm.example.com"
        );
    }

    #[test]
    fn status_cache_name_is_per_instance() {
        let public = format!("{}/__api__/status", DEFAULT_PPM_URL);
        assert_eq!(status_cache_name(&public), "p3m-status.json");
        // The old name of the public instance shares its cache entry.
        assert_eq!(
            status_cache_name("https://packagemanager.posit.co/__api__/status"),
            "p3m-status.json"
        );

        // Two instances must not share a cache entry, or one would be resolved
        // against the other's build targets.
        let a = status_cache_name("https://a.example.com/__api__/status");
        let b = status_cache_name("https://b.example.com/__api__/status");
        assert_ne!(a, b);
        assert_ne!(a, "p3m-status.json");
        for name in [&a, &b] {
            assert!(name.starts_with("p3m-status-"), "{}", name);
            assert!(name.ends_with(".json"), "{}", name);
        }
    }

    #[test]
    fn parses_the_whole_status_document() {
        let s = status();
        assert_eq!(s.version, "2026.06.0");
        assert_eq!(s.build_date, "2026-06-30T20:39:32Z");
        assert_eq!(s.cran_repo, "cran");
        assert!(s.r_configured);
        assert!(s.python_configured);
        assert!(s.binaries_enabled);
        assert!(!s.auth_enabled);
        assert_eq!(s.support_window.days_left, 513);
        assert_eq!(s.support_window.days_reminder, 90);

        assert_eq!(s.r_versions, ["4.6", "4.5", "4.4", "4.3", "4.2", "3.6"]);
        assert_eq!(s.bioc_versions[0].bioc_version, "3.24");
        assert_eq!(s.bioc_versions[0].r_version, "4.6");
        assert_eq!(s.bioc_versions[0].cran_snapshot, "latest");

        assert_eq!(s.macos_urls["default"].arm64, "sonoma-arm64");
        assert_eq!(s.macos_urls["default"].x86_64, "big-sur-x86_64");
        // R 4.0 has an x86_64 key with an empty value and no arm64 key at all.
        assert_eq!(s.macos_urls["4.0"].x86_64, "");
        assert_eq!(s.macos_urls["4.0"].arm64, "");

        assert_eq!(s.distros.len(), 36);
        let centos7 = s.distros.iter().find(|d| d.name == "centos7").unwrap();
        assert_eq!(centos7.binary_display, "CentOS/RHEL 7");
        assert_eq!(centos7.display, "CentOS 7");
        assert!(centos7.sys_reqs);
        assert!(centos7.official_rspm);
        assert!(!centos7.hidden);
        // Retired releases are reported, and marked.
        assert!(s.distros.iter().find(|d| d.name == "focal").unwrap().hidden);
        // macOS binaries are built somewhere else.
        let macos = s.distros.iter().find(|d| d.os == "macos").unwrap();
        assert_eq!(macos.build_distribution, "jammy");
    }

    /// `--json` must not silently drop what rig does not model, and a field
    /// whose type differs between instances must not fail the parse.
    #[test]
    fn status_json_keeps_unmodeled_fields() {
        let s = status();
        assert!(s.extra.contains_key("custom_home_title"));
        assert!(s.extra.contains_key("ga_id"));
        assert!(s.extra.contains_key("custom_home"));
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("Posit Public Package Manager"), "{}", json);
        assert!(json.contains("GTM-KHBDBW7"), "{}", json);
    }

    /// A status document cached by an older rig only has `distros`, with the
    /// camelCase spelling.
    #[test]
    fn legacy_cached_status_still_parses() {
        let s = PpmStatus::parse(br#"{"distros":[{"name":"jammy","os":"linux","binaryURL":"jammy","distribution":"ubuntu","release":"22.04","binaries":true,"arch":["x86_64"]}]}"#).unwrap();
        assert_eq!(s.distros.len(), 1);
        assert_eq!(s.distros[0].binary_url, "jammy");
        assert_eq!(s.distros[0].platform(), "jammy");
        assert!(s.version.is_empty());
        assert!(s.r_versions.is_empty());
        assert!(s.macos_urls.is_empty());
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

    /// The `<platform>-<arch>` spelling of lockfiles (and `--platform`) names
    /// the P3M target directly, with no distro version.
    #[test]
    fn matches_p3m_target_names() {
        assert_eq!(
            platform_of("x86_64", "linux", Some("noble"), None),
            expect("noble", "x86_64")
        );
        assert_eq!(
            platform_of("arm64", "linux", Some("noble"), None),
            expect("noble", "arm64")
        );
        assert_eq!(
            platform_of("x86_64", "linux", Some("opensuse156"), None),
            expect("opensuse156", "x86_64")
        );
        assert_eq!(
            platform_of("x86_64", "linux", Some(MANYLINUX), None),
            expect(MANYLINUX, "x86_64")
        );
        // jammy is x86_64 only, as above.
        assert_eq!(
            platform_of("arm64", "linux", Some("jammy"), None),
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
    /// actually use, and every platform of the indices must be produced.
    #[test]
    fn produced_platforms_exist_in_a_real_index() {
        let dplyr = index("dplyr", "dplyr.tsv.zst");
        let mut seen: Vec<&str> = dplyr
            .versions()
            .iter()
            .flat_map(|v| dplyr.platforms_for(v))
            .filter(|p| *p != "source")
            .collect();
        seen.sort();
        seen.dedup();
        let s = status();
        let mut produced: Vec<String> = s
            .distros
            .iter()
            .filter(|d| d.binaries)
            .flat_map(|d| d.arch.iter().map(|a| s.ppm_target_of(d, a)))
            .flatten()
            .map(|t| t.triple)
            .collect();
        produced.sort();
        produced.dedup();
        assert_eq!(produced, seen);
    }

    /// Every target P3M builds binaries for has a canonical platform string
    /// that parses back to the same target.
    #[test]
    fn target_triples_round_trip() {
        let s = status();
        for distro in s.distros.iter().filter(|d| d.binaries) {
            for arch in &distro.arch {
                let triple = target_triple(s.building_distro(distro), arch);
                let parsed = crate::platform::parse_platform_string(&triple).unwrap();
                let target = s.ppm_target(&parsed).unwrap();
                assert_eq!(
                    (target.platform.as_str(), target.arch.as_str()),
                    (distro.platform(), arch.as_str()),
                    "{} ({}) -> {}",
                    distro.name,
                    arch,
                    triple
                );
                assert_eq!(target.triple, triple);
            }
        }
    }

    #[test]
    fn targets_have_canonical_names() {
        let s = status();
        let triple = |p: &str| {
            let parsed = crate::platform::parse_platform_string(p).unwrap();
            s.ppm_target(&parsed).map(|t| t.triple)
        };
        assert_eq!(triple("macos-arm64").unwrap(), "aarch64-apple-darwin");
        assert_eq!(triple("windows-x86_64").unwrap(), "x86_64-w64-mingw32");
        assert_eq!(
            triple("jammy-x86_64").unwrap(),
            "x86_64-unknown-linux-gnu-ubuntu-22.04"
        );
        assert_eq!(
            triple("x86_64-unknown-linux-gnu-rocky-9.4").unwrap(),
            "x86_64-unknown-linux-gnu-rhel-9"
        );
        assert_eq!(
            triple("x86_64-unknown-linux-gnu-opensuse-156").unwrap(),
            "x86_64-unknown-linux-gnu-opensuse-15.6"
        );
        assert_eq!(
            triple("manylinux_2_28-arm64").unwrap(),
            "aarch64-unknown-linux-gnu-manylinux_2_28"
        );
        // No arm64 build for jammy, so the generic one.
        assert_eq!(
            triple("aarch64-unknown-linux-gnu-ubuntu-22.04").unwrap(),
            "aarch64-unknown-linux-gnu-manylinux_2_28"
        );
        assert_eq!(
            triple("x86_64-unknown-linux-gnu-fedora-42").unwrap(),
            "x86_64-unknown-linux-gnu-manylinux_2_28"
        );
        // Distros that use the binaries of another one have its name.
        assert_eq!(
            triple("x86_64-unknown-linux-gnu-centos-8").unwrap(),
            "x86_64-unknown-linux-gnu-rhel-8"
        );
        assert_eq!(
            triple("x86_64-unknown-linux-gnu-sles-15.6").unwrap(),
            "x86_64-unknown-linux-gnu-opensuse-15.6"
        );
        assert_eq!(
            triple("x86_64-unknown-linux-gnu-opensuse-15.0").unwrap(),
            "x86_64-unknown-linux-gnu-opensuse-15.0"
        );
    }
}
