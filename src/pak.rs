use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::install::{format_linkingto, REMOTE_HASH_FIELD, REMOTE_LINKINGTO_FIELD};
use crate::proj::BASE_PKGS;
use crate::solver::*;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all(serialize = "snake_case"))]
pub struct PakLockfilePackage {
    pub r#ref: String,
    pub package: String,
    pub version: String,
    pub r#type: String,
    pub direct: bool,
    pub binary: bool,
    pub dependencies: Vec<String>,
    pub vignettes: bool,
    pub metadata: HashMap<String, String>,
    pub sources: Vec<String>,
    pub target: String,
    pub platform: String,
    pub rversion: String,
    pub directpkg: bool,
    pub license: String,
    pub dep_types: Vec<String>,
    pub params: Vec<String>,
    pub install_args: String,
    pub sysreqs: String,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all(serialize = "snake_case"))]
pub struct PakLockfile {
    pub lockfile_version: usize,
    pub os: String,
    pub r_version: String,
    pub platform: String,
    pub packages: Vec<PakLockfilePackage>,
}

/// Archive suffixes a repository serves packages as. Matched whole, because a
/// package version contains dots (`pak_0.9.5.tgz`), so neither the first nor
/// the last `.` of a file name marks where its extension starts.
const ARCHIVE_SUFFIXES: [&str; 4] = [".tar.gz", ".tgz", ".zip", ".tar.bz2"];

/// Short identity of one artifact, for the cache file name.
///
/// `sha256` is the upstream CRAN source hash, which is the same on *every*
/// build of a version, so it identifies a version rather than a build; what
/// tells two builds of one version apart is `linkingto`, the dependency
/// versions the binary was compiled against. Both together identify the
/// artifact, and neither the P3M snapshot date nor the repository URL is part
/// of it, so the same build served at several snapshot dates shares one cache
/// entry.
///
/// Pass `linkingto` for a *binary* only. A source artifact also has a
/// `LinkingTo` provenance, but it describes what the tarball will be compiled
/// against later, not what the file is, and keying on it would cache a
/// separate copy of one tarball per solve.
fn artifact_cache_key(sha256: Option<&str>, linkingto: Option<&str>) -> Option<String> {
    let sha256 = sha256?;
    let hash = crate::utils::calculate_hash(&format!("{}\n{}", sha256, linkingto.unwrap_or("")));
    Some(hash[..8].to_string())
}

/// `name` with `key` inserted before its archive suffix.
fn keyed_file_name(name: &str, key: &str) -> String {
    match ARCHIVE_SUFFIXES.iter().find(|s| name.ends_with(**s)) {
        Some(suffix) => format!("{}-{}{}", &name[..name.len() - suffix.len()], key, suffix),
        None => format!("{}-{}", name, key),
    }
}

/// Where a downloaded artifact is put, relative to the download directory.
///
/// P3M URLs carry the repository layout the file belongs to
/// (`.../bin/macosx/big-sur-arm64/contrib/4.5/pak_0.9.5.tgz`), and the path
/// from the `src/` or `bin/` component onwards is the basis for the cache path.
/// Anything we cannot read that way falls back to `fallback`, a bare file name.
///
/// Two adjustments to that path:
///
/// * The `contrib` components are CRAN repository boilerplate and carry no
///   information, as does the second `src` that a Linux binary URL has after
///   the R version, so everything after the leading `src`/`bin` that is one of
///   those is dropped. What is left is what actually distinguishes targets:
///   OS, arch, R version.
/// * `key` goes into the file name, because the repository path is *not* unique
///   on its own: several binary builds share one `(version, platform, arch,
///   r_version)` and therefore one URL path, differing only in the snapshot
///   date that this path drops. See [`artifact_cache_key`].
fn target_path(url: &str, fallback: &str, key: Option<&str>) -> String {
    let path = url.split('?').next().unwrap_or(url);
    let mut pieces = path.split('/');
    let mut rest: Vec<&str> = vec![];
    for piece in pieces.by_ref() {
        if piece == "src" || piece == "bin" {
            rest.push(piece);
            break;
        }
    }
    if rest.is_empty() {
        return match key {
            Some(key) => keyed_file_name(fallback, key),
            None => fallback.to_string(),
        };
    }
    rest.extend(pieces.filter(|p| *p != "contrib" && *p != "src"));
    let keyed = match (key, rest.last()) {
        (Some(key), Some(file)) => Some(keyed_file_name(file, key)),
        _ => None,
    };
    if let Some(keyed) = &keyed {
        *rest.last_mut().unwrap() = keyed;
    }
    rest.join("/")
}

impl PakLockfile {
    pub fn from_solution(
        registry: &RPackageRegistry,
        solution: &HashMap<String, RegistryPackageVersion, rustc_hash::FxBuildHasher>,
    ) -> PakLockfile {
        let r_version = solution
            .get("R")
            .map(|r| r.version.to_string())
            .unwrap_or_default();
        let platform = registry.binary_target();
        let mut pkgs = vec![];
        for (k, v) in solution.iter() {
            if k == "R" || k == "_project" || BASE_PKGS.contains(&k.as_str()) {
                continue;
            }
            let deps = registry
                .get_dependency_summary(k, v)
                .unwrap()
                .into_iter()
                .filter(|dep| dep != "R" && !BASE_PKGS.contains(&dep.as_str()))
                .collect();
            let binary = v.artifact.is_binary();
            // Provenance of the artifact, so that a lockfile install records the
            // same `RemoteHash` / `RemoteLinkingToHashes` a direct install does.
            // A binary knows what it was compiled against; a source build is
            // compiled against whatever the solve picked, so its provenance is
            // read off the solution.
            let mut metadata: HashMap<String, String> = HashMap::new();
            if let Some(sha) = registry.artifact_sha256(k, v) {
                metadata.insert(REMOTE_HASH_FIELD.to_string(), sha);
            }
            let linkingto = if binary {
                registry.artifact_linkingto(k, v)
            } else {
                registry
                    .linkingto_names(k, v)
                    .into_iter()
                    .filter_map(|dep| {
                        let dv = solution.get(&dep)?;
                        let sha = registry.artifact_sha256(&dep, dv)?;
                        Some((dep, dv.version.to_string(), sha))
                    })
                    .collect()
            };
            if !linkingto.is_empty() {
                metadata.insert(
                    REMOTE_LINKINGTO_FIELD.to_string(),
                    format_linkingto(&linkingto),
                );
            }
            // The index's URL is snapshot-pinned; the CRAN ones are guesses, and
            // there are two of them because a version that has been superseded
            // has moved into the archive.
            let filename = format!("{}_{}.tar.gz", k, v.version);
            let sources = match registry.artifact_url(k, v) {
                Some(url) => vec![url],
                None => vec![
                    format!("https://cloud.r-project.org/src/contrib/{}", filename),
                    format!(
                        "https://cloud.r-project.org/src/contrib/Archive/{}/{}",
                        k, filename
                    ),
                ],
            };
            // The cache file name has to tell two builds of one version apart,
            // and the repository path does not: several binaries share it.
            let key = artifact_cache_key(
                metadata.get(REMOTE_HASH_FIELD).map(|s| s.as_str()),
                if binary {
                    metadata.get(REMOTE_LINKINGTO_FIELD).map(|s| s.as_str())
                } else {
                    None
                },
            );
            let target = target_path(&sources[0], &format!("src/{}", filename), key.as_deref());
            pkgs.push(PakLockfilePackage {
                r#ref: k.to_string(),
                package: k.to_string(),
                version: v.version.to_string(),
                r#type: "standard".to_string(),
                direct: false,
                binary,
                dependencies: deps,
                vignettes: false,
                metadata,
                sources,
                target,
                platform: if binary {
                    platform.clone().unwrap_or_else(|| "source".to_string())
                } else {
                    "source".to_string()
                },
                rversion: r_version.clone(),
                directpkg: false,
                license: "UNKNOWN".to_string(),
                dep_types: vec![],
                params: vec![],
                install_args: "".to_string(),
                sysreqs: "".to_string(),
            });
        }

        PakLockfile {
            lockfile_version: 1,
            os: std::env::consts::OS.to_string(),
            r_version,
            platform: platform.unwrap_or_else(|| std::env::consts::ARCH.to_string()),
            packages: pkgs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `linkingto` of two real `dplyr 0.7.4` xenial rows, which differ in
    /// nothing else: same version, platform, arch, R version and `sha256`.
    const DPLYR_SHA: &str = "7b1fc90750fbb46483423da6721832c545d37b157f4f3355784a65e50fada8c2";
    const DPLYR_PLOGR_01: &str = "BH@1.66.0-1=17d9eb5512d74aa7dd02ec98953408422e728b01ce63493a6a473070b9596a92,Rcpp@0.12.16=d4e1636e53e2b656e173b49085b7abbb627981787cd63d63df325c713c83a8e6,bindrcpp@0.2=d0efa1313cb8148880f7902a4267de1dcedae916f28d9a0ef5911f44bf103450,plogr@0.1-1=22755c93c76c26252841f43195df31681ea865e91aa89726010bd1b9288ef48f";
    const DPLYR_PLOGR_02: &str = "BH@1.66.0-1=17d9eb5512d74aa7dd02ec98953408422e728b01ce63493a6a473070b9596a92,Rcpp@0.12.16=d4e1636e53e2b656e173b49085b7abbb627981787cd63d63df325c713c83a8e6,bindrcpp@0.2=d0efa1313cb8148880f7902a4267de1dcedae916f28d9a0ef5911f44bf103450,plogr@0.2.0=0e63ba2e1f624005fe25c67cdd403636a912e063d682eca07f2f1d65e9870d29";

    #[test]
    fn target_path_follows_the_repository_layout() {
        // `contrib` is dropped: it says nothing about which build this is.
        assert_eq!(
            target_path(
                "https://p3m.dev/cran/2026-04-27/bin/macosx/big-sur-arm64/contrib/4.5/pak_0.9.5.tgz",
                "fallback.tgz",
                None
            ),
            "bin/macosx/big-sur-arm64/4.5/pak_0.9.5.tgz"
        );
        assert_eq!(
            target_path(
                "https://p3m.dev/cran/2026-04-27/bin/windows/contrib/4.5/pak_0.9.5.zip",
                "fallback.zip",
                None
            ),
            "bin/windows/4.5/pak_0.9.5.zip"
        );
        assert_eq!(
            target_path(
                "https://p3m.dev/cran/2026-04-27/src/contrib/pak_0.9.5.tar.gz",
                "fallback.tar.gz",
                None
            ),
            "src/pak_0.9.5.tar.gz"
        );
        // Linux binaries live under a second `src/contrib`, and the first
        // component we recognise is the right one; the second one goes away
        // along with the `contrib`.
        assert_eq!(
            target_path(
                "https://p3m.dev/cran/2026-04-27/bin/linux/jammy-x86_64/4.5/src/contrib/pak_0.9.5.tar.gz",
                "fallback.tar.gz",
                None
            ),
            "bin/linux/jammy-x86_64/4.5/pak_0.9.5.tar.gz"
        );
        assert_eq!(
            target_path("https://example.com/pak.tgz", "fallback.tgz", None),
            "fallback.tgz"
        );
    }

    #[test]
    fn target_path_puts_the_key_in_the_file_name() {
        assert_eq!(
            target_path(
                "https://p3m.dev/cran/2026-04-27/bin/macosx/big-sur-arm64/contrib/4.5/pak_0.9.5.tgz",
                "fallback.tgz",
                Some("3f9a1c2e")
            ),
            "bin/macosx/big-sur-arm64/4.5/pak_0.9.5-3f9a1c2e.tgz"
        );
        // The fallback is a file name, so it is keyed too.
        assert_eq!(
            target_path(
                "https://example.com/pak.tgz",
                "pak_0.9.5.tar.gz",
                Some("3f9a1c2e")
            ),
            "pak_0.9.5-3f9a1c2e.tar.gz"
        );
    }

    #[test]
    fn builds_of_one_version_get_different_targets() {
        let url =
            "https://p3m.dev/cran/2018-03-15/bin/linux/xenial-x86_64/3.4/src/contrib/dplyr_0.7.4.tar.gz";
        let one = target_path(
            url,
            "fallback.tar.gz",
            artifact_cache_key(Some(DPLYR_SHA), Some(DPLYR_PLOGR_01)).as_deref(),
        );
        let two = target_path(
            url,
            "fallback.tar.gz",
            artifact_cache_key(Some(DPLYR_SHA), Some(DPLYR_PLOGR_02)).as_deref(),
        );
        assert_ne!(one, two);
    }

    #[test]
    fn one_build_at_two_snapshots_gets_one_target() {
        let key = artifact_cache_key(Some(DPLYR_SHA), Some(DPLYR_PLOGR_01));
        assert_eq!(
            target_path(
                "https://p3m.dev/cran/2018-03-15/bin/linux/xenial-x86_64/3.4/src/contrib/dplyr_0.7.4.tar.gz",
                "fallback.tar.gz",
                key.as_deref()
            ),
            target_path(
                "https://p3m.dev/cran/2018-03-27/bin/linux/xenial-x86_64/3.4/src/contrib/dplyr_0.7.4.tar.gz",
                "fallback.tar.gz",
                key.as_deref()
            )
        );
    }

    #[test]
    fn a_source_artifact_is_keyed_on_its_hash_alone() {
        // `from_solution` passes no `linkingto` for a source artifact, so every
        // solve of one version reuses the one cached tarball.
        assert_eq!(
            artifact_cache_key(Some(DPLYR_SHA), None),
            artifact_cache_key(Some(DPLYR_SHA), None)
        );
        assert_ne!(
            artifact_cache_key(Some(DPLYR_SHA), None),
            artifact_cache_key(Some(DPLYR_SHA), Some(DPLYR_PLOGR_01))
        );
    }

    #[test]
    fn without_a_hash_there_is_no_key() {
        assert_eq!(artifact_cache_key(None, Some(DPLYR_PLOGR_01)), None);
    }

    #[test]
    fn keyed_file_name_keeps_the_archive_suffix() {
        assert_eq!(
            keyed_file_name("pak_0.9.5.tar.gz", "3f9a1c2e"),
            "pak_0.9.5-3f9a1c2e.tar.gz"
        );
        assert_eq!(
            keyed_file_name("pak_0.9.5.tgz", "3f9a1c2e"),
            "pak_0.9.5-3f9a1c2e.tgz"
        );
        assert_eq!(
            keyed_file_name("pak_0.9.5.zip", "3f9a1c2e"),
            "pak_0.9.5-3f9a1c2e.zip"
        );
        // Nothing recognisable to cut before: the key is appended.
        assert_eq!(keyed_file_name("pak", "3f9a1c2e"), "pak-3f9a1c2e");
    }
}
