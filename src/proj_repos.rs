//! The package repositories of a `rig proj` project.
//!
//! A project solves from the repositories of its `rproj.toml`, never from the
//! repositories of an R installation or of the rig configuration: the
//! built-in CRAN (P3M's metadata) and Bioconductor repositories, and the
//! CRAN-like repositories of its `[[repository]]` entries, see
//! [`crate::rproj::Repository`]. The `--with-repos` and `--without-repos`
//! arguments change them for one command.

use std::collections::BTreeMap;
use std::error::Error;

use crate::repos::feed::{BiocSetting, CranlikeRepo, MetadataFeed, PkgRepo, RepoFilter, RepoId};
use crate::repos::interpret_repos_args::{PkgReposArgs, ReposSetupArgs};
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
}

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
        ProjRepos { entries, pins }
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
        Ok(repos)
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

    /// The repository id of `repo` in a solve with Bioconductor release
    /// `bioc_version`. `None` for Bioconductor without a release.
    fn repo_id(repo: &Repository, bioc_version: Option<&str>) -> Option<RepoId> {
        if repo.is_cran() {
            Some(RepoId::Cran)
        } else if repo.is_bioc() {
            bioc_version.map(|v| RepoId::Bioc(v.to_string()))
        } else {
            let url = repo.url.as_deref().unwrap_or_default();
            Some(CranlikeRepo::new(&repo.name, url).repo_id())
        }
    }

    /// The repositories of a solve with Bioconductor release
    /// `bioc_version`, see [`ProjRepos::bioc_setting`], in order.
    pub fn pkg_repos(&self, bioc_version: Option<&str>) -> Vec<PkgRepo> {
        self.enabled()
            .filter_map(|repo| {
                if repo.is_cran() {
                    Some(PkgRepo::Extended(MetadataFeed::cran()))
                } else if repo.is_bioc() {
                    bioc_version.map(|v| PkgRepo::Extended(MetadataFeed::bioc(v)))
                } else {
                    let url = repo.url.as_deref().unwrap_or_default();
                    Some(PkgRepo::Cranlike(CranlikeRepo::new(&repo.name, url)))
                }
            })
            .collect()
    }

    /// The pins and the explicit repositories of a solve with Bioconductor
    /// release `bioc_version`.
    pub fn repo_filter(&self, bioc_version: Option<&str>) -> RepoFilter {
        let mut filter = RepoFilter::default();
        for (pkg, name) in &self.pins {
            let id = self
                .find(name)
                .and_then(|repo| ProjRepos::repo_id(repo, bioc_version));
            filter.pins.insert(pkg.clone(), id);
        }
        for repo in self.enabled().filter(|r| r.is_explicit()) {
            if let Some(id) = ProjRepos::repo_id(repo, bioc_version) {
                filter.explicit.insert(id);
            }
        }
        filter
    }

    /// The repositories to record in the lock file, the ones that are on, in
    /// order of precedence, see
    /// [`crate::rproj::RprojLockOptions::repositories`]. The built-in ones
    /// have the URL of their extended metadata.
    pub fn lock_repositories(&self) -> Vec<LockRepository> {
        self.enabled()
            .map(|repo| {
                let (name, metadata) = if repo.is_cran() {
                    (
                        CRAN_REPOSITORY_NAME.to_string(),
                        Some(MetadataFeed::cran_metadata_url()),
                    )
                } else if repo.is_bioc() {
                    (
                        BIOC_REPOSITORY_NAME.to_string(),
                        Some(MetadataFeed::bioc_metadata_url()),
                    )
                } else {
                    (repo.name.clone(), None)
                };
                LockRepository {
                    name,
                    url: repo.url.clone(),
                    metadata,
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
            if repo.is_cran() {
                locked.is_none()
            } else if repo.is_bioc() {
                locked.is_some_and(|l| l.starts_with("bioc/"))
            } else {
                locked.is_some_and(|l| l == repo.name)
            }
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
        assert_eq!(filter.pins["cli"], Some(acme_id.clone()));
        assert_eq!(filter.pins["limma"], Some(RepoId::Bioc("3.22".to_string())));
        assert!(filter.explicit.contains(&acme_id));
        // No Bioconductor release, no versions for limma.
        assert_eq!(repos.repo_filter(None).pins["limma"], None);

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
