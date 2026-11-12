use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use clap::ArgMatches;
use log::{error, info};
use serde_derive::Deserialize;
use serde_derive::Serialize;

use crate::common::*;
use crate::dcf::{DepVersionSpec, RDepType};
use crate::import::{
    is_bioc_repository_name, is_url, GitRemote, PackageRecords, PackageSource, ProjectImport,
    RecordedPackage, RecordedRepository, RepoReference, CRAN_NAMES,
};
use crate::output::OUTPUT;
use crate::proj::{
    proj_binary_target, proj_lock_r_version, proj_read_manifest_deps, sc_proj_solve_project_deps,
    BASE_PKGS,
};
use crate::rversion::*;
use crate::solver::*;
use crate::utils::*;

pub fn sc_renv(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        Some(("export", s)) => sc_renv_export(s, args, mainargs),
        Some(("import", s)) => sc_renv_import(s, args, mainargs),
        _ => Ok(()), // unreachable
    }
}

/// Solve the project's dependencies (`rproj.toml`) for one `(R version,
/// platform)` target and write the result as `renv.lock`: `rig proj renv export`.
///
/// Always a single target -- `renv.lock` has no multi-target concept, unlike
/// `rproj.lock`. `--r-version` defaults to the same logic `rig proj lock`
/// uses (the default R version if it satisfies the manifest, else the
/// newest installed one that does, else the current release); `--platform`
/// defaults to this machine.
fn sc_renv_export(
    args: &ArgMatches,
    _renvargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let root = Path::new(".");
    let (_name, _version, mut pkg_deps) = proj_read_manifest_deps(root, true)?;
    pkg_deps.dependencies.push(DepVersionSpec {
        name: "renv".to_string(),
        constraints: vec![],
        types: vec![RDepType::Depends],
    });

    let rver = match args.get_one::<String>("r-version") {
        Some(rv) => rv.clone(),
        None => proj_lock_r_version(&pkg_deps, args)?,
    };
    let platform = args.get_one::<String>("platform").cloned();
    let target = proj_binary_target(platform.as_ref(), &rver)?;

    let (registry, solution) = sc_proj_solve_project_deps(&rver, &pkg_deps, target, None, true)?;
    OUTPUT.success("Solved dependencies");
    info!("Solved dependencies");

    let lockfile = REnvLockfile::from_solution(&registry, &solution);
    fs::write(
        root.join("renv.lock"),
        serde_json::to_string_pretty(&lockfile)?,
    )?;
    OUTPUT.success("Written renv lockfile to renv.lock");
    info!("Written renv lockfile to renv.lock");
    Ok(())
}

/// Import an existing `renv.lock` into `rproj.toml`: `rig proj renv import`.
///
/// Mirrors `rig proj import`'s DESCRIPTION import: by default creates a new
/// `rproj.toml` (refusing to run if one already exists), `--dependencies`
/// merges into an existing one instead. Only package-level information
/// makes it across -- `renv.lock` has no project metadata (name, title,
/// authors, ...) to import, so a fresh manifest is named after the current
/// directory.
fn sc_renv_import(
    args: &ArgMatches,
    _renvargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let default_input = "renv.lock".to_string();
    let input: &String = args.get_one::<String>("input").unwrap_or(&default_input);
    let project = ProjectImport::start(args)?;

    OUTPUT.status(&format!("Reading dependencies from {}", input));
    info!("Reading dependencies from {}", input);
    let contents = fs::read_to_string(input).map_err(|e| {
        OUTPUT.error(&format!("Cannot read {}: {}", input, e));
        error!("Cannot read {}: {}", input, e);
        e
    })?;
    let lockfile: REnvLockfile = serde_json::from_str(&contents)?;
    project.finish(args, input, &lockfile.records(), &[])
}

#[derive(Serialize, Deserialize, Debug)]
#[allow(non_snake_case)]
struct REnvLockfileSimpleR {
    Version: String,
}

#[derive(Serialize, Deserialize, Debug)]
#[allow(non_snake_case)]
struct REnvLockfileSimple {
    R: REnvLockfileSimpleR,
}

// -------------------------------------------------------------------------------------

pub fn parse_r_version(lockfile: PathBuf) -> Result<String, Box<dyn Error>> {
    let contents = read_file_string(&lockfile)?;
    let lockf: REnvLockfileSimple = serde_json::from_str(&contents)?;
    Ok(lockf.R.Version.to_string())
}

fn filter_ok_versions(all: Vec<InstalledVersion>) -> Vec<OKInstalledVersion> {
    let mut ok: Vec<OKInstalledVersion> = vec![];
    for ver in all.iter() {
        if let InstalledVersion {
            name: n,
            version: Some(v),
            path: Some(p),
            binary: Some(b),
            aliases: _,
        } = ver
        {
            if let Ok(sv) = semver::Version::parse(v) {
                ok.push(OKInstalledVersion {
                    name: n.to_string(),
                    version: sv,
                    path: p.to_string(),
                    binary: b.to_string(),
                });
            }
        }
    }
    ok
}

pub fn match_r_version(ver: &str) -> Result<OKInstalledVersion, Box<dyn Error>> {
    let allvers = sc_get_list_details()?;
    let mut okvers = filter_ok_versions(allvers);
    okvers.sort();

    let ver = match crate::resolve::parse_plain_version(ver) {
        Some(v) => v,
        None => {
            OUTPUT.error(&format!("Invalid R version in renv.lock file: {:?}", ver));
            error!("Invalid R version in renv.lock file: {:?}", ver);
            bail!("Invalid R version in renv.lock file: {:?}", ver);
        }
    };

    // Matching major.minor
    let goodvers: Vec<OKInstalledVersion> = okvers
        .into_iter()
        .filter(|v| v.version.major == ver.major && v.version.minor == ver.minor)
        .collect();

    // If we have a perfect match, then reduce further
    let goodvers2: Vec<OKInstalledVersion> =
        match goodvers.iter().find(|v| v.version.patch == ver.patch) {
            Some(_) => goodvers
                .into_iter()
                .filter(|v| v.version.patch == ver.patch)
                .collect(),
            None => goodvers,
        };

    // If we have an arm64 version, reduce to those
    let goodvers3: Vec<OKInstalledVersion> =
        match goodvers2.iter().find(|v| v.name.ends_with("-arm64")) {
            Some(_) => goodvers2
                .into_iter()
                .filter(|v| v.name.ends_with("-arm64"))
                .collect(),
            None => goodvers2,
        };

    // Choose the latest one of these (there is surely at least on left)
    match goodvers3.last() {
        Some(v) => Ok(v.to_owned()),
        None => bail!(
            "Cannot find any R version close to R {}, \
                       required by renv lock file.",
            ver
        ),
    }
}

// -------------------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug)]
#[allow(non_snake_case)]
struct REnvLockfileRepository {
    Name: String,
    URL: String,
}

#[derive(Serialize, Deserialize, Debug)]
#[allow(non_snake_case)]
struct REnvLockfileR {
    Version: String,
    #[serde(default)]
    Repositories: Vec<REnvLockfileRepository>,
}

#[derive(Serialize, Deserialize, Debug)]
#[allow(non_snake_case)]
struct REnvLockfileBioconductor {
    Version: String,
}

#[derive(Serialize, Deserialize, Debug, Default)]
#[allow(non_snake_case)]
struct REnvLockfilePackage {
    Package: String,
    Version: String,
    /// Missing in some lock files, renv then infers it from `Repository`.
    #[serde(default)]
    #[serde(skip_serializing_if = "String::is_empty")]
    Source: String,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    Repository: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    Depends: Option<Vec<String>>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    Imports: Option<Vec<String>>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    LinkingTo: Option<Vec<String>>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    RemoteRepos: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    RemoteType: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    RemoteHost: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    RemoteUsername: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    RemoteRepo: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    RemoteUrl: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    RemoteRef: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    RemoteSha: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    RemoteSubdir: Option<String>,
}

impl REnvLockfilePackage {
    /// The package as installed from the GitHub, git, URL or local source
    /// that the solver fetched it from, in renv's `Source` and `Remote*`
    /// fields.
    fn with_remote(self, git: &GitSourceInfo) -> REnvLockfilePackage {
        let opt = |s: &Option<String>| s.clone().filter(|s| !s.is_empty());
        let sha = Some(git.sha.clone()).filter(|s| !s.is_empty());
        let subdir = opt(&git.subdir);
        match git.remote_type {
            "github" => {
                let host = git.host.as_deref().unwrap_or("github.com");
                let api = if host.eq_ignore_ascii_case("github.com") {
                    "api.github.com".to_string()
                } else {
                    format!("{}/api/v3", host)
                };
                // `repo` is `owner/repo`, renv has the two parts separately.
                let repo = git
                    .repo
                    .as_deref()
                    .map(|r| r.rsplit('/').next().unwrap_or(r).to_string());
                REnvLockfilePackage {
                    Source: "GitHub".to_string(),
                    RemoteType: Some("github".to_string()),
                    RemoteHost: Some(api),
                    RemoteUsername: opt(&git.username),
                    RemoteRepo: repo,
                    RemoteRef: Some(opt(&git.ref_).unwrap_or_else(|| "HEAD".to_string())),
                    RemoteSha: sha,
                    RemoteSubdir: subdir,
                    ..self
                }
            }
            "url" => REnvLockfilePackage {
                Source: "URL".to_string(),
                RemoteType: Some("url".to_string()),
                RemoteUrl: Some(git.url.clone()),
                RemoteSubdir: subdir,
                ..self
            },
            "local" => REnvLockfilePackage {
                Source: "Local".to_string(),
                RemoteType: Some("local".to_string()),
                RemoteUrl: Some(git.url.clone()),
                ..self
            },
            _ => REnvLockfilePackage {
                Source: "git".to_string(),
                RemoteType: Some("git".to_string()),
                RemoteUrl: Some(git.url.clone()),
                RemoteRef: opt(&git.ref_),
                RemoteSha: sha,
                RemoteSubdir: subdir,
                ..self
            },
        }
    }

    /// Where the package was installed from, as rig imports it.
    ///
    /// renv installs from `Source` only. `RemoteType` may be left over from
    /// where the package was first installed from.
    fn source(&self) -> PackageSource {
        if GitRemote::is_git_kind(&self.Source) {
            return PackageSource::Git(self.git_remote());
        }
        let reference = || {
            self.repository_reference()
                .map(|(repository, name)| RepoReference {
                    repository: repository.to_string(),
                    name: name.map(str::to_string),
                })
        };
        if self.is_bioconductor() {
            PackageSource::Bioconductor(reference())
        } else if self.is_repository_source() {
            PackageSource::Repository(reference())
        } else {
            PackageSource::Other
        }
    }

    /// The git remote of a package from GitHub, GitLab, Bitbucket or a git
    /// URL.
    fn git_remote(&self) -> GitRemote {
        let field = |s: &Option<String>| non_empty(s).map(str::to_string);
        GitRemote {
            kind: self.Source.trim().to_ascii_lowercase(),
            host: field(&self.RemoteHost),
            username: field(&self.RemoteUsername),
            repo: field(&self.RemoteRepo),
            url: field(&self.RemoteUrl),
            reference: field(&self.RemoteRef),
            sha: field(&self.RemoteSha),
            subdir: field(&self.RemoteSubdir),
        }
    }

    /// Whether the package was installed from a package repository, as
    /// opposed to GitHub, git, a URL or a local file.
    fn is_repository_source(&self) -> bool {
        let source = self.Source.trim();
        if source.is_empty() {
            return non_empty(&self.Repository).is_some();
        }
        source.eq_ignore_ascii_case("Repository")
            || CRAN_NAMES.iter().any(|n| source.eq_ignore_ascii_case(n))
    }

    /// Whether the package was installed from Bioconductor.
    fn is_bioconductor(&self) -> bool {
        self.Source.eq_ignore_ascii_case("Bioconductor")
            || (self.is_repository_source()
                && non_empty(&self.Repository).is_some_and(is_bioc_repository_name))
    }

    /// The repository URL and name a package was installed from, if any.
    fn repository_reference(&self) -> Option<(&str, Option<&str>)> {
        let name = non_empty(&self.Repository);
        match non_empty(&self.RemoteRepos) {
            Some(remote) => Some((remote, name.filter(|n| !is_url(n)))),
            None => name
                .or_else(|| {
                    let source = self.Source.trim();
                    CRAN_NAMES
                        .iter()
                        .copied()
                        .find(|n| source.eq_ignore_ascii_case(n))
                })
                .map(|n| (n, None)),
        }
    }
}

type REnvLockfilePackages = HashMap<String, REnvLockfilePackage>;

#[derive(Serialize, Deserialize, Debug)]
#[allow(non_snake_case)]
pub struct REnvLockfile {
    R: REnvLockfileR,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    Bioconductor: Option<REnvLockfileBioconductor>,
    Packages: REnvLockfilePackages,
}

impl REnvLockfile {
    /// The R version, repositories and packages of the lock file.
    fn records(&self) -> PackageRecords {
        let repositories = self
            .R
            .Repositories
            .iter()
            .map(|r| RecordedRepository {
                name: r.Name.clone(),
                url: r.URL.clone(),
                bioconductor: RENV_BIOC_NAMES
                    .iter()
                    .any(|n| r.Name.eq_ignore_ascii_case(n)),
            })
            .collect();
        let packages = self
            .Packages
            .iter()
            .map(|(name, pkg)| RecordedPackage {
                name: name.clone(),
                version: pkg.Version.clone(),
                source: pkg.source(),
            })
            .collect();
        PackageRecords {
            r_version: self.R.Version.clone(),
            bioc_version: self.Bioconductor.as_ref().map(|b| b.Version.clone()),
            repositories,
            packages,
        }
    }

    pub fn from_solution(registry: &RPackageRegistry, solution: &RegistrySolution) -> REnvLockfile {
        let mut pkgs = REnvLockfilePackages::new();
        for (k, v) in solution.iter() {
            if k == "R" || registry.is_local(k) || BASE_PKGS.contains(&k.as_str()) {
                continue;
            }
            let deps = registry.get_dependency_summary(k, v).unwrap();
            let pkg = REnvLockfilePackage {
                Package: k.to_string(),
                Version: v.version.to_string(),
                Depends: Some(deps),
                ..Default::default()
            };
            let pkg = match registry.git_source(k, v) {
                Some(git) => pkg.with_remote(&git),
                None => REnvLockfilePackage {
                    Source: "Repository".to_string(),
                    Repository: Some("CRAN".to_string()),
                    ..pkg
                },
            };
            pkgs.insert(k.to_string(), pkg);
        }
        REnvLockfile {
            Bioconductor: None,
            R: REnvLockfileR {
                Version: solution.get(&"R".to_string()).unwrap().version.to_string(),
                Repositories: vec![REnvLockfileRepository {
                    Name: "CRAN".to_string(),
                    URL: "https://cloud.r-project.org".to_string(),
                }],
            },
            Packages: pkgs,
        }
    }
}

/// The names renv gives to the repositories of Bioconductor.
const RENV_BIOC_NAMES: [&str; 5] = [
    "BioCsoft",
    "BioCann",
    "BioCexp",
    "BioCworkflows",
    "BioCbooks",
];

fn non_empty(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::tests::{test_detect, test_ref_kind};
    use crate::import::{apply_records, import_warnings, ImportOptions, ImportReport};
    use crate::proj_repos::ProjRepos;
    use crate::rproj::{
        DepTable, Dependency, Repository, Rproj, BIOC_REPOSITORY_NAME, CRAN_REPOSITORY_NAME,
    };

    const INTERNAL_URL: &str = "https://pkgs.example.com/prod";
    const PRIVATE_BIOC_URL: &str = "https://ppm.example.com/bioconductor/latest/packages/3.19/bioc";
    const BIOC_SNAPSHOT_URL: &str =
        "https://ppm.example.com/bioconductor/2024-05-01/packages/3.19/bioc";

    fn lockfile(json: &str) -> REnvLockfile {
        serde_json::from_str(json).unwrap()
    }

    /// The JSON of an `renv.lock` with these repositories and packages.
    fn lock(repositories: &[String], packages: &[String]) -> String {
        lock_json(None, repositories, packages)
    }

    /// The JSON of an `renv.lock` with a Bioconductor version.
    fn bioc_lock(version: &str, repositories: &[String], packages: &[String]) -> String {
        lock_json(Some(version), repositories, packages)
    }

    fn lock_json(bioc: Option<&str>, repositories: &[String], packages: &[String]) -> String {
        let bioc = bioc
            .map(|v| format!(r#""Bioconductor": {{"Version": "{}"}}, "#, v))
            .unwrap_or_default();
        format!(
            r#"{{"R": {{"Version": "4.4.1", "Repositories": [{}]}}, {}"Packages": {{{}}}}}"#,
            repositories.join(", "),
            bioc,
            packages.join(", ")
        )
    }

    /// A repository of the lock file.
    fn repo(name: &str, url: &str) -> String {
        format!(r#"{{"Name": "{}", "URL": "{}"}}"#, name, url)
    }

    fn cran() -> String {
        repo("CRAN", "https://cloud.r-project.org")
    }

    /// A package of the lock file, from the repository `repository`.
    fn pkg(name: &str, version: &str, repository: &str) -> String {
        format!(
            r#""{n}": {{"Package": "{n}", "Version": "{v}", "Source": "Repository", "Repository": "{r}"}}"#,
            n = name,
            v = version,
            r = repository
        )
    }

    /// A package of the lock file from Bioconductor, which may name a
    /// repository.
    fn bioc_pkg(name: &str, version: &str, repository: Option<&str>) -> String {
        let repository = repository
            .map(|r| format!(r#", "Repository": "{}""#, r))
            .unwrap_or_default();
        format!(
            r#""{n}": {{"Package": "{n}", "Version": "{v}", "Source": "Bioconductor"{r}}}"#,
            n = name,
            v = version,
            r = repository
        )
    }

    const OPTIONS: ImportOptions<'static> = ImportOptions {
        detect: &test_detect,
        keep_snapshots: false,
        ref_kind: &test_ref_kind,
    };

    /// `--keep-ppm-snapshots`.
    const KEEP: ImportOptions<'static> = ImportOptions {
        detect: &test_detect,
        keep_snapshots: true,
        ref_kind: &test_ref_kind,
    };

    /// Import the lock file `json` into a new manifest, which must be valid.
    fn import_report(json: &str) -> (Rproj, ImportReport) {
        import_report_with(json, &OPTIONS)
    }

    fn import_report_with(json: &str, options: &ImportOptions) -> (Rproj, ImportReport) {
        let mut manifest = Rproj::minimal("proj");
        let report =
            apply_records(&mut manifest, &lockfile(json).records(), true, options).unwrap();
        manifest.check_repositories().unwrap();
        ProjRepos::from_manifest(&manifest)
            .unwrap()
            .check()
            .unwrap();
        (manifest, report)
    }

    fn import(json: &str) -> Rproj {
        import_report(json).0
    }

    /// Import with `--keep-ppm-snapshots`.
    fn import_keep(json: &str) -> Rproj {
        import_report_with(json, &KEEP).0
    }

    /// Merge the lock file `json` into `manifest`, which must stay valid.
    fn merge(manifest: &mut Rproj, json: &str) {
        merge_with(manifest, json, &OPTIONS);
    }

    fn merge_with(manifest: &mut Rproj, json: &str, options: &ImportOptions) {
        apply_records(manifest, &lockfile(json).records(), false, options).unwrap();
        manifest.check_repositories().unwrap();
    }

    /// A repository that an import adds: it only serves the packages that
    /// are pinned to it.
    fn imported_repo(name: &str, url: &str) -> Repository {
        Repository {
            explicit: Some(true),
            ..Repository::at_url(name, url)
        }
    }

    fn bioc_off() -> Repository {
        Repository::disabled(BIOC_REPOSITORY_NAME)
    }

    /// The built-in Bioconductor repository of a lock file with this
    /// Bioconductor version.
    fn bioc_entry(version: &str) -> Repository {
        Repository {
            version: Some(version.to_string()),
            explicit: Some(true),
            ..Repository::builtin(BIOC_REPOSITORY_NAME)
        }
    }

    fn repository_of(m: &Rproj, package: &str) -> Option<String> {
        match m.dependencies.get(package) {
            Some(Dependency::Detailed(t)) => t.repository.clone(),
            _ => None,
        }
    }

    fn repository_names(m: &Rproj) -> Vec<&str> {
        m.repository.iter().map(|r| r.name.as_str()).collect()
    }

    #[test]
    fn lockfile_without_repositories_adds_none() {
        let m = import(r#"{"R": {"Version": "4.4.1"}, "Packages": {}}"#);
        assert!(m.repository.is_empty());
    }

    #[test]
    fn a_single_cran_repository_only_turns_bioconductor_off() {
        let m = import(&lock(&[cran()], &[pkg("cli", "3.6.3", "CRAN")]));
        assert_eq!(m.repository, vec![bioc_off()]);
        assert_eq!(
            m.dependencies.get("cli"),
            Some(&Dependency::Version("^3.6.3".to_string()))
        );
        assert_eq!(
            m.dependencies.get("R"),
            Some(&Dependency::Version(">= 4.4".to_string()))
        );
    }

    #[test]
    fn locked_versions_are_exact_requirements_of_repository_packages() {
        let json = lock(
            &[cran(), repo("internal", INTERNAL_URL)],
            &[
                pkg("cli", "3.6.3", "CRAN"),
                pkg("acme", "1.2-0", "internal"),
                r#""zeta": {"Package": "zeta", "Version": "1.0.0", "Source": "Local",
                "RemoteUrl": "/tmp/zeta"}"#
                    .to_string(),
                r#""ts": {"Package": "ts", "Version": "0.1.0", "Source": "GitHub",
                "RemoteUsername": "o", "RemoteRepo": "ts", "RemoteSha": "3f2a9c1"}"#
                    .to_string(),
            ],
        );
        let (m, report) = import_report(&json);
        assert_eq!(
            report.locked_versions,
            vec![
                ("acme".to_string(), "= 1.2-0".to_string()),
                ("cli".to_string(), "= 3.6.3".to_string()),
            ]
        );
        assert_eq!(
            m.dependencies.get("cli"),
            Some(&Dependency::Version("^3.6.3".to_string()))
        );
    }

    #[test]
    fn locked_git_shas_are_keyed_like_the_lock_solve() {
        let json = lock(
            &[cran()],
            &[
                r#""a": {"Package": "a", "Version": "0.1.0", "Source": "GitHub",
                "RemoteUsername": "o", "RemoteRepo": "a", "RemoteRef": "main",
                "RemoteSha": "aaaaaaa"}"#
                    .to_string(),
                r#""b": {"Package": "b", "Version": "0.1.0", "Source": "GitHub",
                "RemoteUsername": "o", "RemoteRepo": "b", "RemoteRef": "v1.0.0",
                "RemoteSubdir": "pkg", "RemoteSha": "bbbbbbb"}"#
                    .to_string(),
                r#""c": {"Package": "c", "Version": "0.1.0", "Source": "GitHub",
                "RemoteUsername": "o", "RemoteRepo": "c", "RemoteRef": "HEAD",
                "RemoteSha": "ccccccc"}"#
                    .to_string(),
                r#""d": {"Package": "d", "Version": "0.1.0", "Source": "GitHub",
                "RemoteUsername": "o", "RemoteRepo": "d", "RemoteSha": "ddddddd"}"#
                    .to_string(),
            ],
        );
        let (m, report) = import_report(&json);
        let key = |repo: &str, refspec: Option<&str>, subdir: Option<&str>| {
            (
                format!("https://github.com/o/{}.git", repo),
                refspec.map(str::to_string),
                subdir.map(str::to_string),
            )
        };
        // `d` has no ref, so the manifest pins its commit.
        assert_eq!(
            report.locked_git_shas,
            vec![
                (key("a", Some("main"), None), "aaaaaaa".to_string()),
                (key("b", Some("v1.0.0"), Some("pkg")), "bbbbbbb".to_string()),
                (key("c", None, None), "ccccccc".to_string()),
            ]
        );
        assert!(report.locked_versions.is_empty());
        let dep = |name: &str| match m.dependencies.get(name) {
            Some(Dependency::Detailed(t)) => (**t).clone(),
            other => panic!("unexpected dependency: {:?}", other),
        };
        assert_eq!(dep("a").branch.as_deref(), Some("main"));
        assert_eq!(dep("a").rev, None);
        assert_eq!(dep("b").tag.as_deref(), Some("v1.0.0"));
        assert_eq!(dep("b").branch, None);
        assert_eq!(
            (dep("c").branch, dep("c").tag, dep("c").rev),
            (None, None, None)
        );
        assert_eq!(dep("d").rev.as_deref(), Some("ddddddd"));
    }

    #[test]
    fn a_ref_that_is_not_found_is_a_branch_with_a_warning() {
        let json = lock(
            &[cran()],
            &[
                r#""a": {"Package": "a", "Version": "0.1.0", "Source": "GitHub",
                "RemoteUsername": "o", "RemoteRepo": "a", "RemoteRef": "missing",
                "RemoteSha": "aaaaaaa"}"#
                    .to_string(),
            ],
        );
        let (m, report) = import_report(&json);
        assert_eq!(report.unknown_refs, vec!["a".to_string()]);
        match m.dependencies.get("a") {
            Some(Dependency::Detailed(t)) => assert_eq!(t.branch.as_deref(), Some("missing")),
            other => panic!("unexpected dependency: {:?}", other),
        }
        let warnings = import_warnings(&report, "renv.lock");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("branch or a tag"));
    }

    // Bioconductor is turned off with the repositories that the lock file does
    // not use, but not if it has a Bioconductor version or a package from it.
    #[test]
    fn bioconductor_stays_on_if_the_lock_file_uses_it() {
        let versioned = import(&bioc_lock("3.19", &[cran()], &[]));
        assert!(versioned.repository.iter().all(|r| r.is_enabled()));
        let unversioned = import(&lock(&[cran()], &[pkg("Biobase", "2.64.0", "bioc")]));
        assert!(unversioned.repository.is_empty());
        assert_eq!(
            repository_of(&unversioned, "Biobase").as_deref(),
            Some("bioc")
        );
    }

    // renv installs packages from the repository it recorded for them, so the
    // repositories that were added do not serve any other package, even if they
    // come first.
    #[test]
    fn other_repositories_are_kept_in_order_and_only_serve_their_packages() {
        let m = import(&lock(
            &[repo("internal", INTERNAL_URL), cran()],
            &[
                pkg("cli", "3.6.3", "CRAN"),
                pkg("acme", "1.2.0", "internal"),
            ],
        ));
        assert_eq!(
            m.repository,
            vec![
                imported_repo("internal", INTERNAL_URL),
                Repository::builtin("cran"),
                bioc_off(),
            ]
        );
        assert_eq!(
            m.dependencies.get("cli"),
            Some(&Dependency::Version("^3.6.3".into()))
        );
        assert_eq!(
            m.dependencies.get("acme"),
            Some(&Dependency::Detailed(Box::new(DepTable {
                version: Some("^1.2.0".to_string()),
                repository: Some("internal".to_string()),
                ..Default::default()
            })))
        );
        let text = toml::to_string_pretty(&m).unwrap();
        assert_eq!(toml::from_str::<Rproj>(&text).unwrap(), m);
    }

    #[test]
    fn cran_is_turned_off_if_the_lockfile_does_not_list_it() {
        let m = import(&lock(
            &[repo("internal", INTERNAL_URL)],
            &[pkg("cli", "3.6.3", "internal")],
        ));
        assert_eq!(
            m.repository,
            vec![
                imported_repo("internal", INTERNAL_URL),
                Repository::disabled(CRAN_REPOSITORY_NAME),
                bioc_off(),
            ]
        );
    }

    #[test]
    fn a_cran_named_snapshot_does_not_clash_with_the_builtin() {
        let m = import_keep(&lock(
            &[repo(
                "CRAN",
                "https://packagemanager.posit.co/cran/2024-05-01",
            )],
            &[pkg("cli", "3.6.3", "CRAN")],
        ));
        assert_eq!(m.repository[0].name, "CRAN-repo");
        assert_eq!(m.repository[1].enabled, Some(false));
        // The package came from the snapshot, which is not the built-in CRAN.
        assert_eq!(repository_of(&m, "cli").as_deref(), Some("CRAN-repo"));
    }

    #[test]
    fn a_ppm_snapshot_is_the_builtin_cran() {
        for url in [
            "https://packagemanager.posit.co/cran/2024-05-01",
            "https://p3m.dev/cran/__linux__/noble/2024-05-01",
        ] {
            let m = import(&lock(
                &[repo("CRAN", url)],
                &[pkg("cli", "3.6.3", "CRAN"), pkg("glue", "1.7.0", url)],
            ));
            assert_eq!(m.repository, vec![bioc_off()], "{}", url);
            assert_eq!(repository_of(&m, "cli"), None, "{}", url);
            assert_eq!(repository_of(&m, "glue"), None, "{}", url);
        }
    }

    #[test]
    fn a_ppm_snapshot_of_bioconductor_is_the_builtin_bioconductor() {
        let snapshot = "https://p3m.dev/bioconductor/2024-05-01/packages/3.19/bioc";
        let m = import(&bioc_lock(
            "3.19",
            &[cran(), repo("BioCsoft", snapshot)],
            &[bioc_pkg("Biobase", "2.64.0", None)],
        ));
        assert_eq!(m.repository, vec![bioc_entry("3.19")]);
        assert_eq!(repository_of(&m, "Biobase").as_deref(), Some("bioc"));
    }

    // A private repository may have its own packages or its own policy, e.g.
    // blocked versions, so it is never the built-in CRAN.
    #[test]
    fn a_private_cran_is_kept() {
        for url in [
            "https://ppm.example.com/cran/latest",
            "https://cran.example.com",
        ] {
            let m = import(&lock(&[repo("CRAN", url)], &[pkg("cli", "3.6.3", "CRAN")]));
            assert_eq!(
                repository_names(&m),
                ["CRAN-repo", "cran", "bioc"],
                "{}",
                url
            );
            assert_eq!(repository_of(&m, "cli").as_deref(), Some("CRAN-repo"));
        }
    }

    // A private PPM snapshot is its repository at `latest`, like a public
    // snapshot is the built-in CRAN, which is not a snapshot.
    #[test]
    fn a_private_ppm_snapshot_is_its_repository_at_latest() {
        let snapshot = "https://ppm.example.com/cran/__linux__/jammy/2024-05-01";
        let latest = "https://ppm.example.com/cran/latest";
        let m = import(&lock(
            &[repo("CRAN", snapshot)],
            &[pkg("cli", "3.6.3", "CRAN"), pkg("glue", "1.7.0", snapshot)],
        ));
        assert_eq!(m.repository[0], imported_repo("CRAN-repo", latest));
        assert_eq!(repository_of(&m, "cli").as_deref(), Some("CRAN-repo"));
        assert_eq!(repository_of(&m, "glue").as_deref(), Some("CRAN-repo"));

        let m = import_keep(&lock(
            &[repo("CRAN", snapshot)],
            &[pkg("cli", "3.6.3", "CRAN")],
        ));
        assert_eq!(
            m.repository[0],
            imported_repo("CRAN-repo", "https://ppm.example.com/cran/2024-05-01")
        );
    }

    #[test]
    fn two_names_for_one_url_are_one_repository() {
        let m = import(&lock(
            &[
                cran(),
                repo("one", "https://pkgs.example.com/prod/"),
                repo("two", INTERNAL_URL),
            ],
            &[pkg("acme", "1.2.0", "two")],
        ));
        assert_eq!(repository_names(&m), ["cran", "one", "bioc"]);
        assert_eq!(repository_of(&m, "acme").as_deref(), Some("one"));
    }

    #[test]
    fn urls_that_differ_in_case_or_scheme_are_different_repositories() {
        let m = import(&lock(
            &[
                cran(),
                repo("a", "https://Host.example.com/Prod"),
                repo("b", "https://host.example.com/prod"),
                repo("c", "http://host.example.com/prod"),
                repo("d", "HTTPS://HOST.example.com/Prod/"),
            ],
            &[pkg("acme", "1.2.0", "d")],
        ));
        assert_eq!(repository_names(&m), ["cran", "a", "b", "c", "bioc"]);
        // `d` is `a`, with the case of the scheme and host aside.
        assert_eq!(repository_of(&m, "acme").as_deref(), Some("a"));
    }

    // renv writes the URL as the repository of a package if the repository is
    // not one of the lock file's.
    #[test]
    fn a_repository_url_that_is_not_listed_is_added() {
        let m = import(&lock(
            &[cran()],
            &[
                pkg("cli", "3.6.3", "CRAN"),
                pkg("acme", "1.2.0", "https://pkgs.example.com/prod/"),
                pkg("acme2", "1.0.0", INTERNAL_URL),
            ],
        ));
        assert_eq!(
            m.repository,
            vec![
                Repository::builtin("cran"),
                imported_repo("pkgs.example.com-prod", "https://pkgs.example.com/prod/"),
                bioc_off(),
            ]
        );
        assert_eq!(
            repository_of(&m, "acme").as_deref(),
            Some("pkgs.example.com-prod")
        );
        assert_eq!(
            repository_of(&m, "acme2").as_deref(),
            Some("pkgs.example.com-prod")
        );
        assert_eq!(repository_of(&m, "cli"), None);
    }

    #[test]
    fn a_listed_repository_url_is_not_added_twice() {
        let m = import_keep(&lock(
            &[
                cran(),
                repo("snap", "https://packagemanager.posit.co/cran/2024-05-01"),
            ],
            &[pkg(
                "cli",
                "3.6.3",
                "https://packagemanager.posit.co/cran/__linux__/jammy/2024-05-01",
            )],
        ));
        assert_eq!(m.repository.len(), 3);
        assert_eq!(repository_of(&m, "cli").as_deref(), Some("snap"));
    }

    #[test]
    fn a_cran_url_that_is_not_listed_is_the_builtin_cran() {
        let m = import(&lock(
            &[repo("internal", INTERNAL_URL)],
            &[pkg("cli", "3.6.3", "https://cloud.r-project.org")],
        ));
        assert_eq!(
            m.repository,
            vec![
                imported_repo("internal", INTERNAL_URL),
                Repository::builtin("cran"),
                bioc_off(),
            ]
        );
        assert_eq!(repository_of(&m, "cli"), None);
    }

    // renv records RSPM for packages from Posit Package Manager, even if the
    // lock file only lists CRAN, and a package may say it came from CRAN when
    // the lock file does not list it.
    #[test]
    fn a_package_from_a_cran_alias_is_from_the_builtin_cran() {
        let m = import(&lock(&[cran()], &[pkg("renv", "1.1.7", "RSPM")]));
        assert_eq!(m.repository, vec![bioc_off()]);
        assert_eq!(repository_of(&m, "renv"), None);

        for name in ["CRAN", "RSPM"] {
            let m = import(&lock(
                &[repo("internal", INTERNAL_URL)],
                &[pkg("cli", "3.6.3", name)],
            ));
            assert_eq!(m.repository.len(), 3);
            assert_eq!(m.repository[1], Repository::builtin("cran"));
            assert_eq!(repository_of(&m, "cli"), None);
        }
    }

    #[test]
    fn a_cran_alias_as_the_only_source_is_resolved() {
        let m = import_keep(&lock(
            &[
                repo("RSPM", "https://packagemanager.posit.co/cran/2024-05-01"),
                cran(),
            ],
            &[r#""acme": {"Package": "acme", "Version": "1.0.0", "Source": "RSPM"}"#.to_string()],
        ));
        assert_eq!(m.repository[0].name, "RSPM");
        assert_eq!(repository_of(&m, "acme").as_deref(), Some("RSPM"));
    }

    #[test]
    fn remote_repos_is_more_specific_than_the_repository_name() {
        let m = import(&lock(
            &[cran()],
            &[format!(
                r#""acme": {{"Package": "acme", "Version": "1.0.0", "Source": "Repository",
                   "Repository": "internal", "RemoteRepos": "{}"}}"#,
                INTERNAL_URL
            )],
        ));
        assert_eq!(m.repository[1], imported_repo("internal", INTERNAL_URL));
        assert_eq!(repository_of(&m, "acme").as_deref(), Some("internal"));
    }

    #[test]
    fn the_repository_name_can_be_the_source() {
        let (m, report) = import_report(&lock(
            &[cran(), repo("internal", INTERNAL_URL)],
            &[
                r#""cli": {"Package": "cli", "Version": "3.6.3", "Source": "CRAN"}"#.to_string(),
                r#""acme": {"Package": "acme", "Version": "1.0.0", "Source": "RSPM",
                       "Repository": "internal"}"#
                    .to_string(),
            ],
        ));
        assert!(report.unresolved.is_empty());
        assert_eq!(repository_of(&m, "acme").as_deref(), Some("internal"));
    }

    // renv infers the source of a package that only has a repository.
    #[test]
    fn a_package_without_a_source_is_from_its_repository() {
        let (m, report) = import_report(&lock(
            &[cran(), repo("internal", INTERNAL_URL)],
            &[
                r#""acme": {"Package": "acme", "Version": "1.0.0", "Repository": "internal"}"#
                    .to_string(),
                r#""nowhere": {"Package": "nowhere", "Version": "1.0.0"}"#.to_string(),
            ],
        ));
        assert_eq!(repository_of(&m, "acme").as_deref(), Some("internal"));
        // Neither a source nor a repository: it is reported.
        assert_eq!(report.other_sources, ["nowhere"]);
    }

    // renv knows `CRAN` and the names of Posit Package Manager as sources, no
    // other repository name.
    #[test]
    fn other_names_as_the_source_are_not_repositories() {
        let (m, report) = import_report(&lock(
            &[cran(), repo("internal", INTERNAL_URL)],
            &[
                r#""acme": {"Package": "acme", "Version": "1.0.0", "Source": "internal"}"#
                    .to_string(),
            ],
        ));
        assert_eq!(repository_of(&m, "acme"), None);
        assert_eq!(report.other_sources, ["acme"]);
    }

    #[test]
    fn packages_from_an_unlisted_repository_are_reported() {
        let (m, report) = import_report(&lock(
            &[cran()],
            &[
                pkg("acme", "1.2.0", "nowhere"),
                pkg("beta", "1.0.0", "nowhere"),
            ],
        ));
        assert_eq!(report.unresolved, ["acme", "beta"]);
        assert_eq!(
            m.dependencies.get("acme"),
            Some(&Dependency::Version("^1.2.0".to_string()))
        );
        let warnings = import_warnings(&report, "renv.lock");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].starts_with("2 package(s) in renv.lock come from a repository"));
        assert!(warnings[0].ends_with(": acme, beta"), "{}", warnings[0]);
    }

    #[test]
    fn repositories_that_rig_cannot_load_are_not_imported() {
        let (m, report) = import_report(&lock(
            &[
                cran(),
                repo("old", "ftp://ftp.example.com/cran"),
                repo("local", "file:///srv/cran"),
            ],
            &[
                pkg("a", "1.0.0", "old"),
                pkg("b", "1.0.0", "file:///srv/cran"),
                pkg("c", "1.0.0", "ftp://ftp.example.com/other"),
                pkg("cli", "3.6.3", "CRAN"),
            ],
        ));
        assert_eq!(report.unresolved, ["a", "b", "c"]);
        assert_eq!(m.repository, vec![bioc_off()]);
        for name in ["a", "b", "c"] {
            assert_eq!(repository_of(&m, name), None, "{}", name);
        }
    }

    #[test]
    fn packages_from_other_sources_are_listed_in_order() {
        let (_, report) = import_report(&lock(
            &[cran()],
            &[
                pkg("cli", "3.6.3", "CRAN"),
                r#""zeta": {"Package": "zeta", "Version": "1.0.0", "Source": "Local", "RemoteUrl": "/tmp/zeta"}"#
                    .to_string(),
                r#""alpha": {"Package": "alpha", "Version": "1.0.0", "Source": "URL",
                       "RemoteUrl": "https://example.com/alpha.tar.gz"}"#
                    .to_string(),
                r#""beta": {"Package": "beta", "Version": "1.0.0", "Source": "GitHub",
                       "RemoteUsername": "o", "RemoteRepo": "beta"}"#
                    .to_string(),
                bioc_pkg("Biobase", "2.64.0", None),
            ],
        ));
        assert_eq!(
            import_warnings(&report, "renv.lock"),
            vec![
                "2 package(s) in renv.lock come from a source that rig cannot import \
                  (e.g. a URL or a local file) and may need manual adjustment: alpha, zeta"
                    .to_string()
            ]
        );
    }

    #[test]
    fn git_packages_without_a_location_are_reported() {
        let (m, report) = import_report(&lock(
            &[cran()],
            &[
                r#""gh": {"Package": "gh", "Version": "1.0.0", "Source": "GitHub",
                       "RemoteUsername": "o"}"#
                    .to_string(),
                r#""g": {"Package": "g", "Version": "2.0.0", "Source": "git"}"#.to_string(),
                r#""loc": {"Package": "loc", "Version": "1.0.0", "Source": "Local",
                       "RemoteUrl": "/tmp/loc"}"#
                    .to_string(),
            ],
        ));
        assert_eq!(report.incomplete_git, ["g", "gh"]);
        assert_eq!(report.other_sources, ["loc"]);
        assert_eq!(
            m.dependencies.get("gh"),
            Some(&Dependency::Version("^1.0.0".to_string()))
        );
        let warnings = import_warnings(&report, "renv.lock");
        assert_eq!(warnings.len(), 2);
        assert!(
            warnings[1].starts_with("2 package(s) in renv.lock come from GitHub, GitLab"),
            "{}",
            warnings[1]
        );
        assert!(warnings[1].ends_with(": g, gh"), "{}", warnings[1]);
    }

    #[test]
    fn git_urls_that_are_not_http_urls_are_reported() {
        let (m, report) = import_report(&lock(
            &[cran()],
            &[
                r#""ssh": {"Package": "ssh", "Version": "1.0.0", "Source": "git",
                       "RemoteUrl": "git@git.example.com:team/ssh.git", "RemoteSha": "abc"}"#
                    .to_string(),
                r#""gh": {"Package": "gh", "Version": "1.0.0", "Source": "git",
                       "RemoteUrl": "git://github.com/o/gh.git", "RemoteSha": "abc"}"#
                    .to_string(),
                r#""web": {"Package": "web", "Version": "1.0.0", "Source": "git",
                       "RemoteUrl": "https://git.example.com/web.git", "RemoteSha": "abc"}"#
                    .to_string(),
            ],
        ));
        assert_eq!(report.github_git_urls, ["gh"]);
        assert_eq!(report.other_git_urls, ["ssh"]);
        // The SSH URL is kept as it is.
        assert!(matches!(
            m.dependencies.get("ssh"),
            Some(Dependency::Detailed(t))
                if t.git.as_deref() == Some("git@git.example.com:team/ssh.git")
        ));
        assert!(matches!(
            m.dependencies.get("gh"),
            Some(Dependency::Detailed(t))
                if t.git.as_deref() == Some("https://github.com/o/gh.git")
        ));
        assert_eq!(
            import_warnings(&report, "renv.lock"),
            vec![
                "1 package(s) in renv.lock come from a git:// URL of GitHub, which GitHub \
                  does not serve any more; they were imported from the https:// URL: gh"
                    .to_string(),
                "1 package(s) in renv.lock come from a git URL that is not an http:// or \
                  https:// URL, and may need manual adjustment: ssh"
                    .to_string(),
            ]
        );
    }

    // A Linux binary URL only works on one distribution.
    #[test]
    fn linux_binary_urls_are_imported_as_source_urls() {
        let snapshot = "https://p3m.dev/cran/__linux__/noble/2024-05-01";
        let private = "https://ppm.example.com/cran/__linux__/jammy/latest/";
        let m = import_keep(&lock(
            &[repo("CRAN", snapshot)],
            &[pkg("cli", "3.6.3", "CRAN"), pkg("acme", "1.2.0", private)],
        ));
        assert_eq!(
            m.repository,
            vec![
                imported_repo("CRAN-repo", "https://p3m.dev/cran/2024-05-01"),
                imported_repo(
                    "ppm.example.com-cran-latest",
                    "https://ppm.example.com/cran/latest/"
                ),
                Repository::disabled(CRAN_REPOSITORY_NAME),
                bioc_off(),
            ]
        );
    }

    #[test]
    fn a_linux_binary_url_of_a_listed_repository_is_that_repository() {
        let source = "https://p3m.dev/cran/2024-05-01";
        let binary = "https://p3m.dev/cran/__linux__/noble/2024-05-01";
        let m = import_keep(&lock(
            &[repo("snap", source)],
            &[pkg("cli", "3.6.3", binary)],
        ));
        assert_eq!(m.repository.len(), 3);
        assert_eq!(repository_of(&m, "cli").as_deref(), Some("snap"));
    }

    #[test]
    fn a_private_linux_binary_bioconductor_is_imported_as_its_source_url() {
        let binary =
            "https://ppm.example.com/bioconductor/__linux__/noble/latest/packages/3.19/bioc";
        let m = import(&bioc_lock(
            "3.19",
            &[cran(), repo("BioCsoft", binary)],
            &[bioc_pkg("Biobase", "2.64.0", None)],
        ));
        let entry = m.repository.iter().find(|r| r.name == "BioCsoft").unwrap();
        assert_eq!(entry.url.as_deref(), Some(PRIVATE_BIOC_URL));
        assert_eq!(entry.explicit, None);
    }

    #[test]
    fn there_is_no_warning_if_every_package_is_imported() {
        let (_, report) = import_report(&lock(&[cran()], &[pkg("cli", "3.6.3", "CRAN")]));
        assert!(import_warnings(&report, "renv.lock").is_empty());
    }

    fn git_dep(json: &str) -> Option<DepTable> {
        let pkg: REnvLockfilePackage = serde_json::from_str(json).unwrap();
        git_dep_table(&pkg)
    }

    /// The `git` dependency of a package, if it is one.
    fn git_dep_table(pkg: &REnvLockfilePackage) -> Option<DepTable> {
        match pkg.source() {
            PackageSource::Git(remote) => remote.dep_table(),
            _ => None,
        }
    }

    #[test]
    fn github_package_is_a_git_dependency_at_its_ref() {
        let t = git_dep(
            r#"{"Package": "ts", "Version": "0.1.0", "Source": "GitHub", "RemoteType": "github",
                "RemoteHost": "api.github.com", "RemoteUsername": "gaborcsardi", "RemoteRepo": "ts",
                "RemoteRef": "main", "RemoteSha": "3f2a9c1"}"#,
        )
        .unwrap();
        assert_eq!(
            t,
            DepTable {
                git: Some("https://github.com/gaborcsardi/ts.git".to_string()),
                branch: Some("main".to_string()),
                ..Default::default()
            }
        );
    }

    #[test]
    fn github_subdir_and_default_branch() {
        let t = git_dep(
            r#"{"Package": "ts", "Version": "0.1.0", "Source": "GitHub",
                "RemoteUsername": "o", "RemoteRepo": "r", "RemoteRef": "HEAD",
                "RemoteSubdir": "pkg"}"#,
        )
        .unwrap();
        assert_eq!(t.git.as_deref(), Some("https://github.com/o/r.git"));
        assert_eq!((t.branch, t.tag, t.rev), (None, None, None));
        assert_eq!(t.subdir.as_deref(), Some("pkg"));
    }

    #[test]
    fn github_without_a_sha_uses_the_ref() {
        let t = git_dep(
            r#"{"Package": "ts", "Version": "0.1.0", "Source": "GitHub",
                "RemoteUsername": "o", "RemoteRepo": "r", "RemoteRef": "v1.2.0"}"#,
        )
        .unwrap();
        assert_eq!(t.branch.as_deref(), Some("v1.2.0"));
        assert_eq!(t.rev, None);
    }

    #[test]
    fn github_without_a_ref_uses_the_sha() {
        let t = git_dep(
            r#"{"Package": "ts", "Version": "0.1.0", "Source": "GitHub",
                "RemoteUsername": "o", "RemoteRepo": "r", "RemoteSha": "3f2a9c1"}"#,
        )
        .unwrap();
        assert_eq!(t.branch, None);
        assert_eq!(t.rev.as_deref(), Some("3f2a9c1"));
    }

    #[test]
    fn a_ref_that_is_a_commit_is_a_rev() {
        let t = git_dep(
            r#"{"Package": "ts", "Version": "0.1.0", "Source": "GitHub",
                "RemoteUsername": "o", "RemoteRepo": "r", "RemoteRef": "3f2a9c1e",
                "RemoteSha": "3f2a9c1e"}"#,
        )
        .unwrap();
        assert_eq!(t.branch, None);
        assert_eq!(t.rev.as_deref(), Some("3f2a9c1e"));
    }

    #[test]
    fn github_enterprise_host() {
        for host in [
            "github.example.com/api/v3",
            "https://github.example.com/api/v3/",
        ] {
            let t = git_dep(&format!(
                r#"{{"Package": "ts", "Version": "0.1.0", "Source": "GitHub", "RemoteHost": "{}",
                    "RemoteUsername": "o", "RemoteRepo": "r"}}"#,
                host
            ))
            .unwrap();
            assert_eq!(
                t.git.as_deref(),
                Some("https://github.example.com/o/r.git"),
                "{}",
                host
            );
        }
    }

    #[test]
    fn a_repository_name_can_end_in_dot_git() {
        let t = git_dep(
            r#"{"Package": "r", "Version": "0.1.0", "Source": "GitHub",
                "RemoteUsername": "o", "RemoteRepo": "r.git"}"#,
        )
        .unwrap();
        assert_eq!(t.git.as_deref(), Some("https://github.com/o/r.git"));
    }

    #[test]
    fn the_default_api_host_is_case_insensitive() {
        for (source, host, git) in [
            ("GitHub", "API.GitHub.com", "https://github.com/o/r.git"),
            (
                "Bitbucket",
                "API.BITBUCKET.ORG/2.0",
                "https://bitbucket.org/o/r.git",
            ),
        ] {
            let t = git_dep(&format!(
                r#"{{"Package": "r", "Version": "0.1.0", "Source": "{}", "RemoteHost": "{}",
                    "RemoteUsername": "o", "RemoteRepo": "r"}}"#,
                source, host
            ))
            .unwrap();
            assert_eq!(t.git.as_deref(), Some(git), "{}", host);
        }
    }

    #[test]
    fn gitlab_and_bitbucket_hosts() {
        let t = git_dep(
            r#"{"Package": "p", "Version": "1.0.0", "Source": "GitLab", "RemoteHost": "gitlab.com",
                "RemoteUsername": "group/sub", "RemoteRepo": "p", "RemoteSha": "abc"}"#,
        )
        .unwrap();
        assert_eq!(t.git.as_deref(), Some("https://gitlab.com/group/sub/p.git"));
        let t = git_dep(
            r#"{"Package": "p", "Version": "1.0.0", "Source": "GitLab", "RemoteHost": "gitlab.example.com",
                "RemoteUsername": "g", "RemoteRepo": "p"}"#,
        )
        .unwrap();
        assert_eq!(t.git.as_deref(), Some("https://gitlab.example.com/g/p.git"));
        let t = git_dep(
            r#"{"Package": "p", "Version": "1.0.0", "Source": "Bitbucket",
                "RemoteHost": "api.bitbucket.org/2.0", "RemoteUsername": "u", "RemoteRepo": "p"}"#,
        )
        .unwrap();
        assert_eq!(t.git.as_deref(), Some("https://bitbucket.org/u/p.git"));
    }

    #[test]
    fn git_package_uses_its_url() {
        let t = git_dep(
            r#"{"Package": "p", "Version": "1.0.0", "Source": "git", "RemoteType": "git",
                "RemoteUrl": "https://git.example.com/team/p.git", "RemoteSha": "abc123"}"#,
        )
        .unwrap();
        assert_eq!(t.git.as_deref(), Some("https://git.example.com/team/p.git"));
        assert_eq!(t.rev.as_deref(), Some("abc123"));
    }

    fn exported(info: GitSourceInfo) -> REnvLockfilePackage {
        let pkg = REnvLockfilePackage {
            Package: "p".to_string(),
            Version: "1.0.0".to_string(),
            ..Default::default()
        }
        .with_remote(&info);
        // What `renv.lock` has, read back.
        serde_json::from_str(&serde_json::to_string(&pkg).unwrap()).unwrap()
    }

    fn git_info(remote_type: &'static str, url: &str) -> GitSourceInfo {
        GitSourceInfo {
            remote_type,
            url: url.to_string(),
            host: None,
            repo: None,
            username: None,
            subdir: None,
            ref_: None,
            sha: "abc123".to_string(),
            binary: false,
        }
    }

    #[test]
    fn github_packages_are_exported_from_github() {
        let pkg = exported(GitSourceInfo {
            host: Some("github.com".to_string()),
            repo: Some("o/p".to_string()),
            username: Some("o".to_string()),
            subdir: Some("pkg".to_string()),
            ref_: Some("main".to_string()),
            ..git_info("github", "https://github.com/o/p.git")
        });
        assert_eq!(pkg.Source, "GitHub");
        assert_eq!(pkg.Repository, None);
        assert_eq!(pkg.RemoteType.as_deref(), Some("github"));
        assert_eq!(pkg.RemoteHost.as_deref(), Some("api.github.com"));
        assert_eq!(pkg.RemoteUsername.as_deref(), Some("o"));
        assert_eq!(pkg.RemoteRepo.as_deref(), Some("p"));
        assert_eq!(pkg.RemoteRef.as_deref(), Some("main"));
        assert_eq!(pkg.RemoteSha.as_deref(), Some("abc123"));
        // And it is imported back as the same git dependency.
        let t = git_dep_table(&pkg).unwrap();
        assert_eq!(t.git.as_deref(), Some("https://github.com/o/p.git"));
        assert_eq!(t.branch.as_deref(), Some("main"));
        assert_eq!(t.subdir.as_deref(), Some("pkg"));
    }

    #[test]
    fn a_github_package_without_a_ref_is_from_the_default_branch() {
        let pkg = exported(GitSourceInfo {
            host: Some("github.com".to_string()),
            repo: Some("o/p".to_string()),
            username: Some("o".to_string()),
            ..git_info("github", "https://github.com/o/p.git")
        });
        assert_eq!(pkg.RemoteRef.as_deref(), Some("HEAD"));
    }

    #[test]
    fn git_url_and_local_packages_are_exported_from_their_source() {
        let pkg = exported(git_info("git", "https://git.example.com/p.git"));
        assert_eq!(pkg.Source, "git");
        assert_eq!(
            pkg.RemoteUrl.as_deref(),
            Some("https://git.example.com/p.git")
        );
        let t = git_dep_table(&pkg).unwrap();
        assert_eq!(t.git.as_deref(), Some("https://git.example.com/p.git"));
        assert_eq!(t.rev.as_deref(), Some("abc123"));

        let pkg = exported(git_info("url", "https://example.com/p_1.0.0.tar.gz"));
        assert_eq!(pkg.Source, "URL");
        assert_eq!(
            pkg.RemoteUrl.as_deref(),
            Some("https://example.com/p_1.0.0.tar.gz")
        );
        assert_eq!(pkg.RemoteSha, None);

        let pkg = exported(GitSourceInfo {
            sha: String::new(),
            ..git_info("local", "/tmp/p")
        });
        assert_eq!(pkg.Source, "Local");
        assert_eq!(pkg.RemoteUrl.as_deref(), Some("/tmp/p"));
    }

    // The records below are from renv's own test suite.

    #[test]
    fn source_names_are_case_insensitive() {
        for (source, git) in [
            ("github", "https://github.com/kevinushey/skeleton.git"),
            ("Gitlab", "https://gitlab.com/kevinushey/skeleton.git"),
            ("bitbucket", "https://bitbucket.org/kevinushey/skeleton.git"),
        ] {
            let t = git_dep(&format!(
                r#"{{"Package": "skeleton", "Version": "1.0.1", "Source": "{}",
                    "RemoteUsername": "kevinushey", "RemoteRepo": "skeleton",
                    "RemoteSha": "958296dbbbf7f1d82f7f5dd1b121c7558604809f"}}"#,
                source
            ))
            .unwrap();
            assert_eq!(t.git.as_deref(), Some(git), "{}", source);
            assert_eq!(
                t.rev.as_deref(),
                Some("958296dbbbf7f1d82f7f5dd1b121c7558604809f")
            );
        }
    }

    #[test]
    fn a_remote_host_can_have_a_scheme() {
        let t = git_dep(
            r#"{"Package": "skeleton", "Version": "1.0.1", "Source": "github",
                "RemoteHost": "https://api.github.com", "RemoteUsername": "kevinushey",
                "RemoteRepo": "skeleton", "RemoteSha": "958296dbbbf7f1d82f7f5dd1b121c7558604809f"}"#,
        )
        .unwrap();
        assert_eq!(
            t.git.as_deref(),
            Some("https://github.com/kevinushey/skeleton.git")
        );
    }

    #[test]
    fn gitlab_subgroups_and_subdirectories() {
        // A subgroup is part of `RemoteRepo`, and an empty subdirectory is none.
        let t = git_dep(
            r#"{"Package": "testpackage", "Version": "0.1.0", "Source": "GitLab",
                "RemoteType": "gitlab", "RemoteHost": "gitlab.example.de",
                "RemoteUsername": "bioinf", "RemoteRepo": "rlib/testpackage", "RemoteSubdir": "",
                "RemoteRef": "main", "RemoteSha": "babda05db8146b37a552c77651e2cd084a05ce98"}"#,
        )
        .unwrap();
        assert_eq!(
            t,
            DepTable {
                git: Some("https://gitlab.example.de/bioinf/rlib/testpackage.git".to_string()),
                branch: Some("main".to_string()),
                ..Default::default()
            }
        );
        let t = git_dep(
            r#"{"Package": "testpackage", "Version": "0.1.0", "Source": "GitLab",
                "RemoteType": "gitlab", "RemoteUsername": "group", "RemoteRepo": "repo",
                "RemoteSubdir": "testpackage", "RemoteRef": "main",
                "RemoteSha": "babda05db8146b37a552c77651e2cd084a05ce98"}"#,
        )
        .unwrap();
        assert_eq!(t.git.as_deref(), Some("https://gitlab.com/group/repo.git"));
        assert_eq!(t.subdir.as_deref(), Some("testpackage"));
    }

    #[test]
    fn git_urls_of_github_are_https_urls() {
        let t = git_dep(
            r#"{"Package": "skeleton", "Version": "1.0.1", "Source": "git",
                "RemoteUrl": "git://github.com/kevinushey/skeleton.git",
                "RemoteSha": "958296dbbbf7f1d82f7f5dd1b121c7558604809f"}"#,
        );
        assert_eq!(
            t.unwrap().git.as_deref(),
            Some("https://github.com/kevinushey/skeleton.git")
        );
        // Older lock files have no commit, only the ref.
        let t = git_dep(
            r#"{"Package": "renv.git1", "Version": "0.0.0.9000", "Source": "git",
                "RemoteUrl": "https://github.com/kevinushey/renv.git1", "RemoteRef": "main"}"#,
        )
        .unwrap();
        assert_eq!(t.branch.as_deref(), Some("main"));
    }

    // renv installs from `Source` only, so a remote type without one is not
    // a git remote.
    #[test]
    fn the_remote_type_is_not_the_source() {
        assert!(git_dep(
            r#"{"Package": "p", "Version": "1.0.0", "RemoteType": "github",
                "RemoteUsername": "o", "RemoteRepo": "p", "RemoteSha": "abc123"}"#,
        )
        .is_none());
    }

    #[test]
    fn incomplete_or_other_remotes_are_not_git_dependencies() {
        // No repository to clone from.
        assert!(git_dep(r#"{"Package": "p", "Version": "1", "Source": "GitHub"}"#).is_none());
        assert!(git_dep(r#"{"Package": "p", "Version": "1", "Source": "git"}"#).is_none());
        // Not a git remote.
        assert!(git_dep(r#"{"Package": "p", "Version": "1", "Source": "Repository"}"#).is_none());
        assert!(git_dep(
            r#"{"Package": "p", "Version": "1", "Source": "URL", "RemoteUrl": "https://x.org/p.tar.gz"}"#
        )
        .is_none());
        // Installed from a repository, even if it was first installed from git.
        assert!(git_dep(
            r#"{"Package": "p", "Version": "1", "Source": "Repository", "Repository": "CRAN",
                "RemoteType": "github", "RemoteUsername": "o", "RemoteRepo": "p"}"#
        )
        .is_none());
    }

    #[test]
    fn git_packages_replace_version_requirements() {
        let (m, report) = import_report(&lock(
            &[cran()],
            &[
                r#""ts": {"Package": "ts", "Version": "0.1.0", "Source": "GitHub",
                       "RemoteUsername": "gaborcsardi", "RemoteRepo": "ts", "RemoteSha": "3f2a"}"#
                    .to_string(),
                r#""loc": {"Package": "loc", "Version": "2.0.0", "Source": "Local", "RemoteUrl": "/tmp/loc"}"#
                    .to_string(),
                pkg("cli", "3.6.3", "CRAN"),
            ],
        ));
        assert_eq!(report.other_sources, ["loc"]);
        assert_eq!(
            m.dependencies.get("ts"),
            Some(&Dependency::Detailed(Box::new(DepTable {
                git: Some("https://github.com/gaborcsardi/ts.git".to_string()),
                rev: Some("3f2a".to_string()),
                ..Default::default()
            })))
        );
        assert_eq!(
            m.dependencies.get("cli"),
            Some(&Dependency::Version("^3.6.3".to_string()))
        );
        // The unsupported source still has its version requirement.
        assert_eq!(
            m.dependencies.get("loc"),
            Some(&Dependency::Version("^2.0.0".to_string()))
        );
        let text = toml::to_string_pretty(&m).unwrap();
        assert_eq!(toml::from_str::<Rproj>(&text).unwrap(), m);
    }

    // renv takes a package whose repository is called `bioc` or
    // `bioconductor` as from Bioconductor.
    #[test]
    fn a_package_from_a_repository_named_bioc_is_from_bioconductor() {
        for repository in ["bioc", "Bioconductor"] {
            let m = import(&lock(&[cran()], &[pkg("Biobase", "2.64.0", repository)]));
            assert_eq!(
                repository_of(&m, "Biobase").as_deref(),
                Some("bioc"),
                "{}",
                repository
            );
        }
    }

    // A Bioconductor release is built for one R minor version.
    #[test]
    fn a_bioconductor_release_limits_r_to_its_minor_version() {
        let m = import(&bioc_lock("3.19", &[cran()], &[]));
        assert_eq!(
            m.dependencies.get("R"),
            Some(&Dependency::Version(">= 4.4, < 4.5".to_string()))
        );
    }

    #[test]
    fn a_bioconductor_package_is_not_taken_from_a_cran_repository_of_that_name() {
        let m = import(&bioc_lock(
            "3.19",
            &[cran(), repo("Bioconductor", INTERNAL_URL)],
            &[bioc_pkg("Biobase", "2.64.0", None)],
        ));
        assert_eq!(repository_of(&m, "Biobase").as_deref(), Some("bioc"));
    }

    // rig writes the Bioconductor repositories of its own environments as
    // Posit Package Manager URLs.
    #[test]
    fn listed_ppm_bioconductor_repositories_are_the_builtin_one() {
        let ppm = "https://packagemanager.posit.co";
        let m = import(&bioc_lock(
            "3.19",
            &[
                repo("CRAN", &format!("{}/cran/__linux__/jammy/latest", ppm)),
                repo(
                    "BioCsoft",
                    &format!(
                        "{}/bioconductor/__linux__/jammy/latest/packages/3.19/bioc",
                        ppm
                    ),
                ),
                repo(
                    "BioCann",
                    &format!(
                        "{}/bioconductor/__linux__/jammy/latest/packages/3.19/data/annotation",
                        ppm
                    ),
                ),
            ],
            &[pkg("limma", "3.60.0", "BioCsoft")],
        ));
        assert_eq!(m.repository, vec![bioc_entry("3.19")]);
        assert_eq!(repository_of(&m, "limma").as_deref(), Some("bioc"));
    }

    // The built-in Bioconductor repository is served by the public Posit
    // Package Manager, so a private instance stays a repository of its own:
    // the import must not drop the server the lock file installs from.
    #[test]
    fn a_private_package_manager_bioconductor_is_kept() {
        for url in [PRIVATE_BIOC_URL, BIOC_SNAPSHOT_URL] {
            let m = import(&bioc_lock("3.19", &[cran(), repo("BioCsoft", url)], &[]));
            assert_eq!(
                repository_names(&m),
                ["bioc", "cran", "BioCsoft"],
                "{}",
                url
            );
            let entry = m.repository.iter().find(|r| r.name == "BioCsoft").unwrap();
            assert_eq!(entry.url.as_deref(), Some(PRIVATE_BIOC_URL), "{}", url);
        }
    }

    // A Bioconductor repository of another release is not the built-in one.
    #[test]
    fn a_bioconductor_repository_of_another_release_is_kept() {
        let url = "https://bioconductor.org/packages/3.18/bioc";
        let m = import(&bioc_lock("3.19", &[cran(), repo("BioCsoft", url)], &[]));
        assert_eq!(repository_names(&m), ["bioc", "cran", "BioCsoft"]);
    }

    // A listed Bioconductor repository that is not the built-in one serves the
    // packages of Bioconductor, instead of the public one.
    #[test]
    fn bioconductor_packages_are_served_by_a_listed_private_bioconductor() {
        for url in [PRIVATE_BIOC_URL, BIOC_SNAPSHOT_URL] {
            let m = import(&bioc_lock(
                "3.19",
                &[cran(), repo("BioCsoft", url)],
                &[bioc_pkg("Biobase", "2.64.0", None)],
            ));
            assert_eq!(repository_of(&m, "Biobase"), None, "{}", url);
            let entry = m.repository.iter().find(|r| r.name == "BioCsoft").unwrap();
            assert_eq!(entry.explicit, None, "{}", url);
        }
    }

    // The packages that name a private repository are pinned to it, so a
    // package that is also on CRAN is not taken from there.
    #[test]
    fn a_bioconductor_package_that_names_a_private_repository_is_pinned_to_it() {
        let m = import(&bioc_lock(
            "3.19",
            &[cran(), repo("BioCsoft", PRIVATE_BIOC_URL)],
            &[
                bioc_pkg("limma", "3.60.0", Some("BioCsoft")),
                bioc_pkg("Biobase", "2.64.0", None),
            ],
        ));
        assert_eq!(repository_of(&m, "limma").as_deref(), Some("BioCsoft"));
        assert_eq!(repository_of(&m, "Biobase"), None);
    }

    // renv keeps a repository called `Bioconductor` next to the ones of
    // Bioconductor, and prefers it for the packages that name it.
    #[test]
    fn a_bioconductor_package_that_names_a_listed_alias_is_pinned_to_it() {
        for private_bioc in [false, true] {
            let mut repositories = vec![cran(), repo("Bioconductor", INTERNAL_URL)];
            if private_bioc {
                repositories.push(repo("BioCsoft", PRIVATE_BIOC_URL));
            }
            let m = import(&bioc_lock(
                "3.19",
                &repositories,
                &[
                    bioc_pkg("limma", "3.60.0", Some("Bioconductor")),
                    bioc_pkg("Biobase", "2.64.0", None),
                ],
            ));
            assert_eq!(
                repository_of(&m, "limma").as_deref(),
                Some("Bioconductor"),
                "{}",
                private_bioc
            );
            let biobase = (!private_bioc).then_some("bioc");
            assert_eq!(
                repository_of(&m, "Biobase").as_deref(),
                biobase,
                "{}",
                private_bioc
            );
        }
    }

    // The built-in repository is explicit next to a private one, so a package
    // that names the public repository is pinned to it.
    #[test]
    fn a_bioconductor_package_that_names_the_public_repository_is_pinned_to_it() {
        let m = import(&bioc_lock(
            "3.19",
            &[
                cran(),
                repo("BioCsoft", "https://bioconductor.org/packages/3.19/bioc"),
                repo(
                    "BioCann",
                    "https://ppm.example.com/bioconductor/latest/packages/3.19/data/annotation",
                ),
            ],
            &[
                bioc_pkg("limma", "3.60.0", Some("BioCsoft")),
                bioc_pkg("Biobase", "2.64.0", None),
            ],
        ));
        assert_eq!(repository_of(&m, "limma").as_deref(), Some("bioc"));
        assert_eq!(repository_of(&m, "Biobase"), None);
    }

    // Without a Bioconductor version these are repositories like any other.
    #[test]
    fn bioconductor_repositories_without_a_version_are_kept_and_explicit() {
        let m = import(&lock(
            &[
                repo("BioCsoft", "https://bioconductor.org/packages/3.8/bioc"),
                cran(),
            ],
            &[pkg("cli", "3.6.3", "CRAN")],
        ));
        assert_eq!(
            m.repository,
            vec![
                imported_repo("BioCsoft", "https://bioconductor.org/packages/3.8/bioc"),
                Repository::builtin("cran"),
                bioc_off(),
            ]
        );
    }

    // Without a Bioconductor version the built-in repository does not stand
    // for the listed ones, so a Bioconductor package stays in its own.
    #[test]
    fn bioconductor_packages_without_a_version_stay_in_their_repository() {
        let m = import(&lock(
            &[
                cran(),
                repo(
                    "BioCsoft",
                    "https://pkgs.example.com/bioconductor/packages/3.8/bioc",
                ),
            ],
            &[
                bioc_pkg("limma", "3.38.3", Some("BioCsoft")),
                bioc_pkg("Biobase", "2.42.0", None),
            ],
        ));
        assert_eq!(repository_of(&m, "limma").as_deref(), Some("BioCsoft"));
        assert_eq!(repository_of(&m, "Biobase").as_deref(), Some("BioCsoft"));
    }

    // A repository that the lock file does not list, or that rig cannot use,
    // is reported, not replaced by another one.
    #[test]
    fn bioconductor_packages_from_unusable_repositories_are_reported() {
        let (m, report) = import_report(&lock(
            &[
                cran(),
                repo(
                    "BioCsoft",
                    "https://pkgs.example.com/bioconductor/packages/3.8/bioc",
                ),
            ],
            &[
                bioc_pkg("a", "1.0.0", Some("private")),
                r#""b": {"Package": "b", "Version": "1.0.0", "Source": "Bioconductor",
                       "RemoteRepos": "ftp://example.com/bioc"}"#
                    .to_string(),
            ],
        ));
        assert_eq!(report.unresolved, ["a", "b"]);
        assert_eq!(repository_of(&m, "a"), None);
        assert_eq!(repository_of(&m, "b"), None);
    }

    // The lock files below are from renv's own test suite.

    #[test]
    fn a_lock_file_with_every_field_is_imported() {
        let m = import(
            r#"{"R": {"Version": "4.4.3", "Repositories": [{"Name": "CRAN", "URL": "https://cloud.r-project.org"}]},
                "Python": {"Name": ".venv", "Type": "virtualenv", "Version": "3.10.12"},
                "Bioconductor": {"Version": "3.18"},
                "Packages": {"fullpkg": {"Package": "fullpkg", "Version": "1.2.3", "Source": "Repository",
                  "Repository": "CRAN", "Hash": "0123456789abcdef0123456789abcdef", "biocViews": "Genomics",
                  "RemoteType": "github", "RemoteHost": "api.github.com", "RemoteUsername": "rstudio",
                  "RemoteRepo": "fullpkg", "RemoteRepos": "https://cloud.r-project.org", "RemoteRef": "main",
                  "RemotePkgRef": "fullpkg", "RemotePkgPlatform": "x86_64-pc-linux-gnu",
                  "RemoteSha": "abcdefabcdefabcdefabcdefabcdefab", "Authors@R": "person(\"Jane\")",
                  "Depends": ["R (>= 4.0)"], "Imports": ["cli", "rlang"], "LinkingTo": ["cpp11"],
                  "Config/Needs/website": "pkgdown"}}}"#,
        );
        assert_eq!(
            m.dependencies.get("fullpkg"),
            Some(&Dependency::Version("^1.2.3".to_string()))
        );
        assert_eq!(m.repository, vec![bioc_entry("3.18")]);
    }

    // renv lists the Bioconductor repositories of a lock file that has a
    // Bioconductor version, which the built-in repository includes.
    #[test]
    fn listed_bioconductor_repositories_are_the_builtin_one() {
        let m = import(
            r#"{"R": {"Version": "4.2.3", "Nickname": "Shortstop Beagle", "Repositories": [
                  {"Name": "CRAN", "URL": "https://cloud.r-project.org"},
                  {"Name": "BioCsoft", "URL": "https://bioconductor.org/packages/3.8/bioc"},
                  {"Name": "BioCann", "URL": "https://bioconductor.org/packages/3.8/data/annotation"}]},
                "Bioconductor": {"Version": "3.8"},
                "Packages": {
                  "markdown": {"Package": "markdown", "Version": "1.0", "Source": "Repository", "Repository": "CRAN"},
                  "limma": {"Package": "limma", "Version": "3.38.3", "Source": "Repository", "Repository": "BioCsoft"},
                  "mime": {"Package": "mime", "Version": "0.12.1", "Source": "GitHub", "RemoteType": "github",
                    "RemoteHost": "api.github.com", "RemoteUsername": "yihui", "RemoteRepo": "mime",
                    "RemoteRef": "main", "RemoteSha": "1763e0dcb72fb58d97bab97bb834fc71f1e012bc",
                    "Requirements": ["tools"]}}}"#,
        );
        assert_eq!(m.repository, vec![bioc_entry("3.8")]);
        assert_eq!(repository_of(&m, "limma").as_deref(), Some("bioc"));
        assert_eq!(repository_of(&m, "markdown"), None);
        assert_eq!(
            m.dependencies.get("mime"),
            Some(&Dependency::Detailed(Box::new(DepTable {
                git: Some("https://github.com/yihui/mime.git".to_string()),
                branch: Some("main".to_string()),
                ..Default::default()
            })))
        );
    }

    #[test]
    fn a_bioconductor_lock_file_is_imported() {
        let m = import(
            r#"{"R": {"Version": "4.1.2", "Repositories": [{"Name": "CRAN", "URL": "https://cloud.r-project.org"}]},
                "Bioconductor": {"Version": "3.13"},
                "Packages": {
                  "BiocGenerics": {"Package": "BiocGenerics", "Version": "0.38.0", "Source": "Bioconductor",
                    "git_url": "https://git.bioconductor.org/packages/BiocGenerics",
                    "git_branch": "RELEASE_3_13", "git_last_commit": "1db849a",
                    "git_last_commit_date": "2021-05-19", "Hash": "de5e346fed0fc44a0424a0531cf5d12d"},
                  "renv": {"Package": "renv", "Version": "0.14.0-69", "Source": "Local", "RemoteType": "local",
                    "RemoteUrl": "~/r/pkg/renv", "Hash": "41fed5e791a5f7ea1eb290eda0531722"}}}"#,
        );
        assert_eq!(m.repository, vec![bioc_entry("3.13")]);
        assert_eq!(
            m.dependencies.get("BiocGenerics"),
            Some(&Dependency::Detailed(Box::new(DepTable {
                version: Some("^0.38.0".to_string()),
                repository: Some("bioc".to_string()),
                ..Default::default()
            })))
        );
        assert_eq!(
            m.dependencies.get("renv"),
            Some(&Dependency::Version("^0.14.0-69".to_string()))
        );
    }

    // Merging, as with --dependencies.

    #[test]
    fn merging_does_not_turn_bioconductor_off() {
        let mut manifest = Rproj::minimal("proj");
        merge(
            &mut manifest,
            &lock(&[cran()], &[pkg("cli", "3.6.3", "CRAN")]),
        );
        assert!(manifest.repository.is_empty());
    }

    #[test]
    fn merging_keeps_the_existing_repositories() {
        let mut manifest = Rproj::minimal("proj");
        manifest
            .repository
            .push(Repository::at_url("internal", "https://old.example.com"));
        merge(
            &mut manifest,
            &lock(
                &[
                    repo("internal", INTERNAL_URL),
                    repo("extra", "https://extra.example.com"),
                ],
                &[pkg("acme", "1.2.0", "internal")],
            ),
        );
        // The built-in CRAN of the manifest is not turned off, and the
        // repository of the same name as one of the manifest, but at another
        // URL, is one of its own that the package is pinned to.
        assert_eq!(
            manifest.repository,
            vec![
                Repository::at_url("internal", "https://old.example.com"),
                imported_repo("internal-repo", INTERNAL_URL),
                imported_repo("extra", "https://extra.example.com"),
            ]
        );
        assert_eq!(
            repository_of(&manifest, "acme").as_deref(),
            Some("internal-repo")
        );
    }

    #[test]
    fn a_repository_of_the_manifest_at_the_same_url_is_reused() {
        let mut manifest = Rproj::minimal("proj");
        manifest
            .repository
            .push(Repository::at_url("mine", "https://pkgs.example.com/prod/"));
        merge(
            &mut manifest,
            &lock(
                &[repo("internal", INTERNAL_URL)],
                &[pkg("acme", "1.2.0", "internal")],
            ),
        );
        assert_eq!(manifest.repository.len(), 1);
        assert_eq!(repository_of(&manifest, "acme").as_deref(), Some("mine"));
    }

    // A package that renv installed from CRAN is not taken from the repository
    // the manifest pinned it to.
    #[test]
    fn a_package_from_cran_loses_the_pin_of_the_manifest() {
        let mut manifest = Rproj::minimal("proj");
        manifest
            .repository
            .push(Repository::at_url("internal", INTERNAL_URL));
        manifest.dependencies.insert(
            "cli".to_string(),
            Dependency::Detailed(Box::new(DepTable {
                version: Some("^3.0.0".to_string()),
                repository: Some("internal".to_string()),
                ..Default::default()
            })),
        );
        manifest.dependencies.insert(
            "ts".to_string(),
            Dependency::Detailed(Box::new(DepTable {
                git: Some("https://github.com/x/ts".to_string()),
                ..Default::default()
            })),
        );
        merge(
            &mut manifest,
            &lock(
                &[cran()],
                &[pkg("cli", "3.6.3", "CRAN"), pkg("ts", "0.1.0", "CRAN")],
            ),
        );
        assert_eq!(
            manifest.dependencies.get("cli"),
            Some(&Dependency::Version("^3.6.3".to_string()))
        );
        // A git source stays as it is.
        assert!(matches!(
            manifest.dependencies.get("ts"),
            Some(Dependency::Detailed(t)) if t.git.is_some()
        ));
    }

    // A package that renv installed from git is no longer taken from the
    // repository the manifest pinned it to, nor limited to its version
    // requirement.
    #[test]
    fn a_git_package_replaces_the_entry_of_the_manifest() {
        let mut manifest = Rproj::minimal("proj");
        manifest
            .repository
            .push(Repository::at_url("internal", INTERNAL_URL));
        manifest.dependencies.insert(
            "ts".to_string(),
            Dependency::Detailed(Box::new(DepTable {
                version: Some("^2.0.0".to_string()),
                repository: Some("internal".to_string()),
                attach: Some(true),
                ..Default::default()
            })),
        );
        merge(
            &mut manifest,
            &lock(
                &[cran()],
                &[
                    r#""ts": {"Package": "ts", "Version": "0.1.0", "Source": "GitHub",
                       "RemoteUsername": "o", "RemoteRepo": "ts", "RemoteSha": "3f2a"}"#
                        .to_string(),
                ],
            ),
        );
        assert_eq!(
            manifest.dependencies.get("ts"),
            Some(&Dependency::Detailed(Box::new(DepTable {
                attach: Some(true),
                git: Some("https://github.com/o/ts.git".to_string()),
                rev: Some("3f2a".to_string()),
                ..Default::default()
            })))
        );
    }

    // Every table that lists the package gets the git source, so that the
    // manifest has one source for it.
    #[test]
    fn a_git_package_replaces_its_entries_in_other_tables() {
        let mut manifest = Rproj::minimal("proj");
        manifest
            .repository
            .push(Repository::at_url("internal", INTERNAL_URL));
        manifest
            .dependency_groups
            .entry("dev".to_string())
            .or_default()
            .dependencies
            .insert(
                "ts".to_string(),
                Dependency::Detailed(Box::new(DepTable {
                    version: Some("^2.0.0".to_string()),
                    repository: Some("internal".to_string()),
                    vignette_builder: Some(true),
                    ..Default::default()
                })),
            );
        manifest
            .linking_dependencies
            .insert("ts".to_string(), Dependency::Version("^2.0.0".to_string()));
        merge(
            &mut manifest,
            &lock(
                &[cran()],
                &[
                    r#""ts": {"Package": "ts", "Version": "0.1.0", "Source": "GitHub",
                       "RemoteUsername": "o", "RemoteRepo": "ts", "RemoteSha": "3f2a"}"#
                        .to_string(),
                ],
            ),
        );
        let git = DepTable {
            git: Some("https://github.com/o/ts.git".to_string()),
            rev: Some("3f2a".to_string()),
            ..Default::default()
        };
        assert_eq!(
            manifest.dependencies.get("ts"),
            Some(&Dependency::Detailed(Box::new(git.clone())))
        );
        assert_eq!(
            manifest.linking_dependencies.get("ts"),
            Some(&Dependency::Detailed(Box::new(git.clone())))
        );
        assert_eq!(
            manifest.dependency_groups["dev"].dependencies.get("ts"),
            Some(&Dependency::Detailed(Box::new(DepTable {
                vignette_builder: Some(true),
                ..git
            })))
        );
        assert!(manifest.repository_pins().unwrap().is_empty());
    }

    // An existing entry for a private Bioconductor serves the Bioconductor
    // packages, so it cannot be explicit.
    #[test]
    fn an_existing_private_bioconductor_is_not_explicit() {
        let mut manifest = Rproj::minimal("proj");
        manifest
            .repository
            .push(imported_repo("mirror", PRIVATE_BIOC_URL));
        merge(
            &mut manifest,
            &bioc_lock(
                "3.19",
                &[cran(), repo("BioCsoft", PRIVATE_BIOC_URL)],
                &[bioc_pkg("Biobase", "2.64.0", None)],
            ),
        );
        assert_eq!(manifest.repository[0].name, "mirror");
        assert_eq!(manifest.repository[0].explicit, None);
        assert_eq!(repository_of(&manifest, "Biobase"), None);
    }

    #[test]
    fn merging_records_the_bioconductor_version() {
        let lock = bioc_lock("3.19", &[cran()], &[bioc_pkg("Biobase", "2.64.0", None)]);
        let mut manifest = Rproj::minimal("proj");
        merge(&mut manifest, &lock);
        // Not explicit: the packages of the manifest that are not pinned keep
        // using Bioconductor.
        assert_eq!(
            manifest.repository,
            vec![Repository {
                version: Some("3.19".to_string()),
                ..Repository::builtin("bioc")
            }]
        );
        assert_eq!(repository_of(&manifest, "Biobase").as_deref(), Some("bioc"));

        // An existing entry is kept as it is.
        let mut manifest = Rproj::minimal("proj");
        manifest.repository.push(Repository {
            version: Some("3.18".to_string()),
            ..Repository::builtin("bioc")
        });
        merge(&mut manifest, &lock);
        assert_eq!(manifest.repository.len(), 1);
        assert_eq!(manifest.repository[0].version.as_deref(), Some("3.18"));
    }

    #[test]
    fn merging_keeps_private_bioconductor_ahead_of_the_built_in() {
        let mut manifest = Rproj::minimal("proj");
        merge(
            &mut manifest,
            &bioc_lock(
                "3.19",
                &[
                    repo(
                        "BioCsoft",
                        "https://pkgs.example.com/bioconductor/packages/3.19/bioc",
                    ),
                    cran(),
                ],
                &[bioc_pkg("Biobase", "2.64.0", None)],
            ),
        );
        assert_eq!(repository_names(&manifest), ["BioCsoft", "bioc"]);
    }

    // A dependency that comes from git, a URL or a path is not pinned, so the
    // repository it would have been pinned to may be turned off.
    #[test]
    fn merging_does_not_pin_to_a_repository_that_is_turned_off() {
        let lock = lockfile(&bioc_lock(
            "3.19",
            &[cran()],
            &[bioc_pkg("Biobase", "2.64.0", None)],
        ));
        let bioc_off_manifest = || {
            let mut manifest = Rproj::minimal("proj");
            manifest.repository.push(bioc_off());
            manifest
        };

        let err =
            apply_records(&mut bioc_off_manifest(), &lock.records(), false, &OPTIONS).unwrap_err();
        assert!(err.to_string().contains("turned off"), "{}", err);

        let mut manifest = bioc_off_manifest();
        manifest.dependencies.insert(
            "Biobase".to_string(),
            Dependency::Detailed(Box::new(DepTable {
                git: Some("https://github.com/x/Biobase".to_string()),
                ..Default::default()
            })),
        );
        apply_records(&mut manifest, &lock.records(), false, &OPTIONS).unwrap();
        assert_eq!(repository_of(&manifest, "Biobase"), None);
    }

    #[test]
    fn merging_does_not_pin_a_package_to_two_repositories() {
        let mut manifest = Rproj::minimal("proj");
        manifest
            .repository
            .push(Repository::at_url("other", "https://other.example.com"));
        manifest
            .dependency_groups
            .entry("dev".to_string())
            .or_default()
            .dependencies
            .insert(
                "acme".to_string(),
                Dependency::Detailed(Box::new(DepTable {
                    repository: Some("other".to_string()),
                    ..Default::default()
                })),
            );
        let lock = lockfile(&lock(
            &[repo("internal", INTERNAL_URL)],
            &[pkg("acme", "1.2.0", "internal")],
        ));
        let err = apply_records(&mut manifest, &lock.records(), false, &OPTIONS).unwrap_err();
        assert!(err.to_string().contains("two repositories"), "{}", err);
    }

    // A package from CRAN can come from another repository that serves every
    // package, e.g. a CRAN mirror, but not from nowhere.
    #[test]
    fn merging_a_cran_package_needs_a_repository_that_serves_it() {
        let lock = lockfile(&lock(&[cran()], &[pkg("cli", "3.6.3", "CRAN")]));
        let mut manifest = Rproj::minimal("proj");
        manifest
            .repository
            .push(Repository::disabled(CRAN_REPOSITORY_NAME));
        let err = apply_records(&mut manifest, &lock.records(), false, &OPTIONS).unwrap_err();
        assert!(err.to_string().contains("Turn CRAN on"), "{}", err);

        manifest
            .repository
            .push(Repository::at_url("mirror", INTERNAL_URL));
        apply_records(&mut manifest, &lock.records(), false, &OPTIONS).unwrap();
        assert_eq!(repository_of(&manifest, "cli"), None);
    }

    // A package that another table of the manifest pins comes from there.
    #[test]
    fn merging_a_cran_package_pinned_elsewhere_does_not_need_cran() {
        let mut manifest = Rproj::minimal("proj");
        manifest.repository.extend([
            Repository::disabled(CRAN_REPOSITORY_NAME),
            imported_repo("internal", INTERNAL_URL),
        ]);
        manifest
            .dependency_groups
            .entry("dev".to_string())
            .or_default()
            .dependencies
            .insert(
                "cli".to_string(),
                Dependency::Detailed(Box::new(DepTable {
                    repository: Some("internal".to_string()),
                    ..Default::default()
                })),
            );
        let lock = lockfile(&lock(&[cran()], &[pkg("cli", "3.6.3", "CRAN")]));
        apply_records(&mut manifest, &lock.records(), false, &OPTIONS).unwrap();
        assert_eq!(repository_of(&manifest, "cli"), None);
    }
}
