use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use clap::ArgMatches;
use log::{error, info, warn};
use serde_derive::Deserialize;
use serde_derive::Serialize;

use crate::common::*;
use crate::dcf::{DepVersionSpec, RDepType};
use crate::output::OUTPUT;
use crate::proj::{
    check_project_conflicts, init_rvenv_for_manifest, proj_binary_target, proj_lock_r_version,
    proj_read_manifest_deps, sc_proj_solve_project_deps, BASE_PKGS,
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
    let dependencies_only = args.get_flag("dependencies");
    let root = std::env::current_dir()?;
    let path = root.join(RPROJ_MANIFEST_FILE);
    let path = path.as_path();

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

    // A full import also creates the `.rvenv` layout, so check for conflicts
    // before writing anything. `--dependencies` only touches the manifest.
    if !dependencies_only && !args.get_flag("force") {
        check_project_conflicts(&root)?;
    }

    OUTPUT.status(&format!("Reading dependencies from {}", input));
    info!("Reading dependencies from {}", input);
    let contents = fs::read_to_string(input).map_err(|e| {
        OUTPUT.error(&format!("Cannot read {}: {}", input, e));
        error!("Cannot read {}: {}", input, e);
        e
    })?;
    let lockfile: REnvLockfile = serde_json::from_str(&contents)?;
    let dep_count = lockfile.Packages.len();

    let default_name = || -> String {
        std::env::current_dir()
            .ok()
            .and_then(|cwd| cwd.file_name().map(|n| n.to_string_lossy().into_owned()))
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
    };
    let report = apply_lockfile(
        &mut manifest,
        &lockfile,
        !dependencies_only || !path_existed,
        &options,
    )?;
    let minor = minor_r_version(&lockfile.R.Version)?;

    for msg in import_warnings(&report, input) {
        OUTPUT.warn(&msg);
        warn!("{}", msg);
    }

    fs::write(path, toml::to_string_pretty(&manifest)?)?;

    // Detecting the types needs the network, so not if nobody looks.
    if !OUTPUT.is_quiet() && !manifest.repository.is_empty() {
        let bioc_version = lockfile.Bioconductor.as_ref().map(|b| b.Version.as_str());
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
        init_rvenv_for_manifest(args, &root, path)?;
    }

    Ok(())
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

    /// The `git` remote a GitHub, GitLab, Bitbucket or git package was
    /// installed from, if any.
    fn git_dep_table(&self) -> Option<DepTable> {
        // renv installs from `Source` only. `RemoteType` may be left over from
        // where the package was first installed from.
        let kind = self.Source.trim().to_ascii_lowercase();
        let host_repo = |default_host: &str, api: &str| -> Option<String> {
            let user = non_empty(&self.RemoteUsername)?;
            let repo = non_empty(&self.RemoteRepo)?;
            let repo = repo.strip_suffix(".git").unwrap_or(repo);
            let host = non_empty(&self.RemoteHost)
                .map(|h| git_host(h, api))
                .unwrap_or_else(|| default_host.to_string());
            Some(format!("{}/{}/{}.git", host, user, repo))
        };
        let git = match kind.as_str() {
            "github" => host_repo("https://github.com", "api.github.com")?,
            "gitlab" => host_repo("https://gitlab.com", "")?,
            "bitbucket" => host_repo("https://bitbucket.org", "api.bitbucket.org")?,
            "git" | "xgit" | "git2r" => {
                let url = self.git_remote_url()?;
                github_https_url(url).unwrap_or_else(|| url.to_string())
            }
            _ => return None,
        };
        // `HEAD` is renv's spelling of "the default branch".
        let rev = non_empty(&self.RemoteSha)
            .or_else(|| non_empty(&self.RemoteRef).filter(|r| !r.eq_ignore_ascii_case("HEAD")))
            .map(str::to_string);
        Some(DepTable {
            git: Some(git),
            rev,
            subdir: non_empty(&self.RemoteSubdir).map(str::to_string),
            ..Default::default()
        })
    }

    /// Whether the package was installed from GitHub, GitLab, Bitbucket or
    /// a git URL, even if the lock file lacks the fields to import it.
    fn is_git_source(&self) -> bool {
        let kind = self.Source.trim().to_ascii_lowercase();
        matches!(
            kind.as_str(),
            "github" | "gitlab" | "bitbucket" | "git" | "xgit" | "git2r"
        )
    }

    /// The `RemoteUrl` of a package installed from a git URL, as opposed to
    /// GitHub, GitLab or Bitbucket.
    fn git_remote_url(&self) -> Option<&str> {
        let kind = self.Source.trim().to_ascii_lowercase();
        if matches!(kind.as_str(), "git" | "xgit" | "git2r") {
            non_empty(&self.RemoteUrl)
        } else {
            None
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

// -------------------------------------------------------------------------------------

/// The names renv uses for CRAN, and for Posit Package Manager's CRAN, which
/// it records for packages installed from there.
const CRAN_NAMES: [&str; 4] = ["CRAN", "RSPM", "PPM", "P3M"];

/// Whether `name` is a name that renv takes for Bioconductor.
fn is_bioc_repository_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("bioc") || name.eq_ignore_ascii_case("bioconductor")
}

/// Extract the base URL of a git host from the `RemoteHost` of a lock file
/// entry. That field points to the web API.
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

fn non_empty(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

fn is_url(s: &str) -> bool {
    has_scheme(s, &["http://", "https://", "ftp://", "file://"])
}

/// Whether `s` is a URL of a scheme that rig can load a repository from.
fn is_http_url(s: &str) -> bool {
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

/// How a repository of the lock file is imported.
#[derive(Debug, PartialEq)]
enum ImportAs {
    /// As this built-in repository.
    Builtin(&'static str),
    /// As a repository of its own, at this URL.
    Url(String),
}

/// How the repository `repo` at `url` of the lock file is imported, from its
/// detected type. `bioc_version` is the Bioconductor release of the lock file.
///
/// A public repository is the built-in one it stands for: a CRAN mirror and
/// Posit Package Manager's CRAN are the built-in CRAN, a Bioconductor
/// repository of the lock file's release is the built-in Bioconductor, and a
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

/// The names renv gives to the repositories of Bioconductor.
const RENV_BIOC_NAMES: [&str; 5] = [
    "BioCsoft",
    "BioCann",
    "BioCexp",
    "BioCworkflows",
    "BioCbooks",
];

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
    /// From a repository the lock file does not describe, or that rig cannot
    /// use, so it is an ordinary version requirement.
    Unresolved,
    /// Not from a package repository or git, e.g. a URL or a local file.
    OtherSource,
    /// From a repository the lock file does not name, so any pin of the
    /// manifest stays.
    Keep,
}

/// The repositories of an imported `renv.lock` file.
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
    /// The name each renv repository (lower case) has in the manifest.
    names: HashMap<String, String>,
    /// The name of every repository that is not the built-in CRAN, by its
    /// normalized URL.
    urls: Vec<(String, String)>,
    /// Whether the lock file lists any repository.
    listed: bool,
    /// Whether the lock file has a Bioconductor version, so that the built-in
    /// Bioconductor repository stands for the Bioconductor repositories it
    /// lists.
    bioc: bool,
    /// The Bioconductor version of the lock file.
    bioc_version: Option<String>,
    /// The names of the Bioconductor repositories of the lock file that are not
    /// the built-in one, e.g. a private mirror or a dated snapshot. renv
    /// installs the packages of Bioconductor from these, and there is no
    /// telling which package is in which, so they serve every package.
    other_bioc: Vec<String>,
}

impl<'a> ImportedRepositories<'a> {
    /// The repositories of an `renv.lock`, in order. `bioc_version` is its
    /// Bioconductor version, and `detect` finds the type of a repository.
    fn new(
        repos: &[REnvLockfileRepository],
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
            let key = repo.Name.to_lowercase();
            if imported.names.contains_key(&key) || !is_http_url(&repo.URL) {
                continue;
            }
            let name = imported.add_url(&repo.Name, &repo.URL);
            if imported.bioc
                && RENV_BIOC_NAMES
                    .iter()
                    .any(|n| repo.Name.eq_ignore_ascii_case(n))
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
        // renv installs a package from the repository it recorded for it, and
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
    /// the lock file does not list. `None` if it is a name the lock file does
    /// not list, or a repository rig cannot load packages from, see
    /// [`is_http_url`].
    ///
    /// A package that says it is from CRAN, or from Posit Package Manager's
    /// CRAN, is from the built-in CRAN, even if the lock file does not list it,
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
    fn placement(&mut self, pkg: &REnvLockfilePackage) -> Placement {
        if pkg.is_bioconductor() {
            return self.bioc_placement(pkg);
        }
        if !pkg.is_repository_source() {
            return Placement::OtherSource;
        }
        let Some((reference, name)) = pkg.repository_reference() else {
            return Placement::Keep;
        };
        match self.resolve(reference, name) {
            Some(repo) if repo != CRAN_REPOSITORY_NAME => Placement::Pin(repo),
            Some(_) => Placement::Cran,
            None => Placement::Unresolved,
        }
    }

    /// Where the manifest takes the Bioconductor package `pkg` from.
    ///
    /// A package that names a repository of the lock file is pinned to it, as
    /// renv prefers that repository. Otherwise, with a Bioconductor version in
    /// the lock file, a package is pinned to the built-in Bioconductor
    /// repository, unless the lock file lists another Bioconductor repository,
    /// which then serves it. Without a Bioconductor version, a package stays
    /// in the repository it comes from.
    fn bioc_placement(&mut self, pkg: &REnvLockfilePackage) -> Placement {
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
        let Some((reference, name)) = pkg.repository_reference() else {
            return fallback;
        };
        match self.resolve(reference, name) {
            Some(repo) if repo != CRAN_REPOSITORY_NAME => Placement::Pin(repo),
            _ if is_bioc_repository_name(reference) => builtin(),
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
        // renv installs from the repositories it lists only, so the built-in
        // Bioconductor is off too, unless the lock file uses it.
        if self.listed && !self.bioc && !bioc_used {
            self.entries
                .push(Repository::disabled(BIOC_REPOSITORY_NAME));
        }
        self.entries
    }
}

/// How the repositories of an `renv.lock` are imported.
struct ImportOptions<'a> {
    /// Finds the type of the repository at a URL, see [`resolve_repo`].
    detect: &'a dyn Fn(&str) -> DetectedRepo,
    /// Whether PPM snapshots stay as they are, see [`import_as`].
    keep_snapshots: bool,
}

/// What an import found in an `renv.lock` that it could not make full use of.
#[derive(Debug, Default)]
struct ImportReport {
    /// The packages from a repository the lock file does not describe, in
    /// order. They are imported as plain version requirements.
    unresolved: Vec<String>,
    /// The packages from a source that rig cannot import, e.g. a URL or a
    /// local file, in order. They are imported as plain version requirements.
    other_sources: Vec<String>,
    /// The packages from GitHub, GitLab, Bitbucket or a git URL whose lock
    /// file entry does not say where, in order. They are imported as plain
    /// version requirements.
    incomplete_git: Vec<String>,
    /// The packages from a `git://github.com/` URL, in order. They are
    /// imported from the `https://` URL instead.
    github_git_urls: Vec<String>,
    /// The packages from a git URL that is not an `http://` or `https://`
    /// URL, e.g. an SSH URL, in order. They are imported with that URL.
    other_git_urls: Vec<String>,
}

/// The warnings about the packages of the lock file `input` that the import
/// could not make full use of, so that they may need manual adjustment.
fn import_warnings(report: &ImportReport, input: &str) -> Vec<String> {
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
            "{} package(s) in {} come from GitHub, GitLab, Bitbucket or git, but the \
             lock file does not say where (it needs `RemoteUsername` and `RemoteRepo`, \
             or `RemoteUrl`); they were imported as ordinary version requirements: {}",
            report.incomplete_git.len(),
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

    if !report.unresolved.is_empty() {
        warnings.push(format!(
            "{} package(s) in {} come from a repository that is not listed in the \
             lock file, or that rig cannot use (it must be at an http:// or \
             https:// URL); they were imported as ordinary version requirements: {}",
            report.unresolved.len(),
            input,
            report.unresolved.join(", ")
        ));
    }

    warnings
}

/// The `R` requirement of the manifest for `lockfile`. A Bioconductor
/// release is built for one R minor version, so a lock file with one is
/// limited to its R minor version.
fn r_requirement(lockfile: &REnvLockfile) -> Result<String, Box<dyn Error>> {
    let minor = minor_r_version(&lockfile.R.Version)?;
    if lockfile.Bioconductor.is_none() {
        return Ok(format!(">= {}", minor));
    }
    let (major, minor_part) = minor
        .split_once('.')
        .ok_or_else(|| format!("Invalid R version: {}", minor))?;
    let next = minor_part.parse::<u64>()? + 1;
    Ok(format!(">= {}, < {}.{}", minor, major, next))
}

/// Merge the R version, repositories and packages of `lockfile` into
/// `manifest`.
///
/// `repositories` is whether to import the repositories as well. Merging into
/// an existing manifest keeps its own: it only gains the repositories it does
/// not have yet, never a change to its built-in ones.
///
/// See [`ImportedRepositories::placement`] for the repository each package
/// is taken from. A package installed from a git remote becomes a `git`
/// dependency at the locked commit instead.
fn apply_lockfile(
    manifest: &mut Rproj,
    lockfile: &REnvLockfile,
    repositories: bool,
    options: &ImportOptions,
) -> Result<ImportReport, Box<dyn Error>> {
    manifest.add_dependency("R", &r_requirement(lockfile)?, false);

    let mut repos = ImportedRepositories::new(
        &lockfile.R.Repositories,
        lockfile.Bioconductor.as_ref().map(|b| b.Version.as_str()),
        &manifest.repository,
        options.detect,
        options.keep_snapshots,
    );
    let mut report = ImportReport::default();
    let mut bioc_used = false;
    let mut from_cran: Vec<String> = vec![];

    // In order, so that the names of repositories found on the way do not
    // depend on the order of the lock file's hash map.
    let mut packages: Vec<_> = lockfile.Packages.iter().collect();
    packages.sort_by(|a, b| a.0.cmp(b.0));
    for (name, pkg) in packages {
        if let Some(table) = pkg.git_dep_table() {
            if let Some(url) = pkg.git_remote_url() {
                if github_https_url(url).is_some() {
                    report.github_git_urls.push(name.clone());
                } else if !is_http_url(url) {
                    report.other_git_urls.push(name.clone());
                }
            }
            set_git_source(manifest, name, &table);
            continue;
        }
        manifest.add_dependency(name, &format!("^{}", pkg.Version), false);
        match repos.placement(pkg) {
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
            Placement::OtherSource if pkg.is_git_source() => {
                report.incomplete_git.push(name.clone())
            }
            Placement::OtherSource => report.other_sources.push(name.clone()),
            Placement::Keep => {}
        }
    }

    let other_bioc = std::mem::take(&mut repos.other_bioc);
    let mut entries = repos.finish(bioc_used);
    if let Some(bioc) = &lockfile.Bioconductor {
        entries.insert(
            0,
            Repository {
                version: Some(bioc.Version.clone()),
                explicit: Some(true),
                ..Repository::builtin(BIOC_REPOSITORY_NAME)
            },
        );
    }
    // When merging, the built-in Bioconductor entry goes last, so that it does
    // not take precedence over the lock file's own Bioconductor repositories.
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
                    "Repository {} of the lock file differs from the one in {}, keeping the latter",
                    entry.name, RPROJ_MANIFEST_FILE
                );
            }
            Some(_) => {}
            None if repositories || !entry.is_builtin() => manifest.repository.push(entry),
            // The manifest gets the Bioconductor release of the lock file, but
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
                "Cannot import the lock file into {}: {}",
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
                 repository serves every package. Turn CRAN on to import the lock file.",
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
/// or repository pin: the locked commit replaces them, and it may not
/// satisfy them.
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
/// `bioc_version` is the Bioconductor release of the lock file, if any, and
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

#[cfg(test)]
mod tests {
    use super::*;

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

    /// The type of the repository at `url`, as the detection finds it: CRAN
    /// mirrors, Posit Package Manager, Bioconductor mirrors and r-universe by
    /// their URL, anything else is a CRAN-like repository. `ppm.example.com`
    /// is a private PPM.
    fn test_detect(url: &str) -> DetectedRepo {
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

    const OPTIONS: ImportOptions<'static> = ImportOptions {
        detect: &test_detect,
        keep_snapshots: false,
    };

    /// `--keep-ppm-snapshots`.
    const KEEP: ImportOptions<'static> = ImportOptions {
        detect: &test_detect,
        keep_snapshots: true,
    };

    /// Import the lock file `json` into a new manifest, which must be valid.
    fn import_report(json: &str) -> (Rproj, ImportReport) {
        import_report_with(json, &OPTIONS)
    }

    fn import_report_with(json: &str, options: &ImportOptions) -> (Rproj, ImportReport) {
        let mut manifest = Rproj::minimal("proj");
        let report = apply_lockfile(&mut manifest, &lockfile(json), true, options).unwrap();
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
        apply_lockfile(manifest, &lockfile(json), false, options).unwrap();
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
    fn two_names_for_cran_are_one_repository() {
        let mut repos = ImportedRepositories::new(
            &[
                REnvLockfileRepository {
                    Name: "RSPM".to_string(),
                    URL: "https://packagemanager.posit.co/cran/latest".to_string(),
                },
                REnvLockfileRepository {
                    Name: "CRAN".to_string(),
                    URL: "https://cloud.r-project.org".to_string(),
                },
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
        pkg.git_dep_table()
    }

    #[test]
    fn github_package_is_a_git_dependency_at_the_locked_commit() {
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
                rev: Some("3f2a9c1".to_string()),
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
        assert_eq!(t.rev, None);
        assert_eq!(t.subdir.as_deref(), Some("pkg"));
    }

    #[test]
    fn github_without_a_sha_uses_the_ref() {
        let t = git_dep(
            r#"{"Package": "ts", "Version": "0.1.0", "Source": "GitHub",
                "RemoteUsername": "o", "RemoteRepo": "r", "RemoteRef": "v1.2.0"}"#,
        )
        .unwrap();
        assert_eq!(t.rev.as_deref(), Some("v1.2.0"));
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
        let t = pkg.git_dep_table().unwrap();
        assert_eq!(t.git.as_deref(), Some("https://github.com/o/p.git"));
        assert_eq!(t.rev.as_deref(), Some("abc123"));
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
        let t = pkg.git_dep_table().unwrap();
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
                rev: Some("babda05db8146b37a552c77651e2cd084a05ce98".to_string()),
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
        assert_eq!(t.rev.as_deref(), Some("main"));
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
                rev: Some("1763e0dcb72fb58d97bab97bb834fc71f1e012bc".to_string()),
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

        let err = apply_lockfile(&mut bioc_off_manifest(), &lock, false, &OPTIONS).unwrap_err();
        assert!(err.to_string().contains("turned off"), "{}", err);

        let mut manifest = bioc_off_manifest();
        manifest.dependencies.insert(
            "Biobase".to_string(),
            Dependency::Detailed(Box::new(DepTable {
                git: Some("https://github.com/x/Biobase".to_string()),
                ..Default::default()
            })),
        );
        apply_lockfile(&mut manifest, &lock, false, &OPTIONS).unwrap();
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
        let err = apply_lockfile(&mut manifest, &lock, false, &OPTIONS).unwrap_err();
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
        let err = apply_lockfile(&mut manifest, &lock, false, &OPTIONS).unwrap_err();
        assert!(err.to_string().contains("Turn CRAN on"), "{}", err);

        manifest
            .repository
            .push(Repository::at_url("mirror", INTERNAL_URL));
        apply_lockfile(&mut manifest, &lock, false, &OPTIONS).unwrap();
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
        apply_lockfile(&mut manifest, &lock, false, &OPTIONS).unwrap();
        assert_eq!(repository_of(&manifest, "cli"), None);
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
        let m = import(&lock(
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
        let m = import(&lock(
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
        let m = import(&lock(
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
        let m = import(&bioc_lock(
            "3.19",
            &[
                cran(),
                repo("BioCsoft", "https://bioconductor.org/packages/3.19/bioc"),
            ],
            &[bioc_pkg("limma", "3.60.0", None)],
        ));
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
