//! The package metadata the solver reads. There are two kinds of
//! repositories:
//!
//! * Repositories with extended metadata ("feeds"): CRAN's, at
//!   `https://ppm.r-pkg.org`, and one per Bioconductor release, at
//!   `https://ppm-bioc.r-pkg.org/<bioc-version>`. Every feed has the same
//!   layout: an `ALLPACKAGES.zst` history of every version ever published, an
//!   `ARCHIVEDPACKAGES.zst` list of removed packages, and per-package binary
//!   indices at `binaries/<package>.tsv.zst`.
//! * Plain CRAN-like repositories, with `PACKAGES` files for the current
//!   packages only, at `src/contrib` and `bin/<os>/.../contrib/<R version>`.

use std::fmt;

/// Default base URL of CRAN's extended metadata.
pub const PPM_METADATA_URL: &str = "https://ppm.r-pkg.org";

/// Default base URL of the extended metadata of the Bioconductor releases,
/// `%v` is the Bioconductor version.
pub const BIOC_METADATA_URL: &str = "https://ppm-bioc.r-pkg.org/%v";

/// The repository a package version comes from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RepoId {
    Cran,
    /// A Bioconductor release, e.g. `Bioc("3.22")`.
    Bioc(String),
    /// A plain CRAN-like repository, by its name in the R installation's
    /// `repositories` file and its URL, without a trailing `/`.
    Cranlike {
        name: String,
        url: String,
    },
}

impl RepoId {
    pub fn is_bioc(&self) -> bool {
        matches!(self, RepoId::Bioc(_))
    }

    pub fn is_cranlike(&self) -> bool {
        matches!(self, RepoId::Cranlike { .. })
    }
}

/// `cran` or `bioc/<version>`, as recorded in the lock file, or the name of a
/// CRAN-like repository.
impl fmt::Display for RepoId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RepoId::Cran => write!(f, "cran"),
            RepoId::Bioc(v) => write!(f, "bioc/{}", v),
            RepoId::Cranlike { name, .. } => write!(f, "{}", name),
        }
    }
}

/// A plain CRAN-like repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CranlikeRepo {
    pub name: String,
    /// Without a trailing `/`.
    pub url: String,
}

impl CranlikeRepo {
    pub fn new(name: &str, url: &str) -> CranlikeRepo {
        CranlikeRepo {
            name: name.to_string(),
            url: url.trim_end_matches('/').to_string(),
        }
    }

    pub fn repo_id(&self) -> RepoId {
        RepoId::Cranlike {
            name: self.name.clone(),
            url: self.url.clone(),
        }
    }
}

/// A repository of a solve. A solve searches a list of these, and if several
/// of them have the same version of a package, the first one wins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PkgRepo {
    Extended(MetadataFeed),
    Cranlike(CranlikeRepo),
}

impl PkgRepo {
    pub fn repo_id(&self) -> RepoId {
        match self {
            PkgRepo::Extended(feed) => feed.repo.clone(),
            PkgRepo::Cranlike(repo) => repo.repo_id(),
        }
    }

    /// `feeds`, as repositories, in the same order.
    pub fn from_feeds(feeds: Vec<MetadataFeed>) -> Vec<PkgRepo> {
        feeds.into_iter().map(PkgRepo::Extended).collect()
    }

    /// The extended feeds in `repos`, in order.
    pub fn feeds(repos: &[PkgRepo]) -> Vec<MetadataFeed> {
        repos
            .iter()
            .filter_map(|r| match r {
                PkgRepo::Extended(feed) => Some(feed.clone()),
                PkgRepo::Cranlike(_) => None,
            })
            .collect()
    }

    /// The CRAN-like repositories in `repos`, in order.
    pub fn cranlike(repos: &[PkgRepo]) -> Vec<CranlikeRepo> {
        repos
            .iter()
            .filter_map(|r| match r {
                PkgRepo::Extended(_) => None,
                PkgRepo::Cranlike(repo) => Some(repo.clone()),
            })
            .collect()
    }
}

/// Where a feed's metadata lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataFeed {
    pub repo: RepoId,
    pub allpackages_url: String,
    pub archived_url: String,
    /// Base URL of the per-package binary indices, without a trailing `/`.
    pub binaries_url: String,
}

impl MetadataFeed {
    /// CRAN's feed, overridable with `RIG_ALLPACKAGES_URL`,
    /// `RIG_ARCHIVEDPACKAGES_URL` and `RIG_BINARIES_URL`.
    pub fn cran() -> MetadataFeed {
        let env =
            |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.to_string());
        MetadataFeed {
            repo: RepoId::Cran,
            allpackages_url: env(
                "RIG_ALLPACKAGES_URL",
                "https://ppm.r-pkg.org/ALLPACKAGES.zst",
            ),
            archived_url: env(
                "RIG_ARCHIVEDPACKAGES_URL",
                "https://ppm.r-pkg.org/ARCHIVEDPACKAGES.zst",
            ),
            binaries_url: env("RIG_BINARIES_URL", "https://ppm.r-pkg.org/binaries")
                .trim_end_matches('/')
                .to_string(),
        }
    }

    /// The feed of Bioconductor release `version`, under
    /// `RIG_BIOC_METADATA_URL` (default `https://ppm-bioc.r-pkg.org`).
    pub fn bioc(version: &str) -> MetadataFeed {
        let base = std::env::var("RIG_BIOC_METADATA_URL")
            .unwrap_or_else(|_| "https://ppm-bioc.r-pkg.org".to_string());
        MetadataFeed::bioc_at(&base, version)
    }

    fn bioc_at(base: &str, version: &str) -> MetadataFeed {
        let base = format!("{}/{}", base.trim_end_matches('/'), version);
        MetadataFeed::at(RepoId::Bioc(version.to_string()), &base)
    }

    /// The feed of `repo` with base URL `base`.
    pub fn at(repo: RepoId, base: &str) -> MetadataFeed {
        let base = base.trim_end_matches('/');
        MetadataFeed {
            repo,
            allpackages_url: format!("{}/ALLPACKAGES.zst", base),
            archived_url: format!("{}/ARCHIVEDPACKAGES.zst", base),
            binaries_url: format!("{}/binaries", base),
        }
    }

    /// The feed of the extended metadata at `base`, the `metadata` field of a
    /// repository entry, see [`crate::repos::config::RepoEntry`]. The default
    /// URLs give [`MetadataFeed::cran`] and [`MetadataFeed::bioc`], with their
    /// environment variable overrides. A URL with `%v` is a Bioconductor feed
    /// and needs `bioc_version`, otherwise there is no feed. Any other URL is a
    /// CRAN feed.
    pub fn from_metadata_url(base: &str, bioc_version: Option<&str>) -> Option<MetadataFeed> {
        let base = base.trim_end_matches('/');
        if base == PPM_METADATA_URL {
            return Some(MetadataFeed::cran());
        }
        if !base.contains("%v") {
            return Some(MetadataFeed::at(RepoId::Cran, base));
        }
        let version = bioc_version?;
        if base == BIOC_METADATA_URL {
            return Some(MetadataFeed::bioc(version));
        }
        Some(MetadataFeed::at(
            RepoId::Bioc(version.to_string()),
            &base.replace("%v", version),
        ))
    }

    /// The feeds of a target: Bioconductor release `bioc`, if any, and CRAN.
    /// Bioconductor comes first, so it wins if both have the same version of a
    /// package.
    pub fn for_target(bioc: Option<&str>) -> Vec<MetadataFeed> {
        let mut feeds = vec![];
        if let Some(v) = bioc {
            feeds.push(MetadataFeed::bioc(v));
        }
        feeds.push(MetadataFeed::cran());
        feeds
    }
}

/// Whether and which Bioconductor release a solve uses, besides CRAN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BiocSetting {
    pub enabled: bool,
    /// A pinned release, instead of the one that belongs to the R version.
    pub version: Option<String>,
}

impl Default for BiocSetting {
    /// Bioconductor is on by default, `RIG_BIOCONDUCTOR=false` turns it off.
    fn default() -> Self {
        let enabled = !matches!(
            std::env::var("RIG_BIOCONDUCTOR")
                .map(|v| v.to_lowercase())
                .as_deref(),
            Ok("false" | "no" | "0" | "off")
        );
        BiocSetting {
            enabled,
            version: None,
        }
    }
}

impl BiocSetting {
    /// The Bioconductor release to use with R version `rver`, if any, see
    /// [`crate::repos::bioc_version_for`]. `cutoff` is the `--exclude-newer`
    /// day.
    pub fn bioc_version(&self, rver: &str, cutoff: Option<&str>) -> Option<String> {
        if !self.enabled {
            return None;
        }
        let ver = crate::repos::bioc_version_for(rver, self.version.as_deref(), cutoff);
        if ver.is_none() {
            log::debug!("No Bioconductor release for R {}, using CRAN only", rver);
        }
        ver
    }

    /// The feeds of a solve for R version `rver`.
    pub fn feeds(&self, rver: &str, cutoff: Option<&str>) -> Vec<MetadataFeed> {
        MetadataFeed::for_target(self.bioc_version(rver, cutoff).as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bioc_feed_urls() {
        let feed = MetadataFeed::bioc_at("https://ppm-bioc.r-pkg.org/", "3.22");
        assert_eq!(feed.repo, RepoId::Bioc("3.22".to_string()));
        assert_eq!(
            feed.allpackages_url,
            "https://ppm-bioc.r-pkg.org/3.22/ALLPACKAGES.zst"
        );
        assert_eq!(
            feed.archived_url,
            "https://ppm-bioc.r-pkg.org/3.22/ARCHIVEDPACKAGES.zst"
        );
        assert_eq!(
            feed.binaries_url,
            "https://ppm-bioc.r-pkg.org/3.22/binaries"
        );
    }

    #[test]
    fn repo_id_labels() {
        assert_eq!(RepoId::Cran.to_string(), "cran");
        assert_eq!(RepoId::Bioc("3.22".to_string()).to_string(), "bioc/3.22");
        assert_eq!(
            CranlikeRepo::new("acme", "https://cran.acme.com/")
                .repo_id()
                .to_string(),
            "acme"
        );
    }

    #[test]
    fn feeds_from_metadata_urls() {
        let feed = MetadataFeed::from_metadata_url("https://example.com/cran/", None).unwrap();
        assert_eq!(feed.repo, RepoId::Cran);
        assert_eq!(
            feed.allpackages_url,
            "https://example.com/cran/ALLPACKAGES.zst"
        );
        assert!(MetadataFeed::from_metadata_url("https://example.com/%v", None).is_none());
        let feed = MetadataFeed::from_metadata_url("https://example.com/%v", Some("3.22")).unwrap();
        assert_eq!(feed.repo, RepoId::Bioc("3.22".to_string()));
        assert_eq!(feed.binaries_url, "https://example.com/3.22/binaries");
    }

    #[test]
    fn bioc_feed_comes_first() {
        let feeds = MetadataFeed::for_target(Some("3.22"));
        assert_eq!(feeds[0].repo, RepoId::Bioc("3.22".to_string()));
        assert_eq!(feeds[1].repo, RepoId::Cran);
    }
}
