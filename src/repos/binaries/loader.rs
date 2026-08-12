//! Adapter from the per-package binary index to the dependency solver.
//!
//! The index lists every artifact of every version, for every target P3M has
//! ever built for. A solve is for exactly one target, so this narrows an index
//! down to that target and translates what is left into the solver's own types:
//! one [`BinaryArtifact`] per usable build, plus the source tarball URLs, which
//! are worth keeping because they are snapshot-pinned and the CRAN URLs we would
//! otherwise construct are guesses.

use std::error::Error;

use log::*;

use crate::dcf::RPackageVersion;
use crate::repos::binaries::{load_binary_index, BinaryIndex, PpmStatus};
use crate::repos::cranlike_metadata::minor_r_version;
use crate::rversion::OsVersion;
use crate::solver::{BinaryArtifact, BinaryIndexLoader, PackageArtifacts};

/// The build target a solve resolves binaries for, in the index's vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryTarget {
    /// P3M platform name: `macos`, `windows`, or a Linux codename such as
    /// `jammy`.
    pub platform: String,
    /// `x86_64` or `arm64`.
    pub arch: String,
    /// Minor R version, e.g. `4.5`.
    pub r_version: String,
}

impl BinaryTarget {
    /// The target for a rig platform and R version, or `None` when P3M builds
    /// nothing usable for it (an unknown distro, or one that is x86_64-only on an
    /// arm64 machine).
    ///
    /// Needs P3M's status document, so it can fail without a network and a cold
    /// cache. That is the caller's decision to make: solving from source is
    /// always still possible.
    pub fn detect(
        platform: &OsVersion,
        r_version: &str,
    ) -> Result<Option<BinaryTarget>, Box<dyn Error>> {
        let status = PpmStatus::load(None)?;
        let r_version = minor_r_version(r_version)?;
        Ok(status
            .ppm_platform(platform)
            .map(|(platform, arch)| BinaryTarget {
                platform,
                arch,
                r_version,
            }))
    }

    /// How the target is spelled in a lockfile, e.g. `macos-arm64`.
    pub fn name(&self) -> String {
        format!("{}-{}", self.platform, self.arch)
    }
}

/// A [`BinaryIndexLoader`] backed by the P3M per-package indices.
///
/// One HTTP request per package, cached for a day, made lazily as the solver
/// visits packages.
pub struct P3mBinaryLoader {
    target: BinaryTarget,
}

impl P3mBinaryLoader {
    pub fn new(target: BinaryTarget) -> Self {
        P3mBinaryLoader { target }
    }
}

impl BinaryIndexLoader for P3mBinaryLoader {
    fn load_artifacts(&self, package: &str) -> Result<PackageArtifacts, Box<dyn Error>> {
        match load_binary_index(package, None)? {
            None => Ok(PackageArtifacts::default()),
            Some(cached) => Ok(artifacts_for_target(&cached.index, &self.target)),
        }
    }

    fn target_name(&self) -> String {
        self.target.name()
    }
}

/// Narrow an index to one target.
///
/// Split out from the loader so it can be exercised against the fixtures without
/// touching the network.
///
/// Rows are dropped rather than reported when they cannot be used: a version or
/// a `linkingto` version that does not parse as an [`RPackageVersion`] cannot be
/// compared with what the source metadata says, and a build we cannot pin
/// correctly is worse than no build at all.
pub fn artifacts_for_target(index: &BinaryIndex, target: &BinaryTarget) -> PackageArtifacts {
    let mut out = PackageArtifacts::default();
    for version in index.versions() {
        let parsed = match RPackageVersion::from_str(version) {
            Ok(v) => v,
            Err(_) => {
                debug!(
                    "Skipping unparseable version '{}' of '{}' in binary index",
                    version,
                    index.package()
                );
                continue;
            }
        };
        for row in index.rows_for_version(version) {
            if row.is_source() {
                out.source_urls
                    .entry(parsed.clone())
                    .or_insert_with(|| row.url().to_string());
                continue;
            }
            if row.platform() != target.platform
                || row.arch() != target.arch
                || row.r_version() != target.r_version
            {
                continue;
            }
            let mut linkingto = Vec::new();
            let mut ok = true;
            for lt in row.linkingto() {
                match RPackageVersion::from_str(lt.version) {
                    Ok(v) => linkingto.push((lt.package.to_string(), v)),
                    Err(_) => {
                        debug!(
                            "Skipping binary {} {} (row {}): unparseable LinkingTo version \
                            '{} {}'",
                            index.package(),
                            version,
                            row.row_index(),
                            lt.package,
                            lt.version
                        );
                        ok = false;
                        break;
                    }
                }
            }
            if !ok {
                continue;
            }
            out.binaries.push(BinaryArtifact {
                version: parsed.clone(),
                row: row.row_index() as u32,
                url: row.url().to_string(),
                linkingto,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn fixture_index(name: &str) -> BinaryIndex {
        let path = PathBuf::from("tests/fixtures/binaries").join(name);
        let rows = crate::repos::binaries::parse_binaries_tsv(&fs::read(path).unwrap()).unwrap();
        // `BinaryRow` has no package name, so take it from the file name.
        let package = name.split('.').next().unwrap();
        let blob = crate::repos::binaries::blob::build(package, &rows).unwrap();
        BinaryIndex::open_blob(&blob).unwrap()
    }

    fn target(platform: &str, arch: &str, r_version: &str) -> BinaryTarget {
        BinaryTarget {
            platform: platform.to_string(),
            arch: arch.to_string(),
            r_version: r_version.to_string(),
        }
    }

    #[test]
    fn source_urls_come_from_the_index() {
        let index = fixture_index("pak.tsv.zst");
        let artifacts = artifacts_for_target(&index, &target("macos", "arm64", "4.5"));
        let v = RPackageVersion::from_str("0.9.0").unwrap();
        assert!(artifacts.source_urls[&v].ends_with("/src/contrib/pak_0.9.0.tar.gz"));
        // Source rows are collected whatever the target is.
        let other = artifacts_for_target(&index, &target("nosuchdistro", "x86_64", "4.5"));
        assert_eq!(other.source_urls.len(), artifacts.source_urls.len());
        assert!(other.binaries.is_empty());
    }

    #[test]
    fn only_the_target_s_binaries_are_offered() {
        let index = fixture_index("pak.tsv.zst");
        let artifacts = artifacts_for_target(&index, &target("macos", "arm64", "4.5"));
        assert!(!artifacts.binaries.is_empty());
        for bin in artifacts.binaries.iter() {
            let row = index
                .rows_for_version(&bin.version.original)
                .find(|r| r.row_index() as u32 == bin.row)
                .unwrap();
            assert_eq!(row.platform(), "macos");
            assert_eq!(row.arch(), "arm64");
            assert_eq!(row.r_version(), "4.5");
            assert_eq!(row.url(), bin.url);
        }
    }

    #[test]
    fn several_builds_of_one_version_differ_by_linkingto() {
        // dplyr 0.7.4 on xenial/R 3.4 has several builds that differ only in the
        // plogr version they were compiled against.
        let index = fixture_index("dplyr.tsv.zst");
        let artifacts = artifacts_for_target(&index, &target("xenial", "x86_64", "3.4"));
        let mut builds: Vec<&BinaryArtifact> = artifacts
            .binaries
            .iter()
            .filter(|b| b.version.original == "0.7.4")
            .collect();
        builds.sort_by_key(|b| b.row);
        assert!(
            builds.len() > 1,
            "expected several 0.7.4 builds, got {}",
            builds.len()
        );
        // Each one pins its own LinkingTo versions, and they are not all the same.
        let plogr: Vec<String> = builds
            .iter()
            .filter_map(|b| {
                b.linkingto
                    .iter()
                    .find(|(p, _)| p == "plogr")
                    .map(|(_, v)| v.original.clone())
            })
            .collect();
        assert!(plogr.len() > 1);
        assert!(
            plogr.iter().any(|v| v != &plogr[0]),
            "expected differing plogr versions, got {:?}",
            plogr
        );
    }
}
