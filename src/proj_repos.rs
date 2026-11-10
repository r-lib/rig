//! The package repositories of a `rig proj` project.
//!
//! A project solves from the repositories of its `rproj.toml`, never from the
//! repositories of an R installation or of the rig configuration: the
//! built-in CRAN (P3M's metadata) and Bioconductor repositories, and the
//! repositories of its `[[repository]]` entries, see
//! [`crate::rproj::Repository`]. The `--with-repos` and `--without-repos`
//! arguments change them for one command.
//!
//! How rig reads a repository with a URL depends on its type, see
//! [`crate::repos::protocol`]. The type is declared with `type`, or it is
//! detected, see [`ProjRepos::resolve_types`].

use std::collections::BTreeMap;
use std::error::Error;

use crate::repos::detect::{describe, DetectedRepo, RepoType};
use crate::repos::feed::{BiocSetting, CranlikeRepo, MetadataFeed, PkgRepo, RepoFilter, RepoId};
use crate::repos::interpret_repos_args::{PkgReposArgs, ReposSetupArgs};
use crate::repos::protocol::repos_for;
use crate::repos::repo_types::resolve_repo;
use crate::rproj::{LockRepository, Repository, Rproj, BIOC_REPOSITORY_NAME, CRAN_REPOSITORY_NAME};

/// The repositories of a solve, and the dependencies pinned to them.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProjRepos {
    /// Every repository, in order of precedence, including the ones that are
    /// turned off. The built-in repositories are always here: the ones the
    /// manifest does not list come last, Bioconductor first, so it wins a
    /// tie with CRAN.
    entries: Vec<Repository>,
    /// Package name to the name of the repository it is pinned to.
    pins: BTreeMap<String, String>,
    /// The repositories with a URL, by lowercase name, with their type, see
    /// [`ProjRepos::resolve_types`].
    types: BTreeMap<String, DetectedRepo>,
}

/// CRAN, for the built-in `cran` entry with `extend = false`.
const CRAN_URL: &str = "https://cloud.r-project.org";

/// A Bioconductor version for [`ProjRepos::locked_repository_fits`], which
/// does not know the version of the target. Only the names and kinds of the
/// repositories matter there, not their URLs.
const ANY_BIOC_VERSION: &str = "0.0";

impl Default for ProjRepos {
    fn default() -> Self {
        ProjRepos::new(&[], BTreeMap::new())
    }
}

impl ProjRepos {
    /// The repositories `repos`, in this order, with the built-in ones
    /// added if they are not listed, and the dependency pins `pins`.
    pub fn new(repos: &[Repository], pins: BTreeMap<String, String>) -> ProjRepos {
        let mut entries = repos.to_vec();
        for name in [BIOC_REPOSITORY_NAME, CRAN_REPOSITORY_NAME] {
            if !entries.iter().any(|r| r.name.eq_ignore_ascii_case(name)) {
                entries.push(Repository::builtin(name));
            }
        }
        ProjRepos {
            entries,
            pins,
            types: BTreeMap::new(),
        }
    }

    /// The repositories and pins of a single manifest.
    pub fn from_manifest(manifest: &Rproj) -> Result<ProjRepos, Box<dyn Error>> {
        Ok(ProjRepos::new(
            &manifest.repository,
            manifest.repository_pins()?,
        ))
    }

    /// The repositories of the project in `root`, after the `--with-repos`
    /// and `--without-repos` arguments in `args`, if any.
    pub fn for_args(
        manifest: &Rproj,
        args: &clap::ArgMatches,
    ) -> Result<ProjRepos, Box<dyn Error>> {
        let mut repos = ProjRepos::from_manifest(manifest)?;
        if let Some(over) = crate::repos::interpret_repos_args::interpret_pkg_repos_args(args)? {
            repos.apply_args(&over)?;
        }
        repos.check()?;
        repos.resolve_types(&[]);
        Ok(repos)
    }

    /// Find out the type of every repository with a URL that is on: the
    /// declared `type`, or the one in the lock file's repositories `locked`
    /// for the same name and URL, or detect it. Without this, a repository
    /// without a declared type is a plain CRAN-like repository.
    pub fn resolve_types(&mut self, locked: &[crate::rproj::LockRepository]) {
        let mut types = BTreeMap::new();
        for repo in self.enabled() {
            let Some(url) = repo.url.as_deref() else {
                continue;
            };
            let url = url.trim_end_matches('/');
            let from_lock = locked
                .iter()
                .find(|l| {
                    l.name.eq_ignore_ascii_case(&repo.name)
                        && l.url.as_deref().map(|u| u.trim_end_matches('/')) == Some(url)
                })
                .and_then(|l| l.repo_type);
            let detected = resolve_repo(url, repo.repo_type.or(from_lock));
            types.insert(repo.name.to_lowercase(), detected);
        }
        self.types = types;
    }

    /// Set the type of repository `name`, instead of
    /// [`ProjRepos::resolve_types`].
    #[cfg(test)]
    pub fn set_type(&mut self, name: &str, repo: DetectedRepo) {
        self.types.insert(name.to_lowercase(), repo);
    }

    /// The repository with a URL `repo`, with its type.
    fn detected(&self, repo: &Repository) -> DetectedRepo {
        if let Some(d) = self.types.get(&repo.name.to_lowercase()) {
            return d.clone();
        }
        let url = repo.url.as_deref().unwrap_or_default();
        describe(url, repo.repo_type.unwrap_or(RepoType::CranLike))
    }

    /// The repositories of a solve for the entry `repo`, with Bioconductor
    /// release `bioc_version`.
    fn repos_of(&self, repo: &Repository, bioc_version: Option<&str>) -> Vec<PkgRepo> {
        let extend = repo.is_extended();
        if repo.is_cran() {
            if extend {
                vec![PkgRepo::Extended(MetadataFeed::cran())]
            } else {
                vec![PkgRepo::Cranlike(CranlikeRepo::new(
                    CRAN_REPOSITORY_NAME,
                    CRAN_URL,
                ))]
            }
        } else if repo.is_bioc() {
            let mirror = DetectedRepo::new(&crate::repos::bioc_mirror(), RepoType::Bioc);
            repos_for(BIOC_REPOSITORY_NAME, &mirror, extend, bioc_version)
        } else {
            repos_for(&repo.name, &self.detected(repo), extend, bioc_version)
        }
    }

    /// Replace the dependency pins, e.g. with the pins of every workspace
    /// member.
    pub fn set_pins(&mut self, pins: BTreeMap<String, String>) {
        self.pins = pins;
    }

    fn find(&self, name: &str) -> Option<&Repository> {
        self.entries
            .iter()
            .find(|r| r.name.eq_ignore_ascii_case(name))
    }

    fn find_mut(&mut self, name: &str) -> Option<&mut Repository> {
        self.entries
            .iter_mut()
            .find(|r| r.name.eq_ignore_ascii_case(name))
    }

    /// The names of all repositories, for error messages.
    fn names(&self) -> String {
        self.entries
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Apply `--with-repos` and `--without-repos`. A repository name turns a
    /// repository on or off, a bare `--without-repos` turns every repository
    /// off that `--with-repos` does not name. A repository given by URL comes
    /// first, before the project's own.
    pub fn apply_args(&mut self, over: &PkgReposArgs) -> Result<(), Box<dyn Error>> {
        let (whitelist, blacklist): (&[String], &[String]) = match &over.setup {
            ReposSetupArgs::Default {
                whitelist,
                blacklist,
            } => (whitelist, blacklist),
            ReposSetupArgs::Empty { whitelist } => (whitelist, &[]),
        };
        for name in whitelist.iter().chain(blacklist.iter()) {
            if self.find(name).is_none() {
                bail!(
                    "Unknown repository `{}`, the repositories of the project are: {}",
                    name,
                    self.names()
                );
            }
        }
        if over.is_empty_base() {
            for repo in self.entries.iter_mut() {
                repo.enabled = Some(false);
            }
        }
        for name in blacklist {
            if let Some(repo) = self.find_mut(name) {
                repo.enabled = Some(false);
            }
        }
        for name in whitelist {
            if let Some(repo) = self.find_mut(name) {
                repo.enabled = Some(true);
            }
        }

        let mut first: Vec<Repository> = vec![];
        for (name, url) in &over.urls {
            let url = url.trim_end_matches('/');
            if let Some(pos) = self
                .entries
                .iter()
                .position(|r| r.name.eq_ignore_ascii_case(name))
            {
                let same_url = self.entries[pos]
                    .url
                    .as_deref()
                    .is_some_and(|u| u.trim_end_matches('/') == url);
                if !same_url {
                    bail!(
                        "Repository `{}` of --with-repos is already a repository of the \
                         project, with another URL",
                        name
                    );
                }
                self.entries.remove(pos);
            }
            first.push(Repository::at_url(name, url));
        }
        first.append(&mut self.entries);
        self.entries = first;
        Ok(())
    }

    /// Check that at least one repository is on, and that every pin names a
    /// repository that is on.
    pub fn check(&self) -> Result<(), Box<dyn Error>> {
        if !self.entries.iter().any(|r| r.is_enabled()) {
            bail!("All repositories are turned off, there is nothing to solve from");
        }
        for (pkg, name) in &self.pins {
            match self.find(name) {
                None => bail!(
                    "Dependency `{}` is pinned to repository `{}`, which is not a \
                     repository of the project, those are: {}",
                    pkg,
                    name,
                    self.names()
                ),
                Some(repo) if !repo.is_enabled() => bail!(
                    "Dependency `{}` is pinned to repository `{}`, which is turned off",
                    pkg,
                    name
                ),
                Some(_) => {}
            }
        }
        Ok(())
    }

    /// The repositories that are on, in order of precedence.
    pub fn enabled(&self) -> impl Iterator<Item = &Repository> {
        self.entries.iter().filter(|r| r.is_enabled())
    }

    /// Whether and which Bioconductor release to use. `RIG_BIOCONDUCTOR`
    /// turns Bioconductor off for every project.
    pub fn bioc_setting(&self) -> BiocSetting {
        let mut setting = BiocSetting::default();
        if let Some(repo) = self.find(BIOC_REPOSITORY_NAME) {
            setting.enabled = setting.enabled && repo.is_enabled();
            setting.version = repo.version.clone();
        }
        setting
    }

    /// The repository ids of `repo` in a solve with Bioconductor release
    /// `bioc_version`. None for Bioconductor without a release.
    fn repo_ids(&self, repo: &Repository, bioc_version: Option<&str>) -> Vec<RepoId> {
        self.repos_of(repo, bioc_version)
            .iter()
            .map(|r| r.repo_id())
            .collect()
    }

    /// The repositories of a solve with Bioconductor release
    /// `bioc_version`, see [`ProjRepos::bioc_setting`], in order. If two
    /// entries give the same repository, e.g. a CRAN mirror and the built-in
    /// CRAN, the first one wins.
    pub fn pkg_repos(&self, bioc_version: Option<&str>) -> Vec<PkgRepo> {
        let mut out: Vec<PkgRepo> = vec![];
        for repo in self.enabled().flat_map(|r| self.repos_of(r, bioc_version)) {
            if !out.iter().any(|o| o.repo_id() == repo.repo_id()) {
                out.push(repo);
            }
        }
        out
    }

    /// The pins and the explicit repositories of a solve with Bioconductor
    /// release `bioc_version`.
    pub fn repo_filter(&self, bioc_version: Option<&str>) -> RepoFilter {
        let mut filter = RepoFilter::default();
        for (pkg, name) in &self.pins {
            let ids = self
                .find(name)
                .map(|repo| self.repo_ids(repo, bioc_version))
                .unwrap_or_default();
            filter.pins.insert(pkg.clone(), ids);
        }
        for repo in self.enabled().filter(|r| r.is_explicit()) {
            filter.explicit.extend(self.repo_ids(repo, bioc_version));
        }
        filter
    }

    /// The repositories to record in the lock file, the ones that are on, in
    /// order of precedence, see
    /// [`crate::rproj::RprojLockOptions::repositories`]. The built-in ones
    /// have the URL of their extended metadata, unless it is turned off. The
    /// others have their type, if it is known.
    pub fn lock_repositories(&self) -> Vec<LockRepository> {
        self.enabled()
            .map(|repo| {
                let extended = repo.is_extended();
                let (name, metadata, repo_type) = if repo.is_cran() {
                    (
                        CRAN_REPOSITORY_NAME.to_string(),
                        extended.then(MetadataFeed::cran_metadata_url),
                        None,
                    )
                } else if repo.is_bioc() {
                    (
                        BIOC_REPOSITORY_NAME.to_string(),
                        extended.then(MetadataFeed::bioc_metadata_url),
                        None,
                    )
                } else {
                    let repo_type = self
                        .types
                        .get(&repo.name.to_lowercase())
                        .map(|d| d.repo_type)
                        .or(repo.repo_type);
                    (repo.name.clone(), None, repo_type)
                };
                LockRepository {
                    name,
                    url: repo.url.clone(),
                    metadata,
                    repo_type,
                    extend: repo.extend,
                    explicit: repo.is_explicit(),
                }
            })
            .collect()
    }

    /// Whether the package of a lock file entry with repository `locked`
    /// (see [`crate::rproj::RprojLockPackage::repository`]) is where the pins
    /// say, if `package` is pinned, or not in an explicit repository, if it
    /// is not.
    pub fn locked_repository_fits(&self, package: &str, locked: Option<&str>) -> bool {
        let is = |repo: &Repository| -> bool {
            self.repo_ids(repo, Some(ANY_BIOC_VERSION))
                .iter()
                .any(|id| match id {
                    RepoId::Cran => locked.is_none(),
                    RepoId::Bioc(_) => locked.is_some_and(|l| l.starts_with("bioc/")),
                    RepoId::Cranlike { name, .. } => locked == Some(name.as_str()),
                })
        };
        match self.pins.get(package) {
            Some(name) => self.find(name).is_some_and(is),
            None => !self.enabled().filter(|r| r.is_explicit()).any(is),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repos::feed::CranlikeProtocol;

    fn args(with: &[&str], without: &[&str]) -> PkgReposArgs {
        let mut argv: Vec<String> = vec!["test".to_string()];
        for w in with {
            argv.push(format!("--with-repos={}", w));
        }
        for w in without {
            if w.is_empty() {
                argv.push("--without-repos".to_string());
            } else {
                argv.push(format!("--without-repos={}", w));
            }
        }
        let cmd = clap::Command::new("test").args(crate::args::proj_repos_args());
        let m = cmd.try_get_matches_from(argv).unwrap();
        crate::repos::interpret_repos_args::interpret_pkg_repos_args(&m)
            .unwrap()
            .unwrap()
    }

    fn names(repos: &ProjRepos) -> Vec<String> {
        repos.enabled().map(|r| r.name.clone()).collect()
    }

    fn acme() -> Repository {
        Repository::at_url("acme", "https://cran.acme.com/")
    }

    fn typed(name: &str, url: &str, rtype: RepoType) -> Repository {
        Repository {
            repo_type: Some(rtype),
            ..Repository::at_url(name, url)
        }
    }

    fn feed_mirror(repo: &PkgRepo) -> Option<String> {
        match repo {
            PkgRepo::Extended(f) => f.mirror.clone(),
            PkgRepo::Cranlike(_) => panic!("not a feed: {:?}", repo),
        }
    }

    #[test]
    fn a_cran_mirror_is_the_cran_feed() {
        let mirror = typed("acme", "https://cran.acme.com/", RepoType::Cran);
        let repos = ProjRepos::new(&[mirror], BTreeMap::new());
        let pkg = repos.pkg_repos(None);
        // The mirror and the built-in CRAN are the same feed, the first wins.
        assert_eq!(pkg, vec![PkgRepo::Extended(MetadataFeed::cran())]);
        assert_eq!(
            feed_mirror(&pkg[0]).as_deref(),
            Some("https://cran.acme.com")
        );

        // A package of the CRAN feed, `repository` is not set in the lock
        // file, is in the mirror.
        let mut pins = BTreeMap::new();
        pins.insert("cli".to_string(), "acme".to_string());
        let mut repos = repos;
        repos.set_pins(pins);
        assert!(repos.locked_repository_fits("cli", None));
        assert!(!repos.locked_repository_fits("cli", Some("acme")));
        assert_eq!(repos.repo_filter(None).pins["cli"], vec![RepoId::Cran]);

        let lock = repos.lock_repositories();
        assert_eq!(lock[0].repo_type, Some(RepoType::Cran));
        assert_eq!(lock[2].name, "cran");
        assert_eq!(lock[2].repo_type, None);
    }

    #[test]
    fn extend_false_gives_cranlike_repos() {
        let mut mirror = typed("acme", "https://cran.acme.com", RepoType::Cran);
        mirror.extend = Some(false);
        let mut cran = Repository::builtin("cran");
        cran.extend = Some(false);
        let mut bioc = Repository::builtin("bioc");
        bioc.extend = Some(false);
        let repos = ProjRepos::new(&[mirror, bioc, cran], BTreeMap::new());
        let ids: Vec<String> = repos
            .pkg_repos(Some("3.22"))
            .iter()
            .map(|r| r.repo_id().to_string())
            .collect();
        assert_eq!(
            ids,
            vec![
                "acme",
                "BioCsoft",
                "BioCann",
                "BioCexp",
                "BioCworkflows",
                "BioCbooks",
                "cran"
            ]
        );
        let lock = repos.lock_repositories();
        assert_eq!(lock[0].extend, Some(false));
        assert_eq!(lock[1].metadata, None);
        assert_eq!(lock[2].metadata, None);

        // A package pinned to Bioconductor may come from any of its
        // repositories.
        let mut pins = BTreeMap::new();
        pins.insert("limma".to_string(), "bioc".to_string());
        let mut repos = repos;
        repos.set_pins(pins);
        assert_eq!(repos.repo_filter(Some("3.22")).pins["limma"].len(), 5);
        assert!(repos.locked_repository_fits("limma", Some("BioCann")));
        assert!(!repos.locked_repository_fits("limma", Some("acme")));
    }

    #[test]
    fn detected_types_are_used() {
        let mut repos = ProjRepos::new(
            &[Repository::at_url("rlib", "https://r-lib.r-universe.dev")],
            BTreeMap::new(),
        );
        // Without a type, a plain CRAN-like repository.
        match &repos.pkg_repos(None)[0] {
            PkgRepo::Cranlike(r) => assert_eq!(r.protocol, CranlikeProtocol::Cran),
            other => panic!("{:?}", other),
        }
        repos.set_type(
            "rlib",
            DetectedRepo::new("https://r-lib.r-universe.dev", RepoType::RUniverse),
        );
        match &repos.pkg_repos(None)[0] {
            PkgRepo::Cranlike(r) => assert_eq!(r.protocol, CranlikeProtocol::RUniverse),
            other => panic!("{:?}", other),
        }
        assert_eq!(
            repos.lock_repositories()[0].repo_type,
            Some(RepoType::RUniverse)
        );
    }

    #[test]
    fn locked_types_are_reused_for_the_same_url() {
        let mut repos = ProjRepos::new(
            &[Repository::at_url("acme", "https://cran.acme.com/")],
            BTreeMap::new(),
        );
        let locked = vec![LockRepository {
            name: "ACME".to_string(),
            url: Some("https://cran.acme.com".to_string()),
            repo_type: Some(RepoType::Cran),
            ..Default::default()
        }];
        // A type from the lock file needs no detection.
        repos.resolve_types(&locked);
        assert!(matches!(repos.pkg_repos(None)[0], PkgRepo::Extended(_)));
    }

    #[test]
    fn builtin_repositories_are_added_last() {
        let repos = ProjRepos::new(&[acme()], BTreeMap::new());
        assert_eq!(names(&repos), vec!["acme", "bioc", "cran"]);
        let mut cran_first = Repository::builtin("CRAN");
        cran_first.enabled = Some(true);
        let repos = ProjRepos::new(&[cran_first, acme()], BTreeMap::new());
        assert_eq!(names(&repos), vec!["CRAN", "acme", "bioc"]);
    }

    #[test]
    fn all_repositories_are_recorded() {
        let lock = ProjRepos::default().lock_repositories();
        let names: Vec<&str> = lock.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["bioc", "cran"]);
        assert_eq!(lock[0].url, None);
        assert_eq!(
            lock[0].metadata.as_deref(),
            Some(MetadataFeed::bioc_metadata_url().as_str())
        );
        assert_eq!(
            lock[1].metadata.as_deref(),
            Some(MetadataFeed::cran_metadata_url().as_str())
        );

        let mut cran = Repository::builtin("CRAN");
        cran.enabled = Some(true);
        let repos = ProjRepos::new(&[cran, acme()], BTreeMap::new());
        let lock = repos.lock_repositories();
        let names: Vec<&str> = lock.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["cran", "acme", "bioc"]);
        assert_eq!(lock[1].url.as_deref(), Some("https://cran.acme.com/"));
        assert_eq!(lock[1].metadata, None);

        let mut off = Repository::builtin("bioc");
        off.enabled = Some(false);
        let lock = ProjRepos::new(&[off], BTreeMap::new()).lock_repositories();
        let names: Vec<&str> = lock.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["cran"]);
    }

    #[test]
    fn turning_repositories_off_and_on() {
        let mut repos = ProjRepos::new(&[acme()], BTreeMap::new());
        repos.apply_args(&args(&[], &["cran,bioc"])).unwrap();
        assert_eq!(names(&repos), vec!["acme"]);
        repos.apply_args(&args(&["cran"], &[])).unwrap();
        assert_eq!(names(&repos), vec!["acme", "cran"]);

        let mut repos = ProjRepos::new(&[acme()], BTreeMap::new());
        repos
            .apply_args(&args(
                &["bioc", "rlib=https://r-lib.r-universe.dev/"],
                &[""],
            ))
            .unwrap();
        assert_eq!(names(&repos), vec!["rlib", "bioc"]);
        assert_eq!(
            repos.pkg_repos(Some("3.22")),
            vec![
                PkgRepo::Cranlike(CranlikeRepo::new("rlib", "https://r-lib.r-universe.dev")),
                PkgRepo::Extended(MetadataFeed::bioc("3.22")),
            ]
        );

        let mut repos = ProjRepos::default();
        repos.apply_args(&args(&[], &[""])).unwrap();
        assert!(repos.check().is_err());

        let mut repos = ProjRepos::default();
        let err = repos.apply_args(&args(&["nope"], &[])).unwrap_err();
        assert!(err.to_string().contains("Unknown repository `nope`"));

        let mut repos = ProjRepos::new(&[acme()], BTreeMap::new());
        let err = repos
            .apply_args(&args(&["acme=https://other.com"], &[]))
            .unwrap_err();
        assert!(err.to_string().contains("another URL"));
        // The same URL again moves it first.
        let mut repos = ProjRepos::new(&[Repository::builtin("cran"), acme()], BTreeMap::new());
        repos
            .apply_args(&args(&["acme=https://cran.acme.com"], &[]))
            .unwrap();
        assert_eq!(names(&repos), vec!["acme", "cran", "bioc"]);
    }

    #[test]
    fn pins_and_explicit_repositories() {
        let mut explicit = acme();
        explicit.explicit = Some(true);
        let mut pins = BTreeMap::new();
        pins.insert("cli".to_string(), "acme".to_string());
        pins.insert("limma".to_string(), "bioc".to_string());
        let repos = ProjRepos::new(&[explicit], pins);
        repos.check().unwrap();

        let acme_id = CranlikeRepo::new("acme", "https://cran.acme.com").repo_id();
        let filter = repos.repo_filter(Some("3.22"));
        assert_eq!(filter.pins["cli"], vec![acme_id.clone()]);
        assert_eq!(filter.pins["limma"], vec![RepoId::Bioc("3.22".to_string())]);
        assert!(filter.explicit.contains(&acme_id));
        // No Bioconductor release, no versions for limma.
        assert!(repos.repo_filter(None).pins["limma"].is_empty());

        assert!(repos.locked_repository_fits("cli", Some("acme")));
        assert!(!repos.locked_repository_fits("cli", None));
        assert!(repos.locked_repository_fits("limma", Some("bioc/3.22")));
        assert!(repos.locked_repository_fits("pak", None));
        assert!(!repos.locked_repository_fits("pak", Some("acme")));

        let mut pins = BTreeMap::new();
        pins.insert("cli".to_string(), "nope".to_string());
        let err = ProjRepos::new(&[], pins).check().unwrap_err();
        assert!(err.to_string().contains("not a repository of the project"));

        let mut off = Repository::builtin("cran");
        off.enabled = Some(false);
        let mut pins = BTreeMap::new();
        pins.insert("cli".to_string(), "cran".to_string());
        let err = ProjRepos::new(&[off], pins).check().unwrap_err();
        assert!(err.to_string().contains("turned off"));
    }
}
