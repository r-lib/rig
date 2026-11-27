//! The repositories of a private Posit Package Manager instance.
//!
//! If `PACKAGEMANAGER_ADDRESS` (or the `ppm-address` config key) names a
//! Package Manager instance other than public P3M, then new R installations
//! use all R and Bioconductor repositories of that instance, and none of the
//! public repositories built into rig. This module builds the `PPM`
//! repository group for that, from the instance's repository list.
//!
//! The URLs of each repository follow the built-in P3M group: the platform
//! globs, archs and Linux build targets come from there, with the base URL and
//! the repository name replaced. Linux targets that the instance does not
//! build binaries for are left out, those platforms use the source URL.

use std::collections::HashSet;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use log::debug;
use serde::{Deserialize, Serialize};

use super::binaries::{ppm_url, PpmStatus};
use super::config::{Enabled, RepoEntry, Repository};
use super::detect::RepoType;
use super::feed::{BIOC_METADATA_URL, PPM_METADATA_URL};
use crate::cache::get_cache_dir;
use crate::hardcoded::HC_REPOS;
use crate::ppm::api;

/// Name of the repository group of the instance.
pub const INSTANCE_GROUP: &str = "PPM";

/// The prefix of the URLs of the built-in P3M group.
const TEMPLATE_PREFIX: &str = "https://p3m.dev/cran/";

/// How long the repository list of an instance is cached.
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// One source of an instance repository.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstanceSource {
    /// PPM's source type, e.g. `cran`, `curated-cran`, `bioc`, `git`, see
    /// [`crate::ppm::ppm_repos::source_type_name`].
    pub source_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bioc_version: Option<String>,
}

/// One repository of an instance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstanceRepo {
    pub name: String,
    /// `R` or `Bioconductor`. Other types are not stored.
    pub repo_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// `None` if the sources could not be listed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources: Option<Vec<InstanceSource>>,
}

/// What rig needs to know about an instance, as cached.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct InstanceInfo {
    pub repos: Vec<InstanceRepo>,
    /// The Linux build targets the instance builds binaries for, e.g.
    /// `noble`, `manylinux_2_28`. `None` if unknown, then all targets of
    /// the template are used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linux_targets: Option<Vec<String>>,
}

impl InstanceRepo {
    fn source_types(&self) -> Option<HashSet<&str>> {
        self.sources
            .as_ref()
            .map(|s| s.iter().map(|x| x.source_type.as_str()).collect())
    }

    /// Whether the repository has a CRAN or curated CRAN source, possibly
    /// among others.
    fn has_cran_source(&self) -> bool {
        self.sources.as_ref().is_some_and(|s| {
            s.iter()
                .any(|x| x.source_type == "cran" || x.source_type == "curated-cran")
        })
    }

    /// The single source type of the repository, if it has exactly one
    /// kind of source.
    fn single_source_type(&self) -> Option<&str> {
        let types = self.source_types()?;
        if types.len() == 1 {
            types.into_iter().next()
        } else {
            None
        }
    }

    /// The Bioconductor versions the repository serves, if known.
    fn bioc_versions(&self) -> Option<Vec<String>> {
        let mut vers: Vec<String> = self
            .sources
            .as_ref()?
            .iter()
            .filter_map(|s| s.bioc_version.clone())
            .collect();
        if vers.is_empty() {
            return None;
        }
        vers.sort();
        vers.dedup();
        Some(vers)
    }
}

fn cache_file(addr: &str) -> Result<PathBuf, Box<dyn Error>> {
    Ok(get_cache_dir()?.join("p3m").join(format!(
        "instance-{}.json",
        &crate::utils::calculate_hash(addr)[..12]
    )))
}

fn read_cache(path: &PathBuf) -> Option<(InstanceInfo, bool)> {
    let meta = fs::metadata(path).ok()?;
    let fresh = meta
        .modified()
        .ok()
        .and_then(|m| m.elapsed().ok())
        .is_some_and(|age| age < CACHE_TTL);
    let info = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    Some((info, fresh))
}

/// Query the instance's API.
fn fetch_info() -> Result<InstanceInfo, Box<dyn Error>> {
    let repos: Vec<crate::ppm::ppm_repos::Repo> = api::get(&["repos"], &vec![])?;
    let mut out = vec![];
    for repo in repos {
        if repo.hidden || (repo.repo_type != "R" && repo.repo_type != "Bioconductor") {
            continue;
        }
        let sources = match crate::ppm::ppm_repos::load_sources(Some(&repo.name)) {
            Ok(sources) => Some(
                sources
                    .iter()
                    .map(|s| InstanceSource {
                        source_type: s.type_name(),
                        bioc_version: s.bioc_version().map(|v| v.to_string()),
                    })
                    .collect(),
            ),
            Err(e) => {
                debug!("Cannot list sources of PPM repo {}: {}", repo.name, e);
                None
            }
        };
        out.push(InstanceRepo {
            name: repo.name,
            repo_type: repo.repo_type,
            description: repo.description,
            sources,
        });
    }
    let status = PpmStatus::load(None);
    if let Err(e) = &status {
        debug!("Cannot load PPM status: {}", e);
    }
    let status = status.ok();
    Ok(InstanceInfo {
        repos: out,
        linux_targets: status.map(|s| {
            s.distros
                .iter()
                .filter(|d| d.os == "linux" && d.binaries)
                .map(|d| d.platform().to_string())
                .collect()
        }),
    })
}

/// The repositories of the instance [`ppm_url`] names, from the cache if it
/// is less than a day old. If the instance cannot be reached, an older cache
/// is used, and without that it is an error: rig never falls back to the
/// public repositories silently.
pub fn load_instance_info() -> Result<InstanceInfo, Box<dyn Error>> {
    let addr = ppm_url();
    let path = cache_file(&addr)?;
    let cached = read_cache(&path);
    if let Some((info, true)) = &cached {
        return Ok(info.clone());
    }
    match fetch_info() {
        Ok(info) => {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Ok(json) = serde_json::to_vec_pretty(&info) {
                let _ = fs::write(&path, json);
            }
            Ok(info)
        }
        Err(e) => match cached {
            Some((info, _)) => {
                debug!("Using stale PPM repo list of {}: {}", addr, e);
                Ok(info)
            }
            None => bail!(
                "Cannot list the repositories of Package Manager at {} \
                 (set by PACKAGEMANAGER_ADDRESS or the ppm-address config key): {}",
                addr,
                e
            ),
        },
    }
}

/// The built-in groups that hold the URL templates: `(group, enabled)` for
/// `P3M` and `P3M-manylinux`.
fn template_groups() -> Vec<&'static Repository> {
    HC_REPOS
        .iter()
        .filter(|r| r.name == "P3M" || r.name == "P3M-manylinux")
        .collect()
}

/// The `Bioconductor` group of the built-in repositories, for the paths of
/// the Bioconductor repositories.
fn bioc_template() -> Option<&'static Repository> {
    HC_REPOS.iter().find(|r| r.name == "Bioconductor")
}

/// The Linux build target of a template URL rest, e.g. `noble` for
/// `__linux__/noble/latest`.
fn linux_target(rest: &str) -> Option<&str> {
    rest.strip_prefix("__linux__/")?.split('/').next()
}

/// Everything that varies between the entries of one instance repository.
struct EntrySpec<'a> {
    addr: &'a str,
    repo: &'a str,
    /// Name of the entry in the `repositories` file.
    name: String,
    /// Path below the repository's snapshot, e.g. `packages/%v/bioc`.
    sub: Option<&'a str>,
    metadata: Option<String>,
    repo_type: RepoType,
    bioc_versions: Option<Vec<String>>,
    /// Only the source URL, no binary URLs.
    source_only: bool,
}

/// The entries of one repository (or one Bioconductor sub-repository), from
/// the URL templates of the built-in P3M groups.
fn entries_for(spec: &EntrySpec, linux_targets: Option<&HashSet<&str>>) -> Vec<RepoEntry> {
    let mut out = vec![];
    for group in template_groups() {
        let manylinux = group.name == "P3M-manylinux";
        for t in group.repos.iter() {
            let Some(rest) = t.url.strip_prefix(TEMPLATE_PREFIX) else {
                continue;
            };
            let target = linux_target(rest);
            if spec.source_only && (target.is_some() || !t.fallback) {
                continue;
            }
            if let (Some(target), Some(targets)) = (target, linux_targets) {
                if !targets.contains(target) {
                    continue;
                }
            }
            let mut url = format!("{}/{}/{}", spec.addr, spec.repo, rest);
            if let Some(sub) = spec.sub {
                url = format!("{}/{}", url, sub);
            }
            out.push(RepoEntry {
                name: spec.name.clone(),
                url,
                metadata: spec.metadata.clone(),
                repo_type: Some(spec.repo_type),
                // The source URL is for every platform without a more
                // specific one, e.g. Windows on arm64, too.
                platforms: if t.fallback || spec.source_only {
                    None
                } else {
                    t.platforms.clone()
                },
                archs: if t.fallback || spec.source_only {
                    None
                } else {
                    t.archs.clone()
                },
                rversions: t.rversions.clone(),
                bioc_versions: spec.bioc_versions.clone(),
                enabled: if manylinux {
                    Some(group.enabled.clone())
                } else {
                    None
                },
                fallback: t.fallback && !spec.source_only,
                ..Default::default()
            });
        }
    }
    out
}

/// The `PPM` group of the instance at `addr`.
///
/// The main CRAN repository comes first, named `CRAN`: the first R
/// repository that has a CRAN or curated CRAN source, possibly among other
/// sources. Then
/// the other R repositories, then the Bioconductor ones. The first Bioconductor
/// repository gets the usual `BioCsoft`, `BioCann`, etc. names, the others
/// are prefixed with their own name.
pub fn instance_group(addr: &str, info: &InstanceInfo) -> Repository {
    let targets: Option<HashSet<&str>> = info
        .linux_targets
        .as_ref()
        .map(|t| t.iter().map(|x| x.as_str()).collect());
    let targets = targets.as_ref();

    let mut rrepos: Vec<&InstanceRepo> = info.repos.iter().filter(|r| r.repo_type == "R").collect();
    let main = rrepos.iter().position(|r| r.has_cran_source());
    if let Some(pos) = main {
        let main = rrepos.remove(pos);
        rrepos.insert(0, main);
    }

    let mut entries = vec![];
    for (i, repo) in rrepos.iter().enumerate() {
        let is_main = i == 0 && main.is_some();
        let single = repo.single_source_type();
        let (metadata, rtype) = match single {
            Some("cran") => (Some(PPM_METADATA_URL.to_string()), RepoType::PpmCran),
            // Not filtered to the curated packages yet.
            Some("curated-cran") => (Some(PPM_METADATA_URL.to_string()), RepoType::PpmOther),
            _ => (None, RepoType::PpmOther),
        };
        let spec = EntrySpec {
            addr,
            repo: &repo.name,
            name: if is_main {
                "CRAN".to_string()
            } else {
                repo.name.clone()
            },
            sub: None,
            metadata,
            repo_type: rtype,
            bioc_versions: None,
            source_only: false,
        };
        entries.extend(entries_for(&spec, targets));
    }

    let bioc_repos: Vec<&InstanceRepo> = info
        .repos
        .iter()
        .filter(|r| r.repo_type == "Bioconductor")
        .collect();
    let template: Vec<&RepoEntry> = bioc_template()
        .map(|g| g.repos.iter().collect())
        .unwrap_or_default();
    for (i, repo) in bioc_repos.iter().enumerate() {
        let single = repo.single_source_type();
        let with_metadata = matches!(single, Some("bioc") | Some("curated-bioc"));
        for t in template.iter() {
            let Some(sub) = t.url.strip_prefix("%bm/") else {
                continue;
            };
            let is_soft = t.name == "BioCsoft";
            let spec = EntrySpec {
                addr,
                repo: &repo.name,
                name: if i == 0 {
                    t.name.clone()
                } else {
                    format!("{}-{}", repo.name, t.name)
                },
                sub: Some(sub),
                metadata: if is_soft && with_metadata {
                    Some(BIOC_METADATA_URL.to_string())
                } else {
                    None
                },
                repo_type: RepoType::PpmBioc,
                bioc_versions: repo.bioc_versions(),
                // Binary packages only for the software repository.
                source_only: !is_soft,
            };
            entries.extend(entries_for(&spec, targets));
        }
    }

    Repository {
        name: INSTANCE_GROUP.to_string(),
        title: Some("Posit Package Manager".to_string()),
        description: Some(format!(
            "All R and Bioconductor repositories of the Package Manager at {}.",
            addr
        )),
        enabled: Enabled::Always(true),
        repos: entries,
        custom: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(t: &str, bioc: Option<&str>) -> InstanceSource {
        InstanceSource {
            source_type: t.to_string(),
            bioc_version: bioc.map(|b| b.to_string()),
        }
    }

    fn repo(name: &str, rtype: &str, sources: Vec<InstanceSource>) -> InstanceRepo {
        InstanceRepo {
            name: name.to_string(),
            repo_type: rtype.to_string(),
            description: None,
            sources: Some(sources),
        }
    }

    /// Like the test instance at `localhost:4242`.
    fn info() -> InstanceInfo {
        InstanceInfo {
            repos: vec![
                repo("all", "R", vec![src("cran", None), src("git", None)]),
                repo(
                    "bioconductor",
                    "Bioconductor",
                    vec![src("bioc", Some("3.22")), src("bioc", Some("3.23"))],
                ),
                repo("cran", "R", vec![src("cran", None)]),
                repo(
                    "curated-bioc",
                    "Bioconductor",
                    vec![src("curated-bioc", Some("3.23"))],
                ),
                repo("curated-cran", "R", vec![src("curated-cran", None)]),
                repo("local", "R", vec![src("local", None)]),
            ],
            linux_targets: Some(vec!["noble".to_string(), "manylinux_2_28".to_string()]),
        }
    }

    fn names(g: &Repository) -> Vec<String> {
        let mut out: Vec<String> = vec![];
        for e in g.repos.iter() {
            if !out.contains(&e.name) {
                out.push(e.name.clone());
            }
        }
        out
    }

    #[test]
    fn main_cran_repo_comes_first_as_cran() {
        let g = instance_group("http://localhost:4242", &info());
        assert_eq!(g.name, "PPM");
        assert!(matches!(g.enabled, Enabled::Always(true)));
        let n = names(&g);
        assert_eq!(
            &n[..4],
            &["CRAN", "cran", "curated-cran", "local"].map(String::from)
        );
        // `all` has a CRAN source among others, and it is the first one.
        assert!(g
            .repos
            .iter()
            .any(|e| e.name == "CRAN" && e.url == "http://localhost:4242/all/latest"));
        assert!(n.contains(&"BioCsoft".to_string()));
        assert!(n.contains(&"curated-bioc-BioCsoft".to_string()));
        for e in g.repos.iter() {
            assert!(e.url.starts_with("http://localhost:4242/"), "{}", e.url);
        }
    }

    #[test]
    fn curated_cran_can_be_the_main_repo() {
        let i = InstanceInfo {
            repos: vec![
                repo("local", "R", vec![src("local", None)]),
                repo("prod", "R", vec![src("curated-cran", None)]),
            ],
            linux_targets: None,
        };
        let g = instance_group("https://ppm.example.com", &i);
        assert_eq!(names(&g), vec!["CRAN".to_string(), "local".to_string()]);
        assert!(g
            .repos
            .iter()
            .any(|e| e.name == "CRAN" && e.url == "https://ppm.example.com/prod/latest"));
    }

    #[test]
    fn urls_follow_the_p3m_template() {
        let g = instance_group("http://localhost:4242", &info());
        let cran: Vec<&RepoEntry> = g.repos.iter().filter(|e| e.name == "cran").collect();
        let urls: Vec<&str> = cran.iter().map(|e| e.url.as_str()).collect();
        assert!(urls.contains(&"http://localhost:4242/cran/latest"));
        assert!(urls.contains(&"http://localhost:4242/cran/__linux__/noble/latest"));
        assert!(urls.contains(&"http://localhost:4242/cran/__linux__/manylinux_2_28/latest"));
        // Targets the instance does not build are left out.
        assert!(!urls.iter().any(|u| u.contains("jammy")));
        // The source URL is a fallback for all platforms.
        let fb: Vec<&&RepoEntry> = cran.iter().filter(|e| e.fallback).collect();
        assert_eq!(fb.len(), 1);
        assert!(fb[0].platforms.is_none());
        // manylinux is only a default on manylinux.
        let ml = cran.iter().find(|e| e.url.contains("manylinux")).unwrap();
        assert!(matches!(ml.enabled, Some(Enabled::OnPlatforms { .. })));
    }

    #[test]
    fn metadata_depends_on_the_sources() {
        let g = instance_group("http://localhost:4242", &info());
        let meta = |name: &str| {
            g.repos
                .iter()
                .find(|e| e.name == name)
                .unwrap()
                .metadata
                .clone()
        };
        assert_eq!(meta("cran").as_deref(), Some(PPM_METADATA_URL));
        assert_eq!(meta("curated-cran").as_deref(), Some(PPM_METADATA_URL));
        // `all`, with mixed sources.
        assert_eq!(meta("CRAN"), None);
        assert_eq!(meta("local"), None);
        assert_eq!(meta("BioCsoft").as_deref(), Some(BIOC_METADATA_URL));
        assert_eq!(meta("BioCann"), None);
    }

    #[test]
    fn bioc_repos_are_limited_to_their_versions() {
        let g = instance_group("http://localhost:4242", &info());
        let soft = g
            .repos
            .iter()
            .find(|e| e.name == "curated-bioc-BioCsoft" && e.fallback)
            .unwrap();
        assert_eq!(
            soft.url,
            "http://localhost:4242/curated-bioc/latest/packages/%v/bioc"
        );
        assert_eq!(soft.bioc_versions, Some(vec!["3.23".to_string()]));
        let ann: Vec<&RepoEntry> = g.repos.iter().filter(|e| e.name == "BioCann").collect();
        assert_eq!(ann.len(), 1);
        assert_eq!(
            ann[0].url,
            "http://localhost:4242/bioconductor/latest/packages/%v/data/annotation"
        );
        assert!(!ann[0].fallback);
    }

    #[test]
    fn unknown_targets_keep_the_whole_template() {
        let mut i = info();
        i.linux_targets = None;
        let g = instance_group("https://ppm.example.com", &i);
        assert!(g
            .repos
            .iter()
            .any(|e| e.url == "https://ppm.example.com/cran/__linux__/jammy/latest"));
    }
}
