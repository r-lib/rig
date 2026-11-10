//! Working out the type of a package repository from its URL.
//!
//! The types:
//!
//! * `cran`: a CRAN mirror, with `src/contrib/Meta/archive.rds`.
//! * `ppm-cran`, `ppm-bioc`: the (non-curated) CRAN and Bioconductor
//!   repositories of a Posit Package Manager (PPM) instance, at `latest`.
//! * `ppm-other`: any other R repository of a PPM instance: curated CRAN, git,
//!   local, or a mix of sources.
//! * `ppm-snapshot`: a PPM repository at a fixed snapshot (date or id).
//! * `ppm`: the root of a PPM instance.
//! * `bioc`: a Bioconductor mirror, either its root, or one of its
//!   repositories (`packages/<version>/bioc`, etc.).
//! * `extended`: a repository of extended metadata (`ALLPACKAGES.zst`), for
//!   CRAN or for the Bioconductor releases.
//! * `r-universe`: an r-universe repository.
//! * `cran-like`: anything else.
//!
//! Detection sends a few `HEAD` requests to well-known paths below the URL,
//! all at once, and looks at the status codes and the `x-repository-type`
//! header. PPM sends `x-repository-type: RSPM` on every response, r-universe
//! sends `x-repository-type: r-universe`. For PPM, the repository list and the
//! sources of the repository come from PPM's API, `<ppm>/__api__/repos` and
//! `<ppm>/__api__/repos/<id>/sources`.
//!
//! The decision itself is in pure functions ([`classify`], [`classify_ppm`]),
//! which get the probe results as input, so they are tested without network.

use std::collections::{HashMap, HashSet};
use std::error::Error;

use futures::future;
use log::debug;
use regex::Regex;
use reqwest::Url;
use serde::{Deserialize, Serialize};

use crate::download::probe_url;
use crate::utils::{http_client, http_client_builder};

use super::feed::{BIOC_METADATA_URL, PPM_METADATA_URL};
use super::latest_bioc_release;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RepoType {
    Cran,
    PpmCran,
    PpmBioc,
    PpmOther,
    PpmSnapshot,
    Ppm,
    Bioc,
    Extended,
    RUniverse,
    CranLike,
}

impl RepoType {
    pub fn as_str(&self) -> &'static str {
        match self {
            RepoType::Cran => "cran",
            RepoType::PpmCran => "ppm-cran",
            RepoType::PpmBioc => "ppm-bioc",
            RepoType::PpmOther => "ppm-other",
            RepoType::PpmSnapshot => "ppm-snapshot",
            RepoType::Ppm => "ppm",
            RepoType::Bioc => "bioc",
            RepoType::Extended => "extended",
            RepoType::RUniverse => "r-universe",
            RepoType::CranLike => "cran-like",
        }
    }

    pub const ALL: [RepoType; 10] = [
        RepoType::Cran,
        RepoType::PpmCran,
        RepoType::PpmBioc,
        RepoType::PpmOther,
        RepoType::PpmSnapshot,
        RepoType::Ppm,
        RepoType::Bioc,
        RepoType::Extended,
        RepoType::RUniverse,
        RepoType::CranLike,
    ];
}

impl std::str::FromStr for RepoType {
    type Err = String;

    fn from_str(s: &str) -> Result<RepoType, String> {
        RepoType::ALL
            .into_iter()
            .find(|t| t.as_str() == s)
            .ok_or_else(|| {
                let all: Vec<&str> = RepoType::ALL.iter().map(|t| t.as_str()).collect();
                format!(
                    "Unknown repository type: '{}', must be one of {}",
                    s,
                    all.join(", ")
                )
            })
    }
}

/// The result of the detection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectedRepo {
    /// The URL, without a trailing `/`.
    pub url: String,
    #[serde(rename = "type")]
    pub repo_type: RepoType,
    /// Whether rig can use extended metadata for this repository.
    pub extensible: bool,
    /// Base URL of the extended metadata. May contain `%v`, the Bioconductor
    /// version. For an `extended` repository this is the URL itself.
    pub metadata: Option<String>,
    /// The root of the PPM instance.
    pub ppm_base: Option<String>,
    /// The name of the PPM repository.
    pub ppm_repo: Option<String>,
    /// The Linux distribution of a PPM binary URL (`__linux__/<distro>`).
    pub distro: Option<String>,
    /// The PPM snapshot: `latest`, a date or a snapshot id.
    pub snapshot: Option<String>,
    pub bioc_version: Option<String>,
    /// The Bioconductor repository, e.g. `bioc` or `data/annotation`, if the
    /// URL is one Bioconductor repository and not the root of a mirror.
    pub bioc_subrepo: Option<String>,
    /// `cran` or `bioc`, for `extended` repositories.
    pub extended_flavor: Option<String>,
    /// The type is a guess from the URL, because the PPM API was not
    /// available.
    pub guessed: bool,
    /// For the root of a PPM instance (`ppm`): all its repositories.
    pub ppm_repos: Vec<PpmRepoInfo>,
    pub notes: Vec<String>,
}

/// One repository of a PPM instance, in the `ppm_repos` of the PPM root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PpmRepoInfo {
    pub name: String,
    /// PPM's own type of the repository: `R` or `Bioconductor`.
    pub ppm_type: String,
    pub hidden: bool,
    /// `ppm-cran`, `ppm-bioc` or `ppm-other`.
    #[serde(rename = "type")]
    pub repo_type: RepoType,
    pub extensible: bool,
    pub metadata: Option<String>,
    /// The URL of the repository at `latest`.
    pub url: String,
    /// The type is a guess from the name, because the sources of the
    /// repository were not available.
    pub guessed: bool,
}

impl DetectedRepo {
    pub(crate) fn new(url: &str, repo_type: RepoType) -> DetectedRepo {
        let (extensible, metadata) = match repo_type {
            RepoType::Extended => (false, Some(url.to_string())),
            _ => extension_of(repo_type),
        };
        DetectedRepo {
            url: url.to_string(),
            repo_type,
            extensible,
            metadata,
            ppm_base: None,
            ppm_repo: None,
            distro: None,
            snapshot: None,
            bioc_version: None,
            bioc_subrepo: None,
            extended_flavor: None,
            guessed: false,
            ppm_repos: vec![],
            notes: vec![],
        }
    }

    /// A single Bioconductor repository is a plain CRAN-like repository.
    fn with_bioc_subrepo(mut self, version: &str, subrepo: &str) -> DetectedRepo {
        self.bioc_version = Some(version.to_string());
        self.bioc_subrepo = Some(subrepo.to_string());
        self.extensible = false;
        self.metadata = None;
        self
    }
}

/// Whether a repository of type `repo_type` is extensible, and the base URL
/// of its extended metadata.
fn extension_of(repo_type: RepoType) -> (bool, Option<String>) {
    match repo_type {
        RepoType::Cran | RepoType::PpmCran => (true, Some(PPM_METADATA_URL.to_string())),
        RepoType::PpmBioc | RepoType::Bioc => (true, Some(BIOC_METADATA_URL.to_string())),
        _ => (false, None),
    }
}

/// Whether a live PPM repository `name` of type `repo_type` is extensible,
/// and the base URL of its extended metadata. A CRAN repository that serves
/// its own extended metadata uses that, instead of P3M's.
fn ppm_extension_of(
    base: &str,
    name: &str,
    repo_type: RepoType,
    api: &PpmApi,
) -> (bool, Option<String>) {
    if repo_type == RepoType::PpmCran && api.own_metadata.contains(name) {
        return (true, Some(format!("{}/{}", base, name)));
    }
    extension_of(repo_type)
}

// ------------------------------------------------------------------------
// Probes
// ------------------------------------------------------------------------

/// What a probe of one path found.
#[derive(Debug, Clone, Default)]
pub struct Probe {
    /// HTTP status, `None` if there was no response.
    pub status: Option<u16>,
    /// The `x-repository-type` header.
    pub repository_type: Option<String>,
}

impl Probe {
    fn ok(&self) -> bool {
        matches!(self.status, Some(200..=299))
    }

    fn has_type(&self, rtype: &str) -> bool {
        self.repository_type
            .as_deref()
            .is_some_and(|t| t.eq_ignore_ascii_case(rtype))
    }
}

/// The probes of [`classify`], in the order of [`probe_paths`].
#[derive(Debug, Clone, Default)]
pub struct Probes {
    pub packages: Probe,
    pub allpackages: Probe,
    pub bioc_allpackages: Probe,
    pub archive: Probe,
    pub bioc_root: Probe,
}

impl Probes {
    fn all(&self) -> [&Probe; 5] {
        [
            &self.packages,
            &self.allpackages,
            &self.bioc_allpackages,
            &self.archive,
            &self.bioc_root,
        ]
    }

    fn is_ppm(&self) -> bool {
        self.all().iter().any(|p| p.has_type("RSPM"))
    }

    fn is_r_universe(&self) -> bool {
        self.all().iter().any(|p| p.has_type("r-universe"))
    }
}

/// The paths to probe below the repository URL, for the Bioconductor version
/// `bioc_version`. The order is that of the fields of [`Probes`].
fn probe_paths(bioc_version: &str) -> Vec<String> {
    vec![
        "src/contrib/PACKAGES".to_string(),
        "ALLPACKAGES.zst".to_string(),
        format!("{}/ALLPACKAGES.zst", bioc_version),
        "src/contrib/Meta/archive.rds".to_string(),
        format!("packages/{}/bioc/src/contrib/PACKAGES", bioc_version),
    ]
}

fn probes_from_vec(mut v: Vec<Probe>) -> Probes {
    v.resize(5, Probe::default());
    let mut it = v.into_iter();
    Probes {
        packages: it.next().unwrap(),
        allpackages: it.next().unwrap(),
        bioc_allpackages: it.next().unwrap(),
        archive: it.next().unwrap(),
        bioc_root: it.next().unwrap(),
    }
}

/// Whether a probe of the file at `url` was redirected to something that is
/// not that file. Some servers redirect a missing file to a page that exists,
/// e.g. `https://www.rforge.net/ALLPACKAGES.zst` to
/// `https://www.rforge.net/ALLPACKAGES.zst/`, which answers 200. The file is
/// only found if the path of the final URL still ends with its file name.
fn redirected_away(url: &str, final_url: Option<&str>) -> bool {
    let final_url = match final_url {
        Some(f) if f != url => f,
        _ => return false,
    };
    let file = url.rsplit('/').next().unwrap_or("");
    match Url::parse(final_url) {
        Ok(u) => !u.path().ends_with(&format!("/{}", file)),
        Err(_) => false,
    }
}

// ------------------------------------------------------------------------
// URL paths
// ------------------------------------------------------------------------

/// The `/`-separated, non-empty segments of the path of `url`.
fn path_segments(url: &str) -> Vec<String> {
    match Url::parse(url) {
        Ok(u) => u
            .path()
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect(),
        Err(_) => vec![],
    }
}

fn is_bioc_version(s: &str) -> bool {
    lazy_static::lazy_static! {
        static ref RE: Regex = Regex::new(r"^\d+\.\d+$").unwrap();
    }
    RE.is_match(s)
}

/// If the path of `url` ends in `packages/<version>/<repo>`, a single
/// Bioconductor repository, then the version and the repository.
fn parse_bioc_subrepo_path(url: &str) -> Option<(String, String)> {
    lazy_static::lazy_static! {
        static ref RE: Regex = Regex::new(
            r"(?:^|/)packages/(\d+\.\d+)/(bioc|data/annotation|data/experiment|workflows|books)$"
        ).unwrap();
    }
    let path = path_segments(url).join("/");
    RE.captures(&path)
        .map(|c| (c[1].to_string(), c[2].to_string()))
}

/// If the path of `url` ends in `packages/<version>`, the version.
fn parse_bioc_version_path(url: &str) -> Option<String> {
    let segs = path_segments(url);
    match segs.as_slice() {
        [.., p, v] if p == "packages" && is_bioc_version(v) => Some(v.clone()),
        _ => None,
    }
}

/// A PPM snapshot that does not move: a date or a snapshot id. `latest` is
/// not one.
fn is_fixed_snapshot(s: &str) -> bool {
    lazy_static::lazy_static! {
        static ref RE: Regex = Regex::new(r"^(\d{4}-\d{2}-\d{2}|\d+)$").unwrap();
    }
    RE.is_match(s)
}

/// The parts of a PPM URL below the root of the PPM instance:
/// `<repo>[/__linux__/<distro>][/<snapshot>][/packages/<version>/<subrepo>]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PpmPath {
    pub repo: Option<String>,
    pub distro: Option<String>,
    pub snapshot: Option<String>,
    pub bioc_version: Option<String>,
    pub bioc_subrepo: Option<String>,
}

fn parse_ppm_path(rest: &[String]) -> PpmPath {
    let mut path = PpmPath::default();
    let mut it = rest.iter().peekable();
    path.repo = it.next().cloned();
    if it.peek().is_some_and(|s| *s == "__linux__") {
        it.next();
        path.distro = it.next().cloned();
    }
    if it.peek().is_some_and(|s| *s != "packages") {
        path.snapshot = it.next().cloned();
    }
    if it.peek().is_some_and(|s| *s == "packages") {
        it.next();
        path.bioc_version = it.next().cloned();
        let sub: Vec<&str> = it.map(|s| s.as_str()).collect();
        if !sub.is_empty() {
            path.bioc_subrepo = Some(sub.join("/"));
        }
    }
    path
}

/// Split a PPM URL into the root of the PPM instance and the rest of the
/// path. If `base` is known (from the API), that is the root. Otherwise the
/// root is guessed: the repository name is the first segment that is followed
/// by `__linux__`, a snapshot or `packages`. Without such a segment the whole
/// URL is taken as the root.
fn split_ppm_url(url: &str, base: Option<&str>) -> (String, Vec<String>) {
    let segs = path_segments(url);
    if let Some(base) = base {
        let nbase = path_segments(base).len();
        return (base.to_string(), segs[nbase.min(segs.len())..].to_vec());
    }
    let marker =
        |s: &str| s == "__linux__" || s == "latest" || s == "packages" || is_fixed_snapshot(s);
    match (0..segs.len().saturating_sub(1)).find(|&i| marker(&segs[i + 1])) {
        Some(i) => (url_with_path(url, &segs[..i]), segs[i..].to_vec()),
        None => (url.to_string(), vec![]),
    }
}

/// `url` with its path replaced by `segs`, without a trailing `/`.
fn url_with_path(url: &str, segs: &[String]) -> String {
    match Url::parse(url) {
        Ok(mut u) => {
            u.set_path(&segs.join("/"));
            u.set_query(None);
            u.set_fragment(None);
            u.to_string().trim_end_matches('/').to_string()
        }
        Err(_) => url.to_string(),
    }
}

// ------------------------------------------------------------------------
// Classification
// ------------------------------------------------------------------------

/// Classify a repository that is not on PPM, from its probes. The caller
/// handles PPM, see [`Probes::is_ppm`] and [`classify_ppm`].
fn classify(url: &str, probes: &Probes) -> DetectedRepo {
    if probes.is_r_universe() {
        return DetectedRepo::new(url, RepoType::RUniverse);
    }

    if probes.allpackages.ok() {
        let mut d = DetectedRepo::new(url, RepoType::Extended);
        // `https://ppm-bioc.r-pkg.org/3.22` is one Bioconductor release.
        match path_segments(url).last() {
            Some(v) if is_bioc_version(v) => {
                d.extended_flavor = Some("bioc".to_string());
                d.bioc_version = Some(v.clone());
            }
            _ => d.extended_flavor = Some("cran".to_string()),
        }
        return d;
    }
    if probes.bioc_allpackages.ok() {
        let mut d = DetectedRepo::new(url, RepoType::Extended);
        d.extended_flavor = Some("bioc".to_string());
        return d;
    }

    if let Some((version, subrepo)) = parse_bioc_subrepo_path(url) {
        return DetectedRepo::new(url, RepoType::Bioc).with_bioc_subrepo(&version, &subrepo);
    }
    if let Some(version) = parse_bioc_version_path(url) {
        let mut d = DetectedRepo::new(url, RepoType::Bioc);
        d.bioc_version = Some(version);
        return d;
    }
    if probes.bioc_root.ok() {
        return DetectedRepo::new(url, RepoType::Bioc);
    }

    if probes.archive.ok() {
        return DetectedRepo::new(url, RepoType::Cran);
    }

    let mut d = DetectedRepo::new(url, RepoType::CranLike);
    if !probes.packages.ok() {
        d.notes.push(match probes.packages.status {
            Some(st) => format!("src/contrib/PACKAGES returned HTTP status {}", st),
            None => "src/contrib/PACKAGES did not respond".to_string(),
        });
    }
    d
}

/// A repository of a PPM instance, from its API.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PpmRepo {
    pub id: i64,
    pub name: String,
    /// `R`, `Bioconductor`, `Python`, etc.
    #[serde(rename = "type")]
    pub repo_type: String,
    #[serde(default)]
    pub hidden: bool,
}

impl PpmRepo {
    fn is_r(&self) -> bool {
        self.repo_type == "R" || self.repo_type == "Bioconductor"
    }

    fn is_bioc(&self) -> bool {
        self.repo_type == "Bioconductor"
    }
}

/// A source of a PPM repository, from its API.
#[derive(Debug, Clone, Deserialize)]
struct PpmSource {
    #[serde(rename = "type")]
    source_type: i64,
}

/// PPM source types.
const PPM_SOURCE_CRAN: i64 = 1;
const PPM_SOURCE_BIOC: i64 = 7;

/// `sources` is not empty and every source is of type `stype`.
fn all_of(sources: &[i64], stype: i64) -> bool {
    !sources.is_empty() && sources.iter().all(|&s| s == stype)
}

/// What the PPM API said about a PPM URL.
#[derive(Debug, Clone, Default)]
pub struct PpmApi {
    /// The root of the PPM instance, where the API is.
    pub base: Option<String>,
    /// All repositories of the instance, `None` if the API was not available.
    pub repos: Option<Vec<PpmRepo>>,
    /// The source types of repositories, by repository id. Only for the
    /// repository of the URL, or for all R repositories of a PPM root URL.
    pub sources: HashMap<i64, Vec<i64>>,
    /// The names of the repositories, of the same ones as `sources`, that
    /// serve their own extended metadata, `<base>/<name>/ALLPACKAGES.zst`.
    /// This metadata has the repository's policy: blocked versions and known
    /// vulnerabilities.
    pub own_metadata: HashSet<String>,
}

/// The type of a live (not snapshot) PPM R repository, from its sources, and
/// whether it is a guess, because the sources are not known.
fn ppm_repo_type(name: &str, is_bioc: bool, sources: Option<&Vec<i64>>) -> (RepoType, bool) {
    match sources {
        // A Bioconductor repository has one source per Bioconductor release.
        Some(src) if all_of(src, PPM_SOURCE_CRAN) => (RepoType::PpmCran, false),
        Some(src) if all_of(src, PPM_SOURCE_BIOC) => (RepoType::PpmBioc, false),
        Some(_) => (RepoType::PpmOther, false),
        None if is_bioc => (RepoType::PpmBioc, true),
        None if name == "cran" => (RepoType::PpmCran, true),
        None => (RepoType::PpmOther, true),
    }
}

/// The R and Bioconductor repositories of a PPM instance at `base`, for the
/// PPM root. Other repositories (Python, etc.) are left out.
fn ppm_repo_list(base: &str, repos: &[PpmRepo], api: &PpmApi) -> Vec<PpmRepoInfo> {
    repos
        .iter()
        .filter(|r| r.is_r())
        .map(|r| {
            let (repo_type, guessed) = ppm_repo_type(&r.name, r.is_bioc(), api.sources.get(&r.id));
            let (extensible, metadata) = ppm_extension_of(base, &r.name, repo_type, api);
            PpmRepoInfo {
                name: r.name.clone(),
                ppm_type: r.repo_type.clone(),
                hidden: r.hidden,
                repo_type,
                extensible,
                metadata,
                url: format!("{}/{}/latest", base, r.name),
                guessed,
            }
        })
        .collect()
}

/// Classify a URL of a PPM instance.
fn classify_ppm(url: &str, api: &PpmApi) -> Result<DetectedRepo, Box<dyn Error>> {
    let (base, rest) = split_ppm_url(url, api.base.as_deref());
    let path = parse_ppm_path(&rest);

    let mk = |rtype: RepoType| {
        let mut d = DetectedRepo::new(url, rtype);
        d.ppm_base = Some(base.clone());
        d.ppm_repo = path.repo.clone();
        d.distro = path.distro.clone();
        d.snapshot = path.snapshot.clone();
        if let Some(v) = &path.bioc_version {
            d.bioc_version = Some(v.clone());
        }
        if let Some(sub) = &path.bioc_subrepo {
            let v = path.bioc_version.clone().unwrap_or_default();
            d = d.with_bioc_subrepo(&v, sub);
        }
        d
    };

    let Some(name) = &path.repo else {
        let mut d = mk(RepoType::Ppm);
        match &api.repos {
            Some(repos) => {
                d.ppm_repos = ppm_repo_list(&base, repos, api);
                // Extensible if at least one of its repositories is.
                d.extensible = d.ppm_repos.iter().any(|r| r.extensible);
            }
            None => {
                d.guessed = true;
                d.notes
                    .push("PPM API is not available, cannot list the repositories".to_string());
            }
        }
        return Ok(d);
    };

    let repo = match &api.repos {
        Some(repos) => match repos.iter().find(|r| &r.name == name) {
            Some(r) => Some(r),
            None => bail!("PPM at {} has no repository called '{}'", base, name),
        },
        None => None,
    };
    if let Some(repo) = repo {
        if !repo.is_r() {
            bail!(
                "PPM repository '{}' is a {} repository, not an R package repository",
                name,
                repo.repo_type
            );
        }
    }
    let is_bioc = match repo {
        Some(r) => r.is_bioc(),
        None => name == "bioconductor",
    };

    match path.snapshot.as_deref() {
        Some(s) if s != "latest" => {
            let mut d = mk(RepoType::PpmSnapshot);
            if !is_fixed_snapshot(s) {
                d.notes.push(format!("Unknown PPM snapshot: '{}'", s));
            }
            return Ok(d);
        }
        // No snapshot is the same as `latest`.
        _ => {}
    }

    let sources = repo.and_then(|r| api.sources.get(&r.id));
    let (rtype, guessed) = ppm_repo_type(name, is_bioc, sources);
    let mut d = mk(rtype);
    if rtype == RepoType::PpmCran {
        (d.extensible, d.metadata) = ppm_extension_of(&base, name, rtype, api);
    }
    d.guessed = guessed;
    if guessed {
        d.notes
            .push("PPM API is not available, the type is a guess from the URL".to_string());
    }
    Ok(d)
}

/// A repository of a known type `rtype` at `url`, with the details that
/// follow from the URL alone, without network access: the parts of a PPM
/// URL, the Bioconductor version and repository, and the flavor of extended
/// metadata. The repositories of a PPM root are not known this way.
pub(crate) fn describe(url: &str, rtype: RepoType) -> DetectedRepo {
    let url = normalize_url(url);
    let mut d = DetectedRepo::new(&url, rtype);
    match rtype {
        RepoType::PpmCran
        | RepoType::PpmBioc
        | RepoType::PpmOther
        | RepoType::PpmSnapshot
        | RepoType::Ppm => {
            let (base, rest) = split_ppm_url(&url, None);
            let path = parse_ppm_path(&rest);
            d.ppm_base = Some(base);
            d.ppm_repo = path.repo;
            d.distro = path.distro;
            d.snapshot = path.snapshot;
            if let Some(v) = &path.bioc_version {
                d.bioc_version = Some(v.clone());
            }
            if let Some(sub) = &path.bioc_subrepo {
                let v = path.bioc_version.unwrap_or_default();
                d = d.with_bioc_subrepo(&v, sub);
            }
        }
        RepoType::Bioc => {
            if let Some((version, subrepo)) = parse_bioc_subrepo_path(&url) {
                d = d.with_bioc_subrepo(&version, &subrepo);
            } else if let Some(version) = parse_bioc_version_path(&url) {
                d.bioc_version = Some(version);
            }
        }
        RepoType::Extended => match path_segments(&url).last() {
            Some(v) if is_bioc_version(v) => {
                d.extended_flavor = Some("bioc".to_string());
                d.bioc_version = Some(v.clone());
            }
            _ if url.to_lowercase().contains("bioc") => {
                d.extended_flavor = Some("bioc".to_string());
            }
            _ => d.extended_flavor = Some("cran".to_string()),
        },
        RepoType::Cran | RepoType::RUniverse | RepoType::CranLike => {}
    }
    d
}

// ------------------------------------------------------------------------
// Network
// ------------------------------------------------------------------------

fn normalize_url(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

/// Probe the paths of [`probe_paths`] on disk, for a `file://` URL.
fn probe_files(url: &str, paths: &[String]) -> Result<Probes, Box<dyn Error>> {
    let root = match Url::parse(url).ok().and_then(|u| u.to_file_path().ok()) {
        Some(p) => p,
        None => bail!("Invalid file URL: {}", url),
    };
    let probes = paths
        .iter()
        .map(|p| Probe {
            status: Some(if root.join(p).exists() { 200 } else { 404 }),
            repository_type: None,
        })
        .collect();
    Ok(probes_from_vec(probes))
}

async fn get_json<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
) -> Option<T> {
    let resp = crate::auth::get(client, url).send().await.ok()?;
    if !resp.status().is_success() {
        debug!("{} returned HTTP status {}", url, resp.status());
        return None;
    }
    resp.json::<T>().await.ok()
}

/// Ask the PPM API about `url`. The API is at the root of the instance, which
/// may be below a path on the server, so every prefix of the path is tried.
async fn ppm_api(client: &reqwest::Client, url: &str) -> PpmApi {
    let segs = path_segments(url);
    let bases: Vec<String> = (0..=segs.len())
        .map(|k| url_with_path(url, &segs[..k]))
        .collect();
    let found = future::join_all(bases.iter().map(|b| async move {
        get_json::<Vec<PpmRepo>>(client, &format!("{}/__api__/repos", b)).await
    }))
    .await;

    let Some((base, repos)) = bases
        .into_iter()
        .zip(found)
        .find_map(|(b, r)| r.map(|r| (b, r)))
    else {
        return PpmApi::default();
    };

    // The sources of the repository of the URL, or of every R repository for
    // the PPM root.
    let (_, rest) = split_ppm_url(url, Some(&base));
    let path = parse_ppm_path(&rest);
    let wanted: Vec<&PpmRepo> = match &path.repo {
        Some(name) => repos.iter().filter(|r| &r.name == name).collect(),
        None => repos.iter().filter(|r| r.is_r()).collect(),
    };
    let found = future::join_all(wanted.iter().map(|r| {
        let surl = format!("{}/__api__/repos/{}/sources", base, r.id);
        async move { get_json::<Vec<PpmSource>>(client, &surl).await }
    }))
    .await;
    let sources = wanted
        .iter()
        .zip(found)
        .filter_map(|(r, s)| s.map(|s| (r.id, s.iter().map(|s| s.source_type).collect())))
        .collect();

    let found = future::join_all(wanted.iter().map(|r| {
        let murl = format!("{}/{}/ALLPACKAGES.zst", base, r.name);
        async move { probe_url(client, &murl).await }
    }))
    .await;
    let own_metadata = wanted
        .iter()
        .zip(found)
        .filter(|(_, p)| matches!(p.status, Some(200..=299)))
        .map(|(r, _)| r.name.clone())
        .collect();

    PpmApi {
        base: Some(base),
        repos: Some(repos),
        sources,
        own_metadata,
    }
}

/// Detect the type of the repository at `url`.
#[tokio::main]
pub async fn detect_repo_type(url: &str) -> Result<DetectedRepo, Box<dyn Error>> {
    let url = normalize_url(url);
    let bioc_version = match latest_bioc_release() {
        Some(v) => v,
        None => bail!("Cannot determine the current Bioconductor version"),
    };
    let paths = probe_paths(&bioc_version);

    if url.to_lowercase().starts_with("file://") {
        return Ok(classify(&url, &probe_files(&url, &paths)?));
    }
    if Url::parse(&url).is_err() {
        bail!("Invalid repository URL: {}", url);
    }

    let client = http_client_builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_else(|_| http_client());
    let client = &client;
    let probes = future::join_all(paths.iter().map(|p| {
        let purl = format!("{}/{}", url, p);
        async move {
            let r = probe_url(client, &purl).await;
            Probe {
                status: redirected_away(&purl, r.final_url.as_deref())
                    .then_some(404)
                    .or(r.status),
                repository_type: r.repository_type,
            }
        }
    }))
    .await;
    let probes = probes_from_vec(probes);

    if probes.is_ppm() {
        let api = ppm_api(client, &url).await;
        classify_ppm(&url, &api)
    } else {
        Ok(classify(&url, &probes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_to_another_page_is_not_found() {
        let url = "https://www.rforge.net/ALLPACKAGES.zst";
        assert!(redirected_away(
            url,
            Some("https://www.rforge.net/ALLPACKAGES.zst/")
        ));
        assert!(redirected_away(
            url,
            Some("https://www.rforge.net/index.html")
        ));
        assert!(!redirected_away(url, Some(url)));
        assert!(!redirected_away(url, None));
        // A redirect to the same file elsewhere, e.g. a CDN, is fine.
        assert!(!redirected_away(
            url,
            Some("https://cdn.example.com/rforge/ALLPACKAGES.zst")
        ));
        assert!(!redirected_away(
            "https://example.com/src/contrib/PACKAGES",
            Some("https://mirror.example.com/cran/src/contrib/PACKAGES?token=x")
        ));
    }

    fn ok() -> Probe {
        Probe {
            status: Some(200),
            repository_type: None,
        }
    }

    fn not_found() -> Probe {
        Probe {
            status: Some(404),
            repository_type: None,
        }
    }

    fn probes(packages: Probe, allp: Probe, ballp: Probe, archive: Probe, broot: Probe) -> Probes {
        Probes {
            packages,
            allpackages: allp,
            bioc_allpackages: ballp,
            archive,
            bioc_root: broot,
        }
    }

    fn nothing() -> Probes {
        probes(
            not_found(),
            not_found(),
            not_found(),
            not_found(),
            not_found(),
        )
    }

    fn s(x: &[&str]) -> Vec<String> {
        x.iter().map(|x| x.to_string()).collect()
    }

    fn repo(id: i64, name: &str, rtype: &str) -> PpmRepo {
        PpmRepo {
            id,
            name: name.to_string(),
            repo_type: rtype.to_string(),
            hidden: false,
        }
    }

    /// `sources` are the sources of the repository of the URL; the tests use
    /// it for `all`, `cran`, `bioconductor` and `snapshot`.
    fn p3m_api(sources: Option<Vec<i64>>) -> PpmApi {
        let sources = match sources {
            Some(s) => [1, 2, 4, 6].into_iter().map(|id| (id, s.clone())).collect(),
            None => HashMap::new(),
        };
        PpmApi {
            base: Some("https://p3m.dev".to_string()),
            repos: Some(vec![
                repo(1, "all", "R"),
                repo(2, "cran", "R"),
                repo(4, "bioconductor", "Bioconductor"),
                repo(5, "pypi", "Python"),
                repo(6, "snapshot", "R"),
            ]),
            sources,
            own_metadata: HashSet::new(),
        }
    }

    #[test]
    fn test_snapshot_segment() {
        assert!(is_fixed_snapshot("2026-04-27"));
        assert!(is_fixed_snapshot("123456"));
        assert!(!is_fixed_snapshot("latest"));
        assert!(!is_fixed_snapshot("jammy"));
    }

    #[test]
    fn test_parse_ppm_path() {
        assert_eq!(parse_ppm_path(&[]), PpmPath::default());
        assert_eq!(
            parse_ppm_path(&s(&["cran", "__linux__", "jammy", "latest"])),
            PpmPath {
                repo: Some("cran".to_string()),
                distro: Some("jammy".to_string()),
                snapshot: Some("latest".to_string()),
                ..Default::default()
            }
        );
        assert_eq!(
            parse_ppm_path(&s(&[
                "bioconductor",
                "2026-04-30",
                "packages",
                "3.23",
                "data",
                "annotation"
            ])),
            PpmPath {
                repo: Some("bioconductor".to_string()),
                snapshot: Some("2026-04-30".to_string()),
                bioc_version: Some("3.23".to_string()),
                bioc_subrepo: Some("data/annotation".to_string()),
                ..Default::default()
            }
        );
        assert_eq!(
            parse_ppm_path(&s(&["bioconductor", "packages", "3.22", "bioc"])),
            PpmPath {
                repo: Some("bioconductor".to_string()),
                bioc_version: Some("3.22".to_string()),
                bioc_subrepo: Some("bioc".to_string()),
                ..Default::default()
            }
        );
    }

    #[test]
    fn test_split_ppm_url() {
        assert_eq!(
            split_ppm_url("https://p3m.dev/cran/latest", Some("https://p3m.dev")),
            ("https://p3m.dev".to_string(), s(&["cran", "latest"]))
        );
        assert_eq!(
            split_ppm_url("https://x.com/rspm/cran/latest", None),
            ("https://x.com/rspm".to_string(), s(&["cran", "latest"]))
        );
        assert_eq!(
            split_ppm_url("https://x.com/rspm/cran/__linux__/noble/2026-01-01", None),
            (
                "https://x.com/rspm".to_string(),
                s(&["cran", "__linux__", "noble", "2026-01-01"])
            )
        );
        assert_eq!(
            split_ppm_url("https://x.com/rspm", None),
            ("https://x.com/rspm".to_string(), vec![])
        );
    }

    #[test]
    fn test_bioc_paths() {
        assert_eq!(
            parse_bioc_subrepo_path("https://bioconductor.org/packages/3.22/bioc"),
            Some(("3.22".to_string(), "bioc".to_string()))
        );
        assert_eq!(
            parse_bioc_subrepo_path("https://mirror.org/bioc/packages/3.22/data/experiment"),
            Some(("3.22".to_string(), "data/experiment".to_string()))
        );
        assert_eq!(parse_bioc_subrepo_path("https://bioconductor.org"), None);
        assert_eq!(
            parse_bioc_version_path("https://bioconductor.org/packages/3.22"),
            Some("3.22".to_string())
        );
        assert_eq!(parse_bioc_version_path("https://x.org/packages/x"), None);
    }

    #[test]
    fn test_classify_cran() {
        let p = probes(ok(), not_found(), not_found(), ok(), not_found());
        let d = classify("https://cloud.r-project.org", &p);
        assert_eq!(d.repo_type, RepoType::Cran);
        assert!(d.extensible);
        assert_eq!(d.metadata.as_deref(), Some(PPM_METADATA_URL));
    }

    #[test]
    fn test_classify_cran_like() {
        let mut p = nothing();
        p.packages = ok();
        let d = classify("https://cran.acme.com", &p);
        assert_eq!(d.repo_type, RepoType::CranLike);
        assert!(!d.extensible);
        assert!(d.notes.is_empty());

        let d = classify("https://nothing.here", &nothing());
        assert_eq!(d.repo_type, RepoType::CranLike);
        assert_eq!(d.notes.len(), 1);
    }

    #[test]
    fn test_classify_r_universe() {
        let mut p = nothing();
        p.packages = Probe {
            status: Some(200),
            repository_type: Some("r-universe".to_string()),
        };
        let d = classify("https://r-lib.r-universe.dev", &p);
        assert_eq!(d.repo_type, RepoType::RUniverse);
        assert!(!d.extensible);
    }

    #[test]
    fn test_classify_extended() {
        let mut p = nothing();
        p.allpackages = ok();
        let d = classify("https://ppm.r-pkg.org", &p);
        assert_eq!(d.repo_type, RepoType::Extended);
        assert_eq!(d.extended_flavor.as_deref(), Some("cran"));
        assert_eq!(d.metadata.as_deref(), Some("https://ppm.r-pkg.org"));

        let d = classify("https://ppm-bioc.r-pkg.org/3.22", &p);
        assert_eq!(d.extended_flavor.as_deref(), Some("bioc"));
        assert_eq!(d.bioc_version.as_deref(), Some("3.22"));

        let mut p = nothing();
        p.bioc_allpackages = ok();
        let d = classify("https://ppm-bioc.r-pkg.org", &p);
        assert_eq!(d.repo_type, RepoType::Extended);
        assert_eq!(d.extended_flavor.as_deref(), Some("bioc"));
    }

    #[test]
    fn test_classify_bioc() {
        let mut p = nothing();
        p.bioc_root = ok();
        let d = classify("https://bioconductor.org", &p);
        assert_eq!(d.repo_type, RepoType::Bioc);
        assert!(d.extensible);
        assert_eq!(d.metadata.as_deref(), Some(BIOC_METADATA_URL));
        assert_eq!(d.bioc_subrepo, None);

        let mut p = nothing();
        p.packages = ok();
        let d = classify("https://bioconductor.org/packages/3.22/bioc", &p);
        assert_eq!(d.repo_type, RepoType::Bioc);
        assert!(!d.extensible);
        assert_eq!(d.metadata, None);
        assert_eq!(d.bioc_version.as_deref(), Some("3.22"));
        assert_eq!(d.bioc_subrepo.as_deref(), Some("bioc"));

        let d = classify("https://bioconductor.org/packages/3.22", &nothing());
        assert_eq!(d.repo_type, RepoType::Bioc);
        assert!(d.extensible);
        assert_eq!(d.bioc_version.as_deref(), Some("3.22"));
    }

    #[test]
    fn test_is_ppm() {
        let mut p = nothing();
        assert!(!p.is_ppm());
        p.archive.repository_type = Some("RSPM".to_string());
        assert!(p.is_ppm());
    }

    #[test]
    fn test_classify_ppm_cran() {
        let api = p3m_api(Some(vec![PPM_SOURCE_CRAN]));
        let d = classify_ppm("https://p3m.dev/cran/__linux__/jammy/latest", &api).unwrap();
        assert_eq!(d.repo_type, RepoType::PpmCran);
        assert!(d.extensible);
        assert!(!d.guessed);
        assert_eq!(d.ppm_base.as_deref(), Some("https://p3m.dev"));
        assert_eq!(d.ppm_repo.as_deref(), Some("cran"));
        assert_eq!(d.distro.as_deref(), Some("jammy"));
        assert_eq!(d.snapshot.as_deref(), Some("latest"));

        // A repository that is called `snapshot`, at `latest`.
        let d = classify_ppm("https://p3m.dev/snapshot/latest", &api).unwrap();
        assert_eq!(d.repo_type, RepoType::PpmCran);
        assert_eq!(d.metadata.as_deref(), Some(PPM_METADATA_URL));
    }

    #[test]
    fn test_classify_ppm_cran_own_metadata() {
        let mut api = p3m_api(Some(vec![PPM_SOURCE_CRAN]));
        api.own_metadata.insert("cran".to_string());
        let d = classify_ppm("https://p3m.dev/cran/latest", &api).unwrap();
        assert_eq!(d.repo_type, RepoType::PpmCran);
        assert!(d.extensible);
        assert_eq!(d.metadata.as_deref(), Some("https://p3m.dev/cran"));

        // Only a CRAN repository uses its own metadata, for now.
        let mut other = p3m_api(Some(vec![PPM_SOURCE_CRAN, 3]));
        other.own_metadata.insert("all".to_string());
        let d = classify_ppm("https://p3m.dev/all/latest", &other).unwrap();
        assert_eq!(d.repo_type, RepoType::PpmOther);
        assert_eq!(d.metadata, None);

        // And the same for the repositories of a PPM root.
        let d = classify_ppm("https://p3m.dev", &api).unwrap();
        let cran = d.ppm_repos.iter().find(|r| r.name == "cran").unwrap();
        assert_eq!(cran.metadata.as_deref(), Some("https://p3m.dev/cran"));
    }

    #[test]
    fn test_classify_ppm_other() {
        let api = p3m_api(Some(vec![PPM_SOURCE_CRAN, 3]));
        let d = classify_ppm("https://p3m.dev/all/latest", &api).unwrap();
        assert_eq!(d.repo_type, RepoType::PpmOther);
        assert!(!d.extensible);

        // Curated CRAN has a source type of its own.
        let api = p3m_api(Some(vec![2]));
        let d = classify_ppm("https://p3m.dev/cran/latest", &api).unwrap();
        assert_eq!(d.repo_type, RepoType::PpmOther);
    }

    #[test]
    fn test_classify_ppm_bioc() {
        let api = p3m_api(Some(vec![PPM_SOURCE_BIOC]));
        let d = classify_ppm("https://p3m.dev/bioconductor/latest", &api).unwrap();
        assert_eq!(d.repo_type, RepoType::PpmBioc);
        assert!(d.extensible);
        assert_eq!(d.metadata.as_deref(), Some(BIOC_METADATA_URL));

        let api = p3m_api(Some(vec![
            PPM_SOURCE_BIOC,
            PPM_SOURCE_BIOC,
            PPM_SOURCE_BIOC,
        ]));
        let d = classify_ppm("https://p3m.dev/bioconductor/latest", &api).unwrap();
        assert_eq!(d.repo_type, RepoType::PpmBioc);

        // No snapshot is fine for Bioconductor.
        let d = classify_ppm("https://p3m.dev/bioconductor", &api).unwrap();
        assert_eq!(d.repo_type, RepoType::PpmBioc);

        let d = classify_ppm(
            "https://p3m.dev/bioconductor/latest/packages/3.22/bioc",
            &api,
        )
        .unwrap();
        assert_eq!(d.repo_type, RepoType::PpmBioc);
        assert!(!d.extensible);
        assert_eq!(d.bioc_version.as_deref(), Some("3.22"));
        assert_eq!(d.bioc_subrepo.as_deref(), Some("bioc"));
    }

    #[test]
    fn test_classify_ppm_snapshot() {
        let api = p3m_api(Some(vec![PPM_SOURCE_CRAN]));
        for url in [
            "https://p3m.dev/cran/2026-04-27",
            "https://p3m.dev/cran/__linux__/jammy/123456",
            "https://p3m.dev/bioconductor/2026-04-30/packages/3.23/bioc",
        ] {
            let d = classify_ppm(url, &api).unwrap();
            assert_eq!(d.repo_type, RepoType::PpmSnapshot, "{}", url);
            assert!(!d.extensible);
        }
    }

    #[test]
    fn test_classify_ppm_root() {
        let mut api = p3m_api(None);
        api.sources = HashMap::from([
            (1, vec![PPM_SOURCE_CRAN, 3]),
            (2, vec![PPM_SOURCE_CRAN]),
            (4, vec![PPM_SOURCE_BIOC, PPM_SOURCE_BIOC]),
        ]);
        let d = classify_ppm("https://p3m.dev", &api).unwrap();
        assert_eq!(d.repo_type, RepoType::Ppm);
        assert!(!d.guessed);
        assert!(d.extensible);
        // `pypi` is left out.
        let types: Vec<(&str, RepoType, bool)> = d
            .ppm_repos
            .iter()
            .map(|r| (r.name.as_str(), r.repo_type, r.guessed))
            .collect();
        assert_eq!(
            types,
            vec![
                ("all", RepoType::PpmOther, false),
                ("cran", RepoType::PpmCran, false),
                ("bioconductor", RepoType::PpmBioc, false),
                // No sources for this one: a guess from the name.
                ("snapshot", RepoType::PpmOther, true),
            ]
        );
        let cran = &d.ppm_repos[1];
        assert!(cran.extensible);
        assert_eq!(cran.url, "https://p3m.dev/cran/latest");

        let d = classify_ppm("https://x.com/rspm", &PpmApi::default()).unwrap();
        assert_eq!(d.repo_type, RepoType::Ppm);
        assert!(d.guessed);
        assert!(d.ppm_repos.is_empty());
    }

    #[test]
    fn test_classify_ppm_errors() {
        let api = p3m_api(None);
        assert!(classify_ppm("https://p3m.dev/pypi/latest", &api).is_err());
        assert!(classify_ppm("https://p3m.dev/nope/latest", &api).is_err());
    }

    #[test]
    fn test_classify_ppm_no_snapshot() {
        let api = p3m_api(Some(vec![PPM_SOURCE_CRAN]));
        let d = classify_ppm("https://p3m.dev/cran", &api).unwrap();
        assert_eq!(d.repo_type, RepoType::PpmCran);
        assert!(d.extensible);
        assert_eq!(d.snapshot, None);
        let d = classify_ppm("https://p3m.dev/cran/__linux__/noble", &api).unwrap();
        assert_eq!(d.repo_type, RepoType::PpmCran);
        assert_eq!(d.distro.as_deref(), Some("noble"));
    }

    #[test]
    fn test_classify_ppm_no_api() {
        let api = PpmApi::default();
        let cases = [
            ("https://x.com/rspm/cran/latest", RepoType::PpmCran),
            ("https://x.com/rspm/bioconductor/latest", RepoType::PpmBioc),
            ("https://x.com/rspm/internal/latest", RepoType::PpmOther),
            ("https://x.com/rspm/cran/2026-01-01", RepoType::PpmSnapshot),
            ("https://x.com/rspm", RepoType::Ppm),
        ];
        for (url, rtype) in cases {
            let d = classify_ppm(url, &api).unwrap();
            assert_eq!(d.repo_type, rtype, "{}", url);
            assert_eq!(d.ppm_base.as_deref(), Some("https://x.com/rspm"), "{}", url);
        }
        let d = classify_ppm("https://x.com/rspm/cran/latest", &api).unwrap();
        assert!(d.guessed);
    }

    #[test]
    fn test_repo_type_names() {
        let names: Vec<&str> = RepoType::ALL.iter().map(|t| t.as_str()).collect();
        assert_eq!(names, crate::args::REPO_TYPES);
        for t in RepoType::ALL {
            assert_eq!(t.as_str().parse::<RepoType>().unwrap(), t);
            let json = serde_json::to_string(&t).unwrap();
            assert_eq!(json, format!("\"{}\"", t.as_str()));
        }
        assert!("nope".parse::<RepoType>().is_err());
    }

    #[test]
    fn test_describe() {
        let d = describe(
            "https://p3m.dev/cran/__linux__/jammy/latest/",
            RepoType::PpmCran,
        );
        assert_eq!(d.url, "https://p3m.dev/cran/__linux__/jammy/latest");
        assert_eq!(d.ppm_base.as_deref(), Some("https://p3m.dev"));
        assert_eq!(d.ppm_repo.as_deref(), Some("cran"));
        assert_eq!(d.distro.as_deref(), Some("jammy"));
        assert_eq!(d.snapshot.as_deref(), Some("latest"));
        assert!(d.extensible);

        let d = describe(
            "https://bioconductor.org/packages/3.22/bioc",
            RepoType::Bioc,
        );
        assert_eq!(d.bioc_version.as_deref(), Some("3.22"));
        assert_eq!(d.bioc_subrepo.as_deref(), Some("bioc"));
        assert!(!d.extensible);

        let d = describe("https://bioconductor.org/packages/3.22", RepoType::Bioc);
        assert_eq!(d.bioc_version.as_deref(), Some("3.22"));
        assert_eq!(d.bioc_subrepo, None);

        let d = describe("https://ppm.r-pkg.org", RepoType::Extended);
        assert_eq!(d.extended_flavor.as_deref(), Some("cran"));
        let d = describe("https://ppm-bioc.r-pkg.org", RepoType::Extended);
        assert_eq!(d.extended_flavor.as_deref(), Some("bioc"));
        let d = describe("https://x.org/meta/3.21", RepoType::Extended);
        assert_eq!(d.extended_flavor.as_deref(), Some("bioc"));
        assert_eq!(d.bioc_version.as_deref(), Some("3.21"));

        let d = describe("https://cloud.r-project.org", RepoType::Cran);
        assert_eq!(
            d,
            DetectedRepo::new("https://cloud.r-project.org", RepoType::Cran)
        );
    }

    #[test]
    fn test_probe_files() {
        let dir = tempfile::tempdir().unwrap();
        let contrib = dir.path().join("src").join("contrib");
        std::fs::create_dir_all(contrib.join("Meta")).unwrap();
        std::fs::write(contrib.join("PACKAGES"), "").unwrap();
        let url = Url::from_directory_path(dir.path()).unwrap().to_string();
        let url = normalize_url(&url);

        let p = probe_files(&url, &probe_paths("3.22")).unwrap();
        assert_eq!(classify(&url, &p).repo_type, RepoType::CranLike);

        std::fs::write(contrib.join("Meta").join("archive.rds"), "").unwrap();
        let p = probe_files(&url, &probe_paths("3.22")).unwrap();
        assert_eq!(classify(&url, &p).repo_type, RepoType::Cran);
    }
}
