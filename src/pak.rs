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

/// Where a downloaded artifact is put, relative to the download directory.
///
/// P3M URLs carry the repository layout the file belongs to
/// (`.../bin/macosx/big-sur-arm64/contrib/4.5/pak_0.9.5.tgz`), and everything
/// from the `src/` or `bin/` component onwards is exactly that path. Anything we
/// cannot read that way falls back to the bare file name, which is still unique
/// per package version.
fn target_path(url: &str, fallback: &str) -> String {
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
        return fallback.to_string();
    }
    rest.extend(pieces);
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
            let target = target_path(&sources[0], &format!("src/contrib/{}", filename));
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

    #[test]
    fn target_path_follows_the_repository_layout() {
        assert_eq!(
            target_path(
                "https://p3m.dev/cran/2026-04-27/bin/macosx/big-sur-arm64/contrib/4.5/pak_0.9.5.tgz",
                "fallback"
            ),
            "bin/macosx/big-sur-arm64/contrib/4.5/pak_0.9.5.tgz"
        );
        assert_eq!(
            target_path(
                "https://p3m.dev/cran/2026-04-27/src/contrib/pak_0.9.5.tar.gz",
                "fallback"
            ),
            "src/contrib/pak_0.9.5.tar.gz"
        );
        // Linux binaries live under a second `src/contrib`, and the first
        // component we recognise is the right one.
        assert_eq!(
            target_path(
                "https://p3m.dev/cran/2026-04-27/bin/linux/jammy-x86_64/4.5/src/contrib/pak_0.9.5.tar.gz",
                "fallback"
            ),
            "bin/linux/jammy-x86_64/4.5/src/contrib/pak_0.9.5.tar.gz"
        );
        assert_eq!(
            target_path("https://example.com/pak.tgz", "fallback"),
            "fallback"
        );
    }
}
