//! The package metadata the solver reads. There are two kinds of
//! repositories:
//!
//! * Repositories with extended metadata ("feeds", format 1.x of Tidyup 12):
//!   CRAN's, at `https://ppm.r-pkg.org/v1`, and one per Bioconductor release,
//!   at `https://ppm-bioc.r-pkg.org/v1/<bioc-version>`. Every feed has the
//!   same layout: a `METADATA` file with the format version, an
//!   `ALLPACKAGES.zst` history of every version ever published, an
//!   `ARCHIVEDPACKAGES.zst` list of removed packages, and per-package binary
//!   indices at `binaries/<package>.tsv.zst`.
//! * Plain CRAN-like repositories, with `PACKAGES` files for the current
//!   packages only, at `src/contrib` and `bin/<os>/.../contrib/<R version>`.

use std::collections::{HashMap, HashSet};
use std::fmt;

/// Default base URL of CRAN's extended metadata.
pub const PPM_METADATA_URL: &str = "https://ppm.r-pkg.org/v1";

/// Default base URL of the extended metadata of the Bioconductor releases,
/// `%v` is the Bioconductor version.
pub const BIOC_METADATA_URL: &str = "https://ppm-bioc.r-pkg.org/v1/%v";

/// [`BIOC_METADATA_URL`] without the version.
const BIOC_METADATA_BASE: &str = "https://ppm-bioc.r-pkg.org/v1";

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

/// How rig talks to a CRAN-like repository. Both read the `PACKAGES` files;
/// r-universe also has an API with the git commit of each package.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CranlikeProtocol {
    #[default]
    Cran,
    RUniverse,
}

/// A plain CRAN-like repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CranlikeRepo {
    pub name: String,
    /// Without a trailing `/`.
    pub url: String,
    pub protocol: CranlikeProtocol,
}

impl CranlikeRepo {
    pub fn new(name: &str, url: &str) -> CranlikeRepo {
        // So that `RIG_REPO_<NAME>_*` apply to it. This also removes a
        // password from the URL.
        let url = crate::auth::register_repo(name, url);
        CranlikeRepo {
            name: name.to_string(),
            url: url.trim_end_matches('/').to_string(),
            protocol: CranlikeProtocol::Cran,
        }
    }

    /// An r-universe repository.
    pub fn r_universe(name: &str, url: &str) -> CranlikeRepo {
        CranlikeRepo {
            protocol: CranlikeProtocol::RUniverse,
            ..CranlikeRepo::new(name, url)
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

    /// `repos` with a single CRAN feed. A solve has one CRAN: if one of
    /// the feeds is not the default (P3M's) feed, e.g. the own metadata of a
    /// PPM CRAN repository, it replaces the default one. Otherwise the first
    /// CRAN feed wins.
    pub fn one_cran_feed(repos: Vec<PkgRepo>) -> Vec<PkgRepo> {
        let is_cran = |r: &PkgRepo| matches!(r, PkgRepo::Extended(f) if f.repo == RepoId::Cran);
        let default_url = MetadataFeed::cran().allpackages_url;
        let keep = repos
            .iter()
            .position(|r| {
                matches!(r, PkgRepo::Extended(f)
                    if f.repo == RepoId::Cran && f.allpackages_url != default_url)
            })
            .or_else(|| repos.iter().position(is_cran));
        repos
            .into_iter()
            .enumerate()
            .filter(|(i, r)| !is_cran(r) || Some(*i) == keep)
            .map(|(_, r)| r)
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

/// Which repositories a package may come from, on top of the list of
/// repositories of a solve. `rig proj` pins dependencies to a repository with
/// `{ repository = "<name>" }`, and an `explicit` repository only serves the
/// packages pinned to it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoFilter {
    /// Package name to the only repositories it may come from: the ones of
    /// the repository it is pinned to, e.g. several for the root of a
    /// Bioconductor mirror. Empty if that repository is not part of the
    /// solve, e.g. Bioconductor for an R version without a Bioconductor
    /// release: then there are no versions.
    pub pins: HashMap<String, Vec<RepoId>>,
    /// The repositories that only serve the packages pinned to them.
    pub explicit: HashSet<RepoId>,
}

impl RepoFilter {
    /// Whether `package` may come from `repo`.
    pub fn allows(&self, package: &str, repo: &RepoId) -> bool {
        match self.pins.get(package) {
            Some(pin) => pin.contains(repo),
            None => !self.explicit.contains(repo),
        }
    }
}

/// Where a feed's metadata lives.
#[derive(Debug, Clone)]
pub struct MetadataFeed {
    pub repo: RepoId,
    pub allpackages_url: String,
    pub archived_url: String,
    /// Base URL of the per-package binary indices, without a trailing `/`.
    pub binaries_url: String,
    /// A CRAN-like repository with the same packages, e.g. a CRAN mirror,
    /// without a trailing `/`. Source packages are downloaded from here
    /// first, see [`MetadataFeed::with_mirror`].
    pub mirror: Option<String>,
    /// The name of the configured repository of the feed, if it is not one
    /// of the built-in feeds, see [`MetadataFeed::with_name`].
    pub name: Option<String>,
}

/// The mirror and the name are not part of a feed's identity: two feeds with
/// the same metadata are the same repository, wherever their files are
/// downloaded from.
impl PartialEq for MetadataFeed {
    fn eq(&self, other: &MetadataFeed) -> bool {
        self.repo == other.repo
            && self.allpackages_url == other.allpackages_url
            && self.archived_url == other.archived_url
            && self.binaries_url == other.binaries_url
    }
}

impl Eq for MetadataFeed {}

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
                &format!("{}/ALLPACKAGES.zst", PPM_METADATA_URL),
            ),
            archived_url: env(
                "RIG_ARCHIVEDPACKAGES_URL",
                &format!("{}/ARCHIVEDPACKAGES.zst", PPM_METADATA_URL),
            ),
            binaries_url: env(
                "RIG_BINARIES_URL",
                &format!("{}/binaries", PPM_METADATA_URL),
            )
            .trim_end_matches('/')
            .to_string(),
            mirror: None,
            name: None,
        }
    }

    /// This feed, with source packages downloaded from `mirror` first.
    pub fn with_mirror(self, mirror: &str) -> MetadataFeed {
        MetadataFeed {
            mirror: Some(mirror.trim_end_matches('/').to_string()),
            ..self
        }
    }

    /// This feed, of the configured repository `name`. The lock file names
    /// this repository for the versions of a CRAN feed that do not come from
    /// CRAN, e.g. the GitHub packages of a PPM repository that has both, see
    /// [`crate::dcf::is_cran_source`].
    pub fn with_name(self, name: &str) -> MetadataFeed {
        MetadataFeed {
            name: Some(name.to_string()),
            ..self
        }
    }

    /// The feed of Bioconductor release `version`, under
    /// `RIG_BIOC_METADATA_URL` (default `https://ppm-bioc.r-pkg.org/v1`).
    pub fn bioc(version: &str) -> MetadataFeed {
        let base = std::env::var("RIG_BIOC_METADATA_URL")
            .unwrap_or_else(|_| BIOC_METADATA_BASE.to_string());
        MetadataFeed::bioc_at(&base, version)
    }

    /// The base URL of CRAN's extended metadata, see [`MetadataFeed::cran`].
    /// The URL of `ALLPACKAGES.zst` if `RIG_ALLPACKAGES_URL` is not under a
    /// base URL.
    pub fn cran_metadata_url() -> String {
        let url = MetadataFeed::cran().allpackages_url;
        match url.strip_suffix("/ALLPACKAGES.zst") {
            Some(base) => base.to_string(),
            None => url,
        }
    }

    /// The base URL of the Bioconductor releases' extended metadata, with
    /// `%v` for the version, see [`MetadataFeed::bioc`].
    pub fn bioc_metadata_url() -> String {
        match std::env::var("RIG_BIOC_METADATA_URL") {
            Ok(base) => format!("{}/%v", base.trim_end_matches('/')),
            Err(_) => BIOC_METADATA_URL.to_string(),
        }
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
            mirror: None,
            name: None,
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

/// The host that serves the package files of the P3M feeds, see
/// [`p3m_download_url`].
const P3M_DOWNLOAD_URL: &str = "https://p3m.dev";

/// The full URL of `url`, a `DownloadURL` from the metadata file at
/// `file_url`, e.g. ALLPACKAGES, or a `url` from a binary index. It is either
/// a full URL, an absolute path on the same server (`/cran/...`), or a path
/// relative to the file that contains it, as in RFC 3986. A `url` that cannot
/// be resolved is kept as is.
///
/// A path in the P3M feeds is resolved to the P3M server that serves the
/// file, see [`p3m_download_url`].
pub fn resolve_download_url(file_url: &str, url: &str) -> String {
    if url.is_empty() || reqwest::Url::parse(url).is_ok() {
        return url.to_string();
    }
    match reqwest::Url::parse(file_url).and_then(|base| base.join(url)) {
        Ok(full) => p3m_download_url(&full).unwrap_or_else(|| full.to_string()),
        Err(_) => url.to_string(),
    }
}

/// The download URL of a package file of the P3M feeds at
/// `https://ppm.r-pkg.org` and `https://ppm-bioc.r-pkg.org`, from its URL
/// resolved against the metadata. These hosts only serve the metadata, the
/// files are on `p3m.dev`:
///
/// * `https://ppm.r-pkg.org/v1/<path>` is
///   `https://p3m.dev/cran/<path>`, e.g. `<path>` is
///   `2026-04-10/src/contrib/cli_3.6.6.tar.gz`.
/// * `https://ppm-bioc.r-pkg.org/v1/<bioc-version>/<path>` is
///   `https://p3m.dev/bioconductor/<path>`, e.g. `<path>` is
///   `2025-04-17/packages/3.22/bioc/src/contrib/limma_3.65.0.tar.gz`.
///
/// Any other path on these hosts is the same path on `p3m.dev`. `None` for
/// other hosts.
fn p3m_download_url(url: &reqwest::Url) -> Option<String> {
    if url.scheme() != "https" {
        return None;
    }
    let path = url.path();
    let rest = match url.host_str()? {
        "ppm.r-pkg.org" => match path.strip_prefix("/v1/") {
            Some(rest) => format!("/cran/{}", rest),
            None => path.to_string(),
        },
        "ppm-bioc.r-pkg.org" => match path
            .strip_prefix("/v1/")
            .and_then(|rest| rest.split_once('/'))
        {
            Some((_version, rest)) => format!("/bioconductor/{}", rest),
            None => path.to_string(),
        },
        _ => return None,
    };
    let query = url.query().map(|q| format!("?{}", q)).unwrap_or_default();
    Some(format!("{}{}{}", P3M_DOWNLOAD_URL, rest, query))
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
        let feed = MetadataFeed::bioc_at("https://ppm-bioc.r-pkg.org/v1/", "3.22");
        assert_eq!(feed.repo, RepoId::Bioc("3.22".to_string()));
        assert_eq!(
            feed.allpackages_url,
            "https://ppm-bioc.r-pkg.org/v1/3.22/ALLPACKAGES.zst"
        );
        assert_eq!(
            feed.archived_url,
            "https://ppm-bioc.r-pkg.org/v1/3.22/ARCHIVEDPACKAGES.zst"
        );
        assert_eq!(
            feed.binaries_url,
            "https://ppm-bioc.r-pkg.org/v1/3.22/binaries"
        );
    }

    #[test]
    fn own_cran_feed_replaces_the_default() {
        let own = PkgRepo::Extended(MetadataFeed::at(RepoId::Cran, "http://localhost:4242/cran"));
        let default = PkgRepo::Extended(MetadataFeed::cran());
        let bioc = PkgRepo::Extended(MetadataFeed::bioc("3.22"));
        let other = PkgRepo::Cranlike(CranlikeRepo::new("acme", "https://acme.com"));

        let repos = vec![default.clone(), other.clone(), own.clone(), bioc.clone()];
        assert_eq!(
            PkgRepo::one_cran_feed(repos),
            vec![other.clone(), own.clone(), bioc.clone()]
        );

        let repos = vec![bioc.clone(), default.clone(), default.clone()];
        assert_eq!(PkgRepo::one_cran_feed(repos), vec![bioc, default]);

        assert_eq!(PkgRepo::one_cran_feed(vec![other.clone()]), vec![other]);
    }

    #[test]
    fn relative_ppm_download_urls_resolve() {
        let feed = MetadataFeed::at(RepoId::Cran, "http://localhost:4242/cran");
        assert_eq!(
            resolve_download_url(
                &feed.allpackages_url,
                "2025-08-30/src/contrib/otel_0.2.0.tar.gz"
            ),
            "http://localhost:4242/cran/2025-08-30/src/contrib/otel_0.2.0.tar.gz"
        );
    }

    #[test]
    fn download_urls_resolve_against_allpackages() {
        let feed = MetadataFeed::at(RepoId::Cran, "https://example.com/meta/");
        assert_eq!(
            resolve_download_url(
                &feed.allpackages_url,
                "https://p3m.dev/cran/2020-01-01/src/contrib/a_1.0.tar.gz"
            ),
            "https://p3m.dev/cran/2020-01-01/src/contrib/a_1.0.tar.gz"
        );
        assert_eq!(
            resolve_download_url(&feed.allpackages_url, "src/contrib/a_1.0.tar.gz"),
            "https://example.com/meta/src/contrib/a_1.0.tar.gz"
        );
        assert_eq!(
            resolve_download_url(&feed.allpackages_url, "../cran/a_1.0.tar.gz"),
            "https://example.com/cran/a_1.0.tar.gz"
        );
        assert_eq!(
            resolve_download_url(
                &feed.allpackages_url,
                "/cran/2020-01-01/src/contrib/a_1.0.tar.gz"
            ),
            "https://example.com/cran/2020-01-01/src/contrib/a_1.0.tar.gz"
        );
        assert_eq!(resolve_download_url(&feed.allpackages_url, ""), "");
    }

    #[test]
    fn binary_index_urls_resolve_against_the_index() {
        let index = "https://example.com/meta/v1/binaries/cli.tsv.zst";
        assert_eq!(
            resolve_download_url(index, "../2017-11-07/src/contrib/cli_1.0.0.tar.gz"),
            "https://example.com/meta/v1/2017-11-07/src/contrib/cli_1.0.0.tar.gz"
        );
    }

    /// The P3M feeds only serve metadata, their files are on `p3m.dev`.
    #[test]
    fn p3m_feed_urls_resolve_to_p3m_dev() {
        let cran = MetadataFeed::at(RepoId::Cran, PPM_METADATA_URL);
        assert_eq!(
            resolve_download_url(
                &cran.allpackages_url,
                "2026-04-10/src/contrib/cli_3.6.6.tar.gz"
            ),
            "https://p3m.dev/cran/2026-04-10/src/contrib/cli_3.6.6.tar.gz"
        );
        assert_eq!(
            resolve_download_url(
                &format!("{}/cli.tsv.zst", cran.binaries_url),
                "../2026-04-27/bin/macosx/big-sur-arm64/contrib/4.5/pak_0.9.5.tgz"
            ),
            "https://p3m.dev/cran/2026-04-27/bin/macosx/big-sur-arm64/contrib/4.5/pak_0.9.5.tgz"
        );
        let bioc = MetadataFeed::at(
            RepoId::Bioc("3.22".to_string()),
            "https://ppm-bioc.r-pkg.org/v1/3.22",
        );
        let path = "2025-04-17/packages/3.22/bioc/src/contrib/limma_3.65.0.tar.gz";
        let want = format!("https://p3m.dev/bioconductor/{}", path);
        assert_eq!(resolve_download_url(&bioc.allpackages_url, path), want);
        assert_eq!(
            resolve_download_url(
                &format!("{}/limma.tsv.zst", bioc.binaries_url),
                &format!("../{}", path)
            ),
            want
        );
        // An absolute path keeps its path, on p3m.dev.
        assert_eq!(
            resolve_download_url(&cran.allpackages_url, "/cran/2026-04-10/x.tar.gz"),
            "https://p3m.dev/cran/2026-04-10/x.tar.gz"
        );
        // Full URLs are kept as they are.
        assert_eq!(
            resolve_download_url(&cran.allpackages_url, "https://ppm.r-pkg.org/v1/x.tar.gz"),
            "https://ppm.r-pkg.org/v1/x.tar.gz"
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
