//! The package metadata feeds the solver reads: CRAN's, at
//! `https://ppm.r-pkg.org`, and one per Bioconductor release, at
//! `https://ppm-bioc.r-pkg.org/<bioc-version>`. Every feed has the same
//! layout: an `ALLPACKAGES.zst` history of every version ever published, an
//! `ARCHIVEDPACKAGES.zst` list of removed packages, and per-package binary
//! indices at `binaries/<package>.tsv.zst`.

use std::fmt;

/// The repository a package version comes from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RepoId {
    Cran,
    /// A Bioconductor release, e.g. `Bioc("3.22")`.
    Bioc(String),
}

impl RepoId {
    pub fn is_bioc(&self) -> bool {
        matches!(self, RepoId::Bioc(_))
    }
}

/// `cran` or `bioc/<version>`, as recorded in the lock file.
impl fmt::Display for RepoId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RepoId::Cran => write!(f, "cran"),
            RepoId::Bioc(v) => write!(f, "bioc/{}", v),
        }
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
        MetadataFeed {
            repo: RepoId::Bioc(version.to_string()),
            allpackages_url: format!("{}/ALLPACKAGES.zst", base),
            archived_url: format!("{}/ARCHIVEDPACKAGES.zst", base),
            binaries_url: format!("{}/binaries", base),
        }
    }

    /// The feeds of a target: CRAN, and Bioconductor release `bioc`, if any.
    pub fn for_target(bioc: Option<&str>) -> Vec<MetadataFeed> {
        let mut feeds = vec![MetadataFeed::cran()];
        if let Some(v) = bioc {
            feeds.push(MetadataFeed::bioc(v));
        }
        feeds
    }
}

/// Whether and which Bioconductor release a solve uses, besides CRAN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BiocSetting {
    pub enabled: bool,
    /// A pinned release, instead of the one that belongs to the R version.
    pub version: Option<String>,
    /// Packages that must come from Bioconductor, e.g. from a `bioc::`
    /// reference, even if CRAN has them, too.
    pub only: std::collections::BTreeSet<String>,
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
            only: Default::default(),
        }
    }
}

impl BiocSetting {
    pub fn disabled() -> Self {
        BiocSetting {
            enabled: false,
            version: None,
            only: Default::default(),
        }
    }

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
    }
}
