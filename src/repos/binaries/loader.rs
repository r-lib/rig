//! Adapter from the per-package binary index to the dependency solver.
//!
//! The index lists every artifact of every version, for every target P3M has
//! ever built for. A solve is for exactly one target, so this narrows an index
//! down to that target and translates what is left into the solver's own types:
//! one [`BinaryArtifact`] per usable build, plus the source tarball URLs, which
//! are worth keeping because they are snapshot-pinned and the CRAN URLs we would
//! otherwise construct are guesses.

use std::collections::HashSet;
use std::error::Error;

use log::*;

use crate::dcf::RPackageVersion;
use crate::platform::platform_to_pkg_type;
use crate::repos::binaries::{
    load_binary_index_in, prefetch_binary_indices_in, BinaryIndex, PpmStatus,
};
use crate::repos::cranlike_metadata::{
    cranlike_index_rows, cranlike_key, ensure_cranlike_index, feed_package_names, minor_r_version,
    open_metadata_db, package_type_to_path,
};
use crate::repos::feed::{CranlikeRepo, MetadataFeed, RepoId};
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
    /// The canonical rig platform string of the target, e.g.
    /// `aarch64-apple-darwin`, see [`crate::platform::parse_platform_string`].
    pub triple: String,
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
        Ok(status.ppm_target(platform).map(|t| BinaryTarget {
            platform: t.platform,
            arch: t.arch,
            r_version,
            triple: t.triple,
        }))
    }

    /// How the target is spelled in a lockfile and in messages: its
    /// canonical platform string, e.g. `aarch64-apple-darwin`.
    pub fn name(&self) -> String {
        self.triple.clone()
    }

    /// The OS family of the target, to filter packages by their `OS_type`.
    pub fn os_type(&self) -> crate::dcf::OsType {
        crate::dcf::OsType::from_rig_os(&self.platform)
    }
}

/// A [`BinaryIndexLoader`] backed by the P3M per-package indices of one or
/// more feeds: CRAN's, and possibly a Bioconductor release's. If several feeds
/// have the same version of a package, the first feed's source URL wins, the
/// same way the source metadata picks the first repository's version.
///
/// One HTTP request per package and feed, cached for a day, made lazily as the
/// solver visits packages. A Bioconductor index is only requested for the
/// packages that release has, so CRAN packages cost no extra requests.
pub struct P3mBinaryLoader {
    target: BinaryTarget,
    feeds: Vec<(MetadataFeed, Option<HashSet<String>>)>,
}

impl P3mBinaryLoader {
    /// A loader for the indices of `feeds`, whose metadata must already be
    /// in the cache, see [`crate::repos::cranlike_metadata::ensure_feeds_fresh`].
    pub fn new_for(target: BinaryTarget, feeds: &[MetadataFeed]) -> Self {
        let feeds = feeds
            .iter()
            .map(|feed| {
                let names = if feed.repo.is_bioc() {
                    // Without the names, no Bioconductor index is fetched,
                    // and the release's packages solve from source.
                    Some(feed_package_names(feed).unwrap_or_else(|e| {
                        debug!("Cannot list the packages of {}: {}", feed.repo, e);
                        HashSet::new()
                    }))
                } else {
                    None
                };
                (feed.clone(), names)
            })
            .collect();
        P3mBinaryLoader { target, feeds }
    }

    fn has(names: &Option<HashSet<String>>, package: &str) -> bool {
        names.as_ref().is_none_or(|n| n.contains(package))
    }
}

impl BinaryIndexLoader for P3mBinaryLoader {
    fn load_artifacts(&self, package: &str) -> Result<PackageArtifacts, Box<dyn Error>> {
        let mut out = PackageArtifacts::default();
        for (feed, names) in &self.feeds {
            if !Self::has(names, package) {
                continue;
            }
            let Some(cached) = load_binary_index_in(feed, package, None)? else {
                continue;
            };
            let mut artifacts = artifacts_for_target(&cached.index, &self.target);
            for bin in &mut artifacts.binaries {
                bin.repository = Some(feed.repo.clone());
            }
            out.merge(artifacts);
        }
        Ok(out)
    }

    fn target_name(&self) -> String {
        self.target.name()
    }

    fn prefetch(&self, packages: &[String]) {
        for (feed, names) in &self.feeds {
            let packages: Vec<String> = packages
                .iter()
                .filter(|p| Self::has(names, p))
                .cloned()
                .collect();
            prefetch_binary_indices_in(feed, &packages, None);
        }
    }
}

impl PackageArtifacts {
    /// Add the artifacts of a lower priority source: all of its binaries,
    /// but only the source URLs and hashes of versions we do not have yet.
    fn merge(&mut self, mut other: PackageArtifacts) {
        self.binaries.append(&mut other.binaries);
        for (ver, url) in other.source_urls {
            self.source_urls.entry(ver).or_insert(url);
        }
        for (ver, sha) in other.source_sha256 {
            self.source_sha256.entry(ver).or_insert(sha);
        }
    }
}

/// The binary package type, its path in a CRAN-like repository, and the file
/// extension of its packages, for a build target. Only macOS and Windows
/// x86_64 have a standard binary layout, everything else is source only.
pub fn cranlike_binary_layout(target: &BinaryTarget) -> Option<(String, String, &'static str)> {
    let (pkg_type, ext) = match target.platform.as_str() {
        "windows" if target.arch == "x86_64" => ("win.binary".to_string(), ".zip"),
        "macos" => {
            let platform = OsVersion {
                rig_platform: None,
                arch: if target.arch == "arm64" {
                    "aarch64".to_string()
                } else {
                    target.arch.clone()
                },
                vendor: "apple".to_string(),
                os: "darwin".to_string(),
                distro: None,
                version: None,
            };
            let rver = format!("{}.0", target.r_version);
            (platform_to_pkg_type(&platform, &rver)?, ".tgz")
        }
        _ => return None,
    };
    let path = package_type_to_path(&pkg_type, &target.r_version).ok()?;
    Some((pkg_type, path, ext))
}

/// A [`BinaryIndexLoader`] backed by the binary `PACKAGES` indices of
/// CRAN-like repositories.
///
/// The indices are downloaded when the loader is created, one per
/// repository, and then queried per package from the metadata database. A
/// repository without binaries for the target simply has none.
pub struct CranlikeBinaryLoader {
    target: BinaryTarget,
    pkg_type: String,
    /// The repositories, with the database keys of their binary and source
    /// indices.
    repos: Vec<(RepoId, String, String)>,
    conn: Option<rusqlite::Connection>,
}

impl CranlikeBinaryLoader {
    pub fn new_for(target: BinaryTarget, repos: &[CranlikeRepo]) -> Self {
        let mut loader = CranlikeBinaryLoader {
            target,
            pkg_type: String::new(),
            repos: vec![],
            conn: None,
        };
        let Some((pkg_type, path, ext)) = cranlike_binary_layout(&loader.target) else {
            return loader;
        };
        for repo in repos {
            let r_version = loader.target.r_version.clone();
            if let Err(e) = ensure_cranlike_index(repo, &path, &pkg_type, Some(&r_version), ext) {
                debug!("Cannot load binary packages of {}: {}", repo.url, e);
                continue;
            }
            loader.repos.push((
                repo.repo_id(),
                cranlike_key(repo, &path),
                cranlike_key(repo, "src/contrib"),
            ));
        }
        loader.pkg_type = pkg_type;
        loader.conn = open_metadata_db()
            .map_err(|e| debug!("Cannot open the package metadata database: {}", e))
            .ok();
        loader
    }
}

impl BinaryIndexLoader for CranlikeBinaryLoader {
    fn load_artifacts(&self, package: &str) -> Result<PackageArtifacts, Box<dyn Error>> {
        let mut out = PackageArtifacts::default();
        let Some(conn) = &self.conn else {
            return Ok(out);
        };
        for (idx, (repo, key, src_key)) in self.repos.iter().enumerate() {
            let rows = cranlike_index_rows(conn, key, &self.pkg_type, package)?;
            if rows.is_empty() {
                continue;
            }
            // Like P3M, a binary carries the hash of the source package it
            // was built from, if the repository has it.
            let sources = cranlike_index_rows(conn, src_key, "source", package)?;
            for row in rows {
                let Some(url) = row.download_url else {
                    continue;
                };
                let sha256 = sources
                    .iter()
                    .find(|s| s.version == row.version)
                    .and_then(|s| s.checksum.clone())
                    .or(row.checksum)
                    .unwrap_or_default();
                out.binaries.push(BinaryArtifact {
                    version: row.version,
                    // The row of a P3M index is small, so this does not
                    // clash with the builds of the same version there.
                    row: u32::MAX - idx as u32,
                    url,
                    // Empty if the repository has no checksums: then an
                    // installed package of the same version is up to date.
                    sha256,
                    linkingto: vec![],
                    repository: Some(repo.clone()),
                    built: row.built,
                });
            }
        }
        Ok(out)
    }

    fn target_name(&self) -> String {
        self.target.name()
    }
}

/// Several [`BinaryIndexLoader`]s for the same target, in order of
/// precedence: their binaries together, and the source URLs and hashes of
/// the first loader that has them.
pub struct ChainedBinaryLoader {
    loaders: Vec<Box<dyn BinaryIndexLoader>>,
}

impl ChainedBinaryLoader {
    pub fn new(loaders: Vec<Box<dyn BinaryIndexLoader>>) -> Self {
        ChainedBinaryLoader { loaders }
    }
}

impl BinaryIndexLoader for ChainedBinaryLoader {
    fn load_artifacts(&self, package: &str) -> Result<PackageArtifacts, Box<dyn Error>> {
        let mut out = PackageArtifacts::default();
        for loader in &self.loaders {
            match loader.load_artifacts(package) {
                Ok(artifacts) => out.merge(artifacts),
                Err(e) => debug!("Failed to load binary artifacts for '{}': {}", package, e),
            }
        }
        Ok(out)
    }

    fn target_name(&self) -> String {
        self.loaders
            .first()
            .map(|l| l.target_name())
            .unwrap_or_default()
    }

    fn prefetch(&self, packages: &[String]) {
        for loader in &self.loaders {
            loader.prefetch(packages);
        }
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
                out.source_sha256
                    .entry(parsed.clone())
                    .or_insert_with(|| row.sha256().to_string());
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
                    Ok(v) => linkingto.push((lt.package.to_string(), v, lt.sha256.to_string())),
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
                sha256: row.sha256().to_string(),
                linkingto,
                repository: None,
                built: None,
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
            triple: format!("{}-{}", platform, arch),
        }
    }

    #[test]
    fn cranlike_binary_layouts() {
        let layout = |p: &str, a: &str, r: &str| cranlike_binary_layout(&target(p, a, r));
        assert_eq!(
            layout("macos", "arm64", "4.5"),
            Some((
                "mac.binary.big-sur-arm64".to_string(),
                "bin/macosx/big-sur-arm64/contrib/4.5".to_string(),
                ".tgz"
            ))
        );
        assert_eq!(
            layout("macos", "x86_64", "4.5").unwrap().1,
            "bin/macosx/big-sur-x86_64/contrib/4.5"
        );
        assert_eq!(
            layout("macos", "arm64", "4.6").unwrap().1,
            "bin/macosx/sonoma-arm64/contrib/4.6"
        );
        assert_eq!(
            layout("windows", "x86_64", "4.5"),
            Some((
                "win.binary".to_string(),
                "bin/windows/contrib/4.5".to_string(),
                ".zip"
            ))
        );
        assert_eq!(layout("windows", "arm64", "4.5"), None);
        assert_eq!(layout("jammy", "x86_64", "4.5"), None);
    }

    struct FixedLoader(Vec<(&'static str, &'static str, &'static str)>);

    impl BinaryIndexLoader for FixedLoader {
        fn load_artifacts(&self, _package: &str) -> Result<PackageArtifacts, Box<dyn Error>> {
            let mut out = PackageArtifacts::default();
            for (ver, url, sha) in &self.0 {
                let v = RPackageVersion::from_str(ver).unwrap();
                out.source_urls.insert(v.clone(), url.to_string());
                out.source_sha256.insert(v, sha.to_string());
            }
            Ok(out)
        }
        fn target_name(&self) -> String {
            "testos-x86_64".to_string()
        }
    }

    #[test]
    fn chained_loader_prefers_the_first_loader() {
        let chained = ChainedBinaryLoader::new(vec![
            Box::new(FixedLoader(vec![("1.0", "first", "a")])),
            Box::new(FixedLoader(vec![
                ("1.0", "second", "b"),
                ("2.0", "second", "c"),
            ])),
        ]);
        let artifacts = chained.load_artifacts("pkg").unwrap();
        let v1 = RPackageVersion::from_str("1.0").unwrap();
        let v2 = RPackageVersion::from_str("2.0").unwrap();
        assert_eq!(artifacts.source_urls[&v1], "first");
        assert_eq!(artifacts.source_sha256[&v1], "a");
        assert_eq!(artifacts.source_urls[&v2], "second");
        assert_eq!(chained.target_name(), "testos-x86_64");
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
                    .find(|(p, _, _)| p == "plogr")
                    .map(|(_, v, _)| v.original.clone())
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
