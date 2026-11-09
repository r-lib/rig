//! The type of a repository, declared or detected.
//!
//! A declared type (`type = "..."` in the configuration) is used as is, with
//! the details that follow from the URL, see [`describe`]. Otherwise rig
//! detects the type, see [`detect_repo_type`], and caches the result in
//! `repo-types.json` in the cache directory, for [`CACHE_TTL`].

use std::collections::HashMap;
use std::error::Error;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use log::debug;
use serde::{Deserialize, Serialize};

use crate::output::OUTPUT;

use super::detect::{describe, detect_repo_type, DetectedRepo, RepoType};

/// How long a detected type is used before it is detected again.
pub const CACHE_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

const CACHE_FILE: &str = "repo-types.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    /// Seconds since the epoch.
    detected_at: u64,
    repo: DetectedRepo,
}

type Cache = HashMap<String, CacheEntry>;

/// The repository at `url`, of type `declared` if that is given, detected
/// otherwise.
pub fn resolve_repo(url: &str, declared: Option<RepoType>) -> DetectedRepo {
    let path = match crate::cache::get_cache_dir() {
        Ok(dir) => Some(dir.join(CACHE_FILE)),
        Err(e) => {
            debug!("No cache directory for repository types: {}", e);
            None
        }
    };
    resolve_repo_with(url, declared, path.as_deref(), now(), |u| {
        detect_repo_type(u)
    })
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// [`resolve_repo`] with the cache file at `cache` (none if `None`), the time
/// `now`, and the detection `detect`, for testing.
fn resolve_repo_with<F>(
    url: &str,
    declared: Option<RepoType>,
    cache: Option<&Path>,
    now: u64,
    detect: F,
) -> DetectedRepo
where
    F: Fn(&str) -> Result<DetectedRepo, Box<dyn Error>>,
{
    let url = url.trim().trim_end_matches('/');
    match declared {
        // The repositories of a PPM root are only known from its API.
        Some(RepoType::Ppm) => {
            let detected = cached_or_detect(url, cache, now, &detect);
            if detected.repo_type == RepoType::Ppm {
                detected
            } else {
                OUTPUT.warn(&format!(
                    "Repository {} is not the root of a Posit Package Manager instance",
                    url
                ));
                describe(url, RepoType::Ppm)
            }
        }
        Some(rtype) => describe(url, rtype),
        None => cached_or_detect(url, cache, now, &detect),
    }
}

fn cached_or_detect<F>(url: &str, cache: Option<&Path>, now: u64, detect: &F) -> DetectedRepo
where
    F: Fn(&str) -> Result<DetectedRepo, Box<dyn Error>>,
{
    let mut entries = cache.map(read_cache).unwrap_or_default();
    if let Some(entry) = entries.get(url) {
        if now.saturating_sub(entry.detected_at) < CACHE_TTL.as_secs() {
            debug!("Type of repository {} from the cache", url);
            return entry.repo.clone();
        }
    }

    let repo = match detect(url) {
        Ok(repo) => repo,
        Err(e) => {
            OUTPUT.warn(&format!(
                "Cannot detect the type of repository {}, using it as a CRAN-like \
                 repository: {}",
                url, e
            ));
            return DetectedRepo::new(url, RepoType::CranLike);
        }
    };

    // A repository that did not respond is not remembered, so it is detected
    // again next time.
    let unreachable = repo.repo_type == RepoType::CranLike && !repo.notes.is_empty();
    if let (Some(path), false) = (cache, unreachable) {
        entries.insert(
            url.to_string(),
            CacheEntry {
                detected_at: now,
                repo: repo.clone(),
            },
        );
        if let Err(e) = write_cache(path, &entries) {
            debug!("Cannot write {}: {}", path.display(), e);
        }
    }
    repo
}

fn read_cache(path: &Path) -> Cache {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            debug!("Ignoring invalid {}: {}", path.display(), e);
            HashMap::new()
        }),
        Err(_) => HashMap::new(),
    }
}

fn write_cache(path: &Path, entries: &Cache) -> Result<(), Box<dyn Error>> {
    let bytes = serde_json::to_vec_pretty(entries)?;
    crate::utils::write_atomically(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn detector(
        rtype: RepoType,
        calls: &Cell<u32>,
    ) -> impl Fn(&str) -> Result<DetectedRepo, Box<dyn Error>> + '_ {
        move |url| {
            calls.set(calls.get() + 1);
            Ok(DetectedRepo::new(url, rtype))
        }
    }

    #[test]
    fn declared_type_needs_no_detection() {
        let calls = Cell::new(0);
        let d = resolve_repo_with(
            "https://p3m.dev/cran/latest/",
            Some(RepoType::PpmCran),
            None,
            0,
            detector(RepoType::CranLike, &calls),
        );
        assert_eq!(calls.get(), 0);
        assert_eq!(d.repo_type, RepoType::PpmCran);
        assert_eq!(d.ppm_repo.as_deref(), Some("cran"));
    }

    #[test]
    fn detected_type_is_cached() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CACHE_FILE);
        let calls = Cell::new(0);
        let det = detector(RepoType::Cran, &calls);

        let url = "https://cloud.r-project.org";
        let d = resolve_repo_with(url, None, Some(&path), 1000, &det);
        assert_eq!(d.repo_type, RepoType::Cran);
        assert_eq!(calls.get(), 1);
        assert!(path.exists());

        // From the cache, also with a trailing slash.
        let d = resolve_repo_with(
            "https://cloud.r-project.org/",
            None,
            Some(&path),
            2000,
            &det,
        );
        assert_eq!(d.repo_type, RepoType::Cran);
        assert_eq!(calls.get(), 1);

        // Too old.
        let later = 1000 + CACHE_TTL.as_secs();
        resolve_repo_with(url, None, Some(&path), later, &det);
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn unreachable_repos_are_not_cached() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CACHE_FILE);
        let calls = Cell::new(0);
        let det = |url: &str| -> Result<DetectedRepo, Box<dyn Error>> {
            calls.set(calls.get() + 1);
            let mut d = DetectedRepo::new(url, RepoType::CranLike);
            d.notes
                .push("src/contrib/PACKAGES did not respond".to_string());
            Ok(d)
        };
        resolve_repo_with("https://down.org", None, Some(&path), 0, det);
        resolve_repo_with("https://down.org", None, Some(&path), 0, det);
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn detection_errors_give_cranlike() {
        let d = resolve_repo_with("https://p3m.dev/nope/latest", None, None, 0, |_| {
            Err("PPM has no repository called 'nope'".into())
        });
        assert_eq!(d.repo_type, RepoType::CranLike);
    }

    #[test]
    fn invalid_cache_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CACHE_FILE);
        std::fs::write(&path, "not json").unwrap();
        let calls = Cell::new(0);
        let d = resolve_repo_with(
            "https://x.org",
            None,
            Some(&path),
            0,
            detector(RepoType::Cran, &calls),
        );
        assert_eq!(d.repo_type, RepoType::Cran);
        assert_eq!(read_cache(&path).len(), 1);
    }

    #[test]
    fn declared_ppm_root_is_detected_for_its_repos() {
        let calls = Cell::new(0);
        let d = resolve_repo_with(
            "https://p3m.dev",
            Some(RepoType::Ppm),
            None,
            0,
            detector(RepoType::Ppm, &calls),
        );
        assert_eq!(calls.get(), 1);
        assert_eq!(d.repo_type, RepoType::Ppm);
    }
}
