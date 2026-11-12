//! rsconnect `manifest.json` import: `rig proj rsconnect import`.
//!
//! An rsconnect manifest lists every package of a deployed application, with
//! its exact version, the repository or git remote it came from, and its
//! `DESCRIPTION` fields. The import reads these into [`PackageRecords`], see
//! [`crate::import`]: `rproj.toml` gets the packages and their repositories,
//! and `rproj.lock` the exact versions of the manifest.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::error::Error;
use std::fs;

use clap::ArgMatches;
use log::{error, info};
use serde_json::Value;

use crate::import::{
    is_http_url, GitRemote, PackageRecords, PackageSource, ProjectImport, RecordedPackage,
    RecordedRepository, RepoReference,
};
use crate::output::OUTPUT;
use crate::proj::BASE_PKGS;
use crate::repos::detect::DetectedRepo;
use crate::repos::repo_types::resolve_repo;

/// `Source` values that name no place packages can be installed from.
const UNINSTALLABLE_SOURCES: [&str; 5] = ["local", "source", "unknown", "url", ""];

pub fn sc_rsconnect(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        Some(("import", s)) => sc_rsconnect_import(s, args, mainargs),
        _ => Ok(()), // unreachable
    }
}

/// `rig proj rsconnect import`: create a project from the packages of an
/// rsconnect `manifest.json`, with `rproj.lock` at their exact versions.
fn sc_rsconnect_import(
    args: &ArgMatches,
    _rsconnectargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let input = args
        .get_one::<String>("input")
        .map(|s| s.as_str())
        .unwrap_or("manifest.json");
    let platforms: Vec<String> = args
        .get_many::<String>("platform")
        .map(|vs| vs.cloned().collect())
        .unwrap_or_default();
    // A deployed application needs exactly its packages, so a package that
    // cannot come from where it was installed from fails the import.
    let project = ProjectImport::start(args)?.strict();

    OUTPUT.status(&format!("Reading packages from {}", input));
    info!("Reading packages from {}", input);
    let text = fs::read_to_string(input).map_err(|e| {
        let msg = format!("Cannot read {}: {}", input, e);
        OUTPUT.error(&msg);
        error!("{}", msg);
        msg
    })?;
    let manifest = Manifest::parse(&text).map_err(|e| format!("Invalid {}: {}", input, e))?;
    let r_version = manifest.r_version_for(args.get_one::<String>("r-version"), input)?;
    let detect = |url: &str| resolve_repo(url, None);
    let records = manifest.records(&r_version, &detect)?;
    project.finish(args, input, &records, &platforms)
}

// ------------------------------------------------------------------------
// The manifest

/// The R version and packages of a manifest.
#[derive(Debug)]
struct Manifest {
    /// `platform`: the R version the content was deployed with.
    r_version: Option<String>,
    /// The packages, by name.
    packages: BTreeMap<String, ManifestPackage>,
}

#[derive(Debug)]
struct ManifestPackage {
    name: String,
    /// `Source`: a repository name, `Bioconductor`, `github`, ...
    source: String,
    /// `Repository`: the URL of the repository, if recorded.
    repository: Option<String>,
    /// The package's own fields, and its `description`'s, by lowercase name.
    /// The package's own come first, so they win.
    fields: Vec<(String, String)>,
}

impl ManifestPackage {
    /// The first non-empty value of any of `names`, in the order of `names`.
    fn field(&self, names: &[&str]) -> Option<&str> {
        names.iter().find_map(|name| {
            let name = name.to_lowercase();
            self.fields
                .iter()
                .find(|(k, v)| *k == name && !v.is_empty())
                .map(|(_, v)| v.as_str())
        })
    }

    fn field_string(&self, names: &[&str]) -> Option<String> {
        self.field(names).map(str::to_string)
    }

    fn version(&self) -> Result<&str, Box<dyn Error>> {
        match self.field(&["Version"]) {
            Some(v) => Ok(v),
            None => bail!("Package {} has no Version in the manifest", self.name),
        }
    }

    /// The names of the packages this one needs installed: its `Depends`,
    /// `Imports` and `LinkingTo`, without R itself and version requirements.
    fn dependencies(&self) -> Vec<String> {
        let mut names: Vec<String> = vec![];
        for field in ["Depends", "Imports", "LinkingTo"] {
            for item in self.field(&[field]).unwrap_or_default().split(',') {
                let name = item.split('(').next().unwrap_or_default().trim();
                if !name.is_empty() && name != "R" && !names.iter().any(|n| n == name) {
                    names.push(name.to_string());
                }
            }
        }
        names
    }

    fn is_git_source(&self) -> bool {
        GitRemote::is_git_kind(&self.source)
    }

    fn is_bioconductor(&self) -> bool {
        self.source.eq_ignore_ascii_case("Bioconductor")
    }

    fn is_uninstallable(&self) -> bool {
        UNINSTALLABLE_SOURCES.contains(&self.source.to_lowercase().as_str())
    }

    /// The recorded repository URL, without the `src/contrib` or
    /// `bin/<platform>/contrib/<R version>` path of the package index it was
    /// found in.
    fn repository_url(&self) -> Option<&str> {
        let url = self.repository.as_deref()?.trim().trim_end_matches('/');
        let url = match url.strip_suffix("/src/contrib") {
            Some(root) => root,
            None => match url.rsplit_once("/bin/") {
                Some((root, tail)) if tail.contains("/contrib/") => root,
                _ => url,
            },
        };
        Some(url).filter(|u| !u.is_empty())
    }

    /// The git remote of a package from a git host or URL. rsconnect also
    /// records the GitHub fields of old versions of `remotes` and
    /// `devtools`.
    fn git_remote(&self) -> GitRemote {
        GitRemote {
            kind: self.source.to_ascii_lowercase(),
            host: self.field_string(&["RemoteHost"]),
            username: self.field_string(&["RemoteUsername", "GithubUsername"]),
            repo: self.field_string(&["RemoteRepo", "GithubRepo"]),
            url: self.field_string(&["RemoteUrl"]),
            reference: self.field_string(&["RemoteRef", "GithubRef"]),
            sha: self.field_string(&["RemoteSha", "GithubSha1"]),
            subdir: self.field_string(&["RemoteSubdir", "GithubSubdir"]),
        }
    }

    /// The recorded package. A package from a repository is from the one at
    /// its recorded URL, named after its `Source`, or else from the one its
    /// `Source` names.
    fn record(&self) -> Result<RecordedPackage, Box<dyn Error>> {
        let source = if self.is_git_source() {
            PackageSource::Git(self.git_remote())
        } else {
            let reference = match self.repository_url() {
                Some(url) => RepoReference {
                    repository: url.to_string(),
                    name: Some(self.source.clone()),
                },
                None => RepoReference {
                    repository: self.source.clone(),
                    name: None,
                },
            };
            if self.is_bioconductor() {
                PackageSource::Bioconductor(Some(reference))
            } else {
                PackageSource::Repository(Some(reference))
            }
        };
        Ok(RecordedPackage {
            name: self.name.clone(),
            version: self.version()?.to_string(),
            source,
        })
    }
}

/// A JSON scalar as text. `null`, objects and arrays are no value.
fn json_text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// The member of a JSON object whose name is `name`, ignoring case.
fn member<'a>(object: &'a serde_json::Map<String, Value>, name: &str) -> Option<&'a Value> {
    object
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v)
}

impl Manifest {
    fn parse(text: &str) -> Result<Manifest, Box<dyn Error>> {
        let root: Value = serde_json::from_str(text)?;
        let root = root.as_object().ok_or("not a JSON object")?;
        let r_version = member(root, "platform")
            .and_then(json_text)
            .filter(|v| !v.is_empty());
        let mut packages = BTreeMap::new();
        let Some(Value::Object(listed)) = member(root, "packages") else {
            return Ok(Manifest {
                r_version,
                packages,
            });
        };
        for (name, entry) in listed {
            let Some(entry) = entry.as_object() else {
                bail!("Package {} is not an object", name);
            };
            let text_of = |field: &str| member(entry, field).and_then(json_text);
            let mut fields: Vec<(String, String)> = vec![];
            for (key, value) in entry {
                if let Some(value) = json_text(value) {
                    fields.push((key.to_lowercase(), value));
                }
            }
            if let Some(Value::Object(description)) = member(entry, "description") {
                for (key, value) in description {
                    if let Some(value) = json_text(value) {
                        fields.push((key.to_lowercase(), value));
                    }
                }
            }
            packages.insert(
                name.clone(),
                ManifestPackage {
                    name: name.clone(),
                    source: text_of("Source").unwrap_or_default(),
                    repository: text_of("Repository").filter(|r| !r.is_empty()),
                    fields,
                },
            );
        }
        Ok(Manifest {
            r_version,
            packages,
        })
    }

    /// The R version of the project: `option`, the `--r-version` if given,
    /// or else the one the content was deployed with. The `R` requirement and
    /// `rproj.lock` both follow it.
    fn r_version_for(
        &self,
        option: Option<&String>,
        input: &str,
    ) -> Result<String, Box<dyn Error>> {
        match option.or(self.r_version.as_ref()) {
            Some(v) => Ok(v.clone()),
            None => bail!("{} records no R version, give one with --r-version", input),
        }
    }

    /// The packages to import: all but R's base packages.
    fn packages(&self) -> Vec<&ManifestPackage> {
        self.packages
            .values()
            .filter(|p| !BASE_PKGS.contains(&p.name.as_str()))
            .collect()
    }

    /// The packages of the manifest, for R `r_version`. `detect` finds the
    /// type of the repository at a URL, see [`resolve_repo`].
    ///
    /// Every repository URL of the manifest that rig can load packages from
    /// is a recorded repository, named after the `Source` of its first
    /// package. The Bioconductor release is
    /// the one of the Bioconductor packages' repositories.
    ///
    /// Fails if a package cannot be installed from where it came from, or a
    /// package depends on one that the manifest does not list.
    fn records(
        &self,
        r_version: &str,
        detect: &dyn Fn(&str) -> DetectedRepo,
    ) -> Result<PackageRecords, Box<dyn Error>> {
        let packages = self.packages();
        check_sources(&packages)?;
        check_dependencies(&packages)?;

        let mut repositories: Vec<RecordedRepository> = vec![];
        let mut bioc_versions: BTreeSet<String> = BTreeSet::new();
        let mut records: Vec<RecordedPackage> = vec![];
        for pkg in packages {
            if let Some(url) = pkg.repository_url().filter(|_| !pkg.is_git_source()) {
                if pkg.is_bioconductor() {
                    bioc_versions.extend(detect(url).bioc_version);
                }
                if is_http_url(url) && !repositories.iter().any(|r| r.url == url) {
                    repositories.push(RecordedRepository {
                        name: pkg.source.clone(),
                        url: url.to_string(),
                        bioconductor: false,
                    });
                }
            }
            records.push(pkg.record()?);
        }
        if bioc_versions.len() > 1 {
            bail!(
                "The manifest has packages from more than one Bioconductor release: {}",
                bioc_versions.into_iter().collect::<Vec<_>>().join(", ")
            );
        }
        Ok(PackageRecords {
            r_version: r_version.to_string(),
            bioc_version: bioc_versions.into_iter().next(),
            repositories,
            packages: records,
        })
    }
}

/// Fail if a package does not come from a repository or a git remote, or
/// the manifest does not say which commit of which git remote.
fn check_sources(packages: &[&ManifestPackage]) -> Result<(), Box<dyn Error>> {
    let mut problems: Vec<String> = vec![];
    for pkg in packages {
        if pkg.is_git_source() {
            let remote = pkg.git_remote();
            if remote.sha.is_none() {
                problems.push(format!(
                    "{} is from {}, but the manifest has no commit for it",
                    pkg.name, pkg.source
                ));
            } else if remote.dep_table().is_none() {
                problems.push(format!(
                    "{} is from {}, but the manifest does not say where to clone it from",
                    pkg.name, pkg.source
                ));
            }
        } else if pkg.is_uninstallable() {
            let what = if pkg.source.is_empty() {
                "has no Source".to_string()
            } else {
                format!("has Source `{}`", pkg.source)
            };
            problems.push(format!(
                "{} {}, it is not from a repository or a git remote",
                pkg.name, what
            ));
        }
    }
    if !problems.is_empty() {
        bail!(
            "rig cannot install these packages of the manifest:\n  {}",
            problems.join("\n  ")
        );
    }
    Ok(())
}

/// Fail if a package depends on one that the manifest does not list. The
/// solver would add the latest version of it, which is not what was deployed.
fn check_dependencies(packages: &[&ManifestPackage]) -> Result<(), Box<dyn Error>> {
    let present: HashSet<&str> = packages.iter().map(|p| p.name.as_str()).collect();
    let mut missing: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for pkg in packages {
        for dep in pkg.dependencies() {
            if !BASE_PKGS.contains(&dep.as_str()) && !present.contains(dep.as_str()) {
                missing.entry(dep).or_default().push(pkg.name.clone());
            }
        }
    }
    if !missing.is_empty() {
        let list: Vec<String> = missing
            .iter()
            .map(|(dep, by)| format!("{} (needed by {})", dep, by.join(", ")))
            .collect();
        bail!(
            "The manifest does not list these packages that others depend on: {}",
            list.join("; ")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::tests::{test_detect, test_ref_kind};
    use crate::import::{apply_records, strict_import_error, ImportOptions, ImportReport};
    use crate::rproj::Rproj;

    const OPTIONS: ImportOptions<'static> = ImportOptions {
        detect: &test_detect,
        keep_snapshots: false,
        ref_kind: &test_ref_kind,
    };

    fn fixture_path(name: &str) -> String {
        format!(
            "{}/tests/fixtures/rsconnect/{}",
            env!("CARGO_MANIFEST_DIR"),
            name
        )
    }

    fn parse(json: &str) -> Manifest {
        Manifest::parse(json).unwrap()
    }

    fn records(json: &str) -> Result<PackageRecords, Box<dyn Error>> {
        parse(json).records("4.4.3", &test_detect)
    }

    /// Import the manifest `json` into a new project.
    fn import(json: &str) -> (Rproj, ImportReport) {
        let mut rproj = Rproj::minimal("app");
        let report = apply_records(&mut rproj, &records(json).unwrap(), true, &OPTIONS).unwrap();
        (rproj, report)
    }

    /// Import the fixture manifest `<name>.json`, and compare the project
    /// with `<name>.toml`. `RIG_UPDATE_GOLDEN=1` writes the project instead.
    fn import_fixture(name: &str) -> (Rproj, ImportReport) {
        let json = fs::read_to_string(fixture_path(&format!("{}.json", name))).unwrap();
        let (rproj, report) = import(&json);
        let actual = toml::to_string_pretty(&rproj).unwrap();
        let golden = fixture_path(&format!("{}.toml", name));
        if std::env::var_os("RIG_UPDATE_GOLDEN").is_some() {
            fs::write(&golden, &actual).unwrap();
        }
        let expected = fs::read_to_string(&golden).unwrap();
        assert_eq!(actual, expected, "{} differs from {}", name, golden);
        (rproj, report)
    }

    fn locked(report: &ImportReport) -> Vec<(&str, &str)> {
        report
            .locked_versions
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect()
    }

    fn error_of(json: &str) -> String {
        records(json).unwrap_err().to_string()
    }

    #[test]
    fn cran_packages_lock_their_versions() {
        let (_, report) = import_fixture("cran");
        assert_eq!(
            locked(&report),
            [
                ("Matrix", "= 1.7-0"),
                ("R6", "= 2.5.1"),
                ("lattice", "= 0.22-6"),
                ("rowr", "= 1.1.3"),
            ]
        );
        assert!(report.unresolved.is_empty());
    }

    #[test]
    fn bioconductor_release_comes_from_the_urls() {
        let json = fs::read_to_string(fixture_path("bioc.json")).unwrap();
        let records = records(&json).unwrap();
        assert_eq!(records.bioc_version.as_deref(), Some("3.20"));
        import_fixture("bioc");
    }

    #[test]
    fn private_repositories_are_pinned() {
        import_fixture("custom-repo");
        import_fixture("ppm");
    }

    #[test]
    fn git_packages_lock_their_commits() {
        let (_, report) = import_fixture("git");
        assert!(report.locked_versions.is_empty());
        let mut shas: Vec<_> = report
            .locked_git_shas
            .iter()
            .map(|((url, refspec, subdir), sha)| {
                (
                    url.as_str(),
                    refspec.as_deref(),
                    subdir.as_deref(),
                    sha.as_str(),
                )
            })
            .collect();
        shas.sort();
        assert_eq!(
            shas,
            [
                (
                    "https://bitbucket.org/quanttools/quanttools.git",
                    Some("master"),
                    None,
                    "3104c6d7108295d2c17b7ab712b62602bf867904"
                ),
                (
                    "https://github.com/acme-testing/adder.git",
                    None,
                    None,
                    "8f1248149d636f7427fc07a44cb750e8b49d741b"
                ),
                (
                    "https://github.com/acme-testing/sub_adder.git",
                    None,
                    Some("subadder"),
                    "7d7f31bda54403a7aad80a4c9a5a0b1c44e2625e"
                ),
                (
                    "https://gitlab.com/acme-testing/glab.git",
                    Some("v1.0.0"),
                    None,
                    "1111111111111111111111111111111111111111"
                ),
            ]
        );
    }

    #[test]
    fn r_version_comes_from_the_platform() {
        let manifest = parse(r#"{"platform": "4.3.2", "packages": {}}"#);
        assert_eq!(manifest.r_version.as_deref(), Some("4.3.2"));
        let manifest = parse(r#"{"packages": {}}"#);
        assert_eq!(manifest.r_version, None);
    }

    #[test]
    fn r_version_option_wins_over_the_platform() {
        let manifest = parse(r#"{"platform": "4.4.1", "packages": {}}"#);
        let option = "4.3.3".to_string();
        assert_eq!(manifest.r_version_for(Some(&option), "m").unwrap(), "4.3.3");
        assert_eq!(manifest.r_version_for(None, "m").unwrap(), "4.4.1");
        let records = manifest.records("4.3.3", &test_detect).unwrap();
        let mut rproj = Rproj::minimal("app");
        apply_records(&mut rproj, &records, true, &OPTIONS).unwrap();
        assert_eq!(
            rproj.dependencies.get("R"),
            Some(&crate::rproj::Dependency::Version(">= 4.3".to_string()))
        );

        let manifest = parse(r#"{"packages": {}}"#);
        assert!(manifest.r_version_for(None, "m").is_err());
    }

    #[test]
    fn base_packages_are_left_out() {
        let records = records(
            r#"{"packages": {
                "grid": {"Source": "CRAN", "description": {"Version": "4.4.3"}},
                "R6": {"Source": "CRAN", "description": {"Version": "2.5.1", "Imports": "grid"}}
            }}"#,
        )
        .unwrap();
        let names: Vec<&str> = records.packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["R6"]);
    }

    #[test]
    fn repository_urls_lose_their_index_path() {
        let records = records(
            r#"{"packages": {
                "a": {"Source": "CRAN", "Repository": "https://cran.example.com/src/contrib",
                      "description": {"Version": "1.0"}},
                "b": {"Source": "CRAN",
                      "Repository": "https://cran.example.com/bin/macosx/big-sur-arm64/contrib/4.4",
                      "description": {"Version": "1.0"}},
                "c": {"Source": "internal", "Repository": "https://pkgs.example.com/prod/",
                      "description": {"Version": "1.0"}}
            }}"#,
        )
        .unwrap();
        let repos: Vec<(&str, &str)> = records
            .repositories
            .iter()
            .map(|r| (r.name.as_str(), r.url.as_str()))
            .collect();
        assert_eq!(
            repos,
            [
                ("CRAN", "https://cran.example.com"),
                ("internal", "https://pkgs.example.com/prod"),
            ]
        );
        let b = records.packages.iter().find(|p| p.name == "b").unwrap();
        assert_eq!(
            b.source,
            PackageSource::Repository(Some(RepoReference {
                repository: "https://cran.example.com".to_string(),
                name: Some("CRAN".to_string()),
            }))
        );
    }

    #[test]
    fn uninstallable_sources_fail() {
        let msg = error_of(
            r#"{"packages": {
                "a": {"Source": "local", "description": {"Version": "1.0"}},
                "b": {"description": {"Version": "1.0"}},
                "c": {"Source": "github", "description": {"Version": "1.0",
                      "RemoteUsername": "acme", "RemoteRepo": "c"}},
                "d": {"Source": "git", "description": {"Version": "1.0",
                      "RemoteSha": "1111111111111111111111111111111111111111"}}
            }}"#,
        );
        assert!(msg.contains("a has Source `local`"), "{}", msg);
        assert!(msg.contains("b has no Source"), "{}", msg);
        assert!(
            msg.contains("c is from github, but the manifest has no commit"),
            "{}",
            msg
        );
        assert!(
            msg.contains("d is from git, but the manifest does not say where"),
            "{}",
            msg
        );
    }

    #[test]
    fn missing_dependencies_fail() {
        let msg = error_of(
            r#"{"packages": {
                "a": {"Source": "CRAN", "description": {"Version": "1.0",
                      "Imports": "b (>= 1.0),\n  methods", "LinkingTo": "cpp11"}}
            }}"#,
        );
        assert!(
            msg.contains("b (needed by a); cpp11 (needed by a)"),
            "{}",
            msg
        );
    }

    #[test]
    fn mixed_bioconductor_releases_fail() {
        let msg = error_of(
            r#"{"packages": {
                "a": {"Source": "Bioconductor",
                      "Repository": "https://bioconductor.org/packages/3.19/bioc/src/contrib",
                      "description": {"Version": "1.0"}},
                "b": {"Source": "Bioconductor",
                      "Repository": "https://bioconductor.org/packages/3.20/bioc/src/contrib",
                      "description": {"Version": "1.0"}}
            }}"#,
        );
        assert!(msg.contains("3.19, 3.20"), "{}", msg);
    }

    /// The error of the strict import of the manifest `json`, if any.
    fn strict_error(json: &str) -> Option<String> {
        let (_, report) = import(json);
        strict_import_error(&report, "manifest.json")
    }

    #[test]
    fn packages_from_an_unknown_repository_fail() {
        let msg = strict_error(
            r#"{"packages": {
                "a": {"Source": "internal", "description": {"Version": "1.0"}},
                "b": {"Source": "files", "Repository": "file:///srv/cran",
                      "description": {"Version": "1.0"}}
            }}"#,
        )
        .unwrap();
        assert!(
            msg.contains("has no http:// or https:// URL for: a, b"),
            "{}",
            msg
        );
    }

    #[test]
    fn packages_from_a_known_repository_need_no_url() {
        let json = r#"{"packages": {
            "a": {"Source": "internal", "description": {"Version": "1.0"}},
            "b": {"Source": "internal", "Repository": "https://pkgs.example.com/prod",
                  "description": {"Version": "1.0"}},
            "c": {"Source": "CRAN", "description": {"Version": "1.0"}},
            "d": {"Source": "Bioconductor", "description": {"Version": "1.0"}}
        }}"#;
        assert_eq!(strict_error(json), None);
        let (rproj, _) = import(json);
        assert_eq!(
            rproj
                .repository_pins()
                .unwrap()
                .get("a")
                .map(String::as_str),
            Some("internal")
        );
    }

    #[test]
    fn fixtures_import_strictly() {
        for name in ["bioc", "cran", "custom-repo", "git", "ppm"] {
            let json = fs::read_to_string(fixture_path(&format!("{}.json", name))).unwrap();
            assert_eq!(strict_error(&json), None, "{}", name);
        }
    }

    #[test]
    fn package_without_version_fails() {
        let msg = error_of(r#"{"packages": {"a": {"Source": "CRAN", "description": {}}}}"#);
        assert!(msg.contains("a has no Version"), "{}", msg);
    }
}
