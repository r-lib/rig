use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use flate2::read::GzDecoder;
use log::{debug, error, info, warn};
use rds2rust::RObject;
use rds2rust::RObject::*;
use rds2rust::VectorData;
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use xz2::read::XzDecoder;
use zstd::stream::read::Decoder as ZstdDecoder;

use crate::cache::get_cache_dir;
use crate::dcf::*;
use crate::download::{
    download_first_available_, fetch_optional_if_modified_, fetch_range_suffix_, ConditionalFetch,
    RangeFetch,
};
use crate::output::OUTPUT;
use crate::rds::*;
use crate::repos::feed::{CranlikeRepo, MetadataFeed, PkgRepo, RepoId};
use crate::solver::PackageVersionLoader;
use crate::utils::{calculate_hash, create_parent_dir_if_needed};

// `rig proj lock` now solves several `(R version, platform)` targets in
// parallel, each opening its own connection to the same on-disk cache
// database. Without a busy timeout, a connection that finds the database
// briefly locked by another thread's write (e.g. `ensure_db_schema`'s
// `CREATE TABLE IF NOT EXISTS`) would fail immediately with `SQLITE_BUSY`
// instead of waiting its turn.
fn open_db<P: AsRef<Path>>(path: P) -> Result<Connection, Box<dyn Error>> {
    if let Some(parent) = path.as_ref().parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;
    conn.busy_timeout(std::time::Duration::from_secs(30))?;
    Ok(conn)
}

pub(crate) fn package_type_to_path(
    pkg_type: &str,
    r_version: &str,
) -> Result<String, Box<dyn Error>> {
    use regex::Regex;

    if pkg_type == "source" {
        return Ok("src/contrib".to_string());
    }

    // Pattern: ^([[:lower:]]+)[.]binary(|[.]([[:alnum:]_-]+))$
    // In Rust regex: ^([a-z]+)\.binary(|\.([a-zA-Z0-9_-]+))$
    let re = Regex::new(r"^([a-z]+)\.binary(|\.([a-zA-Z0-9_-]+))$")?;

    if let Some(caps) = re.captures(pkg_type) {
        let os_raw = caps.get(1).map(|m| m.as_str()).unwrap_or("");

        // Switch "mac" -> "macosx", "win" -> "windows"
        let os = match os_raw {
            "mac" => "macosx",
            "win" => "windows",
            other => other,
        };

        // Check if there's a subtype (group 3)
        if let Some(subtype) = caps.get(3) {
            // bin/{os}/{subtype}/contrib/{ver}
            Ok(format!(
                "bin/{}/{}/contrib/{}",
                os,
                subtype.as_str(),
                r_version
            ))
        } else {
            // bin/{os}/contrib/{ver}
            Ok(format!("bin/{}/contrib/{}", os, r_version))
        }
    } else {
        OUTPUT.error(&format!("Invalid package type: {}", pkg_type));
        error!("Invalid package type {}", pkg_type);
        bail!("Invalid package type: {}", pkg_type);
    }
}

pub(crate) fn minor_r_version(r_version: &str) -> Result<String, Box<dyn Error>> {
    // If version has only 2 parts (e.g., "4.3"), append ".0" for semver parsing
    let version_str = if r_version.matches('.').count() == 1 {
        format!("{}.0", r_version)
    } else {
        r_version.to_string()
    };

    let version = match semver::Version::parse(&version_str) {
        Ok(v) => v,
        Err(e) => {
            // Some callers (e.g. the build cache) treat this as a routine
            // "not a numeric version" case and handle it quietly; the ones
            // that consider it a real failure report it themselves.
            debug!("Invalid R version format '{}': {}", r_version, e);
            bail!("Invalid R version format '{}': {}", r_version, e)
        }
    };
    Ok(format!("{}.{}", version.major, version.minor))
}

/// Candidate metadata URLs for a repo path, in preference order: `PACKAGES.gz`,
/// `PACKAGES.rds`, `PACKAGES`. The plain `PACKAGES` URL doubles as the cache-key
/// for the temporary download file.
pub(crate) fn cranlike_urls(repo_url: &str, path: &str) -> [String; 3] {
    [
        repo_url.to_string() + "/" + path + "/PACKAGES.gz",
        repo_url.to_string() + "/" + path + "/PACKAGES.rds",
        repo_url.to_string() + "/" + path + "/PACKAGES",
    ]
}

/// Downloads/refreshes the ALLPACKAGES and ARCHIVEDPACKAGES caches of `feed`
/// if stale.
fn ensure_feed_fresh(feed: &MetadataFeed) -> Result<(), Box<dyn Error>> {
    let url = feed.allpackages_url.as_str();
    ensure_packages_cached(
        &[url],
        url,
        url,
        "source",
        None,
        "ALLPACKAGES",
        Feed::Cranlike,
        &feed_display_name(feed),
    )?;
    ensure_archived_fresh(feed)?;
    Ok(())
}

fn ensure_archived_fresh(feed: &MetadataFeed) -> Result<(), Box<dyn Error>> {
    let url = feed.archived_url.as_str();
    ensure_packages_cached(
        &[url],
        url,
        url,
        "source",
        None,
        "ARCHIVEDPACKAGES",
        Feed::Archived,
        &feed_display_name(feed),
    )?;
    Ok(())
}

/// The name of `feed` in status messages.
fn feed_display_name(feed: &MetadataFeed) -> String {
    match &feed.repo {
        RepoId::Cran => "P3M".to_string(),
        RepoId::Bioc(v) => format!("Bioconductor {}", v),
        RepoId::Cranlike { name, .. } => name.clone(),
    }
}

lazy_static::lazy_static! {
    /// The Bioconductor feeds that failed to load in this process. They are
    /// not tried again, so that the solves of several targets do not each
    /// download (and warn about) the same missing feed.
    static ref FAILED_FEEDS: std::sync::Mutex<std::collections::HashSet<String>> =
        std::sync::Mutex::new(std::collections::HashSet::new());

    /// The repositories whose metadata update was already reported in this
    /// process, see [`announce_update`].
    static ref ANNOUNCED_REPOS: std::sync::Mutex<std::collections::HashSet<String>> =
        std::sync::Mutex::new(std::collections::HashSet::new());
}

/// Report that the metadata of repository `name` is being updated, once per
/// process. A repository has several metadata files (e.g. a source and a
/// binary index), and they should not each print a line.
fn announce_update(name: &str) {
    if ANNOUNCED_REPOS.lock().unwrap().insert(name.to_string()) {
        OUTPUT.status(&format!("Updating metadata of repository {}", name));
    }
}

/// Downloads/refreshes the caches of `feeds` if stale, and returns the feeds
/// that are usable. `rig proj lock` calls this once, sequentially, before
/// fanning solves for several targets out to threads, so those threads only
/// ever read the cache (via [`DbSourcePackageLoader::new_for`], which also
/// calls this but then finds nothing to download).
///
/// CRAN's feed must load. A Bioconductor feed that fails to load (e.g. a
/// release older than the metadata server has) is dropped with a warning, and
/// the solve goes on with CRAN only.
pub(crate) fn ensure_feeds_fresh(
    feeds: &[MetadataFeed],
) -> Result<Vec<MetadataFeed>, Box<dyn Error>> {
    let mut out = vec![];
    for feed in feeds {
        if !feed.repo.is_bioc() {
            ensure_feed_fresh(feed)?;
            out.push(feed.clone());
            continue;
        }
        if FAILED_FEEDS.lock().unwrap().contains(&feed.allpackages_url) {
            continue;
        }
        match ensure_feed_fresh(feed) {
            Ok(()) => out.push(feed.clone()),
            Err(e) => {
                warn!(
                    "Cannot load Bioconductor metadata from {}, using CRAN only: {}",
                    feed.allpackages_url, e
                );
                OUTPUT.warn(&format!(
                    "Cannot load Bioconductor {} metadata, using CRAN packages only.",
                    feed.repo
                ));
                FAILED_FEEDS
                    .lock()
                    .unwrap()
                    .insert(feed.allpackages_url.clone());
            }
        }
    }
    Ok(out)
}

/// The database key of the `PACKAGES` index at `path` (e.g. `src/contrib`)
/// in a CRAN-like repository: its full URL, which is unique per repository,
/// package type and R version.
pub(crate) fn cranlike_key(repo: &CranlikeRepo, path: &str) -> String {
    format!("{}/{}", repo.url, path)
}

/// Ensure the source index of each of `repos` is fresh in the database, and
/// return the repositories that are usable. A repository whose index cannot
/// be loaded is dropped with a warning.
fn ensure_cranlike_sources_fresh(repos: &[PkgRepo]) -> Vec<PkgRepo> {
    let mut out = vec![];
    for repo in repos {
        let PkgRepo::Cranlike(cranlike) = repo else {
            out.push(repo.clone());
            continue;
        };
        let key = cranlike_key(cranlike, "src/contrib");
        if FAILED_FEEDS.lock().unwrap().contains(&key) {
            continue;
        }
        match ensure_cranlike_index(cranlike, "src/contrib", "source", None, ".tar.gz") {
            Ok(()) => out.push(repo.clone()),
            Err(e) => {
                warn!("Cannot load package metadata from {}: {}", key, e);
                OUTPUT.warn(&format!(
                    "Cannot load package metadata from repository {}, skipping it.",
                    cranlike.name
                ));
                FAILED_FEEDS.lock().unwrap().insert(key);
            }
        }
    }
    out
}

/// Ensure the `PACKAGES` index at `path` of a CRAN-like repository is in the
/// database and fresh (24h), downloading it if needed.
///
/// `pkg_type` is `source` or a binary type, e.g. `mac.binary.big-sur-arm64`,
/// `r_version` the minor R version of a binary index, and `ext` the file
/// extension of the packages, used to build their download URLs.
///
/// A repository without this index (404 for both `PACKAGES.gz` and
/// `PACKAGES`) is stored as an empty index, so that a repository without
/// binaries is not asked again for a day. Unlike ALLPACKAGES, a `PACKAGES`
/// file is rewritten, not appended to, so it is always downloaded in full.
pub(crate) fn ensure_cranlike_index(
    repo: &CranlikeRepo,
    path: &str,
    pkg_type: &str,
    r_version: Option<&str>,
    ext: &str,
) -> Result<(), Box<dyn Error>> {
    let key = cranlike_key(repo, path);
    let db = metadata_db_file()?;
    ensure_db_schema(&db)?;
    if is_repo_cache_recent(&db, &key, pkg_type).unwrap_or(false) {
        debug!("Metadata of {} is up to date (cached)", key);
        return Ok(());
    }

    announce_update(&repo.name);
    let etag = get_repo_etag(&db, &key, pkg_type).ok();
    for file in ["PACKAGES.gz", "PACKAGES"] {
        let url = format!("{}/{}", key, file);
        match fetch_optional_if_modified_(&url, etag.as_deref(), None)? {
            ConditionalFetch::NotFound => continue,
            ConditionalFetch::NotModified => {
                touch_repo_last_updated(&db, &key, pkg_type)?;
                return Ok(());
            }
            ConditionalFetch::Fetched { bytes, etag } => {
                let data = decompress_gzip_maybe(bytes)?;
                let packages = parse_cranlike_packages(&data, repo, path, ext)?;
                info!("Parsed {} packages from {}", packages.len(), url);
                save_packages_to_db(
                    &packages,
                    &db,
                    &key,
                    r_version,
                    pkg_type,
                    path,
                    etag.as_deref(),
                    true,
                    None,
                )?;
                return Ok(());
            }
        }
    }

    debug!("No PACKAGES file at {}", key);
    save_packages_to_db(
        &vec![],
        &db,
        &key,
        r_version,
        pkg_type,
        path,
        None,
        true,
        None,
    )?;
    Ok(())
}

fn decompress_gzip_maybe(bytes: Vec<u8>) -> Result<Vec<u8>, Box<dyn Error>> {
    if bytes.len() >= 2 && bytes[0..2] == [0x1f, 0x8b] {
        let mut data = Vec::new();
        GzDecoder::new(bytes.as_slice()).read_to_end(&mut data)?;
        Ok(data)
    } else {
        Ok(bytes)
    }
}

/// Parse the `PACKAGES` file of a CRAN-like repository, at `path` in `repo`.
///
/// The packages get a download URL, unless the index has one (`DownloadURL`):
/// `File` or `<package>_<version><ext>` in the directory of the index, or in
/// its `Path` subdirectory. Their `sha256sum` is the `SHA256` checksum of the
/// file, or else its `MD5sum`: there is no upstream CRAN tarball here, so the
/// file itself is the identity of the package. If it changes, the package is
/// reinstalled, even if its version is the same. A package without either
/// checksum has no hash, and an installed package of the same version counts
/// as up to date.
fn parse_cranlike_packages(
    data: &[u8],
    repo: &CranlikeRepo,
    path: &str,
    ext: &str,
) -> Result<Vec<Package>, Box<dyn Error>> {
    let paragraphs = parse_dcf_reader(data)?;
    let mut packages = vec![];
    for para in paragraphs.iter() {
        let mut pkg = match Package::from_dcf_paragraph(para) {
            Ok(pkg) => pkg,
            Err(e) => {
                debug!(
                    "Skipping unparseable package metadata in {}: {}",
                    repo.url, e
                );
                continue;
            }
        };
        if pkg.download_url.is_none() {
            let file = pkg
                .file
                .clone()
                .unwrap_or_else(|| format!("{}_{}{}", pkg.name, pkg.version.original, ext));
            let dir = match &pkg.path {
                Some(sub) => format!("{}/{}/{}", repo.url, path, sub.trim_matches('/')),
                None => format!("{}/{}", repo.url, path),
            };
            pkg.download_url = Some(format!("{}/{}", dir, file));
        }
        if pkg.sha256sum.is_none() {
            pkg.sha256sum = para
                .get("SHA256")
                .or_else(|| para.get("MD5sum"))
                .map(|s| s.trim().to_string());
        }
        packages.push(pkg);
    }
    Ok(packages)
}

/// One package version in the `PACKAGES` index of a CRAN-like repository.
#[derive(Debug, Clone)]
pub(crate) struct CranlikeRow {
    pub version: RPackageVersion,
    pub download_url: Option<String>,
    pub checksum: Option<String>,
    /// The timestamp of the `Built` field, e.g. `2025-06-20 10:00:00 UTC`.
    pub built: Option<String>,
}

/// The timestamp of a `Built` field, as stored in the database, as JSON.
fn built_timestamp(json: Option<&str>) -> Option<String> {
    let built: DCFBuilt = serde_json::from_str(json?).ok()?;
    Some(built.timestamp).filter(|t| !t.is_empty())
}

/// The versions of `package` in the index with database key `key` (see
/// [`cranlike_key`]), from the cache, without refreshing it.
pub(crate) fn cranlike_index_rows(
    conn: &Connection,
    key: &str,
    pkg_type: &str,
    package: &str,
) -> Result<Vec<CranlikeRow>, Box<dyn Error>> {
    let repo_ids = source_repo_ids(conn, key, pkg_type)?;
    let mut stmt = conn.prepare_cached(
        "SELECT version, download_url, sha256sum, repo_id, built FROM packages WHERE name = ?1",
    )?;
    let rows = stmt.query_map(params![package], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, Option<String>>(4)?,
        ))
    })?;
    let mut out = vec![];
    for row in rows {
        let (ver, download_url, checksum, repo_id, built) = row?;
        if !repo_ids.contains(&repo_id) {
            continue;
        }
        out.push(CranlikeRow {
            version: RPackageVersion::from_str(&ver)?,
            download_url,
            checksum,
            built: built_timestamp(built.as_deref()),
        });
    }
    Ok(out)
}

/// The database key and package type of the source index of a repository,
/// as stored by [`ensure_feeds_fresh`] or [`ensure_cranlike_index`].
fn source_index_key(repo: &PkgRepo) -> String {
    match repo {
        PkgRepo::Extended(feed) => feed.allpackages_url.clone(),
        PkgRepo::Cranlike(cranlike) => cranlike_key(cranlike, "src/contrib"),
    }
}

/// A [`PackageVersionLoader`] backed by the shared SQLite database. It queries a
/// single package's versions on demand from the ALLPACKAGES histories of its
/// feeds (CRAN, and possibly a Bioconductor release), so the solver only
/// materializes the packages it actually visits instead of the whole version
/// history.
///
/// ALLPACKAGES already lists every version of every package ever published on
/// CRAN, including the current ones, so the current `PACKAGES` file of a CRAN
/// mirror is not consulted: it would only add versions published in the window
/// between the last ALLPACKAGES rebuild and now.
pub struct DbSourcePackageLoader {
    conn: Connection,
    /// repo ids of the indices to search, with the position of the
    /// repository they belong to in [`DbSourcePackageLoader::repos`].
    repo_ids: Vec<(i64, usize)>,
    /// The repositories to search, in order: if several of them have the same
    /// version of a package, the first one wins.
    repos: Vec<RepoId>,
    /// `--exclude-newer` cutoff day, `YYYY-MM-DD`: versions whose snapshot
    /// date is after it are hidden from the solver.
    exclude_newer: Option<String>,
    /// Packages whose CRAN versions are hidden, because they must come from
    /// Bioconductor.
    bioc_only: std::collections::BTreeSet<String>,
}

impl DbSourcePackageLoader {
    /// A loader for `feeds`. Ensure their metadata is fresh in the database,
    /// then open a connection ready to serve per-package queries. See
    /// [`ensure_feeds_fresh`] for the feeds that fail to load.
    pub fn new_for(feeds: &[MetadataFeed]) -> Result<Self, Box<dyn Error>> {
        DbSourcePackageLoader::new_for_repos(&PkgRepo::from_feeds(feeds.to_vec()))
    }

    /// A loader for `repos`, extended feeds and CRAN-like repositories, in
    /// order of precedence. A repository that fails to load is not searched,
    /// except for CRAN's feed, which must load.
    pub fn new_for_repos(repos: &[PkgRepo]) -> Result<Self, Box<dyn Error>> {
        let repos = ensure_repos_fresh(repos)?;
        let conn = open_metadata_db()?;
        let mut repo_ids = vec![];
        for (idx, repo) in repos.iter().enumerate() {
            for id in source_repo_ids(&conn, &source_index_key(repo), "source")? {
                repo_ids.push((id, idx));
            }
        }
        Ok(DbSourcePackageLoader {
            conn,
            repo_ids,
            repos: repos.iter().map(|r| r.repo_id()).collect(),
            exclude_newer: None,
            bioc_only: Default::default(),
        })
    }

    /// A loader for the indices with database keys `keys`, in order of
    /// precedence, on an open connection, without refreshing anything.
    #[cfg(test)]
    fn from_conn(conn: Connection, keys: &[(&str, RepoId)]) -> Self {
        let mut repo_ids = vec![];
        for (idx, (key, _)) in keys.iter().enumerate() {
            for id in source_repo_ids(&conn, key, "source").unwrap() {
                repo_ids.push((id, idx));
            }
        }
        DbSourcePackageLoader {
            conn,
            repo_ids,
            repos: keys.iter().map(|(_, repo)| repo.clone()).collect(),
            exclude_newer: None,
            bioc_only: Default::default(),
        }
    }

    /// Hide the CRAN versions of `packages`, see [`BiocSetting::only`].
    ///
    /// [`BiocSetting::only`]: crate::repos::feed::BiocSetting::only
    pub fn with_bioc_only(mut self, packages: std::collections::BTreeSet<String>) -> Self {
        self.bioc_only = packages;
        self
    }

    /// Hide the versions published after `cutoff` (`YYYY-MM-DD`), see
    /// [`crate::exclude_newer`]. `None` keeps every version.
    pub fn with_exclude_newer(mut self, cutoff: Option<String>) -> Self {
        self.exclude_newer = cutoff;
        self
    }

    /// The repositories this loader searches, in order.
    pub fn repositories(&self) -> Vec<RepoId> {
        self.repos.clone()
    }

    /// The position and the repository of a database repo id.
    fn repo_of(&self, repo_id: i64) -> Option<(usize, &RepoId)> {
        self.repo_ids
            .iter()
            .find(|(id, _)| *id == repo_id)
            .map(|(_, idx)| (*idx, &self.repos[*idx]))
    }
}

/// Ensure the metadata of `repos` is fresh, and return the usable ones, in
/// order, see [`ensure_feeds_fresh`] and [`ensure_cranlike_sources_fresh`].
pub(crate) fn ensure_repos_fresh(repos: &[PkgRepo]) -> Result<Vec<PkgRepo>, Box<dyn Error>> {
    let usable_feeds = ensure_feeds_fresh(&PkgRepo::feeds(repos))?;
    let repos: Vec<PkgRepo> = repos
        .iter()
        .filter(|r| match r {
            PkgRepo::Extended(feed) => usable_feeds.contains(feed),
            PkgRepo::Cranlike(_) => true,
        })
        .cloned()
        .collect();
    Ok(ensure_cranlike_sources_fresh(&repos))
}

/// The shared metadata database, the same file every feed is cached in.
pub(crate) fn open_metadata_db() -> Result<Connection, Box<dyn Error>> {
    open_db(metadata_db_file()?)
}

fn metadata_db_file() -> Result<PathBuf, Box<dyn Error>> {
    let repo_local = repo_local_file(&MetadataFeed::cran().allpackages_url)?;
    repo_db_file(&repo_local)
}

/// The P3M snapshot date, `YYYY-MM-DD`, in an ALLPACKAGES `DownloadURL`, e.g.
/// `2026-06-08` for
/// `https://p3m.dev/cran/2026-06-08/src/contrib/pak_0.10.0.tar.gz`. This is
/// the date the version was first published in a snapshot.
pub fn snapshot_date(url: &str) -> Option<&str> {
    lazy_static::lazy_static! {
        static ref SNAPSHOT: regex::Regex = regex::Regex::new(r"/(\d{4}-\d{2}-\d{2})/").unwrap();
    }
    Some(SNAPSHOT.captures(url)?.get(1)?.as_str())
}

/// Whether a version with `download_url` is published on or before `cutoff`
/// (`YYYY-MM-DD`). Versions without a snapshot date are kept.
fn published_by(download_url: Option<&str>, cutoff: Option<&str>) -> bool {
    let (Some(cutoff), Some(date)) = (cutoff, download_url.and_then(snapshot_date)) else {
        return true;
    };
    date <= cutoff
}

/// Resolve the repo id(s) for a given `(url, pkg_type)` in the shared database.
fn source_repo_ids(
    conn: &Connection,
    url: &str,
    pkg_type: &str,
) -> Result<Vec<i64>, Box<dyn Error>> {
    let url = url.trim_end_matches('/');
    let mut stmt = conn.prepare("SELECT id FROM repos WHERE url = ?1 AND pkg_type = ?2")?;
    let ids = stmt
        .query_map(params![url, pkg_type], |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}

impl PackageVersionLoader for DbSourcePackageLoader {
    fn load_versions(&self, package: &str) -> Result<Vec<Package>, Box<dyn Error>> {
        // Query by name only: this uses the `(name, ...)` index and touches just
        // the handful of rows for this package, whereas adding `repo_id = ?`
        // makes SQLite pick the repo_id index and scan the whole (200k-row)
        // ALLPACKAGES repo. We filter to our repos and dedup by version here.
        // `sha256sum` comes along for the ride: it is the identity of the
        // upstream CRAN tarball, which `rig pkg install` records in the
        // installed package as `RemoteHash`. It is the only source of that hash
        // on a source-only solve, where no binary index is loaded at all.
        // `system_requirements` too: the lockfile records it, so that
        // `rig proj sync` can install the OS packages a Linux install needs.
        //
        // A package may be in several repositories: the solver sees the
        // versions of all of them, and if the same version is in several, the
        // row of the first repository wins.
        struct Row {
            deps_json: String,
            sha256sum: Option<String>,
            download_url: Option<String>,
            system_requirements: Option<String>,
            // Only for a CRAN-like repository, to reinstall a newer build.
            built: Option<String>,
            repo: RepoId,
            rank: usize,
        }
        let mut best: HashMap<String, Row> = HashMap::new();
        // The `SystemRequirements` of each version, from any repository. A
        // CRAN-like `PACKAGES` file, e.g. CRAN's own, usually does not have
        // them, so a version from there takes them from P3M's metadata.
        let mut sysreqs_any: HashMap<String, String> = HashMap::new();
        let mut stmt = self.conn.prepare_cached(
            "SELECT version, dependencies, sha256sum, repo_id, download_url, \
             system_requirements, built FROM packages WHERE name = ?1",
        )?;
        let rows = stmt.query_map(params![package], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })?;
        for row in rows {
            let (ver, deps_json, sha256sum, repo_id, download_url, sysreqs, built) = row?;
            let Some((rank, repo)) = self.repo_of(repo_id) else {
                continue; // row from a repo we do not source from
            };
            if !repo.is_bioc() && self.bioc_only.contains(package) {
                continue; // must come from Bioconductor
            }
            if !published_by(download_url.as_deref(), self.exclude_newer.as_deref()) {
                continue; // published after the --exclude-newer cutoff
            }
            if let Some(s) = &sysreqs {
                sysreqs_any.entry(ver.clone()).or_insert_with(|| s.clone());
            }
            if best.get(&ver).is_some_and(|b| b.rank <= rank) {
                continue;
            }
            best.insert(
                ver,
                Row {
                    deps_json,
                    sha256sum,
                    download_url,
                    system_requirements: sysreqs,
                    built: built.filter(|_| repo.is_cranlike()),
                    repo: repo.clone(),
                    rank,
                },
            );
        }

        let mut out: Vec<Package> = Vec::with_capacity(best.len());
        for (ver, row) in best {
            let version = RPackageVersion::from_str(&ver)?;
            let deps: PackageDependencies = serde_json::from_str(&row.deps_json)?;
            let mut pkg = Package::from_crandb(package.to_string(), version, deps.dependencies);
            pkg.sha256sum = row.sha256sum;
            pkg.download_url = row.download_url;
            pkg.built = row
                .built
                .and_then(|b| serde_json::from_str::<DCFBuilt>(&b).ok());
            pkg.system_requirements = row
                .system_requirements
                .or_else(|| sysreqs_any.get(&ver).cloned());
            pkg.repository = Some(row.repo);
            out.push(pkg);
        }
        Ok(out)
    }
}

/// The names of the packages in `feed`'s ALLPACKAGES history, from the
/// cache, without refreshing it.
pub fn feed_package_names(
    feed: &MetadataFeed,
) -> Result<std::collections::HashSet<String>, Box<dyn Error>> {
    let conn = open_metadata_db()?;
    let repo_ids = source_repo_ids(&conn, &feed.allpackages_url, "source")?;
    let mut out = std::collections::HashSet::new();
    let mut stmt = conn.prepare_cached("SELECT DISTINCT name FROM packages WHERE repo_id = ?1")?;
    for id in repo_ids {
        for name in stmt.query_map(params![id], |row| row.get::<_, String>(0))? {
            out.insert(name?);
        }
    }
    Ok(out)
}

/// One version of a package in the ALLPACKAGES history, with the fields that
/// identify the original CRAN tarball it was built from.
#[derive(Debug, Clone)]
pub struct AllPackagesVersion {
    pub version: RPackageVersion,
    /// P3M snapshot URL of the source tarball, e.g.
    /// `https://p3m.dev/cran/2026-06-08/src/contrib/pak_0.10.0.tar.gz`.
    pub download_url: Option<String>,
    /// sha256 of the original CRAN tarball (ALLPACKAGES' `SHA256Original`).
    pub sha256sum: Option<String>,
}

impl AllPackagesVersion {
    /// The P3M snapshot date the version was published in, as `YYYY-MM-DD`,
    /// taken from the date component of [`Self::download_url`].
    pub fn snapshot(&self) -> Option<String> {
        snapshot_date(self.download_url.as_deref()?).map(|s| s.to_string())
    }
}

/// Every version of `package` in the ALLPACKAGES history, refreshing the
/// metadata first if the cache is stale.
pub fn allpackages_versions(package: &str) -> Result<Vec<AllPackagesVersion>, Box<dyn Error>> {
    let feed = MetadataFeed::cran();
    ensure_feed_fresh(&feed)?;

    let conn = open_metadata_db()?;
    let repo_ids = source_repo_ids(&conn, &feed.allpackages_url, "source")?;

    // Query by name only, for the same reason as `load_versions()` above: it
    // keeps SQLite on the `(name, ...)` index instead of scanning the whole
    // ALLPACKAGES repo.
    let mut stmt = conn.prepare(
        "SELECT version, download_url, sha256sum, repo_id FROM packages WHERE name = ?1",
    )?;
    let rows = stmt.query_map(params![package], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;

    let mut out: Vec<AllPackagesVersion> = vec![];
    for row in rows {
        let (ver, download_url, sha256sum, repo_id) = row?;
        if !repo_ids.contains(&repo_id) {
            continue; // row from a repo we do not source from
        }
        out.push(AllPackagesVersion {
            version: RPackageVersion::from_str(&ver)?,
            download_url,
            sha256sum,
        });
    }

    Ok(out)
}

/// Every package of `repos`, at its latest version, from the shared metadata
/// database, refreshing the metadata first if the cache is stale.
///
/// ALLPACKAGES keeps the full history of every version ever published,
/// archived or not, so packages a feed has archived are omitted by cross
/// referencing its ARCHIVEDPACKAGES, unless `include_archived` is set. A
/// package archived in one feed is still listed if it is alive in another
/// repository. The `PACKAGES` index of a CRAN-like repository only lists its
/// current packages. A package in several repositories is listed at its
/// highest version, and on a tie with the version of the first repository.
pub fn all_available_packages(
    repos: &[PkgRepo],
    include_archived: bool,
) -> Result<Vec<Package>, Box<dyn Error>> {
    let repos = ensure_repos_fresh(repos)?;
    let conn = open_metadata_db()?;

    let mut best: HashMap<String, (RPackageVersion, String, RepoId)> = HashMap::new();
    for repo in &repos {
        let feed_best = feed_latest_packages(&conn, &source_index_key(repo))?;
        let archived = match repo {
            PkgRepo::Extended(feed) if !include_archived => {
                feed_archived_names(&conn, &feed.archived_url)?
            }
            _ => std::collections::HashSet::new(),
        };
        for (name, (version, deps_json)) in feed_best {
            if archived.contains(&name) {
                continue;
            }
            let better = match best.get(&name) {
                None => true,
                Some((v, _, _)) => version > *v,
            };
            if better {
                best.insert(name, (version, deps_json, repo.repo_id()));
            }
        }
    }

    let mut out = Vec::with_capacity(best.len());
    for (name, (version, deps_json, repo)) in best {
        let deps: PackageDependencies = serde_json::from_str(&deps_json)?;
        let mut pkg = Package::from_crandb(name, version, deps.dependencies);
        pkg.repository = Some(repo);
        out.push(pkg);
    }
    Ok(out)
}

/// `name -> (latest version, dependencies JSON)` of the ALLPACKAGES feed at
/// `url`.
fn feed_latest_packages(
    conn: &Connection,
    url: &str,
) -> Result<HashMap<String, (RPackageVersion, String)>, Box<dyn Error>> {
    let repo_ids = source_repo_ids(conn, url, "source")?;
    let mut best: HashMap<String, (RPackageVersion, String)> = HashMap::new();
    let placeholders = repo_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT name, version, dependencies FROM packages WHERE repo_id IN ({})",
        placeholders
    );
    let mut stmt = conn.prepare(&sql)?;
    let sql_params: Vec<&dyn rusqlite::ToSql> = repo_ids
        .iter()
        .map(|id| id as &dyn rusqlite::ToSql)
        .collect();
    let rows = stmt.query_map(sql_params.as_slice(), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (name, ver, deps_json) = row?;
        let version = RPackageVersion::from_str(&ver)?;
        match best.get(&name) {
            Some((best_version, _)) if *best_version >= version => {}
            _ => {
                best.insert(name, (version, deps_json));
            }
        }
    }
    Ok(best)
}

/// The names of the packages in the ARCHIVEDPACKAGES feed at `url`.
fn feed_archived_names(
    conn: &Connection,
    url: &str,
) -> Result<std::collections::HashSet<String>, Box<dyn Error>> {
    let archived_repo_ids = source_repo_ids(conn, url, "source")?;
    let placeholders = archived_repo_ids
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT DISTINCT name FROM archived_packages WHERE repo_id IN ({})",
        placeholders
    );
    let mut stmt = conn.prepare(&sql)?;
    let sql_params: Vec<&dyn rusqlite::ToSql> = archived_repo_ids
        .iter()
        .map(|id| id as &dyn rusqlite::ToSql)
        .collect();
    let rows = stmt.query_map(sql_params.as_slice(), |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

#[derive(Debug, Clone)]
pub struct ArchivedPackage {
    /// The date CRAN archived the package, as `YYYY-MM-DD`.
    pub archived: String,
}

/// Whether CRAN has archived `package`, and if so when.
pub fn archived_package(package: &str) -> Result<Option<ArchivedPackage>, Box<dyn Error>> {
    let feed = MetadataFeed::cran();
    ensure_archived_fresh(&feed)?;

    let repo_local = repo_local_file(&feed.archived_url)?;
    let repo_db = repo_db_file(&repo_local)?;
    archived_package_in_db(&repo_db, &feed.archived_url, package)
}

/// The `archived_packages` row of `package` for the feed at `feed_url`, without
/// refreshing anything.
fn archived_package_in_db(
    db_path: &PathBuf,
    feed_url: &str,
    package: &str,
) -> Result<Option<ArchivedPackage>, Box<dyn Error>> {
    let conn = open_db(db_path)?;
    let repo_ids = source_repo_ids(&conn, feed_url, "source")?;

    let mut stmt =
        conn.prepare("SELECT archived, repo_id FROM archived_packages WHERE name = ?1")?;
    let rows = stmt.query_map(params![package], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;

    for row in rows {
        let (archived, repo_id) = row?;
        if !repo_ids.contains(&repo_id) {
            continue; // row from a feed we do not use
        }
        return Ok(Some(ArchivedPackage { archived }));
    }

    Ok(None)
}

/// Which metadata feed is being cached, i.e. how a freshly downloaded file is
/// stored and which table holds its rows.
#[derive(Clone, Copy, PartialEq)]
enum Feed {
    /// A cranlike `PACKAGES` / `ALLPACKAGES` file, stored in the `packages`
    /// table.
    Cranlike,
    /// The `ARCHIVEDPACKAGES` file, stored in the `archived_packages` table.
    Archived,
}

/// Outcome of ensuring a cranlike metadata file is present and fresh in the DB.
enum CacheState {
    /// The metadata was (re)downloaded, parsed and stored.
    FreshlyParsed,
    /// The database already holds a fresh copy; nothing was parsed.
    Cached,
}

/// Number of trailing bytes of previously-parsed text kept as a fingerprint,
/// to detect the origin rewriting a feed rather than only appending to it.
const TAIL_WINDOW: usize = 512;

fn tail_window_len(parsed_len: i64) -> usize {
    (parsed_len.max(0) as usize).min(TAIL_WINDOW)
}

/// The last [`TAIL_WINDOW`] bytes of `data` (or all of it, if shorter).
fn tail_slice(data: &[u8]) -> &[u8] {
    &data[data.len().saturating_sub(TAIL_WINDOW)..]
}

fn hash_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

/// How much of a feed has already been parsed and stored, if any:
/// `(parsed_len, tail_hash)`, as recorded by the last successful parse.
fn get_repo_progress(db_path: &PathBuf, repo_url: &str, pkg_type: &str) -> Option<(i64, String)> {
    let conn = open_db(db_path).ok()?;
    let repo_url = repo_url.trim_end_matches('/');
    conn.query_row(
        "SELECT parsed_len, tail_hash FROM repos
         WHERE url = ?1 AND pkg_type = ?2 AND parsed_len IS NOT NULL AND tail_hash IS NOT NULL",
        params![repo_url, pkg_type],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
    )
    .ok()
}

fn touch_repo_last_updated(
    db_path: &PathBuf,
    repo_url: &str,
    pkg_type: &str,
) -> Result<(), Box<dyn Error>> {
    let conn = open_db(db_path)?;
    let repo_url = repo_url.trim_end_matches('/');
    conn.execute(
        "UPDATE repos SET last_updated = CURRENT_TIMESTAMP WHERE url = ?1 AND pkg_type = ?2",
        params![repo_url, pkg_type],
    )?;
    Ok(())
}

/// Parse `packages` and store them, dispatching to the feed's table.
#[allow(clippy::too_many_arguments)]
fn store_packages(
    packages: &Vec<Package>,
    repo_db: &PathBuf,
    repo_url_key: &str,
    r_version: Option<&str>,
    pkg_type: &str,
    path: &str,
    feed: Feed,
    etag: Option<&str>,
    replace: bool,
    progress: Option<(i64, &str)>,
) -> Result<(), Box<dyn Error>> {
    match feed {
        Feed::Cranlike => save_packages_to_db(
            packages,
            repo_db,
            repo_url_key,
            r_version,
            pkg_type,
            path,
            etag,
            replace,
            progress,
        ),
        Feed::Archived => save_archived_to_db(
            packages,
            repo_db,
            repo_url_key,
            pkg_type,
            path,
            etag,
            replace,
            progress,
        ),
    }
}

/// Outcome of a trailing-only refresh attempt. See [`try_trailing_refresh`].
enum TrailingOutcome {
    /// The feed has not grown since `parsed_len`; nothing to parse.
    Unchanged,
    /// The tail overlap matched: the feed only grew by appending. Holds the
    /// overlap plus the genuinely new bytes, exactly as fetched.
    Appended(Vec<u8>),
    /// The server returned the whole plain-text body (it ignored `Range`).
    Full(Vec<u8>),
}

/// Try to refresh a feed by fetching only what was appended after
/// `parsed_len`, verifying the origin didn't rewrite anything at or before
/// that point (see the `tail_hash` fingerprint in the plan/schema).
///
/// Returns `Err` on any transport failure, or when the tail fingerprint does
/// not match — i.e. whenever the caller should fall back to a full download.
fn try_trailing_refresh(
    plain_url: &str,
    parsed_len: i64,
    tail_hash: &str,
) -> Result<TrailingOutcome, Box<dyn Error>> {
    let window_len = tail_window_len(parsed_len);
    let from = parsed_len as u64 - window_len as u64;

    match fetch_range_suffix_(plain_url, from, None)? {
        RangeFetch::OutOfRange => Ok(TrailingOutcome::Unchanged),
        RangeFetch::Full(bytes) => Ok(TrailingOutcome::Full(bytes)),
        RangeFetch::Partial(data) => {
            if data.len() < window_len || hash_bytes(&data[..window_len]) != tail_hash {
                bail!(
                    "Metadata at {} was rewritten, not just appended to",
                    plain_url
                );
            }
            Ok(TrailingOutcome::Appended(data))
        }
    }
}

/// The full-download path: (re)download `candidate_urls[0]` (or a fallback),
/// parse it, and store it, replacing whatever was there before.
///
/// `use_etag`: whether to send the repo's stored etag (a 304 then means
/// "already cached, nothing to do"). Set to `false` to force a full response
/// when recovering from a database that lost its rows despite a fresh-looking
/// cached download — in that case a failed download is a hard error, not a
/// cache hit.
#[allow(clippy::too_many_arguments)]
fn force_full_download(
    candidate_urls: &[&str],
    repo_local: &PathBuf,
    repo_db: &PathBuf,
    repo_url_key: &str,
    r_version: Option<&str>,
    pkg_type: &str,
    path: &str,
    feed: Feed,
    use_etag: bool,
) -> Result<CacheState, Box<dyn Error>> {
    create_parent_dir_if_needed(repo_local)?;
    info!(
        "Checking for repo metadata updates from {}",
        candidate_urls[0]
    );

    let existing_etag = if use_etag {
        get_repo_etag(repo_db, repo_url_key, pkg_type).ok()
    } else {
        // Drop the stale download file so it is not treated as cached.
        let _ = std::fs::remove_file(repo_local);
        None
    };

    let (dl_status, new_etag) = download_first_available_(
        candidate_urls,
        repo_local,
        None,
        None,
        existing_etag.as_deref(),
    )?;

    if !dl_status {
        if use_etag {
            return Ok(CacheState::Cached);
        }
        OUTPUT.error("Failed to load package metadata, database is corrupt?");
        error!(
            "Failed to recover package metadata from {}",
            candidate_urls[0]
        );
        bail!(
            "Failed to refresh package metadata from {}",
            candidate_urls[0]
        );
    }

    parse_store_and_cleanup(
        repo_local,
        repo_db,
        repo_url_key,
        r_version,
        pkg_type,
        path,
        new_etag.as_deref(),
        feed,
    )?;
    Ok(CacheState::FreshlyParsed)
}

/// Ensure a cranlike metadata file is present and fresh in the SQLite database,
/// downloading and parsing it if the 24h / etag cache is stale. Does **not**
/// load the stored rows back into memory when the cache is already fresh, so
/// callers that query the database lazily avoid materializing everything.
///
/// For feeds that only ever grow by appending (ALLPACKAGES, ARCHIVEDPACKAGES),
/// a stale cache first tries a trailing-only refresh — fetching just the bytes
/// appended since the last parse from the plain-text mirror of `candidate_urls
/// [0]` (its `.zst` counterpart can't be range-fetched: zstd is a single
/// compressed frame) — before falling back to a full re-download.
#[allow(clippy::too_many_arguments)]
fn ensure_packages_cached(
    candidate_urls: &[&str],
    cache_key: &str,
    repo_url_key: &str,
    pkg_type: &str,
    r_version: Option<&str>,
    path: &str,
    feed: Feed,
    display_name: &str,
) -> Result<CacheState, Box<dyn Error>> {
    // Use a temporary file for downloads (will be deleted after parsing)
    let repo_local = repo_local_file(cache_key)?;
    let repo_db = repo_db_file(&repo_local)?;

    // Ensure database schema exists early
    ensure_db_schema(&repo_db)?;

    // Check if we have recent data in the database
    let should_download = match is_repo_cache_recent(&repo_db, repo_url_key, pkg_type) {
        Ok(is_recent) => {
            if is_recent {
                info!("Database cache is recent, skipping download");
            }
            !is_recent
        }
        Err(_) => {
            // No database entry, need to download
            info!("No database cache found, will download");
            true
        }
    };

    if !should_download {
        info!("Repo metadata is up to date (cached)");
        // The database should hold the rows. It may not if a previous run
        // downloaded the metadata but was interrupted (or aborted) before
        // storing it: the cached download file then looks fresh while the
        // database is empty. Recover by forcing a fresh download rather than
        // dead-ending on a "database is corrupt" error.
        // A recorded parse progress also counts: it is written in the same
        // transaction as the rows, and a feed may have no rows at all (e.g.
        // the ARCHIVEDPACKAGES of an old Bioconductor release).
        if repo_has_packages(&repo_db, repo_url_key, pkg_type, feed)?
            || get_repo_progress(&repo_db, repo_url_key, pkg_type).is_some()
        {
            return Ok(CacheState::Cached);
        }
        info!("Cached metadata missing from database, forcing a fresh download");
        announce_update(display_name);
        return force_full_download(
            candidate_urls,
            &repo_local,
            &repo_db,
            repo_url_key,
            r_version,
            pkg_type,
            path,
            feed,
            false,
        );
    }

    announce_update(display_name);
    if let Some((parsed_len, tail_hash)) = get_repo_progress(&repo_db, repo_url_key, pkg_type) {
        let plain_url = candidate_urls[0].trim_end_matches(".zst");
        match try_trailing_refresh(plain_url, parsed_len, &tail_hash) {
            Ok(TrailingOutcome::Unchanged) => {
                touch_repo_last_updated(&repo_db, repo_url_key, pkg_type)?;
                return Ok(CacheState::Cached);
            }
            Ok(TrailingOutcome::Appended(data)) => {
                let window_len = tail_window_len(parsed_len);
                let new_suffix = &data[window_len..];
                let packages = parse_dcf_bytes(new_suffix)?;
                let new_parsed_len = parsed_len + new_suffix.len() as i64;
                let new_tail_hash = hash_bytes(tail_slice(&data));
                info!(
                    "Appended {} new package record(s) from {}",
                    packages.len(),
                    plain_url
                );
                store_packages(
                    &packages,
                    &repo_db,
                    repo_url_key,
                    r_version,
                    pkg_type,
                    path,
                    feed,
                    None,
                    false,
                    Some((new_parsed_len, &new_tail_hash)),
                )?;
                return Ok(CacheState::FreshlyParsed);
            }
            Ok(TrailingOutcome::Full(data)) => {
                let packages = parse_dcf_bytes(&data)?;
                let new_tail_hash = hash_bytes(tail_slice(&data));
                store_packages(
                    &packages,
                    &repo_db,
                    repo_url_key,
                    r_version,
                    pkg_type,
                    path,
                    feed,
                    None,
                    true,
                    Some((data.len() as i64, &new_tail_hash)),
                )?;
                return Ok(CacheState::FreshlyParsed);
            }
            Err(e) => {
                warn!(
                    "Trailing refresh of {} failed, falling back to a full download: {}",
                    plain_url, e
                );
            }
        }
    }

    force_full_download(
        candidate_urls,
        &repo_local,
        &repo_db,
        repo_url_key,
        r_version,
        pkg_type,
        path,
        feed,
        true,
    )
}

/// Whether the database holds at least one row for the given repo, in the table
/// the feed stores its rows in.
fn repo_has_packages(
    db_path: &PathBuf,
    repo_url: &str,
    pkg_type: &str,
    feed: Feed,
) -> Result<bool, Box<dyn Error>> {
    let conn = open_db(db_path)?;
    let repo_url = repo_url.trim_end_matches('/');
    let query = match feed {
        Feed::Cranlike => {
            "SELECT COUNT(*) FROM packages p
             JOIN repos r ON p.repo_id = r.id
             WHERE r.url = ?1 AND r.pkg_type = ?2"
        }
        Feed::Archived => {
            "SELECT COUNT(*) FROM archived_packages p
             JOIN repos r ON p.repo_id = r.id
             WHERE r.url = ?1 AND r.pkg_type = ?2"
        }
    };
    let count: i64 = conn.query_row(query, params![repo_url, pkg_type], |row| row.get(0))?;
    Ok(count > 0)
}

/// Parse a freshly downloaded metadata file, store it in the database, delete
/// the temporary download, and return the parsed packages.
///
/// [`Feed::Archived`] stores its rows in the `archived_packages` table and
/// returns an empty vector: those rows are only ever queried per package.
#[allow(clippy::too_many_arguments)]
fn parse_store_and_cleanup(
    repo_local: &PathBuf,
    repo_db: &PathBuf,
    repo_url_key: &str,
    r_version: Option<&str>,
    pkg_type: &str,
    path: &str,
    etag: Option<&str>,
    feed: Feed,
) -> Result<Vec<Package>, Box<dyn Error>> {
    info!("Downloaded new repo metadata to {}", repo_local.display());
    // Parse DCF/RDS file and save to database
    let (packages, data) = parse_packages(repo_local)?;
    let tail_hash = hash_bytes(tail_slice(&data));

    // Save to database with the etag from the download, recording how much of
    // the (decompressed) feed has now been parsed, for a trailing-only
    // refresh next time.
    store_packages(
        &packages,
        repo_db,
        repo_url_key,
        r_version,
        pkg_type,
        path,
        feed,
        etag,
        true,
        Some((data.len() as i64, &tail_hash)),
    )?;

    // Delete the temporary data file after saving to database
    if let Err(e) = std::fs::remove_file(repo_local) {
        info!(
            "Could not delete temporary file {}: {}",
            repo_local.display(),
            e
        );
    }

    info!("Saved {} packages to database cache", packages.len());
    match feed {
        Feed::Cranlike => Ok(packages),
        Feed::Archived => Ok(vec![]),
    }
}

fn parse_packages(dcf_path: &PathBuf) -> Result<(Vec<Package>, Vec<u8>), Box<dyn Error>> {
    let mut file = File::open(dcf_path)?;

    // Peek at first 6 bytes to check for compression magic numbers
    // gzip: 0x1f, 0x8b (2 bytes)
    // xz: 0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00 (6 bytes: 0xFD, '7', 'z', 'X', 'Z', 0x00)
    // zstd: 0x28, 0xB5, 0x2F, 0xFD (4 bytes)
    let mut magic = [0u8; 6];
    let bytes_read = file.read(&mut magic)?;

    // Rewind to start
    file.seek(SeekFrom::Start(0))?;

    info!("Parsing repo metadata from {}", dcf_path.display());

    // Decompress if needed and read into memory to check format
    let data: Vec<u8> = if bytes_read >= 2 && magic[0..2] == [0x1f, 0x8b] {
        // Gzip compressed
        let mut decoder = GzDecoder::new(file);
        let mut data = Vec::new();
        decoder.read_to_end(&mut data)?;
        data
    } else if bytes_read >= 6 && magic == [0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00] {
        // XZ compressed
        let mut decoder = XzDecoder::new(file);
        let mut data = Vec::new();
        decoder.read_to_end(&mut data)?;
        data
    } else if bytes_read >= 4 && magic[0..4] == [0x28, 0xB5, 0x2F, 0xFD] {
        // Zstandard compressed (e.g. ALLPACKAGES.zst)
        let mut decoder = ZstdDecoder::new(file)?;
        let mut data = Vec::new();
        decoder.read_to_end(&mut data)?;
        data
    } else {
        // Uncompressed - read entire file
        let mut data = Vec::new();
        file.read_to_end(&mut data)?;
        data
    };

    // Check if decompressed data is RDS format
    // RDS files start with: 0x58 0x00 (X), 0x41 0x00 (A), or 0x42 0x00 (B)
    if data.len() >= 2 {
        // X (0x58), A (0x41) or B (0x42) format, each followed by 0x00
        let is_rds = (data[0] == 0x58 || data[0] == 0x41 || data[0] == 0x42) && data[1] == 0x00;

        if is_rds {
            info!("Detected RDS format, parsing as RDS");
            let robj = read_rds(&data)?;
            let packages = parse_packages_from_rds_object(robj)?;
            return Ok((packages, data));
        }
    }

    let packages = parse_dcf_bytes(&data)?;
    Ok((packages, data))
}

/// Parse `data` as a standalone DCF paragraph stream (no decompression, no
/// RDS detection — for feeds known to be plain DCF text, such as a
/// trailing-only refresh's fetched suffix).
fn parse_dcf_bytes(data: &[u8]) -> Result<Vec<Package>, Box<dyn Error>> {
    info!("Parsing as DCF format");
    let desc = parse_dcf_reader(data)?;
    info!("Parsed {} packages from repo metadata", desc.len());

    let mut packages: Vec<Package> = vec![];

    // Historical metadata (e.g. ALLPACKAGES) contains a handful of very old
    // packages with malformed dependency fields (stray URLs, junk version
    // constraints, ...). Skip those individual paragraphs with a warning
    // rather than aborting the whole file.
    let mut skipped = 0usize;
    for pkg in desc.iter() {
        match Package::from_dcf_paragraph(pkg) {
            Ok(p) => packages.push(p),
            Err(e) => {
                skipped += 1;
                let ident = pkg
                    .get("Package")
                    .map(|name| match pkg.get("Version") {
                        Some(ver) => format!("{} {}", name, ver),
                        None => name.to_string(),
                    })
                    .unwrap_or_else(|| "<unknown package>".to_string());
                debug!("Skipping unparseable package metadata for {}: {}", ident, e);
            }
        }
    }
    if skipped > 0 {
        info!("Skipped {} package(s) with unparseable metadata", skipped);
    }

    Ok(packages)
}

fn parse_packages_from_rds_object(robj: RObject) -> Result<Vec<Package>, Box<dyn Error>> {
    let (data, attr) = match robj {
        WithAttributes { object, attributes } => (object, attributes),
        _ => {
            OUTPUT.error("Failed to parse PACKAGES.rds file.");
            error!("Expected R object with attributes when reading PACKAGES.rds.");
            bail!("Expected R object with attributes when reading PACKAGES.rds.")
        }
    };

    let data = match *data {
        Character(vd) => {
            if let VectorData::Owned(v) = vd {
                v
            } else {
                OUTPUT.error("Failed to parse PACKAGES.rds file.");
                error!("Expected data to be owned character vector in PACKAGES.rds.");
                bail!("Expected data to be owned character vector in PACKAGES.rds.");
            }
        }
        _ => {
            OUTPUT.error("Failed to parse PACKAGES.rds file.");
            error!("Expected data to be a character vector in PACKAGES.rds.");
            bail!("Expected data to be a character vector in PACKAGES.rds.");
        }
    };

    let dim = attr
        .get("dim")
        .ok_or("Missing 'dim' attribute in PACKAGES.rds")?;
    let dim = match dim {
        Integer(vd) => {
            if let VectorData::Owned(v) = vd {
                if let [nrow, ncol] = &v[..] {
                    (*nrow as usize, *ncol as usize)
                } else {
                    OUTPUT.error("Failed to parse PACKAGES.rds file.");
                    error!("Expected 'dim' to have length 2 in PACKAGES.rds.");
                    bail!("Expected 'dim' to have length 2 in PACKAGES.rds.");
                }
            } else {
                OUTPUT.error("Failed to parse PACKAGES.rds file.");
                error!("Expected 'dim' to be owned integer vector in PACKAGES.rds.");
                bail!("Expected 'dim' to be owned integer vector in PACKAGES.rds.");
            }
        }
        _ => {
            OUTPUT.error("Failed to parse PACKAGES.rds file.");
            error!("Expected 'dim' to be an integer vector in PACKAGES.rds.");
            bail!("Expected 'dim' to be an integer vector in PACKAGES.rds.");
        }
    };
    let dimnames = attr
        .get("dimnames")
        .ok_or("Missing 'dimnames' attribute in PACKAGES.rds")?;
    let names = match dimnames {
        RObject::List(dn) => {
            if dn.len() != 2 {
                OUTPUT.error("Failed to parse PACKAGES.rds file.");
                error!("Expected 'dimnames' to have length 2 in PACKAGES.rds.");
                bail!("Expected 'dimnames' to have length 2 in PACKAGES.rds.");
            }
            if let Character(vd) = &dn[1] {
                if let VectorData::Owned(v) = vd {
                    v
                } else {
                    OUTPUT.error("Failed to parse PACKAGES.rds file.");
                    error!("Expected 'dimnames' second element to be owned character vector in PACKAGES.rds.");
                    bail!("Expected 'dimnames' second element to be owned character vector in PACKAGES.rds.");
                }
            } else {
                OUTPUT.error("Failed to parse PACKAGES.rds file.");
                error!(
                    "Expected 'dimnames' second element to be character vector in PACKAGES.rds."
                );
                bail!("Expected 'dimnames' second element to be character vector in PACKAGES.rds.");
            }
        }
        _ => {
            OUTPUT.error("Failed to parse PACKAGES.rds file.");
            error!("Expected 'dimnames' to be a list in PACKAGES.rds.");
            bail!("Expected 'dimnames' to be a list in PACKAGES.rds.");
        }
    };
    let mut col_idx = HashMap::new();
    for (idx, nm) in names.iter().enumerate() {
        col_idx.insert(nm.clone(), idx);
    }
    let selected_col_names = vec![
        "Package",
        "Version",
        "Depends",
        "Imports",
        "Suggests",
        "Enhances",
        "LinkingTo",
        "File",
        "Path",
        "DownloadURL",
        "Built",
        "License",
        "Platform",
        "Arch",
        "GraphicsAPIVersion",
        "InternalsID",
        "Filesize",
        "SHA256Original",
        "SystemRequirements",
    ];
    let mut cols: HashMap<&str, Vec<Arc<str>>> = HashMap::new();
    let nacol: Vec<Arc<str>> = vec!["NA".into(); dim.0];
    for nm in selected_col_names.iter() {
        let idx = col_idx.get(*nm);
        let col = match idx {
            Some(i) => {
                let start = i * dim.0;
                let end = start + dim.0;
                data[start..end].to_vec()
            }
            None => nacol.clone(),
        };
        cols.insert(*nm, col);
    }

    fn na_to_none(s: &str) -> Option<String> {
        if s == "NA" {
            None
        } else {
            Some(s.to_string())
        }
    }

    let mut packages: Vec<Package> = vec![];
    for i in 0..dim.0 {
        let mut dependencies = PackageDependencies::new();
        for dep_type in RDepType::all() {
            let dep_type_str = dep_type.to_string();
            let dep_str = cols.get(dep_type_str.as_str()).unwrap()[i].clone();
            if dep_str != "NA".into() {
                dependencies.append(&mut PackageDependencies::from_str(&dep_str, &dep_type_str)?);
            }
        }
        let name = cols.get("Package").unwrap()[i].clone();
        let version = RPackageVersion::from_str(&cols.get("Version").unwrap()[i])?;
        let file = cols.get("File").unwrap()[i].clone();
        let path = cols.get("Path").unwrap()[i].clone();
        let download_url = cols.get("DownloadURL").unwrap()[i].clone();
        let built = cols.get("Built").unwrap()[i].clone();
        let license = cols.get("License").unwrap()[i].clone();
        let platform = cols.get("Platform").unwrap()[i].clone();
        let arch = cols.get("Arch").unwrap()[i].clone();
        let graphics_api_version = cols.get("GraphicsAPIVersion").unwrap()[i].clone();
        let internals_id = cols.get("InternalsID").unwrap()[i].clone();
        let filesize = cols.get("Filesize").unwrap()[i].clone();
        let sha256sum = cols.get("SHA256Original").unwrap()[i].clone();
        let system_requirements = cols.get("SystemRequirements").unwrap()[i].clone();

        let pkg = Package {
            name: name.to_string(),
            version,
            dependencies,
            file: na_to_none(&file),
            path: na_to_none(&path),
            download_url: na_to_none(&download_url),
            built: na_to_none(&built)
                .map(|b| DCFBuilt::from_str(&b))
                .transpose()?,
            license: na_to_none(&license),
            platform: na_to_none(&platform),
            arch: na_to_none(&arch),
            graphics_api_version: na_to_none(&graphics_api_version),
            internals_id: na_to_none(&internals_id),
            filesize: na_to_none(&filesize).and_then(|s| s.parse::<u64>().ok()),
            sha256sum: na_to_none(&sha256sum),
            // Only the ARCHIVEDPACKAGES DCF feed has this, no RDS repo does.
            archived: None,
            repository: None,
            system_requirements: normalize_system_requirements(&system_requirements),
        };
        packages.push(pkg);
    }

    Ok(packages)
}

pub fn parse_packages_from_rds(rds_path: &PathBuf) -> Result<Vec<Package>, Box<dyn Error>> {
    let robj = read_rds_file(rds_path)?;
    parse_packages_from_rds_object(robj)
}

fn ensure_db_schema(db_path: &PathBuf) -> Result<(), Box<dyn Error>> {
    let conn = open_db(db_path)?;

    // Create repos table
    conn.execute(
        "CREATE TABLE IF NOT EXISTS repos (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            url TEXT NOT NULL,
            pkg_type TEXT NOT NULL,
            r_version TEXT,
            path TEXT NOT NULL,
            etag TEXT,
            last_updated TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            parsed_len INTEGER,
            tail_hash TEXT
        )",
        [],
    )?;

    // Databases created before trailing-only refreshes lack these columns.
    for stmt in [
        "ALTER TABLE repos ADD COLUMN parsed_len INTEGER",
        "ALTER TABLE repos ADD COLUMN tail_hash TEXT",
    ] {
        if let Err(e) = conn.execute(stmt, []) {
            if !e.to_string().contains("duplicate column") {
                return Err(e.into());
            }
        }
    }

    conn.execute(
        "CREATE TABLE IF NOT EXISTS packages (
            name TEXT NOT NULL,
            version TEXT NOT NULL,
            dependencies TEXT NOT NULL,
            download_url TEXT,
            file TEXT,
            path TEXT,
            built TEXT,
            license TEXT,
            platform TEXT,
            arch TEXT,
            graphics_api_version TEXT,
            internals_id TEXT,
            filesize INTEGER,
            sha256sum TEXT,
            repo_id INTEGER NOT NULL,
            system_requirements TEXT,
            FOREIGN KEY (repo_id) REFERENCES repos(id)
        )",
        [],
    )?;

    // Databases created before rig stored `SystemRequirements` lack the
    // column. Their rows have no system requirements, so after adding it,
    // forget every repo's cache state: the next lookup then downloads and
    // parses each feed again, in full, instead of a trailing-only refresh
    // that would only fill in the new rows.
    match conn.execute(
        "ALTER TABLE packages ADD COLUMN system_requirements TEXT",
        [],
    ) {
        Ok(_) => {
            info!("Added system_requirements column, invalidating cached metadata");
            conn.execute(
                "UPDATE repos SET etag = NULL, parsed_len = NULL, tail_hash = NULL,
                 last_updated = '1970-01-01 00:00:00'",
                [],
            )?;
        }
        Err(e) => {
            if !e.to_string().contains("duplicate column") {
                return Err(e.into());
            }
        }
    }

    // Create index for fast lookups by name, version, platform, arch
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_packages_lookup
         ON packages (name, version, platform, arch)",
        [],
    )?;

    // Create index for fast lookups by repo_id
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_packages_repo_id
         ON packages (repo_id)",
        [],
    )?;

    // The packages CRAN has archived (ARCHIVEDPACKAGES), one row per package
    // with the date it was archived.
    conn.execute(
        "CREATE TABLE IF NOT EXISTS archived_packages (
            name TEXT NOT NULL,
            archived TEXT NOT NULL,
            repo_id INTEGER NOT NULL,
            FOREIGN KEY (repo_id) REFERENCES repos(id)
        )",
        [],
    )?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_archived_packages_name
         ON archived_packages (name)",
        [],
    )?;

    Ok(())
}

/// Get the stored etag for a repository from the database
fn get_repo_etag(
    db_path: &PathBuf,
    repo_url: &str,
    pkg_type: &str,
) -> Result<String, Box<dyn Error>> {
    let conn = open_db(db_path)?;

    // Normalize repo_url by removing trailing slashes
    let repo_url = repo_url.trim_end_matches('/');

    let etag: String = conn.query_row(
        "SELECT etag FROM repos WHERE url = ?1 AND pkg_type = ?2 AND etag IS NOT NULL",
        params![repo_url, pkg_type],
        |row| row.get(0),
    )?;

    Ok(etag)
}

fn is_repo_cache_recent(
    db_path: &PathBuf,
    repo_url: &str,
    pkg_type: &str,
) -> Result<bool, Box<dyn Error>> {
    let conn = open_db(db_path)?;

    // Normalize repo_url by removing trailing slashes
    let repo_url = repo_url.trim_end_matches('/');

    // Check if last_updated is within the last 24 hours using SQLite's datetime functions
    let is_recent: bool = conn.query_row(
        "SELECT
            CASE
                WHEN (julianday('now') - julianday(last_updated)) * 24 < 24 THEN 1
                ELSE 0
            END as is_recent
         FROM repos
         WHERE url = ?1 AND pkg_type = ?2",
        params![repo_url, pkg_type],
        |row| row.get(0),
    )?;

    Ok(is_recent)
}

#[allow(clippy::too_many_arguments)]
fn save_packages_to_db(
    packages: &Vec<Package>,
    db_path: &PathBuf,
    repo_url: &str,
    r_version: Option<&str>,
    pkg_type: &str,
    path: &str,
    etag: Option<&str>,
    replace: bool,
    progress: Option<(i64, &str)>,
) -> Result<(), Box<dyn Error>> {
    let mut conn = open_db(db_path)?;

    // Normalize repo_url by removing trailing slashes
    let repo_url = repo_url.trim_end_matches('/');

    // For source packages, we don't store r_version (use NULL)
    let r_version_to_store = if pkg_type == "source" {
        None
    } else {
        r_version
    };

    // Use a single transaction for all inserts - much faster!
    let tx = conn.transaction()?;

    // Insert or get the repo_id
    tx.execute(
        "INSERT OR IGNORE INTO repos (url, pkg_type, r_version, path, etag) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![repo_url, pkg_type, r_version_to_store, path, etag],
    )?;

    // Update last_updated always; only overwrite etag when we actually have a
    // new one (a trailing-only refresh has no `.zst` etag of its own).
    tx.execute(
        "UPDATE repos SET etag = COALESCE(?1, etag), last_updated = CURRENT_TIMESTAMP
         WHERE url = ?2 AND pkg_type = ?3 AND r_version IS ?4 AND path = ?5",
        params![etag, repo_url, pkg_type, r_version_to_store, path],
    )?;

    let repo_id: i64 = tx.query_row(
        "SELECT id FROM repos WHERE url = ?1 AND pkg_type = ?2 AND r_version IS ?3 AND path = ?4",
        params![repo_url, pkg_type, r_version_to_store, path],
        |row| row.get(0),
    )?;

    // A trailing-only refresh only ever adds rows; a full (re)download
    // replaces everything for this repo.
    if replace {
        tx.execute("DELETE FROM packages WHERE repo_id = ?1", params![repo_id])?;
    }

    // Insert packages
    let mut stmt = tx.prepare(
        "INSERT INTO packages
         (name, version, dependencies, download_url, file, path, built,
          license, platform, arch, graphics_api_version, internals_id, filesize,
          sha256sum, repo_id, system_requirements)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
    )?;

    for pkg in packages {
        let deps_json = serde_json::to_string(&pkg.dependencies)?;
        let built_json = pkg
            .built
            .as_ref()
            .and_then(|b| serde_json::to_string(b).ok());

        stmt.execute(params![
            &pkg.name,
            pkg.version.to_string(),
            deps_json,
            &pkg.download_url,
            &pkg.file,
            &pkg.path,
            built_json,
            &pkg.license,
            &pkg.platform,
            &pkg.arch,
            &pkg.graphics_api_version,
            &pkg.internals_id,
            pkg.filesize,
            &pkg.sha256sum,
            repo_id,
            &pkg.system_requirements,
        ])?;
    }

    if let Some((parsed_len, tail_hash)) = progress {
        tx.execute(
            "UPDATE repos SET parsed_len = ?1, tail_hash = ?2 WHERE id = ?3",
            params![parsed_len, tail_hash, repo_id],
        )?;
    }

    drop(stmt); // Drop statement before committing
    tx.commit()?;

    Ok(())
}

/// Store the ARCHIVEDPACKAGES records in the `archived_packages` table.
#[allow(clippy::too_many_arguments)]
fn save_archived_to_db(
    packages: &[Package],
    db_path: &PathBuf,
    repo_url: &str,
    pkg_type: &str,
    path: &str,
    etag: Option<&str>,
    replace: bool,
    progress: Option<(i64, &str)>,
) -> Result<(), Box<dyn Error>> {
    let mut conn = open_db(db_path)?;
    let repo_url = repo_url.trim_end_matches('/');

    let tx = conn.transaction()?;

    // The repo row is what carries the etag and the `last_updated` timestamp
    // the 24h cache checks, so it is written the same way as for `packages`.
    tx.execute(
        "INSERT OR IGNORE INTO repos (url, pkg_type, r_version, path, etag) VALUES (?1, ?2, NULL, ?3, ?4)",
        params![repo_url, pkg_type, path, etag],
    )?;

    // Update last_updated always; only overwrite etag when we actually have a
    // new one (a trailing-only refresh has no `.zst` etag of its own).
    tx.execute(
        "UPDATE repos SET etag = COALESCE(?1, etag), last_updated = CURRENT_TIMESTAMP
         WHERE url = ?2 AND pkg_type = ?3 AND r_version IS NULL AND path = ?4",
        params![etag, repo_url, pkg_type, path],
    )?;

    let repo_id: i64 = tx.query_row(
        "SELECT id FROM repos WHERE url = ?1 AND pkg_type = ?2 AND r_version IS NULL AND path = ?3",
        params![repo_url, pkg_type, path],
        |row| row.get(0),
    )?;

    // A trailing-only refresh only ever adds rows; a full (re)download
    // replaces everything for this repo.
    if replace {
        tx.execute(
            "DELETE FROM archived_packages WHERE repo_id = ?1",
            params![repo_id],
        )?;
    }

    let mut stmt = tx.prepare(
        "INSERT INTO archived_packages (name, archived, repo_id)
         VALUES (?1, ?2, ?3)",
    )?;

    let mut stored = 0usize;
    for pkg in packages {
        // Every record of this feed has an `Archived` field; a record without
        // one carries no information we could store.
        let archived = match &pkg.archived {
            Some(archived) => archived,
            None => continue,
        };
        stmt.execute(params![&pkg.name, archived, repo_id])?;
        stored += 1;
    }

    if let Some((parsed_len, tail_hash)) = progress {
        tx.execute(
            "UPDATE repos SET parsed_len = ?1, tail_hash = ?2 WHERE id = ?3",
            params![parsed_len, tail_hash, repo_id],
        )?;
    }

    drop(stmt); // Drop statement before committing
    tx.commit()?;

    info!("Saved {} archived packages to database cache", stored);
    Ok(())
}

fn repo_db_file(dcf_path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let parent = dcf_path
        .parent()
        .ok_or("Cannot determine parent directory for database file")?;
    let db_path = parent.join("packages.db");
    Ok(db_path)
}

fn repo_local_file(url: &str) -> Result<PathBuf, Box<dyn Error>> {
    let mut cache = get_cache_dir()?;
    let urlhash = "repo-".to_string() + &calculate_hash(url) + ".data";

    cache.push("metadata");
    cache.push(urlhash);

    Ok(cache)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_packages_zstd() {
        use std::io::Write;

        // A small DCF PACKAGES file with two versions of pkgA (as ALLPACKAGES
        // would carry) plus pkgB.
        let dcf = "\
Package: pkgA
Version: 1.0.0
Imports: pkgB

Package: pkgA
Version: 0.9.0

Package: pkgB
Version: 2.1.0
Depends: R (>= 3.5.0)
";
        let compressed = zstd::stream::encode_all(dcf.as_bytes(), 0).unwrap();
        // Sanity check: the zstd magic bytes parse_packages sniffs for.
        assert_eq!(&compressed[0..4], &[0x28, 0xB5, 0x2F, 0xFD]);

        let mut path = std::env::temp_dir();
        path.push(format!("rig-test-allpackages-{}.zst", std::process::id()));
        File::create(&path).unwrap().write_all(&compressed).unwrap();

        let result = parse_packages(&path);
        let _ = std::fs::remove_file(&path);

        let (packages, _data) = result.expect("parse zstd-compressed PACKAGES");
        assert_eq!(packages.len(), 3);
        let mut vers: Vec<_> = packages
            .iter()
            .filter(|p| p.name == "pkgA")
            .map(|p| p.version.to_string())
            .collect();
        vers.sort();
        assert_eq!(vers, vec!["0.9.0".to_string(), "1.0.0".to_string()]);
    }

    #[test]
    fn test_parse_packages_reads_archived() {
        use std::io::Write;

        // ARCHIVEDPACKAGES has the same shape as ALLPACKAGES, with one extra
        // field: the date CRAN archived the package.
        let dcf = "\
Package: gpclib
Version: 1.5-6
Depends: R (>= 3.0.0), methods
License: GPL-2
Snapshot: 2020-03-02
DownloadURL: https://p3m.dev/cran/2020-03-02/src/contrib/gpclib_1.5-6.tar.gz
Archived: 2020-03-08

Package: pkgB
Version: 2.1.0
";
        let compressed = zstd::stream::encode_all(dcf.as_bytes(), 0).unwrap();
        let mut path = std::env::temp_dir();
        path.push(format!(
            "rig-test-archivedpackages-{}.zst",
            std::process::id()
        ));
        File::create(&path).unwrap().write_all(&compressed).unwrap();

        let result = parse_packages(&path);
        let _ = std::fs::remove_file(&path);

        let (packages, _data) = result.expect("parse zstd-compressed ARCHIVEDPACKAGES");
        let a = packages.iter().find(|p| p.name == "gpclib").unwrap();
        assert_eq!(a.archived.as_deref(), Some("2020-03-08"));
        // A record without the field, as every other repo's records are.
        let b = packages.iter().find(|p| p.name == "pkgB").unwrap();
        assert_eq!(b.archived, None);
    }

    /// A minimal ARCHIVEDPACKAGES record, as `save_archived_to_db` sees it.
    fn archived_record(name: &str, archived: Option<&str>) -> Package {
        let mut pkg = Package::from_crandb(
            name.to_string(),
            RPackageVersion::from_str("1.0.0").unwrap(),
            vec![],
        );
        pkg.archived = archived.map(|a| a.to_string());
        pkg
    }

    #[test]
    fn test_archived_packages_round_trip() {
        let url = "https://example.com/ARCHIVEDPACKAGES.zst";
        let mut db = std::env::temp_dir();
        db.push(format!("rig-test-archived-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&db);
        ensure_db_schema(&db).unwrap();

        let packages = vec![
            archived_record("gpclib", Some("2020-03-08")),
            archived_record("zipcode", Some("2018-05-14")),
            // Not from this feed, so there is nothing to record about it.
            archived_record("pkgB", None),
        ];
        save_archived_to_db(
            &packages,
            &db,
            url,
            "source",
            "ARCHIVEDPACKAGES",
            None,
            true,
            None,
        )
        .unwrap();

        let found = archived_package_in_db(&db, url, "gpclib").unwrap();
        assert_eq!(found.map(|a| a.archived), Some("2020-03-08".to_string()));
        assert!(archived_package_in_db(&db, url, "pkgB").unwrap().is_none());
        assert!(archived_package_in_db(&db, url, "nosuch")
            .unwrap()
            .is_none());

        // A refreshed feed replaces the old rows: CRAN un-archives packages,
        // and such a package has to stop being reported as archived.
        save_archived_to_db(
            &[archived_record("gpclib", Some("2020-03-08"))],
            &db,
            url,
            "source",
            "ARCHIVEDPACKAGES",
            None,
            true,
            None,
        )
        .unwrap();
        assert!(archived_package_in_db(&db, url, "gpclib")
            .unwrap()
            .is_some());
        assert!(archived_package_in_db(&db, url, "zipcode")
            .unwrap()
            .is_none());

        let _ = std::fs::remove_file(&db);
    }

    #[test]
    fn snapshot_date_is_read_from_the_download_url() {
        assert_eq!(
            snapshot_date("https://p3m.dev/cran/2026-06-08/src/contrib/pak_0.10.0.tar.gz"),
            Some("2026-06-08")
        );
        assert_eq!(
            snapshot_date("https://cran.r-project.org/src/contrib/pak_0.10.0.tar.gz"),
            None
        );
    }

    #[test]
    fn published_by_compares_the_snapshot_date_to_the_cutoff() {
        let url = Some("https://p3m.dev/cran/2020-01-09/src/contrib/cli_2.0.1.tar.gz");
        assert!(published_by(url, None));
        assert!(published_by(url, Some("2020-01-09")));
        assert!(published_by(url, Some("2020-01-10")));
        assert!(!published_by(url, Some("2020-01-08")));
        // No snapshot date to go by: kept.
        assert!(published_by(None, Some("2020-01-08")));
    }

    #[test]
    fn exclude_newer_hides_versions_published_after_the_cutoff() {
        use std::io::Write;

        let url = "https://example.com/ALLPACKAGES.zst";
        let dcf = "\
Package: cli
Version: 2.0.0
DownloadURL: https://p3m.dev/cran/2019-12-10/src/contrib/cli_2.0.0.tar.gz

Package: cli
Version: 2.0.1
DownloadURL: https://p3m.dev/cran/2020-01-09/src/contrib/cli_2.0.1.tar.gz
";
        let compressed = zstd::stream::encode_all(dcf.as_bytes(), 0).unwrap();
        let mut path = std::env::temp_dir();
        path.push(format!("rig-test-exclude-newer-{}.zst", std::process::id()));
        File::create(&path).unwrap().write_all(&compressed).unwrap();
        let (packages, _data) = parse_packages(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        let mut db = std::env::temp_dir();
        db.push(format!("rig-test-exclude-newer-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&db);
        ensure_db_schema(&db).unwrap();
        save_packages_to_db(
            &packages,
            &db,
            url,
            None,
            "source",
            "ALLPACKAGES",
            None,
            true,
            None,
        )
        .unwrap();

        let loader = |cutoff: Option<&str>| {
            let conn = open_db(&db).unwrap();
            DbSourcePackageLoader::from_conn(conn, &[(url, RepoId::Cran)])
                .with_exclude_newer(cutoff.map(|c| c.to_string()))
        };
        let versions = |cutoff: Option<&str>| {
            let mut vers: Vec<String> = loader(cutoff)
                .load_versions("cli")
                .unwrap()
                .iter()
                .map(|p| p.version.to_string())
                .collect();
            vers.sort();
            vers
        };

        assert_eq!(versions(None), vec!["2.0.0", "2.0.1"]);
        assert_eq!(versions(Some("2020-01-09")), vec!["2.0.0", "2.0.1"]);
        assert_eq!(versions(Some("2020-01-01")), vec!["2.0.0"]);
        assert!(versions(Some("2019-01-01")).is_empty());

        let _ = std::fs::remove_file(&db);
    }

    /// Store `dcf` as the ALLPACKAGES feed at `url` in the database `db`.
    fn store_feed(db: &PathBuf, url: &str, dcf: &str) {
        let packages = parse_dcf_bytes(dcf.as_bytes()).unwrap();
        save_packages_to_db(
            &packages,
            db,
            url,
            None,
            "source",
            "ALLPACKAGES",
            None,
            true,
            None,
        )
        .unwrap();
    }

    #[test]
    fn cran_and_bioc_versions_are_merged() {
        let cran_url = "https://example.com/cran/ALLPACKAGES.zst";
        let bioc_url = "https://example.com/bioc/3.22/ALLPACKAGES.zst";
        let mut db = std::env::temp_dir();
        db.push(format!("rig-test-cran-bioc-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&db);
        ensure_db_schema(&db).unwrap();
        store_feed(
            &db,
            cran_url,
            "\
Package: RBGL
Version: 1.0.0
DownloadURL: https://p3m.dev/cran/2020-01-01/src/contrib/RBGL_1.0.0.tar.gz

Package: RBGL
Version: 1.86.0
DownloadURL: https://p3m.dev/cran/2025-11-01/src/contrib/RBGL_1.86.0.tar.gz

Package: cli
Version: 3.6.0
",
        );
        store_feed(
            &db,
            bioc_url,
            "\
Package: RBGL
Version: 1.86.0
DownloadURL: https://p3m.dev/bioconductor/2025-11-04/packages/3.22/bioc/src/contrib/RBGL_1.86.0.tar.gz

Package: limma
Version: 3.66.0
DownloadURL: https://p3m.dev/bioconductor/2025-10-30/packages/3.22/bioc/src/contrib/limma_3.66.0.tar.gz
",
        );

        let conn = open_db(&db).unwrap();
        let bioc = RepoId::Bioc("3.22".to_string());
        // Bioconductor first, the way `MetadataFeed::for_target` orders them.
        let loader = DbSourcePackageLoader::from_conn(
            conn,
            &[(bioc_url, bioc.clone()), (cran_url, RepoId::Cran)],
        );

        let versions = |name: &str| {
            let mut out: Vec<(String, RepoId, String)> = loader
                .load_versions(name)
                .unwrap()
                .into_iter()
                .map(|p| {
                    (
                        p.version.to_string(),
                        p.repository.unwrap(),
                        p.download_url.unwrap_or_default(),
                    )
                })
                .collect();
            out.sort();
            out
        };

        // In both: the versions of both, the Bioconductor one on a tie.
        let rbgl = versions("RBGL");
        assert_eq!(rbgl.len(), 2);
        assert_eq!(rbgl[0].0, "1.0.0");
        assert_eq!(rbgl[0].1, RepoId::Cran);
        assert_eq!(rbgl[1].0, "1.86.0");
        assert_eq!(rbgl[1].1, bioc);
        assert!(rbgl[1].2.contains("/bioconductor/"));

        assert_eq!(versions("limma")[0].1, bioc);
        assert_eq!(versions("cli")[0].1, RepoId::Cran);
        assert!(versions("nope").is_empty());

        let _ = std::fs::remove_file(&db);
    }

    #[test]
    fn the_first_repository_wins_a_tie() {
        let cran_url = "https://example.com/cran/ALLPACKAGES.zst";
        let acme = CranlikeRepo::new("acme", "https://cran.acme.com/");
        let acme_key = cranlike_key(&acme, "src/contrib");
        let mut db = std::env::temp_dir();
        db.push(format!("rig-test-repo-rank-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&db);
        ensure_db_schema(&db).unwrap();
        store_feed(
            &db,
            cran_url,
            "Package: cli\nVersion: 3.6.0\nSystemRequirements: libfoo\n\n\
             Package: cli\nVersion: 3.5.0\n",
        );
        let packages = parse_cranlike_packages(
            b"Package: cli\nVersion: 3.6.0\nMD5sum: abc\n",
            &acme,
            "src/contrib",
            ".tar.gz",
        )
        .unwrap();
        save_packages_to_db(
            &packages,
            &db,
            &acme_key,
            None,
            "source",
            "src/contrib",
            None,
            true,
            None,
        )
        .unwrap();

        let versions = |keys: &[(&str, RepoId)]| {
            let loader = DbSourcePackageLoader::from_conn(open_db(&db).unwrap(), keys);
            let mut out: Vec<(String, RepoId)> = loader
                .load_versions("cli")
                .unwrap()
                .into_iter()
                .map(|p| (p.version.to_string(), p.repository.unwrap()))
                .collect();
            out.sort();
            out
        };

        let acme_first = versions(&[(&acme_key, acme.repo_id()), (cran_url, RepoId::Cran)]);
        assert_eq!(acme_first[0], ("3.5.0".to_string(), RepoId::Cran));
        assert_eq!(acme_first[1], ("3.6.0".to_string(), acme.repo_id()));

        // acme's `PACKAGES` has no `SystemRequirements`, CRAN's metadata does.
        let loader = DbSourcePackageLoader::from_conn(
            open_db(&db).unwrap(),
            &[(&acme_key, acme.repo_id()), (cran_url, RepoId::Cran)],
        );
        let cli = loader.load_versions("cli").unwrap();
        let v360 = cli.iter().find(|p| p.version.original == "3.6.0").unwrap();
        assert_eq!(v360.repository, Some(acme.repo_id()));
        assert_eq!(v360.system_requirements.as_deref(), Some("libfoo"));

        let cran_first = versions(&[(cran_url, RepoId::Cran), (&acme_key, acme.repo_id())]);
        assert_eq!(cran_first[1], ("3.6.0".to_string(), RepoId::Cran));

        let _ = std::fs::remove_file(&db);
    }

    #[test]
    fn cranlike_packages_get_download_urls_and_checksums() {
        let repo = CranlikeRepo::new("acme", "https://cran.acme.com/");
        let dcf = "\
Package: pkgA
Version: 1.0.0
Imports: pkgB
MD5sum: 0123

Package: pkgB
Version: 2.1-3
SHA256: abcd
MD5sum: 0123
Path: old

Package: pkgC
Version: 0.1
File: pkgC-custom.tar.gz

Package: pkgD
Version: 0.2
DownloadURL: https://elsewhere.com/pkgD.tar.gz
";
        let pkgs =
            parse_cranlike_packages(dcf.as_bytes(), &repo, "src/contrib", ".tar.gz").unwrap();
        let url = |i: usize| pkgs[i].download_url.clone().unwrap();
        assert_eq!(
            url(0),
            "https://cran.acme.com/src/contrib/pkgA_1.0.0.tar.gz"
        );
        assert_eq!(
            url(1),
            "https://cran.acme.com/src/contrib/old/pkgB_2.1-3.tar.gz"
        );
        assert_eq!(
            url(2),
            "https://cran.acme.com/src/contrib/pkgC-custom.tar.gz"
        );
        assert_eq!(url(3), "https://elsewhere.com/pkgD.tar.gz");
        assert_eq!(pkgs[0].sha256sum.as_deref(), Some("0123"));
        assert_eq!(pkgs[1].sha256sum.as_deref(), Some("abcd"));
        assert_eq!(pkgs[2].sha256sum, None);

        let gz = {
            use std::io::Write;
            let mut enc = flate2::write::GzEncoder::new(vec![], flate2::Compression::default());
            enc.write_all(dcf.as_bytes()).unwrap();
            enc.finish().unwrap()
        };
        assert_eq!(decompress_gzip_maybe(gz).unwrap(), dcf.as_bytes());
        assert_eq!(
            decompress_gzip_maybe(dcf.as_bytes().to_vec()).unwrap(),
            dcf.as_bytes()
        );
    }

    #[test]
    fn test_parse_packages_reads_sha256original() {
        use std::io::Write;

        // ALLPACKAGES carries both hashes: `SHA256Original` is the upstream CRAN
        // tarball's hash (what we keep as `sha256sum`), `SHA256` is P3M's
        // rewritten tarball (ignored).
        let dcf = "\
Package: pkgA
Version: 1.0.0
SHA256: a03ad0480203b160eea4dab532ceec78e924c9d4c6f793fe9d4e9ca3666697f3
SHA256Original: 43901f7baa265b0262b708dd4c09072768cbb1f8e32123bb07824e0ebfadda5a

Package: pkgB
Version: 2.1.0
";
        let mut path = std::env::temp_dir();
        path.push(format!("rig-test-sha256-{}.PACKAGES", std::process::id()));
        File::create(&path)
            .unwrap()
            .write_all(dcf.as_bytes())
            .unwrap();

        let result = parse_packages(&path);
        let _ = std::fs::remove_file(&path);

        let (packages, _data) = result.expect("parse PACKAGES with SHA256Original");
        let a = packages.iter().find(|p| p.name == "pkgA").unwrap();
        assert_eq!(
            a.sha256sum.as_deref(),
            Some("43901f7baa265b0262b708dd4c09072768cbb1f8e32123bb07824e0ebfadda5a")
        );
        let b = packages.iter().find(|p| p.name == "pkgB").unwrap();
        assert_eq!(b.sha256sum, None);
    }

    #[test]
    fn test_parse_packages_skips_malformed_paragraphs() {
        use std::io::Write;

        // pkgBad has a stray URL where a version constraint should be, mirroring
        // the malformed historical CRAN metadata in ALLPACKAGES. It must be
        // skipped without aborting the parse of the good packages.
        let dcf = "\
Package: pkgGood
Version: 1.0.0

Package: pkgBad
Version: 0.1.0
Depends: methods (http://www.example.com)

Package: pkgAlsoGood
Version: 2.0.0
Imports: pkgGood
";
        let mut path = std::env::temp_dir();
        path.push(format!(
            "rig-test-malformed-{}.PACKAGES",
            std::process::id()
        ));
        File::create(&path)
            .unwrap()
            .write_all(dcf.as_bytes())
            .unwrap();

        let result = parse_packages(&path);
        let _ = std::fs::remove_file(&path);

        let (packages, _data) = result.expect("parse must not abort on a malformed paragraph");
        let mut names: Vec<_> = packages.iter().map(|p| p.name.as_str()).collect();
        names.sort();
        assert_eq!(names, vec!["pkgAlsoGood", "pkgGood"]);
    }

    #[test]
    fn test_parse_packages_from_rds_src() {
        let path = PathBuf::from("tests/fixtures/cran-metadata/src/PACKAGES.rds");
        let result = parse_packages_from_rds(&path);

        assert!(
            result.is_ok(),
            "Failed to parse PACKAGES.rds: {:?}",
            result.err()
        );

        let packages = result.unwrap();
        assert!(!packages.is_empty(), "Expected at least one package");

        // Snapshot test the parsed packages
        insta::assert_debug_snapshot!(packages);
    }

    #[test]
    fn test_parse_packages_from_rds_binary() {
        let path =
            PathBuf::from("tests/fixtures/cran-metadata/bin/macosx/sonoma-arm64/PACKAGES.rds");
        let result = parse_packages_from_rds(&path);

        assert!(
            result.is_ok(),
            "Failed to parse binary PACKAGES.rds: {:?}",
            result.err()
        );

        let packages = result.unwrap();
        assert!(!packages.is_empty(), "Expected at least one package");

        // Snapshot test the parsed binary packages
        insta::assert_debug_snapshot!(packages);
    }

    #[test]
    fn test_parse_packages_from_rds_validates_structure() {
        let path = PathBuf::from("tests/fixtures/cran-metadata/src/PACKAGES.rds");
        let result = parse_packages_from_rds(&path);

        assert!(result.is_ok());
        let packages = result.unwrap();

        // Validate structure and snapshot the first package
        let first_pkg = &packages[0];

        // Name and version are required
        assert!(!first_pkg.name.is_empty());
        assert!(!first_pkg.version.to_string().is_empty());

        // Snapshot the first package to validate its structure
        insta::assert_debug_snapshot!(first_pkg);
    }

    #[test]
    fn test_package_type_to_path_source() {
        let result = package_type_to_path("source", "4.3").unwrap();
        assert_eq!(result, "src/contrib");
    }

    #[test]
    fn test_package_type_to_path_mac_binary() {
        let result = package_type_to_path("mac.binary", "4.3").unwrap();
        assert_eq!(result, "bin/macosx/contrib/4.3");
    }

    #[test]
    fn test_package_type_to_path_mac_binary_with_subtype() {
        let result = package_type_to_path("mac.binary.big-sur-arm64", "4.3").unwrap();
        assert_eq!(result, "bin/macosx/big-sur-arm64/contrib/4.3");
    }

    #[test]
    fn test_package_type_to_path_mac_binary_el_capitan() {
        let result = package_type_to_path("mac.binary.el-capitan", "3.6").unwrap();
        assert_eq!(result, "bin/macosx/el-capitan/contrib/3.6");
    }

    #[test]
    fn test_package_type_to_path_macos_binary_arm64() {
        // R >= 4.7.0 arm64 dropped the "macosx"/codename layout
        let result = package_type_to_path("macos.binary.arm64", "4.7").unwrap();
        assert_eq!(result, "bin/macos/arm64/contrib/4.7");
    }

    #[test]
    fn test_package_type_to_path_win_binary() {
        let result = package_type_to_path("win.binary", "4.3").unwrap();
        assert_eq!(result, "bin/windows/contrib/4.3");
    }

    #[test]
    fn test_package_type_to_path_different_versions() {
        let result1 = package_type_to_path("mac.binary", "4.1").unwrap();
        assert_eq!(result1, "bin/macosx/contrib/4.1");

        let result2 = package_type_to_path("mac.binary", "3.5").unwrap();
        assert_eq!(result2, "bin/macosx/contrib/3.5");
    }

    #[test]
    fn test_package_type_to_path_invalid() {
        let result = package_type_to_path("invalid", "4.3");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid package type:"));
    }

    #[test]
    fn test_package_type_to_path_no_version_in_source() {
        // Source should work with any version string (it's ignored)
        let result1 = package_type_to_path("source", "4.3").unwrap();
        let result2 = package_type_to_path("source", "3.0").unwrap();
        assert_eq!(result1, result2);
    }

    // Tests for minor_r_version

    #[test]
    fn test_minor_r_version_basic() {
        let result = minor_r_version("4.3.2").unwrap();
        assert_eq!(result, "4.3");
    }

    #[test]
    fn test_minor_r_version_patch_zero() {
        let result = minor_r_version("3.6.0").unwrap();
        assert_eq!(result, "3.6");
    }

    #[test]
    fn test_minor_r_version_different_versions() {
        assert_eq!(minor_r_version("4.1.3").unwrap(), "4.1");
        assert_eq!(minor_r_version("3.5.1").unwrap(), "3.5");
        assert_eq!(minor_r_version("4.0.0").unwrap(), "4.0");
    }

    #[test]
    fn test_minor_r_version_two_parts() {
        // Two-part versions should work (we append .0 internally)
        let result = minor_r_version("4.3").unwrap();
        assert_eq!(result, "4.3");

        // Test multiple two-part versions
        assert_eq!(minor_r_version("3.5").unwrap(), "3.5");
        assert_eq!(minor_r_version("4.0").unwrap(), "4.0");
    }

    #[test]
    fn test_minor_r_version_invalid() {
        let result = minor_r_version("invalid");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid R version format"));
    }

    #[test]
    fn test_minor_r_version_empty() {
        let result = minor_r_version("");
        assert!(result.is_err());
    }

    #[test]
    fn test_try_trailing_refresh_appends_when_tail_matches() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let old_text = "Package: pkgA\nVersion: 1.0.0\n\n";
        let new_paragraph = "Package: pkgB\nVersion: 2.0.0\n";
        let parsed_len = old_text.len() as i64;
        let tail_hash = hash_bytes(tail_slice(old_text.as_bytes()));

        // `parsed_len` is well under `TAIL_WINDOW`, so the overlap the real
        // server would be asked for is the whole of `old_text`: a correctly
        // behaving server's response is exactly `old_text` + the new bytes.
        let mut response_body = old_text.to_string();
        response_body.push_str(new_paragraph);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let mock_server = rt.block_on(MockServer::start());
        rt.block_on(
            Mock::given(method("GET"))
                .and(path("/ALLPACKAGES"))
                .respond_with(ResponseTemplate::new(206).set_body_string(response_body))
                .mount(&mock_server),
        );

        let url = format!("{}/ALLPACKAGES", mock_server.uri());
        let outcome = try_trailing_refresh(&url, parsed_len, &tail_hash).unwrap();

        let TrailingOutcome::Appended(data) = outcome else {
            panic!("expected an Appended outcome");
        };
        let window_len = tail_window_len(parsed_len);
        assert_eq!(&data[..window_len], old_text.as_bytes());
        assert_eq!(&data[window_len..], new_paragraph.as_bytes());
    }

    #[test]
    fn test_try_trailing_refresh_detects_rewrite() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let old_text = "Package: pkgA\nVersion: 1.0.0\n\n";
        let parsed_len = old_text.len() as i64;
        let tail_hash = hash_bytes(tail_slice(old_text.as_bytes()));

        // The origin rewrote history: same length, different content, so a
        // naive offset-only fetch would silently look plausible.
        let rewritten = "Package: pkgA\nVersion: 9.9.9\n\n";
        assert_eq!(rewritten.len(), old_text.len());

        let rt = tokio::runtime::Runtime::new().unwrap();
        let mock_server = rt.block_on(MockServer::start());
        rt.block_on(
            Mock::given(method("GET"))
                .and(path("/ALLPACKAGES"))
                .respond_with(ResponseTemplate::new(206).set_body_string(rewritten))
                .mount(&mock_server),
        );

        let url = format!("{}/ALLPACKAGES", mock_server.uri());
        let result = try_trailing_refresh(&url, parsed_len, &tail_hash);
        assert!(result.is_err(), "a tail-hash mismatch must be rejected");
    }

    #[test]
    fn test_try_trailing_refresh_out_of_range_is_unchanged() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let rt = tokio::runtime::Runtime::new().unwrap();
        let mock_server = rt.block_on(MockServer::start());
        rt.block_on(
            Mock::given(method("GET"))
                .and(path("/ALLPACKAGES"))
                .respond_with(ResponseTemplate::new(416))
                .mount(&mock_server),
        );

        let url = format!("{}/ALLPACKAGES", mock_server.uri());
        let outcome = try_trailing_refresh(&url, 100, "irrelevant").unwrap();
        assert!(matches!(outcome, TrailingOutcome::Unchanged));
    }

    /// Simulates two refresh cycles at the DB layer (independent of the
    /// network): a full download followed by a trailing-only append, then a
    /// full-replace fallback as would follow a detected rewrite.
    #[test]
    fn test_store_packages_incremental_append_and_replace_fallback() {
        let mut db = std::env::temp_dir();
        db.push(format!(
            "rig-test-trailing-append-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&db);
        ensure_db_schema(&db).unwrap();

        let url = "https://example.com/ALLPACKAGES.zst";

        // Run 1: full download and parse.
        let first = vec![Package::from_crandb(
            "pkgA".to_string(),
            RPackageVersion::from_str("1.0.0").unwrap(),
            vec![],
        )];
        store_packages(
            &first,
            &db,
            url,
            None,
            "source",
            "ALLPACKAGES",
            Feed::Cranlike,
            None,
            true,
            Some((100, "hash-v1")),
        )
        .unwrap();

        // Run 2: trailing-only append of a newly published package.
        let second = vec![Package::from_crandb(
            "pkgB".to_string(),
            RPackageVersion::from_str("2.0.0").unwrap(),
            vec![],
        )];
        store_packages(
            &second,
            &db,
            url,
            None,
            "source",
            "ALLPACKAGES",
            Feed::Cranlike,
            None,
            false,
            Some((150, "hash-v2")),
        )
        .unwrap();

        let conn = open_db(&db).unwrap();
        let mut stmt = conn
            .prepare("SELECT name FROM packages ORDER BY name")
            .unwrap();
        let names: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(names, vec!["pkgA".to_string(), "pkgB".to_string()]);

        let (parsed_len, tail_hash): (i64, String) = conn
            .query_row(
                "SELECT parsed_len, tail_hash FROM repos WHERE url = ?1 AND pkg_type = 'source'",
                params![url],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(parsed_len, 150);
        assert_eq!(tail_hash, "hash-v2");
        drop(stmt);
        drop(conn);

        // Run 3: a rewrite was detected, so the caller falls back to a full
        // replace rather than trusting the (now invalid) appended state.
        let replacement = vec![Package::from_crandb(
            "pkgC".to_string(),
            RPackageVersion::from_str("3.0.0").unwrap(),
            vec![],
        )];
        store_packages(
            &replacement,
            &db,
            url,
            None,
            "source",
            "ALLPACKAGES",
            Feed::Cranlike,
            None,
            true,
            Some((90, "hash-v3")),
        )
        .unwrap();

        let conn = open_db(&db).unwrap();
        let mut stmt = conn
            .prepare("SELECT name FROM packages ORDER BY name")
            .unwrap();
        let names: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(names, vec!["pkgC".to_string()]);

        let _ = std::fs::remove_file(&db);
    }
}
