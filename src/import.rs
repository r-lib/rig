//! The import of another tool's record of a project's packages into a rig
//! project, e.g. `rig proj renv import` and `rig proj rsconnect import`.
//!
//! Each importer reads its own format into [`PackageRecords`]: the R version,
//! the repositories and the packages, each with where it was installed from.
//! From these, [`apply_records`] makes `rproj.toml`: repository URLs become
//! `[[repository]]` entries, or the built-in CRAN and Bioconductor where they
//! stand for them, and git remotes become `git` dependencies.
//! [`ProjectImport`] then writes the project, and locks the recorded versions
//! in `rproj.lock`.

use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::PathBuf;

use clap::ArgMatches;
use log::{error, info, warn};

use crate::output::OUTPUT;
use crate::pkgsource::git::{remote_ref_kind, RefKind};
use crate::proj::{
    check_project_conflicts, init_rvenv_files, proj_lock_locked_versions, GitSourceKey,
};
use crate::proj_repos::ProjRepos;
use crate::repos::cranlike_metadata::minor_r_version;
use crate::repos::detect::{DetectedRepo, RepoType};
use crate::repos::feed::{CranlikeProtocol, PkgRepo};
use crate::repos::protocol::repos_for;
use crate::repos::repo_types::resolve_repo;
use crate::rproj::{
    DepTable, Dependency, Repository, Rproj, BIOC_REPOSITORY_NAME, CRAN_REPOSITORY_NAME,
    RPROJ_MANIFEST_FILE,
};
use crate::solver::is_base_package;

// ------------------------------------------------------------------------
// The records

/// Another tool's record of the packages of a project, e.g. `renv.lock`.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct PackageRecords {
    /// The R version the packages were installed for.
    pub(crate) r_version: String,
    /// The Bioconductor release the packages were installed from, if any.
    pub(crate) bioc_version: Option<String>,
    /// The repositories that the record lists, in order.
    pub(crate) repositories: Vec<RecordedRepository>,
    /// The packages, in any order.
    pub(crate) packages: Vec<RecordedPackage>,
}

/// A repository that a record lists.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct RecordedRepository {
    pub(crate) name: String,
    pub(crate) url: String,
    /// Whether it is one of the Bioconductor repositories that the packages
    /// from Bioconductor come from, with no telling which package from which.
    pub(crate) bioconductor: bool,
}

/// A recorded package: its name and version, and where it was installed
/// from.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RecordedPackage {
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) source: PackageSource,
}

/// Where a recorded package was installed from.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PackageSource {
    /// A package repository, the one of the reference if there is one.
    Repository(Option<RepoReference>),
    /// Bioconductor, the repository of the reference if there is one.
    Bioconductor(Option<RepoReference>),
    /// A git remote.
    Git(GitRemote),
    /// Somewhere else, e.g. a URL or a local file.
    Other,
}

/// The repository a package was installed from.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RepoReference {
    /// The URL of the repository, or its name in the record.
    pub(crate) repository: String,
    /// A name for the repository, if `repository` is a URL that the record
    /// does not list.
    pub(crate) name: Option<String>,
}

/// The names that R tools record for CRAN, and for Posit Package Manager's
/// CRAN, for packages installed from there.
pub(crate) const CRAN_NAMES: [&str; 4] = ["CRAN", "RSPM", "PPM", "P3M"];

/// Whether `name` is a name that R tools take for Bioconductor.
pub(crate) fn is_bioc_repository_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("bioc") || name.eq_ignore_ascii_case("bioconductor")
}

// ------------------------------------------------------------------------
// Git remotes

/// The git remote a package was installed from, as the `Remote*` fields of
/// its `DESCRIPTION` record it. Every field that is set is trimmed and not
/// empty.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct GitRemote {
    /// How the package was installed, lower case: `github`, `gitlab`,
    /// `bitbucket`, or from a git URL, `git`, `xgit` or `git2r`.
    pub(crate) kind: String,
    /// `RemoteHost`: the web API of the git host.
    pub(crate) host: Option<String>,
    /// `RemoteUsername`.
    pub(crate) username: Option<String>,
    /// `RemoteRepo`.
    pub(crate) repo: Option<String>,
    /// `RemoteUrl`, the URL of a git remote that is not on a known host.
    pub(crate) url: Option<String>,
    /// `RemoteRef`: the branch, tag or commit, `HEAD` for the default
    /// branch.
    pub(crate) reference: Option<String>,
    /// `RemoteSha`: the commit that was installed.
    pub(crate) sha: Option<String>,
    /// `RemoteSubdir`: the package's directory in the repository.
    pub(crate) subdir: Option<String>,
}

/// The kinds of [`GitRemote`].
pub(crate) const GIT_KINDS: [&str; 6] = ["github", "gitlab", "bitbucket", "git", "xgit", "git2r"];

impl GitRemote {
    /// Whether `kind`, in any case, is a kind of [`GitRemote`].
    pub(crate) fn is_git_kind(kind: &str) -> bool {
        GIT_KINDS
            .iter()
            .any(|k| kind.trim().eq_ignore_ascii_case(k))
    }

    /// The `git` dependency for the remote, at its branch or tag, `None` if
    /// the record does not say where the remote is. `rproj.lock` gets the
    /// commit. The ref is a branch until [`apply_records`] finds out
    /// otherwise.
    pub(crate) fn dep_table(&self) -> Option<DepTable> {
        let host_repo = |default_host: &str, api: &str| -> Option<String> {
            let user = self.username.as_deref()?;
            let repo = self.repo.as_deref()?;
            let repo = repo.strip_suffix(".git").unwrap_or(repo);
            let host = self
                .host
                .as_deref()
                .map(|h| git_host(h, api))
                .unwrap_or_else(|| default_host.to_string());
            Some(format!("{}/{}/{}.git", host, user, repo))
        };
        let git = match self.kind.as_str() {
            "github" => host_repo("https://github.com", "api.github.com")?,
            "gitlab" => host_repo("https://gitlab.com", "")?,
            "bitbucket" => host_repo("https://bitbucket.org", "api.bitbucket.org")?,
            "git" | "xgit" | "git2r" => {
                let url = self.remote_url()?;
                github_https_url(url).unwrap_or_else(|| url.to_string())
            }
            _ => return None,
        };
        let mut table = DepTable {
            git: Some(git),
            subdir: self.subdir.clone(),
            ..Default::default()
        };
        match self.git_ref() {
            Some(r) if is_commit_sha(r) => table.rev = Some(r.to_string()),
            Some(r) => table.branch = Some(r.to_string()),
            None if self.reference.is_none() => table.rev = self.sha.clone(),
            None => {}
        }
        Some(table)
    }

    /// The branch or tag the package was installed from, `None` for the
    /// default branch.
    fn git_ref(&self) -> Option<&str> {
        self.reference
            .as_deref()
            .filter(|r| !r.eq_ignore_ascii_case("HEAD"))
    }

    /// The URL of a remote that is a git URL, as opposed to GitHub, GitLab or
    /// Bitbucket.
    pub(crate) fn remote_url(&self) -> Option<&str> {
        match self.kind.as_str() {
            "git" | "xgit" | "git2r" => self.url.as_deref(),
            _ => None,
        }
    }
}

/// Extract the base URL of a git host from a `RemoteHost`. That field points
/// to the web API.
fn git_host(host: &str, default_api: &str) -> String {
    let (scheme, rest) = match host.split_once("://") {
        Some((scheme, rest)) => (scheme, rest),
        None => ("https", host),
    };
    let rest = rest.trim_end_matches('/');
    let rest = rest
        .strip_suffix("/api/v3")
        .or_else(|| rest.strip_suffix("/2.0"))
        .unwrap_or(rest);
    let rest = match default_api.strip_prefix("api.") {
        Some(web) if rest.eq_ignore_ascii_case(default_api) => web,
        _ => rest,
    };
    format!("{}://{}", scheme, rest)
}

/// The `https://` URL of a `git://github.com/` URL. GitHub does not serve
/// the git protocol any more. `None` for any other URL.
fn github_https_url(url: &str) -> Option<String> {
    let prefix = "git://github.com/";
    if has_scheme(url, &[prefix]) {
        Some(format!("https://github.com/{}", &url[prefix.len()..]))
    } else {
        None
    }
}

/// Whether a git ref looks like a commit sha rather than a branch or tag.
fn is_commit_sha(s: &str) -> bool {
    (7..=40).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

// ------------------------------------------------------------------------
// Repositories

pub(crate) fn is_url(s: &str) -> bool {
    has_scheme(s, &["http://", "https://", "ftp://", "file://"])
}

/// Whether `s` is a URL of a scheme that rig can load a repository from.
pub(crate) fn is_http_url(s: &str) -> bool {
    has_scheme(s, &["http://", "https://"])
}

fn has_scheme(s: &str, schemes: &[&str]) -> bool {
    schemes.iter().any(|p| {
        s.get(..p.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(p))
    })
}

/// Strips the Linux binary path from a Posit Package Manager URL, e.g.
/// `https://p3m.dev/cran/__linux__/noble/latest`. `None` for any other URL.
fn strip_binary_path(url: &str) -> Option<String> {
    let (head, tail) = url.split_once("/__linux__/")?;
    let (_, tail) = tail.split_once('/')?;
    Some(format!("{}/{}", head, tail))
}

/// Normalize a repository URL to compare it with others.
fn normalize_repo_url(url: &str) -> String {
    let url = url.trim();
    let url = strip_binary_path(url).unwrap_or_else(|| url.to_string());
    let url = url.trim_end_matches('/');
    let (scheme, rest) = match url.split_once("://") {
        Some((scheme, rest)) => (format!("{}://", scheme.to_ascii_lowercase()), rest),
        None => (String::new(), url),
    };
    let (host, path) = match rest.split_once('/') {
        Some((host, path)) => (host, format!("/{}", path)),
        None => (rest, String::new()),
    };
    let host = match host.rsplit_once('@') {
        Some((userinfo, name)) => format!("{}@{}", userinfo, name.to_ascii_lowercase()),
        None => host.to_ascii_lowercase(),
    };
    format!("{}{}{}", scheme, host, path)
}

/// The hosts of the public repositories that the built-in CRAN and
/// Bioconductor repositories stand for.
const PUBLIC_HOSTS: [&str; 8] = [
    "cloud.r-project.org",
    "cran.r-project.org",
    "cran.rstudio.com",
    "cran.rstudio.org",
    "bioconductor.org",
    "p3m.dev",
    "packagemanager.posit.co",
    "packagemanager.rstudio.com",
];

fn is_public_url(url: &str) -> bool {
    reqwest::Url::parse(url.trim()).is_ok_and(|u| {
        u.host_str()
            .is_some_and(|h| PUBLIC_HOSTS.iter().any(|p| h.eq_ignore_ascii_case(p)))
    })
}

/// How a recorded repository is imported.
#[derive(Debug, PartialEq)]
enum ImportAs {
    /// As this built-in repository.
    Builtin(&'static str),
    /// As a repository of its own, at this URL.
    Url(String),
}

/// How the recorded repository `repo` at `url` is imported, from its
/// detected type. `bioc_version` is the recorded Bioconductor release.
///
/// A public repository is the built-in one it stands for: a CRAN mirror and
/// Posit Package Manager's CRAN are the built-in CRAN, a Bioconductor
/// repository of the recorded release is the built-in Bioconductor, and a
/// PPM snapshot is one of these as well, unless `keep_snapshots`. Private
/// repositories stay, but a snapshot of a private PPM is its repository at
/// `latest`, unless `keep_snapshots`.
fn import_as(
    repo: &DetectedRepo,
    url: &str,
    bioc_version: Option<&str>,
    keep_snapshots: bool,
) -> ImportAs {
    let own = || ImportAs::Url(url.to_string());
    let snapshot = repo.repo_type == RepoType::PpmSnapshot && !keep_snapshots;
    if !is_public_url(url) {
        return match ppm_latest_url(repo) {
            Some(latest) if snapshot => ImportAs::Url(latest),
            _ => own(),
        };
    }
    let bioc = || match (bioc_version, repo.bioc_version.as_deref()) {
        (Some(lock), Some(version)) if lock != version => own(),
        (Some(_), _) => ImportAs::Builtin(BIOC_REPOSITORY_NAME),
        (None, _) => own(),
    };
    match repo.repo_type {
        RepoType::Cran | RepoType::PpmCran => ImportAs::Builtin(CRAN_REPOSITORY_NAME),
        RepoType::Bioc | RepoType::PpmBioc => bioc(),
        RepoType::PpmSnapshot if snapshot && repo.bioc_version.is_some() => bioc(),
        RepoType::PpmSnapshot if snapshot => ImportAs::Builtin(CRAN_REPOSITORY_NAME),
        _ => own(),
    }
}

/// The URL of the PPM repository `repo` at `latest`, of the same
/// Bioconductor repository if it is one. `None` if it is not on PPM.
fn ppm_latest_url(repo: &DetectedRepo) -> Option<String> {
    let base = repo.ppm_base.as_deref()?;
    let name = repo.ppm_repo.as_deref()?;
    let mut url = format!("{}/{}/latest", base, name);
    if let Some(version) = &repo.bioc_version {
        url.push_str(&format!("/packages/{}", version));
        if let Some(subrepo) = &repo.bioc_subrepo {
            url.push_str(&format!("/{}", subrepo));
        }
    }
    Some(url)
}

/// A repository name for the manifest, based on `base`, that is neither
/// taken according to `is_taken`, nor the name of a built-in repository.
fn unique_repository_name(base: &str, is_taken: impl Fn(&str) -> bool) -> String {
    let is_taken = |name: &str| {
        name.eq_ignore_ascii_case(CRAN_REPOSITORY_NAME)
            || name.eq_ignore_ascii_case(BIOC_REPOSITORY_NAME)
            || is_taken(name)
    };
    if !is_taken(base) {
        return base.to_string();
    }
    let mut candidate = format!("{}-repo", base);
    let mut i = 2;
    while is_taken(&candidate) {
        candidate = format!("{}-repo-{}", base, i);
        i += 1;
    }
    candidate
}

/// A repository name made for a given URL.
fn repository_name_from_url(url: &str) -> String {
    let url = strip_binary_path(url).unwrap_or_else(|| url.to_string());
    let url = url.split_once("://").map_or(url.as_str(), |(_, rest)| rest);
    url.trim_matches('/').replace('/', "-")
}

/// Where the manifest takes an imported package from.
#[derive(Debug, PartialEq)]
enum Placement {
    /// From the repository of this name only.
    Pin(String),
    /// From CRAN, which serves every package that is not pinned.
    Cran,
    /// From the repositories that serve every package, e.g. a private
    /// Bioconductor repository.
    Unpin,
    /// From a repository the record does not describe, or that rig cannot
    /// use, so it is an ordinary version requirement.
    Unresolved,
    /// Not from a package repository or git, e.g. a URL or a local file.
    OtherSource,
    /// From a repository the record does not name, so any pin of the
    /// manifest stays.
    Keep,
}

/// The repositories of imported [`PackageRecords`].
struct ImportedRepositories<'a> {
    entries: Vec<Repository>,
    /// Finds the type of the repository at a URL.
    detect: &'a dyn Fn(&str) -> DetectedRepo,
    /// The detected repositories, by URL.
    detected: HashMap<String, DetectedRepo>,
    /// Whether PPM snapshots stay as they are, see [`import_as`].
    keep_snapshots: bool,
    /// The repositories of the manifest that is imported into, whose names are
    /// taken.
    existing: Vec<Repository>,
    /// The name each recorded repository (lower case) has in the manifest.
    names: HashMap<String, String>,
    /// The name of every repository that is not the built-in CRAN, by its
    /// normalized URL.
    urls: Vec<(String, String)>,
    /// Whether the record lists any repository.
    listed: bool,
    /// Whether the record has a Bioconductor version, so that the built-in
    /// Bioconductor repository stands for the Bioconductor repositories it
    /// lists.
    bioc: bool,
    /// The recorded Bioconductor version.
    bioc_version: Option<String>,
    /// The names of the recorded Bioconductor repositories that are not the
    /// built-in one, e.g. a private mirror or a dated snapshot, see
    /// [`RecordedRepository::bioconductor`]. There is no telling which
    /// package is in which, so they serve every package.
    other_bioc: Vec<String>,
}

impl<'a> ImportedRepositories<'a> {
    /// The recorded repositories `repos`, in order. `bioc_version` is the
    /// recorded Bioconductor version, and `detect` finds the type of a
    /// repository.
    fn new(
        repos: &[RecordedRepository],
        bioc_version: Option<&str>,
        existing: &[Repository],
        detect: &'a dyn Fn(&str) -> DetectedRepo,
        keep_snapshots: bool,
    ) -> ImportedRepositories<'a> {
        let mut imported = ImportedRepositories {
            entries: vec![],
            detect,
            detected: HashMap::new(),
            keep_snapshots,
            existing: existing.to_vec(),
            names: HashMap::new(),
            urls: existing
                .iter()
                .filter_map(|r| Some((normalize_repo_url(r.url.as_deref()?), r.name.clone())))
                .collect(),
            listed: !repos.is_empty(),
            bioc: bioc_version.is_some(),
            bioc_version: bioc_version.map(str::to_string),
            other_bioc: vec![],
        };
        for repo in repos {
            let key = repo.name.to_lowercase();
            if imported.names.contains_key(&key) || !is_http_url(&repo.url) {
                continue;
            }
            let name = imported.add_url(&repo.name, &repo.url);
            if imported.bioc
                && repo.bioconductor
                && name != BIOC_REPOSITORY_NAME
                && name != CRAN_REPOSITORY_NAME
            {
                imported.other_bioc.push(name.clone());
            }
            imported.names.insert(key, name);
        }
        imported
    }

    /// The manifest name of the repository at `url`, which is added if it is
    /// not there yet, under `name` if it is free.
    fn add_url(&mut self, name: &str, url: &str) -> String {
        let url = url.trim();
        let detect = self.detect;
        let repo = self
            .detected
            .entry(url.to_string())
            .or_insert_with(|| detect(url));
        let url = match import_as(repo, url, self.bioc_version.as_deref(), self.keep_snapshots) {
            ImportAs::Builtin(CRAN_REPOSITORY_NAME) => return self.list_cran(),
            ImportAs::Builtin(builtin) => return builtin.to_string(),
            ImportAs::Url(url) => url,
        };
        let key = normalize_repo_url(&url);
        if let Some((_, existing)) = self.urls.iter().find(|(k, _)| *k == key) {
            return existing.clone();
        }
        let name = unique_repository_name(name, |n| {
            self.existing
                .iter()
                .chain(&self.entries)
                .any(|r| r.name.eq_ignore_ascii_case(n))
        });
        let url = strip_binary_path(&url).unwrap_or(url);
        // A package was installed from the repository recorded for it, and
        // these are pinned, so no other package is taken from here.
        self.entries.push(Repository {
            explicit: Some(true),
            ..Repository::at_url(&name, &url)
        });
        self.urls.push((key, name.clone()));
        name
    }

    /// The name of the built-in CRAN, which is listed if it was not yet.
    fn list_cran(&mut self) -> String {
        if !self.entries.iter().any(|e| e.is_cran()) {
            self.entries.push(Repository::builtin(CRAN_REPOSITORY_NAME));
        }
        CRAN_REPOSITORY_NAME.to_string()
    }

    /// The manifest name of the repository `reference` of a package, a
    /// repository name or a URL, with `name` as a name for it if it is a URL
    /// the record does not list. `None` if it is a name the record does not
    /// list, or a repository rig cannot load packages from, see
    /// [`is_http_url`].
    ///
    /// A package that says it is from CRAN, or from Posit Package Manager's
    /// CRAN, is from the built-in CRAN, even if the record does not list it,
    /// and so is one from a URL that is not listed.
    fn resolve(&mut self, reference: &str, name: Option<&str>) -> Option<String> {
        if is_url(reference) {
            if !is_http_url(reference) {
                return None;
            }
            let name = name
                .map(str::to_string)
                .unwrap_or_else(|| repository_name_from_url(reference));
            return Some(self.add_url(&name, reference));
        }
        if let Some(name) = self.names.get(&reference.to_lowercase()) {
            return Some(name.clone());
        }
        if CRAN_NAMES.iter().any(|n| reference.eq_ignore_ascii_case(n)) {
            return Some(self.list_cran());
        }
        None
    }

    /// Where the manifest takes the package `pkg` from.
    ///
    /// A package installed from a repository other than CRAN is pinned to it.
    fn placement(&mut self, pkg: &RecordedPackage) -> Placement {
        let reference = match &pkg.source {
            PackageSource::Bioconductor(reference) => {
                return self.bioc_placement(reference.as_ref())
            }
            PackageSource::Repository(Some(reference)) => reference,
            PackageSource::Repository(None) => return Placement::Keep,
            PackageSource::Git(_) | PackageSource::Other => return Placement::OtherSource,
        };
        match self.resolve(&reference.repository, reference.name.as_deref()) {
            Some(repo) if repo != CRAN_REPOSITORY_NAME => Placement::Pin(repo),
            Some(_) => Placement::Cran,
            None => Placement::Unresolved,
        }
    }

    /// Where the manifest takes a Bioconductor package from, which was
    /// installed from the repository of `reference`, if given.
    ///
    /// A package that names a recorded repository is pinned to it, as it was
    /// installed from there. Otherwise, with a recorded Bioconductor version, a
    /// package is pinned to the built-in Bioconductor repository, unless the
    /// record lists another Bioconductor repository, which then serves it.
    /// Without a Bioconductor version, a package stays in the repository it
    /// comes from.
    fn bioc_placement(&mut self, reference: Option<&RepoReference>) -> Placement {
        let builtin = || Placement::Pin(BIOC_REPOSITORY_NAME.to_string());
        let fallback = if !self.bioc {
            match self.names.get("biocsoft") {
                Some(repo) if repo != CRAN_REPOSITORY_NAME => Placement::Pin(repo.clone()),
                _ => builtin(),
            }
        } else if self.other_bioc.is_empty() {
            builtin()
        } else {
            Placement::Unpin
        };
        let Some(reference) = reference else {
            return fallback;
        };
        match self.resolve(&reference.repository, reference.name.as_deref()) {
            Some(repo) if repo != CRAN_REPOSITORY_NAME => Placement::Pin(repo),
            _ if is_bioc_repository_name(&reference.repository) => builtin(),
            _ if self.bioc => fallback,
            // Without a Bioconductor version, a package that says it is from
            // CRAN is still from Bioconductor.
            Some(_) => builtin(),
            None => Placement::Unresolved,
        }
    }

    /// The final repository entries. `bioc_used` is whether a package is
    /// pinned to the built-in Bioconductor repository.
    fn finish(mut self, bioc_used: bool) -> Vec<Repository> {
        if self.listed && !self.entries.iter().any(|e| e.is_cran()) {
            self.entries
                .push(Repository::disabled(CRAN_REPOSITORY_NAME));
        }
        if self.entries == [Repository::builtin(CRAN_REPOSITORY_NAME)] {
            self.entries.clear();
        }
        // The packages were installed from the recorded repositories only, so
        // the built-in Bioconductor is off too, unless the record uses it.
        if self.listed && !self.bioc && !bioc_used {
            self.entries
                .push(Repository::disabled(BIOC_REPOSITORY_NAME));
        }
        self.entries
    }
}

/// How [`PackageRecords`] are imported.
pub(crate) struct ImportOptions<'a> {
    /// Finds the type of the repository at a URL, see [`resolve_repo`].
    pub(crate) detect: &'a dyn Fn(&str) -> DetectedRepo,
    /// Whether PPM snapshots stay as they are, see [`import_as`].
    pub(crate) keep_snapshots: bool,
    /// Finds whether a ref of the git repository at a URL is a branch or a
    /// tag, see [`remote_ref_kind`]. `None` if it cannot tell.
    pub(crate) ref_kind: &'a dyn Fn(&str, &str) -> Option<RefKind>,
}

/// What an import found in [`PackageRecords`]: the versions to lock, and what it
/// could not make full use of.
#[derive(Debug, Default)]
pub(crate) struct ImportReport {
    /// The exact version requirement of each package imported with a version
    /// requirement, for `rproj.lock`, see [`proj_lock_locked_versions`].
    pub(crate) locked_versions: Vec<(String, String)>,
    /// The locked commit of each git package, for `rproj.lock`, see
    /// [`proj_lock_locked_versions`].
    pub(crate) locked_git_shas: Vec<(GitSourceKey, String)>,
    /// The packages from a repository the record does not describe, in
    /// order. They are imported as plain version requirements.
    pub(crate) unresolved: Vec<String>,
    /// The packages from a source that rig cannot import, e.g. a URL or a
    /// local file, in order. They are imported as plain version requirements.
    pub(crate) other_sources: Vec<String>,
    /// The packages from GitHub, GitLab, Bitbucket or a git URL whose record
    /// does not say where, in order. They are imported as plain
    /// version requirements.
    pub(crate) incomplete_git: Vec<String>,
    /// The packages from a `git://github.com/` URL, in order. They are
    /// imported from the `https://` URL instead.
    pub(crate) github_git_urls: Vec<String>,
    /// The packages from a git URL that is not an `http://` or `https://`
    /// URL, e.g. an SSH URL, in order. They are imported with that URL.
    pub(crate) other_git_urls: Vec<String>,
    /// The git packages whose ref is not known to be a branch or a tag, in
    /// order. They are imported as a branch.
    pub(crate) unknown_refs: Vec<String>,
}

/// The warnings about the packages recorded in `input` that the import
/// could not make full use of, so that they may need manual adjustment.
pub(crate) fn import_warnings(report: &ImportReport, input: &str) -> Vec<String> {
    let mut warnings = vec![];

    if !report.other_sources.is_empty() {
        warnings.push(format!(
            "{} package(s) in {} come from a source that rig cannot import \
             (e.g. a URL or a local file) and may need manual adjustment: {}",
            report.other_sources.len(),
            input,
            report.other_sources.join(", ")
        ));
    }

    if !report.incomplete_git.is_empty() {
        warnings.push(format!(
            "{} package(s) in {} come from GitHub, GitLab, Bitbucket or git, but \
             {} does not say where (it needs `RemoteUsername` and `RemoteRepo`, \
             or `RemoteUrl`); they were imported as ordinary version requirements: {}",
            report.incomplete_git.len(),
            input,
            input,
            report.incomplete_git.join(", ")
        ));
    }

    if !report.github_git_urls.is_empty() {
        warnings.push(format!(
            "{} package(s) in {} come from a git:// URL of GitHub, which GitHub \
             does not serve any more; they were imported from the https:// URL: {}",
            report.github_git_urls.len(),
            input,
            report.github_git_urls.join(", ")
        ));
    }

    if !report.other_git_urls.is_empty() {
        warnings.push(format!(
            "{} package(s) in {} come from a git URL that is not an http:// or \
             https:// URL, and may need manual adjustment: {}",
            report.other_git_urls.len(),
            input,
            report.other_git_urls.join(", ")
        ));
    }

    if !report.unknown_refs.is_empty() {
        warnings.push(format!(
            "Cannot tell if the git ref of {} package(s) in {} is a branch or a tag \
             (the repository cannot be reached, or it does not have the ref any \
             more); they were imported as a branch: {}",
            report.unknown_refs.len(),
            input,
            report.unknown_refs.join(", ")
        ));
    }

    if !report.unresolved.is_empty() {
        warnings.push(format!(
            "{} package(s) in {} come from a repository that {} does not list, \
             or that rig cannot use (it must be at an http:// or https:// URL); \
             they were imported as ordinary version requirements: {}",
            report.unresolved.len(),
            input,
            input,
            report.unresolved.join(", ")
        ));
    }

    warnings
}

/// The error of a strict import, see [`ProjectImport::strict`], about the
/// packages recorded in `input` that would only be ordinary version
/// requirements: ones from a repository that rig cannot use, or that the
/// record does not describe, and ones from another source. `None` if there
/// are none.
pub(crate) fn strict_import_error(report: &ImportReport, input: &str) -> Option<String> {
    let mut problems = vec![];
    if !report.unresolved.is_empty() {
        problems.push(format!(
            "from a repository that {} has no http:// or https:// URL for: {}",
            input,
            report.unresolved.join(", ")
        ));
    }
    let other: Vec<&str> = report
        .other_sources
        .iter()
        .chain(&report.incomplete_git)
        .map(String::as_str)
        .collect();
    if !other.is_empty() {
        problems.push(format!(
            "not from a repository or a git remote that rig can use: {}",
            other.join(", ")
        ));
    }
    if problems.is_empty() {
        return None;
    }
    Some(format!(
        "rig cannot install these packages of {} from where they were installed from:\n  {}",
        input,
        problems.join("\n  ")
    ))
}

/// The `R` requirement of the manifest for `records`. A Bioconductor
/// release is built for one R minor version, so records with one are
/// limited to their R minor version.
fn r_requirement(records: &PackageRecords) -> Result<String, Box<dyn Error>> {
    let minor = minor_r_version(&records.r_version)?;
    if records.bioc_version.is_none() {
        return Ok(format!(">= {}", minor));
    }
    let (major, minor_part) = minor
        .split_once('.')
        .ok_or_else(|| format!("Invalid R version: {}", minor))?;
    let next = minor_part.parse::<u64>()? + 1;
    Ok(format!(">= {}, < {}.{}", minor, major, next))
}

/// Merge the R version, repositories and packages of `records` into
/// `manifest`.
///
/// `repositories` is whether to import the repositories as well. Merging into
/// an existing manifest keeps its own: it only gains the repositories it does
/// not have yet, never a change to its built-in ones.
///
/// See [`ImportedRepositories::placement`] for the repository each package
/// is taken from. A package installed from a git remote becomes a `git`
/// dependency at its branch or tag instead.
///
/// The report has the locked versions and commits, to lock them in
/// `rproj.lock`.
pub(crate) fn apply_records(
    manifest: &mut Rproj,
    records: &PackageRecords,
    repositories: bool,
    options: &ImportOptions,
) -> Result<ImportReport, Box<dyn Error>> {
    manifest.add_dependency("R", &r_requirement(records)?, false);

    let mut repos = ImportedRepositories::new(
        &records.repositories,
        records.bioc_version.as_deref(),
        &manifest.repository,
        options.detect,
        options.keep_snapshots,
    );
    let mut report = ImportReport::default();
    let mut bioc_used = false;
    let mut from_cran: Vec<String> = vec![];

    // In order, so that the names of repositories found on the way do not
    // depend on the order of the record.
    let mut packages: Vec<&RecordedPackage> = records.packages.iter().collect();
    packages.sort_by(|a, b| a.name.cmp(&b.name));
    for pkg in packages {
        let name = &pkg.name;
        let git = match &pkg.source {
            PackageSource::Git(remote) => remote.dep_table().map(|table| (remote, table)),
            _ => None,
        };
        if let Some((remote, mut table)) = git {
            if let Some(url) = remote.remote_url() {
                if github_https_url(url).is_some() {
                    report.github_git_urls.push(name.clone());
                } else if !is_http_url(url) {
                    report.other_git_urls.push(name.clone());
                }
            }
            let url = table.git.clone().unwrap_or_default();
            if let Some(branch) = table.branch.clone() {
                match (options.ref_kind)(&url, &branch) {
                    Some(RefKind::Branch) => {}
                    Some(RefKind::Tag) => table.tag = table.branch.take(),
                    None => report.unknown_refs.push(name.clone()),
                }
            }
            if let (None, Some(sha)) = (&table.rev, &remote.sha) {
                let refspec = table.branch.clone().or_else(|| table.tag.clone());
                let key = (url, refspec, table.subdir.clone());
                report.locked_git_shas.push((key, sha.clone()));
            }
            set_git_source(manifest, name, &table);
            continue;
        }
        manifest.add_dependency(name, &format!("^{}", pkg.version), false);
        let placement = repos.placement(pkg);
        if !matches!(placement, Placement::OtherSource) && !is_base_package(name) {
            report
                .locked_versions
                .push((name.clone(), format!("= {}", pkg.version)));
        }
        match placement {
            Placement::Pin(repo) => {
                if set_repository(manifest, name, Some(&repo)) {
                    bioc_used |= repo == BIOC_REPOSITORY_NAME;
                }
            }
            Placement::Cran => {
                if set_repository(manifest, name, None) {
                    from_cran.push(name.clone());
                }
            }
            Placement::Unpin => {
                set_repository(manifest, name, None);
            }
            Placement::Unresolved => report.unresolved.push(name.clone()),
            Placement::OtherSource if matches!(pkg.source, PackageSource::Git(_)) => {
                report.incomplete_git.push(name.clone())
            }
            Placement::OtherSource => report.other_sources.push(name.clone()),
            Placement::Keep => {}
        }
    }

    let other_bioc = std::mem::take(&mut repos.other_bioc);
    let mut entries = repos.finish(bioc_used);
    if let Some(bioc_version) = &records.bioc_version {
        entries.insert(
            0,
            Repository {
                version: Some(bioc_version.clone()),
                explicit: Some(true),
                ..Repository::builtin(BIOC_REPOSITORY_NAME)
            },
        );
    }
    // When merging, the built-in Bioconductor entry goes last, so that it does
    // not take precedence over the recorded Bioconductor repositories.
    if !repositories {
        entries.sort_by_key(|e| e.is_bioc());
    }
    for entry in entries {
        let existing = manifest
            .repository
            .iter()
            .find(|r| r.name.eq_ignore_ascii_case(&entry.name));
        match existing {
            Some(old) if old.url != entry.url => {
                warn!(
                    "Imported repository {} differs from the one in {}, keeping the latter",
                    entry.name, RPROJ_MANIFEST_FILE
                );
            }
            Some(_) => {}
            None if repositories || !entry.is_builtin() => manifest.repository.push(entry),
            // The manifest gets the recorded Bioconductor release, but
            // it keeps using Bioconductor for packages that are not pinned.
            None if entry.is_bioc() && entry.version.is_some() => {
                manifest.repository.push(Repository {
                    explicit: None,
                    ..entry
                })
            }
            None => {}
        }
    }

    // A repository that serves the Bioconductor packages cannot be explicit.
    for repo in manifest
        .repository
        .iter_mut()
        .filter(|r| other_bioc.contains(&r.name))
    {
        repo.explicit = None;
    }

    // The merged manifest must be usable, e.g. a package cannot be pinned to a
    // repository that is turned off, or to two repositories.
    manifest
        .check_repositories()
        .and_then(|_| ProjRepos::from_manifest(manifest))
        .and_then(|repos| repos.check())
        .map_err(|e| {
            format!(
                "Cannot import the packages into {}: {}",
                RPROJ_MANIFEST_FILE, e
            )
        })?;

    // A package from CRAN needs CRAN, or another repository that serves every
    // package, e.g. a CRAN mirror, unless another table of the manifest pins it.
    let pins = manifest.repository_pins()?;
    let serves_cran = |r: &Repository| r.is_enabled() && !r.is_explicit() && !r.is_bioc();
    if let Some(pkg) = from_cran.iter().find(|p| !pins.contains_key(*p)) {
        if manifest.repository.iter().any(|r| r.is_cran())
            && !manifest.repository.iter().any(serves_cran)
        {
            bail!(
                "Package `{}` is from CRAN, which is turned off in {}, and no other \
                 repository serves every package. Turn CRAN on to import it.",
                pkg,
                RPROJ_MANIFEST_FILE
            );
        }
    }
    Ok(report)
}

/// Make `git` the source of the dependency `name`: add it to
/// `[dependencies]`, and replace the entry of every other table that lists
/// it, so that the package has one source. Each entry keeps its `attach`,
/// `enhances` and `vignette-builder` flags, but not its version requirement
/// or repository pin: the git source replaces them, and it may not satisfy
/// them.
fn set_git_source(manifest: &mut Rproj, name: &str, git: &DepTable) {
    let entry = |old: Option<&Dependency>| {
        let table = match old {
            Some(Dependency::Detailed(old)) => DepTable {
                attach: old.attach,
                enhances: old.enhances,
                vignette_builder: old.vignette_builder,
                ..git.clone()
            },
            _ => git.clone(),
        };
        Dependency::Detailed(Box::new(table))
    };
    let new = entry(manifest.dependencies.get(name));
    manifest.dependencies.insert(name.to_string(), new);
    let others = std::iter::once(&mut manifest.linking_dependencies)
        .chain(
            manifest
                .dependency_groups
                .values_mut()
                .map(|g| &mut g.dependencies),
        )
        .chain(manifest.optional_dependencies.values_mut());
    for table in others {
        if let Some(dep) = table.get_mut(name) {
            *dep = entry(Some(dep));
        }
    }
}

/// Pin the dependency `name` to the repository `repo`, or remove its pin if
/// `repo` is `None`, unless it comes from somewhere else than a repository.
/// Returns whether it was changed.
fn set_repository(manifest: &mut Rproj, name: &str, repo: Option<&str>) -> bool {
    let Some(dep) = manifest.dependencies.get_mut(name) else {
        return false;
    };
    let mut table = match dep {
        Dependency::Detailed(table) if table.has_other_source() => return false,
        Dependency::Detailed(table) => (**table).clone(),
        Dependency::Version(version) => DepTable {
            version: Some(version.clone()),
            ..Default::default()
        },
    };
    table.repository = repo.map(str::to_string);
    let plain = DepTable {
        version: table.version.clone(),
        ..Default::default()
    };
    *dep = match table.version.clone() {
        Some(version) if table == plain => Dependency::Version(version),
        _ => Dependency::Detailed(Box::new(table)),
    };
    true
}

/// The name of the protocol rig uses for `repo`.
fn protocol_name(repo: &PkgRepo) -> &'static str {
    match repo {
        PkgRepo::Extended(_) => "extended",
        PkgRepo::Cranlike(r) if r.protocol == CranlikeProtocol::RUniverse => "r-universe",
        PkgRepo::Cranlike(_) => "cran",
    }
}

/// One line about each repository of `manifest`, after an import: its URL,
/// its type, the protocol rig uses for it, and which packages it serves.
/// `bioc_version` is the recorded Bioconductor release, if any, and
/// `detect` finds the type of a repository, see [`resolve_repo`].
fn repository_summary(
    manifest: &Rproj,
    bioc_version: Option<&str>,
    detect: impl Fn(&str, Option<RepoType>) -> DetectedRepo,
) -> Result<Vec<String>, Box<dyn Error>> {
    let pins = manifest.repository_pins()?;
    let mut lines = vec![];
    for repo in &manifest.repository {
        if !repo.is_enabled() {
            lines.push(format!("{}: off", repo.name));
            continue;
        }
        let mut parts = vec![];
        if repo.is_cran() {
            parts.push("built-in CRAN".to_string());
        } else if repo.is_bioc() {
            match repo.version.as_deref().or(bioc_version) {
                Some(v) => parts.push(format!("built-in Bioconductor {}", v)),
                None => parts.push("built-in Bioconductor".to_string()),
            }
        }
        let mut protocols: Vec<&str> = match repo.url.as_deref() {
            Some(url) if !repo.is_builtin() => {
                let detected = detect(url, repo.repo_type);
                parts.push(url.to_string());
                let unreachable =
                    detected.repo_type == RepoType::CranLike && !detected.notes.is_empty();
                parts.push(if unreachable {
                    "type unknown, it did not respond".to_string()
                } else if detected.guessed {
                    format!("type {} (guessed)", detected.repo_type.as_str())
                } else {
                    format!("type {}", detected.repo_type.as_str())
                });
                repos_for(&repo.name, &detected, repo.is_extended(), bioc_version)
                    .iter()
                    .map(protocol_name)
                    .collect()
            }
            _ if repo.is_extended() => vec!["extended"],
            _ => vec!["cran"],
        };
        protocols.dedup();
        if !protocols.is_empty() {
            parts.push(format!("{} protocol", protocols.join(", ")));
        }
        let pinned = pins
            .values()
            .filter(|r| r.eq_ignore_ascii_case(&repo.name))
            .count();
        if repo.is_explicit() {
            parts.push(format!("only for {} pinned package(s)", pinned));
        } else if pinned > 0 {
            parts.push(format!("{} pinned package(s)", pinned));
        }
        lines.push(format!("{}: {}", repo.name, parts.join(", ")));
    }
    Ok(lines)
}
// ------------------------------------------------------------------------
// The import into a project

/// An import of another tool's record of a project's packages, e.g.
/// `renv.lock`, into the project in the current directory: `rproj.toml`, the
/// `.rvenv` layout of a new project, and `rproj.lock` with the recorded
/// versions.
///
/// The import takes these arguments: `--dependencies`, `--no-lock-versions`,
/// `--force`, `--r-version` and `--keep-ppm-snapshots`.
pub(crate) struct ProjectImport {
    root: PathBuf,
    /// `--dependencies`: only merge the dependencies into `rproj.toml`.
    dependencies_only: bool,
    /// Whether to lock the recorded versions in `rproj.lock`.
    lock_versions: bool,
    /// Whether every package has to come from where it was installed from,
    /// see [`ProjectImport::strict`].
    strict: bool,
}

impl ProjectImport {
    /// Check that the import can write the project files, before reading
    /// anything.
    pub(crate) fn start(args: &ArgMatches) -> Result<ProjectImport, Box<dyn Error>> {
        let dependencies_only = args.get_flag("dependencies");
        let root = std::env::current_dir()?;
        let path = root.join(RPROJ_MANIFEST_FILE);

        if !dependencies_only && path.exists() {
            let msg = format!(
                "{} already exists; import would only overwrite dependencies, not \
                 merge full metadata. Use --dependencies to merge into it, or \
                 remove it first.",
                RPROJ_MANIFEST_FILE
            );
            OUTPUT.error(&msg);
            error!("{}", msg);
            bail!("{}", msg);
        }

        // A full import also creates the `.rvenv` layout, so check for
        // conflicts before writing anything. `--dependencies` only touches the
        // manifest.
        if !dependencies_only && !args.get_flag("force") {
            check_project_conflicts(&root)?;
        }

        Ok(ProjectImport {
            root,
            dependencies_only,
            lock_versions: !args.get_flag("no-lock-versions"),
            strict: false,
        })
    }

    /// Fail, before writing anything, if a package cannot be imported from
    /// where it was installed from, instead of importing it as an ordinary
    /// version requirement with a warning. See [`strict_import_error`].
    pub(crate) fn strict(self) -> ProjectImport {
        ProjectImport {
            strict: true,
            ..self
        }
    }

    /// Import `records`, read from `input`, into the project. `platforms`
    /// become the `platforms` of `[tool.rig]`, if there are any, so that
    /// `rproj.lock` is for these platforms only.
    pub(crate) fn finish(
        self,
        args: &ArgMatches,
        input: &str,
        records: &PackageRecords,
        platforms: &[String],
    ) -> Result<(), Box<dyn Error>> {
        let root = self.root.as_path();
        let path = root.join(RPROJ_MANIFEST_FILE);
        let path = path.as_path();
        let dependencies_only = self.dependencies_only;
        let dep_count = records.packages.len();

        let default_name = || -> String {
            root.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "myproject".to_string())
        };

        let path_existed = path.exists();
        let mut manifest = if dependencies_only {
            if path_existed {
                toml::from_str::<Rproj>(&fs::read_to_string(path)?)?
            } else {
                OUTPUT.status(&format!(
                    "{} does not exist, creating a new one",
                    RPROJ_MANIFEST_FILE
                ));
                info!("{} does not exist, creating a new one", RPROJ_MANIFEST_FILE);
                Rproj::minimal(&default_name())
            }
        } else {
            Rproj::minimal(&default_name())
        };

        let detect = |url: &str| resolve_repo(url, None);
        let options = ImportOptions {
            detect: &detect,
            keep_snapshots: args.get_flag("keep-ppm-snapshots"),
            ref_kind: &online_ref_kind,
        };
        let report = apply_records(
            &mut manifest,
            records,
            !dependencies_only || !path_existed,
            &options,
        )?;
        let minor = minor_r_version(&records.r_version)?;

        if self.strict {
            if let Some(msg) = strict_import_error(&report, input) {
                bail!("{}", msg);
            }
        }
        for msg in import_warnings(&report, input) {
            OUTPUT.warn(&msg);
            warn!("{}", msg);
        }

        // The project is for the recorded R version, unless the user asks for
        // another one. `r-versions` keeps `rig proj lock` at that version,
        // instead of solving for the default R, which would drop the recorded
        // versions.
        let r_version = args
            .get_one::<String>("r-version")
            .cloned()
            .unwrap_or_else(|| records.r_version.clone());
        manifest.set_r_versions(std::slice::from_ref(&r_version));
        if !platforms.is_empty() {
            manifest.set_platforms(platforms);
            manifest.platforms()?;
        }

        fs::write(path, toml::to_string_pretty(&manifest)?)?;

        // Detecting the types needs the network, so not if nobody looks.
        if !OUTPUT.is_quiet() && !manifest.repository.is_empty() {
            let bioc_version = records.bioc_version.as_deref();
            OUTPUT.info(&format!("Package repositories in {}:", RPROJ_MANIFEST_FILE));
            for line in repository_summary(&manifest, bioc_version, resolve_repo)? {
                OUTPUT.println(&format!("  {}", line));
            }
        }

        let msg = format!(
            "Imported R {} and {} dependencies from {} into {}",
            minor, dep_count, input, RPROJ_MANIFEST_FILE
        );
        OUTPUT.success(&msg);
        info!("{}", msg);

        // A full import sets up a whole project, not just its manifest, so it
        // creates the same `.rvenv` layout as `rig proj init`.
        if !dependencies_only {
            init_rvenv_files(root, path)?;
        }

        let next = if self.lock_versions {
            let git_shas = report.locked_git_shas.into_iter().collect();
            if let Err(e) =
                proj_lock_locked_versions(root, &r_version, report.locked_versions, git_shas, args)
            {
                let msg = format!(
                    "Cannot lock the package versions of {}. {} is ready, run \
                     `rig proj lock` to lock compatible versions instead, or import \
                     again with --no-lock-versions.",
                    input, RPROJ_MANIFEST_FILE
                );
                OUTPUT.info(&msg);
                info!("{}", msg);
                return Err(e);
            }
            "`rig proj sync`"
        } else {
            "`rig proj lock` and `rig proj sync`"
        };
        if !dependencies_only {
            OUTPUT.info(&format!(
                "Project set up for R {}. Next: run {}.",
                r_version, next
            ));
        }

        Ok(())
    }
}

/// Whether `name` is a branch or a tag of the git repository at `url`, asking
/// the repository. `None` if it cannot tell.
fn online_ref_kind(url: &str, name: &str) -> Option<RefKind> {
    match remote_ref_kind(url, name) {
        Ok(kind) => Some(kind),
        Err(e) => {
            info!("Cannot tell if {} is a branch or a tag: {}", name, e);
            None
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const INTERNAL_URL: &str = "https://pkgs.example.com/prod";
    const PRIVATE_BIOC_URL: &str = "https://ppm.example.com/bioconductor/latest/packages/3.19/bioc";
    const BIOC_SNAPSHOT_URL: &str =
        "https://ppm.example.com/bioconductor/2024-05-01/packages/3.19/bioc";

    /// The type of the repository at `url`, as the detection finds it: CRAN
    /// mirrors, Posit Package Manager, Bioconductor mirrors and r-universe by
    /// their URL, anything else is a CRAN-like repository. `ppm.example.com`
    /// is a private PPM.
    pub(crate) fn test_detect(url: &str) -> DetectedRepo {
        use crate::repos::detect::describe;
        let lower = url.trim().to_ascii_lowercase();
        let rest = lower.split_once("://").map_or(lower.as_str(), |(_, r)| r);
        let host = rest.split('/').next().unwrap_or_default();
        let host = host.rsplit('@').next().unwrap_or_default();
        match host {
            "cloud.r-project.org"
            | "cran.r-project.org"
            | "cran.rstudio.com"
            | "cran.example.com" => describe(url, RepoType::Cran),
            "bioconductor.org" => describe(url, RepoType::Bioc),
            "p3m.dev"
            | "packagemanager.posit.co"
            | "packagemanager.rstudio.com"
            | "ppm.example.com" => {
                let path = describe(url, RepoType::PpmOther);
                let rtype = match (path.snapshot.as_deref(), path.ppm_repo.as_deref()) {
                    (Some(s), _) if s != "latest" => RepoType::PpmSnapshot,
                    (_, Some("bioconductor")) => RepoType::PpmBioc,
                    (_, Some("cran")) => RepoType::PpmCran,
                    _ => RepoType::PpmOther,
                };
                describe(url, rtype)
            }
            _ if host.ends_with(".r-universe.dev") => describe(url, RepoType::RUniverse),
            // Like the detection, a URL that ends in `packages/<version>/bioc`,
            // etc. is a Bioconductor repository.
            _ if describe(url, RepoType::Bioc).bioc_version.is_some() => {
                describe(url, RepoType::Bioc)
            }
            _ => describe(url, RepoType::CranLike),
        }
    }

    /// Whether `name` is a branch or a tag of the git repository at `url`:
    /// names like `v1.2.0` are tags, `missing` is neither, anything else is a
    /// branch.
    pub(crate) fn test_ref_kind(_url: &str, name: &str) -> Option<RefKind> {
        let mut chars = name.chars();
        match (chars.next(), chars.next()) {
            _ if name == "missing" => None,
            (Some('v'), Some(c)) if c.is_ascii_digit() => Some(RefKind::Tag),
            _ => Some(RefKind::Branch),
        }
    }

    const OPTIONS: ImportOptions<'static> = ImportOptions {
        detect: &test_detect,
        keep_snapshots: false,
        ref_kind: &test_ref_kind,
    };

    /// A recorded repository.
    fn repo(name: &str, url: &str) -> RecordedRepository {
        RecordedRepository {
            name: name.to_string(),
            url: url.to_string(),
            bioconductor: false,
        }
    }

    fn cran() -> RecordedRepository {
        repo("CRAN", "https://cloud.r-project.org")
    }

    /// A recorded package from the repository `repository`.
    fn pkg(name: &str, version: &str, repository: &str) -> RecordedPackage {
        RecordedPackage {
            name: name.to_string(),
            version: version.to_string(),
            source: PackageSource::Repository(Some(RepoReference {
                repository: repository.to_string(),
                name: None,
            })),
        }
    }

    /// Records for R 4.4.1 of these repositories and packages.
    fn records(
        repositories: &[RecordedRepository],
        packages: &[RecordedPackage],
    ) -> PackageRecords {
        PackageRecords {
            r_version: "4.4.1".to_string(),
            bioc_version: None,
            repositories: repositories.to_vec(),
            packages: packages.to_vec(),
        }
    }

    /// Import `records` into a new manifest, which must be valid.
    fn import(records: PackageRecords) -> Rproj {
        let mut manifest = Rproj::minimal("proj");
        apply_records(&mut manifest, &records, true, &OPTIONS).unwrap();
        manifest.check_repositories().unwrap();
        manifest
    }

    fn bioc_off() -> Repository {
        Repository::disabled(BIOC_REPOSITORY_NAME)
    }

    fn git(kind: &str) -> GitRemote {
        GitRemote {
            kind: kind.to_string(),
            username: Some("acme".to_string()),
            repo: Some("pkg".to_string()),
            sha: Some("1111111111111111111111111111111111111111".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn git_remotes_are_git_dependencies_at_their_ref() {
        let t = GitRemote {
            reference: Some("main".to_string()),
            ..git("github")
        }
        .dep_table()
        .unwrap();
        assert_eq!(t.git.as_deref(), Some("https://github.com/acme/pkg.git"));
        assert_eq!(t.branch.as_deref(), Some("main"));
        assert_eq!(t.rev, None);

        // The default branch, the commit goes to `rproj.lock`.
        let t = GitRemote {
            reference: Some("HEAD".to_string()),
            ..git("gitlab")
        }
        .dep_table()
        .unwrap();
        assert_eq!(t.git.as_deref(), Some("https://gitlab.com/acme/pkg.git"));
        assert_eq!((t.branch, t.rev), (None, None));

        // Without a ref, the commit is all there is.
        let t = git("bitbucket").dep_table().unwrap();
        assert_eq!(t.git.as_deref(), Some("https://bitbucket.org/acme/pkg.git"));
        assert_eq!(t.rev.as_deref(), git("x").sha.as_deref());
    }

    #[test]
    fn a_git_remote_needs_a_location() {
        let none = |remote: GitRemote| remote.dep_table();
        assert_eq!(
            none(GitRemote {
                username: None,
                ..git("github")
            }),
            None
        );
        assert_eq!(none(git("git")), None);
        assert_eq!(none(git("svn")), None);
        let url = GitRemote {
            url: Some("git://github.com/acme/pkg".to_string()),
            ..git("git")
        };
        assert_eq!(
            url.dep_table().unwrap().git.as_deref(),
            Some("https://github.com/acme/pkg")
        );
    }

    #[test]
    fn git_records_lock_their_commit() {
        let mut manifest = Rproj::minimal("proj");
        let remote = GitRemote {
            reference: Some("v1.0.0".to_string()),
            ..git("github")
        };
        let records = records(
            &[],
            &[RecordedPackage {
                name: "pkg".to_string(),
                version: "1.0.0".to_string(),
                source: PackageSource::Git(remote.clone()),
            }],
        );
        let report = apply_records(&mut manifest, &records, true, &OPTIONS).unwrap();
        let url = "https://github.com/acme/pkg.git".to_string();
        assert_eq!(
            report.locked_git_shas,
            vec![(
                (url, Some("v1.0.0".to_string()), None),
                remote.sha.clone().unwrap()
            )]
        );
        assert!(report.locked_versions.is_empty());
        match manifest.dependencies.get("pkg") {
            Some(Dependency::Detailed(t)) => assert_eq!(t.tag.as_deref(), Some("v1.0.0")),
            other => panic!("{:?}", other),
        }
    }

    /// How a repository of type `rtype` at `url` is imported.
    fn import_as_type(url: &str, rtype: RepoType, bioc_version: Option<&str>) -> ImportAs {
        let d = crate::repos::detect::describe(url, rtype);
        import_as(&d, url, bioc_version, false)
    }

    fn own(url: &str) -> ImportAs {
        ImportAs::Url(url.to_string())
    }

    #[test]
    fn public_cran_repositories_are_the_builtin_cran() {
        let cran = ImportAs::Builtin(CRAN_REPOSITORY_NAME);
        for url in [
            "https://cloud.r-project.org/",
            "http://CRAN.r-project.org",
            "https://cran.rstudio.com",
        ] {
            assert_eq!(import_as_type(url, RepoType::Cran, None), cran, "{}", url);
        }
        for url in [
            "https://p3m.dev/cran/__linux__/noble/latest",
            "https://packagemanager.posit.co/cran/latest",
        ] {
            assert_eq!(
                import_as_type(url, RepoType::PpmCran, None),
                cran,
                "{}",
                url
            );
        }
        // Another repository of the public PPM is not CRAN.
        let url = "https://p3m.dev/all/latest";
        assert_eq!(import_as_type(url, RepoType::PpmOther, None), own(url));
    }

    #[test]
    fn private_repositories_are_kept() {
        for (url, rtype) in [
            ("https://cran.example.com", RepoType::Cran),
            ("https://ppm.example.com/cran/latest", RepoType::PpmCran),
            (PRIVATE_BIOC_URL, RepoType::PpmBioc),
            (
                "https://pkgs.example.com/packages/3.19/bioc",
                RepoType::Bioc,
            ),
            ("https://cloud.r-project.org.example.com", RepoType::Cran),
        ] {
            assert_eq!(
                import_as_type(url, rtype, Some("3.19")),
                own(url),
                "{}",
                url
            );
        }
    }

    #[test]
    fn public_bioconductor_of_the_lock_files_release_is_the_builtin_bioconductor() {
        let bioc = ImportAs::Builtin(BIOC_REPOSITORY_NAME);
        let url = "https://bioconductor.org/packages/3.18/bioc";
        assert_eq!(import_as_type(url, RepoType::Bioc, Some("3.18")), bioc);
        assert_eq!(import_as_type(url, RepoType::Bioc, Some("3.19")), own(url));
        assert_eq!(import_as_type(url, RepoType::Bioc, None), own(url));
        let url = "https://p3m.dev/bioconductor/__linux__/noble/latest/packages/3.18/bioc";
        assert_eq!(import_as_type(url, RepoType::PpmBioc, Some("3.18")), bioc);
    }

    #[test]
    fn public_ppm_snapshots_are_builtin_unless_kept() {
        let cran = "https://p3m.dev/cran/__linux__/noble/2024-05-01";
        let bioc = "https://p3m.dev/bioconductor/2024-05-01/packages/3.18/bioc";
        assert_eq!(
            import_as_type(cran, RepoType::PpmSnapshot, None),
            ImportAs::Builtin(CRAN_REPOSITORY_NAME)
        );
        assert_eq!(
            import_as_type(bioc, RepoType::PpmSnapshot, Some("3.18")),
            ImportAs::Builtin(BIOC_REPOSITORY_NAME)
        );
        assert_eq!(import_as_type(bioc, RepoType::PpmSnapshot, None), own(bioc));

        let d = crate::repos::detect::describe(cran, RepoType::PpmSnapshot);
        assert_eq!(import_as(&d, cran, None, true), own(cran));
    }

    #[test]
    fn private_ppm_snapshots_are_their_repository_at_latest_unless_kept() {
        for (snapshot, latest) in [
            (
                "https://ppm.example.com/cran/2024-05-01",
                "https://ppm.example.com/cran/latest",
            ),
            (
                "https://ppm.example.com/internal/__linux__/noble/2024-05-01/",
                "https://ppm.example.com/internal/latest",
            ),
            (BIOC_SNAPSHOT_URL, PRIVATE_BIOC_URL),
        ] {
            assert_eq!(
                import_as_type(snapshot, RepoType::PpmSnapshot, Some("3.19")),
                own(latest),
                "{}",
                snapshot
            );
            let d = crate::repos::detect::describe(snapshot, RepoType::PpmSnapshot);
            assert_eq!(import_as(&d, snapshot, None, true), own(snapshot));
        }
    }

    #[test]
    fn source_urls_of_linux_binary_urls() {
        assert_eq!(
            strip_binary_path("https://p3m.dev/cran/__linux__/noble/2024-05-01").as_deref(),
            Some("https://p3m.dev/cran/2024-05-01")
        );
        assert_eq!(
            strip_binary_path(
                "https://ppm.example.com/bioconductor/__linux__/manylinux_2_28/latest/packages/3.19/bioc"
            )
            .as_deref(),
            Some("https://ppm.example.com/bioconductor/latest/packages/3.19/bioc")
        );
        assert_eq!(strip_binary_path("https://p3m.dev/cran/2024-05-01"), None);
        assert_eq!(
            strip_binary_path("https://p3m.dev/cran/__linux__/noble"),
            None
        );
    }

    #[test]
    fn a_non_ascii_repository_name_is_not_a_url() {
        assert!(!is_url("日本語"));
        assert!(!is_url("日本語日本語"));
        assert!(is_url("HTTPS://example.com"));
        let mut repos = ImportedRepositories::new(&[], None, &[], &test_detect, false);
        assert_eq!(repos.resolve("日本語のリポジトリ", None), None);
    }

    #[test]
    fn credentials_of_a_repository_url_keep_their_case() {
        assert_ne!(
            normalize_repo_url("https://Alice:Secret@host/cran"),
            normalize_repo_url("https://alice:secret@host/cran")
        );
        assert_eq!(
            normalize_repo_url("https://Alice:Secret@HOST.example.com/cran/"),
            "https://Alice:Secret@host.example.com/cran"
        );
    }

    #[test]
    fn two_names_for_cran_are_one_repository() {
        let mut repos = ImportedRepositories::new(
            &[
                repo("RSPM", "https://packagemanager.posit.co/cran/latest"),
                repo("CRAN", "https://cloud.r-project.org"),
            ],
            None,
            &[],
            &test_detect,
            false,
        );
        assert_eq!(repos.resolve("RSPM", None).as_deref(), Some("cran"));
        assert_eq!(repos.resolve("cran", None).as_deref(), Some("cran"));
        assert_eq!(repos.finish(false), vec![bioc_off()]);
    }

    #[test]
    fn pins_do_not_override_other_sources() {
        let mut manifest = Rproj::minimal("proj");
        manifest.dependencies.insert(
            "acme".to_string(),
            Dependency::Detailed(Box::new(DepTable {
                git: Some("https://github.com/x/acme".to_string()),
                ..Default::default()
            })),
        );
        assert!(!set_repository(&mut manifest, "acme", Some("internal")));
        manifest.check_repositories().unwrap();
    }

    /// A detection that finds the type `rtype` for every URL, unless the
    /// type is declared.
    fn detect_as(rtype: RepoType) -> impl Fn(&str, Option<RepoType>) -> DetectedRepo {
        move |url, declared| crate::repos::detect::describe(url, declared.unwrap_or(rtype))
    }

    #[test]
    fn the_summary_shows_the_type_and_protocol_of_each_repository() {
        let m = import(records(
            &[cran(), repo("internal", INTERNAL_URL)],
            &[
                pkg("acme", "1.0.0", "internal"),
                pkg("acme2", "1.0.0", "internal"),
                pkg("cli", "3.6.3", "CRAN"),
            ],
        ));
        let lines = repository_summary(&m, None, detect_as(RepoType::PpmOther)).unwrap();
        assert_eq!(
            lines,
            vec![
                "cran: built-in CRAN, extended protocol".to_string(),
                format!(
                    "internal: {}, type ppm-other, cran protocol, only for 2 pinned package(s)",
                    INTERNAL_URL
                ),
                "bioc: off".to_string(),
            ]
        );
    }

    #[test]
    fn the_summary_uses_the_detected_protocol() {
        let m = import(records(
            &[repo("acme", "https://acme.r-universe.dev")],
            &[pkg("acme", "1.0.0", "acme")],
        ));
        let lines = repository_summary(&m, None, detect_as(RepoType::RUniverse)).unwrap();
        assert_eq!(
            lines[0],
            "acme: https://acme.r-universe.dev, type r-universe, r-universe protocol, \
             only for 1 pinned package(s)"
        );

        let lines = repository_summary(&m, None, detect_as(RepoType::Cran)).unwrap();
        assert!(
            lines[0].contains("type cran, extended protocol"),
            "{}",
            lines[0]
        );
    }

    #[test]
    fn the_summary_reports_repositories_that_did_not_respond() {
        let m = import(records(
            &[repo("internal", INTERNAL_URL)],
            &[pkg("acme", "1.0.0", "internal")],
        ));
        let down = |url: &str, _: Option<RepoType>| {
            let mut d = DetectedRepo::new(url, RepoType::CranLike);
            d.notes
                .push("src/contrib/PACKAGES did not respond".to_string());
            d
        };
        let lines = repository_summary(&m, None, down).unwrap();
        assert!(
            lines[0].contains("type unknown, it did not respond, cran protocol"),
            "{}",
            lines[0]
        );
    }

    #[test]
    fn the_summary_shows_the_bioconductor_release() {
        let m = import(PackageRecords {
            bioc_version: Some("3.19".to_string()),
            ..records(
                &[
                    cran(),
                    RecordedRepository {
                        bioconductor: true,
                        ..repo("BioCsoft", "https://bioconductor.org/packages/3.19/bioc")
                    },
                ],
                &[RecordedPackage {
                    name: "limma".to_string(),
                    version: "3.60.0".to_string(),
                    source: PackageSource::Bioconductor(None),
                }],
            )
        });
        let lines = repository_summary(&m, Some("3.19"), detect_as(RepoType::Bioc)).unwrap();
        assert!(
            lines.contains(
                &"bioc: built-in Bioconductor 3.19, extended protocol, only for 1 pinned package(s)"
                    .to_string()
            ),
            "{:?}",
            lines
        );
    }
}
