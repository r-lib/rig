use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet, VecDeque};
use std::error::Error;
use std::fmt;

use log::debug;
use pubgrub::*;
use serde::{Deserialize, Serialize};
use simple_error::bail;

use crate::dcf::*;

type RPackageName = String;

/// A source of package metadata that the registry can query lazily, one package
/// at a time, instead of preloading every version up front. Returns all known
/// versions of `package` (with their dependencies); an empty vector means the
/// package is unknown.
pub trait PackageVersionLoader {
    fn load_versions(&self, package: &str) -> Result<Vec<crate::dcf::Package>, Box<dyn Error>>;
}

/// Which artifact of a package version gets installed.
///
/// This is part of the solver's version type rather than a choice made after
/// solving, because a binary build is only usable if the `LinkingTo` dependency
/// versions it was compiled against are the ones the solve actually picks.
/// Expressing that as dependencies *of the artifact* is what lets pubgrub
/// backtrack to another build, or to the source tarball, instead of us silently
/// installing a binary whose ABI does not match its dependencies.
///
/// `LowerBound` and `UpperBound` are not artifacts anybody can install. They
/// exist so that a constraint on a *version* becomes a range that covers, or
/// excludes, all of that version's artifacts — see
/// [`rpackage_version_ranges_from_constraints`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Artifact {
    /// Sorts below every real artifact of the same version.
    LowerBound,
    /// The source tarball. Always available, so it is the fallback.
    Source,
    /// The `n`th row of the package's binary index. Binaries sort above the
    /// source tarball, so `choose_version` prefers them at equal versions, and
    /// later rows (newer P3M snapshots) above earlier ones.
    Binary(u32),
    /// Sorts above every real artifact of the same version.
    UpperBound,
}

impl Artifact {
    pub fn is_binary(&self) -> bool {
        matches!(self, Artifact::Binary(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RegistryPackageVersion {
    pub name: RPackageName,
    pub version: RPackageVersion,
    #[serde(default = "artifact_source")]
    pub artifact: Artifact,
}

fn artifact_source() -> Artifact {
    Artifact::Source
}

impl RegistryPackageVersion {
    pub fn new(name: &str, version_str: &str) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(RegistryPackageVersion {
            name: name.to_string(),
            version: RPackageVersion::from_str(version_str)?,
            artifact: Artifact::Source,
        })
    }

    /// A version used only as a range boundary, never as a candidate.
    fn bound(name: &str, version: &RPackageVersion, artifact: Artifact) -> Self {
        RegistryPackageVersion {
            name: name.to_string(),
            version: version.clone(),
            artifact,
        }
    }

    /// The range that covers every artifact of one version.
    fn artifacts_of(name: &str, version: &RPackageVersion) -> RPackageVersionRanges {
        // `between` is half-open, and `UpperBound` is not a candidate, so this is
        // every artifact of `version` and nothing else.
        RPackageVersionRanges::between(
            RegistryPackageVersion::bound(name, version, Artifact::LowerBound),
            RegistryPackageVersion::bound(name, version, Artifact::UpperBound),
        )
    }
}

impl Ord for RegistryPackageVersion {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // The name is deliberately not compared: pubgrub only ever orders
        // versions of one and the same package. The artifact is the least
        // significant key, so a newer version always beats an older one and a
        // binary only wins against the same version's source tarball.
        self.version
            .cmp(&other.version)
            .then_with(|| self.artifact.cmp(&other.artifact))
    }
}

impl PartialOrd for RegistryPackageVersion {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for RegistryPackageVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The artifact is shown because pubgrub's conflict reports are written in
        // terms of versions, and "1.1.4 (binary 3)" versus "1.1.4" is exactly the
        // distinction those reports need to explain a LinkingTo conflict.
        match self.artifact {
            Artifact::Binary(row) => write!(f, "{} (binary {})", self.version, row),
            _ => write!(f, "{}", self.version),
        }
    }
}

pub type RPackageVersionRanges = version_ranges::Ranges<RegistryPackageVersion>;

pub fn rpackage_version_ranges_from_constraints(
    constraints: &PackageDependencies,
    dev: bool,
) -> HashMap<RPackageName, RPackageVersionRanges, rustc_hash::FxBuildHasher> {
    let mut vranges = HashMap::with_hasher(rustc_hash::FxBuildHasher);
    for dep in constraints.dependencies.iter() {
        if !dev && dep.types.iter().all(|x| DEP_TYPES_SOFT.contains(x)) {
            // we ignore soft dependencies for now, as they are not required for installation
            continue;
        }
        let mut vs = RPackageVersionRanges::full();
        for cs in dep.constraints.iter() {
            // A DESCRIPTION constraint is on the version, and says nothing about
            // which artifact of it to use, so each bound has to fall outside the
            // whole run of that version's artifacts: `>= 1.2` includes 1.2's
            // source *and* binary rows, while `> 1.2` excludes all of them.
            let lo = RegistryPackageVersion::bound(&dep.name, &cs.version, Artifact::LowerBound);
            let hi = RegistryPackageVersion::bound(&dep.name, &cs.version, Artifact::UpperBound);
            match cs.constraint_type {
                VersionConstraintType::Less => {
                    vs = vs.intersection(&Range::strictly_lower_than(lo));
                }
                VersionConstraintType::LessOrEqual => {
                    vs = vs.intersection(&Range::strictly_lower_than(hi));
                }
                VersionConstraintType::Equal => {
                    vs = vs.intersection(&RegistryPackageVersion::artifacts_of(
                        &dep.name,
                        &cs.version,
                    ));
                }
                VersionConstraintType::Greater => {
                    vs = vs.intersection(&Range::higher_than(hi));
                }
                VersionConstraintType::GreaterOrEqual => {
                    vs = vs.intersection(&Range::higher_than(lo));
                }
            }
        }
        vranges.insert(dep.name.clone(), vs);
    }
    vranges
}

/// What a package version's dependencies look like when it is installed from a
/// binary build: the version's own dependencies, with every `LinkingTo`
/// dependency pinned to the version the binary was compiled against.
///
/// Returns `None` when the build must not be offered at all:
///
/// * a `linkingto` entry naming something the version does not declare a
///   dependency on would add a dependency that is not in the source metadata,
///   and it would then also leak into the lockfile's dependency lists;
/// * a pin that contradicts the declared range can never hold, so the build is
///   dead weight for the solver.
///
/// `R` and the base packages are skipped: the solver equates their version with
/// the R version, which is not what a `linkingto` entry means.
fn binary_artifact_deps(
    source: &HashMap<RPackageName, RPackageVersionRanges, rustc_hash::FxBuildHasher>,
    binary: &BinaryArtifact,
) -> Option<HashMap<RPackageName, RPackageVersionRanges, rustc_hash::FxBuildHasher>> {
    let mut deps = source.clone();
    for (name, version, _sha256) in binary.linkingto.iter() {
        if is_base_package(name) {
            continue;
        }
        let declared = deps.get(name)?;
        let pinned = declared.intersection(&RegistryPackageVersion::artifacts_of(name, version));
        if pinned.is_empty() {
            return None;
        }
        deps.insert(name.clone(), pinned);
    }
    Some(deps)
}

/// Whether a name is R itself or one of the base packages, which ship with R
/// and so are never downloaded, resolved or looked up in a binary index.
pub fn is_base_package(name: &str) -> bool {
    name == "R" || crate::proj::BASE_PKGS.contains(&name)
}

/// One binary build of one package version, as the solver needs to see it.
#[derive(Debug, Clone)]
pub struct BinaryArtifact {
    pub version: RPackageVersion,
    /// Row index in the package's binary index, i.e. the payload of
    /// [`Artifact::Binary`]. Two builds of the same version differ in nothing
    /// else, so this is what identifies them.
    pub row: u32,
    pub url: String,
    /// Hash of the upstream CRAN source tarball this version was built from.
    /// The same on every platform's build of a version, and not a checksum of
    /// anything downloadable — see `crate::repos::binaries`.
    pub sha256: String,
    /// The `LinkingTo` dependency versions this build was compiled against, with
    /// their own upstream-CRAN hashes. This, not [`BinaryArtifact::sha256`], is
    /// what tells two builds of the same version apart.
    pub linkingto: Vec<(RPackageName, RPackageVersion, String)>,
}

/// Everything a binary index knows about one package, for one build target.
#[derive(Debug, Default)]
pub struct PackageArtifacts {
    pub binaries: Vec<BinaryArtifact>,
    /// Source tarball URLs, by version. The index carries these too, and they
    /// are snapshot-pinned, unlike the CRAN URLs we would otherwise guess.
    pub source_urls: HashMap<RPackageVersion, String>,
    /// Upstream-CRAN hashes of the source tarballs, by version, from the same
    /// index rows as [`PackageArtifacts::source_urls`].
    pub source_sha256: HashMap<RPackageVersion, String>,
}

/// A source of binary artifacts for one build target, queried lazily per package
/// just like [`PackageVersionLoader`].
pub trait BinaryIndexLoader {
    /// The binary builds of `package` available for the target. An empty result
    /// means the package has no binaries, which is not an error.
    fn load_artifacts(&self, package: &str) -> Result<PackageArtifacts, Box<dyn Error>>;

    /// How the target is spelled in a lockfile, e.g. `macos-arm64`.
    fn target_name(&self) -> String;

    /// Warm whatever [`BinaryIndexLoader::load_artifacts`] reads, for many
    /// packages at once, so that a loader with a per-package round trip can pay
    /// them concurrently instead of one at a time.
    ///
    /// Purely an optimization: doing nothing is a valid implementation, and
    /// `load_artifacts` must behave the same whether or not this ran.
    fn prefetch(&self, _packages: &[String]) {}
}

#[derive(Default)]
pub struct RPackageRegistry {
    // for a package we have a list of versions
    versions: RefCell<HashMap<RPackageName, Vec<RegistryPackageVersion>>>,
    // for a package version, we have a list of dependencies
    #[allow(clippy::type_complexity)]
    deps: RefCell<
        HashMap<
            (RPackageName, RegistryPackageVersion),
            HashMap<RPackageName, RPackageVersionRanges, rustc_hash::FxBuildHasher>,
        >,
    >,
    // Packages whose versions have already been resolved: either added
    // explicitly via `add_package_version`, or lazily loaded (even if the
    // loader found nothing). Used to avoid re-querying the loader.
    loaded: RefCell<HashSet<RPackageName>>,
    // Optional lazy metadata source. When set, packages are loaded on first
    // access instead of being preloaded; when `None`, the registry only knows
    // what was added explicitly.
    loader: Option<Box<dyn PackageVersionLoader>>,
    // Optional binary artifacts for the target being solved for. When `None`,
    // only source artifacts are offered, which is what `--platform source` does.
    binaries: Option<Box<dyn BinaryIndexLoader>>,
    // Download URL of every artifact we offered, for the lockfile writers. The
    // solver itself never looks at these.
    urls: RefCell<HashMap<(RPackageName, RegistryPackageVersion), String>>,
    // Upstream-CRAN hash of every artifact we offered, recorded alongside
    // `urls` and, like it, never read by the solver itself. `rig pkg install`
    // writes it into the installed package's DESCRIPTION, as `RemoteHash`, to
    // recognize later what an installed package came from.
    sha256: RefCell<HashMap<(RPackageName, RegistryPackageVersion), String>>,
    // Build provenance of every *binary* artifact we offered: the `LinkingTo`
    // dependency versions it was compiled against, with their own hashes, as
    // `(package, version, sha256)`. Empty for source artifacts, which have no
    // build to be provenant of.
    #[allow(clippy::type_complexity)]
    linkingto:
        RefCell<HashMap<(RPackageName, RegistryPackageVersion), Vec<(String, String, String)>>>,
    // The names in each artifact's `LinkingTo:` field, for source artifacts,
    // where the provenance has to be assembled from the solution instead: a
    // source build compiles against whatever version the solve picked.
    linkingto_names: RefCell<HashMap<(RPackageName, RegistryPackageVersion), Vec<RPackageName>>>,
    // How many newest binaries win. Can be None.
    prefer_binary: Option<usize>,
    // Passed over newer version that does not have a binary.
    held_back: RefCell<HashMap<(RPackageName, RegistryPackageVersion), RPackageVersion>>,
}

impl RPackageRegistry {
    /// A registry that lazily loads package versions from `loader` on demand,
    /// and also offers the binary builds `binaries` knows about. With no
    /// `binaries` loader only source artifacts are offered.
    pub fn with_loaders(
        loader: Box<dyn PackageVersionLoader>,
        binaries: Option<Box<dyn BinaryIndexLoader>>,
    ) -> Self {
        RPackageRegistry {
            loader: Some(loader),
            binaries,
            ..Default::default()
        }
    }

    /// Let an older binary win against the most recent version.
    pub fn prefer_binary(mut self, lookback: Option<usize>) -> Self {
        self.prefer_binary = lookback;
        self
    }

    /// The version `choose_version` passed over when it picked `version` for
    /// having a binary, if that is why this artifact was chosen.
    pub fn held_back_from(
        &self,
        package: &RPackageName,
        version: &RegistryPackageVersion,
    ) -> Option<RPackageVersion> {
        self.held_back
            .borrow()
            .get(&(package.clone(), version.clone()))
            .cloned()
    }

    /// The download URL of a resolved artifact, when we know one.
    pub fn artifact_url(
        &self,
        package: &RPackageName,
        version: &RegistryPackageVersion,
    ) -> Option<String> {
        self.urls
            .borrow()
            .get(&(package.clone(), version.clone()))
            .cloned()
    }

    /// The upstream-CRAN hash of a resolved artifact, when we know one.
    ///
    /// This identifies the CRAN artifact the version was built from. It is *not*
    /// a checksum of what the artifact's URL serves, and must not be used to
    /// verify a download — see the `crate::repos::binaries` module docs.
    pub fn artifact_sha256(
        &self,
        package: &RPackageName,
        version: &RegistryPackageVersion,
    ) -> Option<String> {
        self.sha256
            .borrow()
            .get(&(package.clone(), version.clone()))
            .cloned()
    }

    /// The `LinkingTo` build provenance of a resolved *binary* artifact, as
    /// `(package, version, sha256)`. Empty for a source artifact; use
    /// [`RPackageRegistry::linkingto_names`] and the solution for those.
    pub fn artifact_linkingto(
        &self,
        package: &RPackageName,
        version: &RegistryPackageVersion,
    ) -> Vec<(String, String, String)> {
        self.linkingto
            .borrow()
            .get(&(package.clone(), version.clone()))
            .cloned()
            .unwrap_or_default()
    }

    /// The names in a resolved artifact's `LinkingTo:` field.
    ///
    /// A source package is compiled against whichever versions of these the
    /// solve picked, so its provenance can only be read off the solution.
    pub fn linkingto_names(
        &self,
        package: &RPackageName,
        version: &RegistryPackageVersion,
    ) -> Vec<RPackageName> {
        self.linkingto_names
            .borrow()
            .get(&(package.clone(), version.clone()))
            .cloned()
            .unwrap_or_default()
    }

    /// The build target binaries were resolved for, `None` for a source-only
    /// solve.
    pub fn binary_target(&self) -> Option<String> {
        self.binaries.as_ref().map(|b| b.target_name())
    }

    pub fn add_package_version(
        &self,
        pkg: RPackageName,
        ver: RegistryPackageVersion,
        deps: HashMap<RPackageName, RPackageVersionRanges, rustc_hash::FxBuildHasher>,
    ) {
        if self.versions.borrow().contains_key(&pkg) {
            self.versions
                .borrow_mut()
                .get_mut(&pkg)
                .unwrap()
                .push(ver.clone());
        } else {
            self.versions
                .borrow_mut()
                .insert(pkg.clone(), vec![ver.clone()]);
        }
        // Once a package has any explicit version it is considered resolved, so
        // the lazy loader is not consulted for it (this protects injected
        // packages like R, the base packages and `_project`).
        self.loaded.borrow_mut().insert(pkg.clone());
        // TODO: PACKAGES has multiple copies of the same version for Recommended packages,
        // but that does not matter for now, they should have the same dependencies.
        if !self.deps.borrow().contains_key(&(pkg.clone(), ver.clone())) {
            self.deps.borrow_mut().insert((pkg, ver), deps);
        }
    }

    /// Ensure a package's versions are available, loading them from the lazy
    /// loader on first access. A package with no versions (unknown) is still
    /// marked loaded so it is not queried again.
    ///
    /// Every version gets a source artifact, plus one candidate per binary build
    /// the target has. Binary builds of versions the source metadata does not
    /// know are skipped: without dependencies there is nothing to install.
    fn ensure_loaded(&self, pkg: &RPackageName) {
        if self.loaded.borrow().contains(pkg) {
            return;
        }
        if let Some(loader) = &self.loader {
            match loader.load_versions(pkg) {
                Ok(packages) => {
                    let artifacts = self.load_artifacts(pkg);
                    for package in packages {
                        let ranges =
                            rpackage_version_ranges_from_constraints(&package.dependencies, false);
                        // The `LinkingTo:` names, needed for both artifact
                        // kinds: a binary's provenance is checked against them,
                        // and a source build's has to be assembled from them.
                        let lt_names: Vec<RPackageName> = package
                            .dependencies
                            .dependencies
                            .iter()
                            .filter(|d| d.types.contains(&RDepType::LinkingTo))
                            .filter(|d| !is_base_package(&d.name))
                            .map(|d| d.name.clone())
                            .collect();
                        let src = RegistryPackageVersion {
                            name: pkg.clone(),
                            version: package.version.clone(),
                            artifact: Artifact::Source,
                        };
                        if let Some(url) = artifacts.source_urls.get(&package.version) {
                            self.urls
                                .borrow_mut()
                                .insert((pkg.clone(), src.clone()), url.clone());
                        }
                        // The index's source row is authoritative when we have
                        // one; the source metadata's own `SHA256Original` is the
                        // fallback, and the only thing available for a
                        // source-only solve, where no index is loaded at all.
                        if let Some(sha) = artifacts
                            .source_sha256
                            .get(&package.version)
                            .cloned()
                            .or_else(|| package.sha256sum.clone())
                        {
                            self.sha256
                                .borrow_mut()
                                .insert((pkg.clone(), src.clone()), sha);
                        }
                        if !lt_names.is_empty() {
                            self.linkingto_names
                                .borrow_mut()
                                .insert((pkg.clone(), src.clone()), lt_names.clone());
                        }
                        self.add_package_version(pkg.clone(), src, ranges.clone());
                        for bin in artifacts
                            .binaries
                            .iter()
                            .filter(|b| b.version == package.version)
                        {
                            match binary_artifact_deps(&ranges, bin) {
                                Some(deps) => {
                                    let v = RegistryPackageVersion {
                                        name: pkg.clone(),
                                        version: package.version.clone(),
                                        artifact: Artifact::Binary(bin.row),
                                    };
                                    self.urls
                                        .borrow_mut()
                                        .insert((pkg.clone(), v.clone()), bin.url.clone());
                                    self.sha256
                                        .borrow_mut()
                                        .insert((pkg.clone(), v.clone()), bin.sha256.clone());
                                    if !bin.linkingto.is_empty() {
                                        let prov: Vec<(String, String, String)> = bin
                                            .linkingto
                                            .iter()
                                            .map(|(n, ver, sha)| {
                                                (n.clone(), ver.to_string(), sha.clone())
                                            })
                                            .collect();
                                        self.linkingto
                                            .borrow_mut()
                                            .insert((pkg.clone(), v.clone()), prov);
                                    }
                                    if !lt_names.is_empty() {
                                        self.linkingto_names
                                            .borrow_mut()
                                            .insert((pkg.clone(), v.clone()), lt_names.clone());
                                    }
                                    self.add_package_version(pkg.clone(), v, deps);
                                }
                                None => {
                                    debug!(
                                        "Not offering binary {} {} (row {}): its LinkingTo \
                                        versions cannot be satisfied",
                                        pkg, bin.version, bin.row
                                    );
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    debug!("Failed to load versions for package '{}': {}", pkg, e);
                }
            }
        }
        // Mark loaded even when the loader found nothing, so a genuinely unknown
        // package is reported as such instead of being queried repeatedly.
        self.loaded.borrow_mut().insert(pkg.clone());
    }

    /// Warm the binary loader's cache for everything the solve is likely to
    /// visit, before solving starts.
    ///
    /// [`RPackageRegistry::ensure_loaded`] fetches one package's binary index at
    /// a time, exactly when pubgrub first looks at that package, so on a cold
    /// cache the solve stops for a network round trip at every package it
    /// discovers. The packages are predictable, though: they are the transitive
    /// closure of the project's dependencies, which the *source* metadata
    /// already describes and which is local. So we walk that closure first and
    /// hand the whole list to the loader, which can fetch it in parallel.
    ///
    /// The closure is taken over the newest version of each package, which is
    /// what a solve visits when nothing forces it to backtrack. That makes this
    /// an approximation in both directions — a backtracking solve reaches
    /// versions with dependencies the newest one does not have, and a solve that
    /// picks an old version never looks at some of what we fetched. Neither
    /// matters: `ensure_loaded` still loads whatever was missed, and a package
    /// fetched needlessly only costs one request.
    pub fn prefetch_binaries(&self, roots: &[RPackageName]) {
        let (binaries, loader) = match (&self.binaries, &self.loader) {
            (Some(binaries), Some(loader)) => (binaries, loader),
            // Source-only solve, or nothing to walk the closure with.
            _ => return,
        };

        let mut seen: HashSet<RPackageName> = HashSet::new();
        let mut queue: VecDeque<RPackageName> = VecDeque::new();
        let mut closure: Vec<RPackageName> = vec![];
        for root in roots {
            if !is_base_package(root) && seen.insert(root.clone()) {
                queue.push_back(root.clone());
            }
        }

        while let Some(pkg) = queue.pop_front() {
            closure.push(pkg.clone());
            let versions = match loader.load_versions(&pkg) {
                Ok(versions) => versions,
                Err(e) => {
                    debug!("Failed to load versions for package '{}': {}", pkg, e);
                    continue;
                }
            };
            let newest = match versions.iter().max_by(|a, b| a.version.cmp(&b.version)) {
                Some(newest) => newest,
                None => continue,
            };
            for dep in newest.dependencies.dependencies.iter() {
                // Soft dependencies are not installed, so the solver never
                // visits them — see `rpackage_version_ranges_from_constraints`.
                if dep.types.iter().all(|t| DEP_TYPES_SOFT.contains(t)) {
                    continue;
                }
                if is_base_package(&dep.name) {
                    continue;
                }
                if seen.insert(dep.name.clone()) {
                    queue.push_back(dep.name.clone());
                }
            }
        }

        binaries.prefetch(&closure);
    }

    /// The binary builds of a package, or nothing if this is a source-only solve
    /// or the index could not be read. A missing index is not fatal: we just
    /// install from source.
    fn load_artifacts(&self, pkg: &RPackageName) -> PackageArtifacts {
        match &self.binaries {
            None => PackageArtifacts::default(),
            Some(binaries) => match binaries.load_artifacts(pkg) {
                Ok(artifacts) => artifacts,
                Err(e) => {
                    debug!("Failed to load binary artifacts for '{}': {}", pkg, e);
                    PackageArtifacts::default()
                }
            },
        }
    }

    pub fn get_dependency_summary(
        &self,
        package: &RPackageName,
        version: &RegistryPackageVersion,
    ) -> Result<Vec<String>, Box<dyn Error>> {
        let key = (package.clone(), version.clone());
        match self.deps.borrow().get(&key) {
            Some(res) => Ok(res.keys().cloned().collect()),
            None => bail!("This should not happen"),
        }
    }
}

#[derive(Debug)]
pub enum ProviderError {
    UnknownPackage,
    // TODO: distinguish between unknown package and unknown version
    // UnknownVersion,
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for ProviderError {}

impl DependencyProvider for RPackageRegistry {
    type P = RPackageName;
    type V = RegistryPackageVersion;
    type VS = RPackageVersionRanges;
    type Priority = Reverse<usize>; // pick fewer versions first
    type M = String; // we won’t use custom messages
    type Err = ProviderError;

    fn prioritize(
        &self,
        package: &Self::P,
        range: &Self::VS,
        _stats: &PackageResolutionStatistics,
    ) -> Self::Priority {
        self.ensure_loaded(package);
        let count = self
            .versions
            .borrow()
            .get(package)
            .map(|vs| vs.iter().filter(|v| range.contains(v)).count())
            .unwrap_or(0);
        Reverse(count)
    }

    fn choose_version(
        &self,
        package: &Self::P,
        range: &Self::VS,
    ) -> Result<Option<Self::V>, Self::Err> {
        // Load the package's versions on demand; an unknown package (none found)
        // cannot be resolved.
        self.ensure_loaded(package);
        let versions = self.versions.borrow();
        let vlist = match versions.get(package) {
            Some(vlist) => vlist,
            None => return Err(ProviderError::UnknownPackage),
        };
        let in_range: Vec<&RegistryPackageVersion> =
            vlist.iter().filter(|v| range.contains(v)).collect();

        // Choice without a binary preference.
        let latest = match in_range.iter().copied().max() {
            Some(latest) => latest,
            None => return Ok(None),
        };
        let lookback = match self.prefer_binary {
            None => return Ok(Some(latest.clone())),
            Some(lookback) => lookback,
        };

        // Only the `lookback` newest versions may win on having a binary.
        let mut vs: Vec<&RPackageVersion> = in_range.iter().map(|v| &v.version).collect();
        vs.sort_unstable();
        vs.dedup();
        let floor = vs[vs.len().saturating_sub(lookback.max(1))];

        let eligible = |v: &RegistryPackageVersion| v.artifact.is_binary() && &v.version >= floor;
        let best = in_range
            .iter()
            .copied()
            // A tie on eligibility falls back to the normal ordering, so this
            // picks the newest eligible binary, or else exactly `latest`.
            .max_by(|a, b| eligible(a).cmp(&eligible(b)).then_with(|| a.cmp(b)))
            .unwrap_or(latest);

        if best.version < latest.version {
            debug!(
                "Choosing {} {} over {}: it has a binary package",
                package, best, latest.version
            );
            self.held_back
                .borrow_mut()
                .insert((package.clone(), best.clone()), latest.version.clone());
        }
        Ok(Some(best.clone()))
    }

    fn get_dependencies(
        &self,
        package: &Self::P,
        version: &Self::V,
    ) -> Result<Dependencies<Self::P, Self::VS, Self::M>, Self::Err> {
        // Look up the version's dependencies, loading the package on demand. A
        // still-missing entry means the package/version is unknown.
        self.ensure_loaded(package);
        let key = (package.clone(), version.clone());
        match self.deps.borrow().get(&key) {
            Some(deps) => Ok(Dependencies::Available(deps.clone())),
            None => Err(ProviderError::UnknownPackage),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    fn version(v: &str) -> RPackageVersion {
        RPackageVersion::from_str(v).unwrap()
    }

    fn source(name: &str, v: &str) -> RegistryPackageVersion {
        RegistryPackageVersion {
            name: name.to_string(),
            version: version(v),
            artifact: Artifact::Source,
        }
    }

    fn binary(name: &str, v: &str, row: u32) -> RegistryPackageVersion {
        RegistryPackageVersion {
            name: name.to_string(),
            version: version(v),
            artifact: Artifact::Binary(row),
        }
    }

    /// `Imports:`-style dependency string, which is what the DB gives us.
    fn imports(deps: &str) -> PackageDependencies {
        PackageDependencies::from_str(deps, "Imports").unwrap()
    }

    fn ranges(
        deps: &str,
    ) -> HashMap<RPackageName, RPackageVersionRanges, rustc_hash::FxBuildHasher> {
        rpackage_version_ranges_from_constraints(&imports(deps), false)
    }

    #[test]
    fn artifacts_sort_below_the_next_version() {
        assert!(source("a", "1.0.0") < binary("a", "1.0.0", 0));
        assert!(binary("a", "1.0.0", 0) < binary("a", "1.0.0", 1));
        // The version is the significant key: no artifact of 1.0.0 reaches 1.0.1.
        assert!(binary("a", "1.0.0", u32::MAX) < source("a", "1.0.1"));
    }

    #[test]
    fn version_constraints_cover_every_artifact() {
        // `>= 1.0.0` and `<= 1.0.0` must admit both artifacts of 1.0.0, and
        // `> 1.0.0` / `< 1.0.0` must admit neither.
        for (spec, source_in, binary_in) in [
            ("p (>= 1.0.0)", true, true),
            ("p (<= 1.0.0)", true, true),
            ("p (== 1.0.0)", true, true),
            ("p (> 1.0.0)", false, false),
            ("p (< 1.0.0)", false, false),
        ] {
            let vs = &ranges(spec)["p"];
            assert_eq!(
                vs.contains(&source("p", "1.0.0")),
                source_in,
                "source artifact, {}",
                spec
            );
            assert_eq!(
                vs.contains(&binary("p", "1.0.0", 7)),
                binary_in,
                "binary artifact, {}",
                spec
            );
        }
    }

    #[test]
    fn strict_bounds_still_admit_the_neighbouring_versions() {
        let vs = &ranges("p (> 1.0.0)")["p"];
        assert!(vs.contains(&source("p", "1.0.1")));
        assert!(!vs.contains(&binary("p", "0.9.9", 3)));

        let vs = &ranges("p (< 1.0.0)")["p"];
        assert!(vs.contains(&binary("p", "0.9.9", 3)));
        assert!(!vs.contains(&source("p", "1.0.1")));
    }

    #[test]
    fn equality_admits_nothing_else() {
        let vs = &ranges("p (== 1.0.0)")["p"];
        assert!(!vs.contains(&source("p", "0.9.9")));
        assert!(!vs.contains(&source("p", "1.0.1")));
        assert!(!vs.contains(&source("p", "1.0.0.1")));
    }

    // ---------------------------------------------------------------------
    // Solving with binaries

    struct StubSource {
        packages: Vec<(&'static str, &'static str, &'static str)>,
    }

    impl PackageVersionLoader for StubSource {
        fn load_versions(&self, package: &str) -> Result<Vec<crate::dcf::Package>, Box<dyn Error>> {
            Ok(self
                .packages
                .iter()
                .filter(|(name, _, _)| *name == package)
                .map(|(name, v, deps)| {
                    crate::dcf::Package::from_crandb(
                        name.to_string(),
                        version(v),
                        stub_deps(deps).dependencies,
                    )
                })
                .collect())
        }
    }

    /// Dependencies of a stub package: `Imports`, plus whatever follows a `|`
    /// as `Suggests`, e.g. `"b (>= 1.0.0) | c"`.
    fn stub_deps(spec: &str) -> PackageDependencies {
        match spec.split_once('|') {
            None => imports(spec),
            Some((hard, soft)) => {
                let mut deps = imports(hard);
                deps.append(&mut PackageDependencies::from_str(soft, "Suggests").unwrap());
                deps
            }
        }
    }

    #[derive(Default)]
    struct StubBinaries {
        /// package, version, row, `LinkingTo` pins as `pkg=version` pairs.
        builds: Vec<(&'static str, &'static str, u32, &'static str)>,
        /// What `prefetch` was called with, for the tests that check it.
        prefetched: Rc<RefCell<Vec<String>>>,
    }

    impl BinaryIndexLoader for StubBinaries {
        fn load_artifacts(&self, package: &str) -> Result<PackageArtifacts, Box<dyn Error>> {
            let binaries = self
                .builds
                .iter()
                .filter(|(name, _, _, _)| *name == package)
                .map(|(name, v, row, pins)| BinaryArtifact {
                    version: version(v),
                    row: *row,
                    url: format!("https://example.com/bin/{}_{}.bin", name, v),
                    sha256: format!("sha-{}-{}", name, v),
                    linkingto: pins
                        .split(',')
                        .filter(|p| !p.is_empty())
                        .map(|p| {
                            let (pkg, pin) = p.split_once('=').unwrap();
                            (
                                pkg.to_string(),
                                version(pin),
                                format!("sha-{}-{}", pkg, pin),
                            )
                        })
                        .collect(),
                })
                .collect();
            Ok(PackageArtifacts {
                binaries,
                source_urls: HashMap::new(),
                source_sha256: HashMap::new(),
            })
        }

        fn target_name(&self) -> String {
            "testos-x86_64".to_string()
        }

        fn prefetch(&self, packages: &[String]) {
            self.prefetched.borrow_mut().extend_from_slice(packages);
        }
    }

    /// Solve `deps` against the stubs, returning the solution keyed by package.
    fn solve(
        source: StubSource,
        binaries: Option<StubBinaries>,
        deps: &str,
    ) -> (
        RPackageRegistry,
        HashMap<String, RegistryPackageVersion, rustc_hash::FxBuildHasher>,
    ) {
        solve_preferring_binaries(source, binaries, deps, None)
    }

    /// [`solve`], with `--prefer-binary=lookback`.
    fn solve_preferring_binaries(
        source: StubSource,
        binaries: Option<StubBinaries>,
        deps: &str,
        lookback: Option<usize>,
    ) -> (
        RPackageRegistry,
        HashMap<String, RegistryPackageVersion, rustc_hash::FxBuildHasher>,
    ) {
        let binaries = binaries.map(|b| Box::new(b) as Box<dyn BinaryIndexLoader>);
        let reg =
            RPackageRegistry::with_loaders(Box::new(source), binaries).prefer_binary(lookback);
        reg.add_package_version(
            "_project".to_string(),
            RegistryPackageVersion::new("_project", "1.0.0").unwrap(),
            ranges(deps),
        );
        let solution = resolve(
            &reg,
            "_project".to_string(),
            RegistryPackageVersion::new("_project", "1.0.0").unwrap(),
        )
        .unwrap();
        (reg, solution)
    }

    #[test]
    fn a_binary_wins_against_the_source_tarball() {
        let (reg, solution) = solve(
            StubSource {
                packages: vec![("a", "1.0.0", "")],
            },
            Some(StubBinaries {
                builds: vec![("a", "1.0.0", 2, ""), ("a", "1.0.0", 5, "")],
                ..Default::default()
            }),
            "a",
        );
        // The newest build of the newest version.
        assert_eq!(solution["a"], binary("a", "1.0.0", 5));
        assert_eq!(
            reg.artifact_url(&"a".to_string(), &solution["a"]).unwrap(),
            "https://example.com/bin/a_1.0.0.bin"
        );
        assert_eq!(reg.binary_target().unwrap(), "testos-x86_64");
    }

    #[test]
    fn a_newer_version_beats_a_binary_of_an_older_one() {
        let (_reg, solution) = solve(
            StubSource {
                packages: vec![("a", "1.0.0", ""), ("a", "2.0.0", "")],
            },
            Some(StubBinaries {
                builds: vec![("a", "1.0.0", 1, "")],
                ..Default::default()
            }),
            "a",
        );
        assert_eq!(solution["a"], source("a", "2.0.0"));
    }

    #[test]
    fn a_binary_pins_the_versions_it_was_built_against() {
        let (_reg, solution) = solve(
            StubSource {
                packages: vec![
                    ("a", "1.0.0", "b (>= 1.0.0)"),
                    ("b", "1.0.0", ""),
                    ("b", "2.0.0", ""),
                ],
            },
            Some(StubBinaries {
                builds: vec![("a", "1.0.0", 1, "b=1.0.0")],
                ..Default::default()
            }),
            "a",
        );
        // Choosing the binary of `a` forces `b` back to the version it was
        // compiled against, even though 2.0.0 is allowed by the DESCRIPTION.
        assert_eq!(solution["a"], binary("a", "1.0.0", 1));
        assert_eq!(solution["b"], source("b", "1.0.0"));
    }

    #[test]
    fn an_unsatisfiable_pin_falls_back_to_the_source_tarball() {
        let (reg, solution) = solve(
            StubSource {
                packages: vec![
                    ("a", "1.0.0", "b (>= 1.0.0)"),
                    // Something else in the project needs the newer `b`, which the
                    // only binary build of `a` was not compiled against.
                    ("c", "1.0.0", "b (== 2.0.0)"),
                    ("b", "1.0.0", ""),
                    ("b", "2.0.0", ""),
                ],
            },
            Some(StubBinaries {
                builds: vec![("a", "1.0.0", 1, "b=1.0.0")],
                ..Default::default()
            }),
            "a, c",
        );
        assert_eq!(solution["a"], source("a", "1.0.0"));
        assert_eq!(solution["b"], source("b", "2.0.0"));
        // The pins never leak into the dependency list the lockfiles record.
        let mut deps = reg
            .get_dependency_summary(&"a".to_string(), &solution["a"])
            .unwrap();
        deps.sort();
        assert_eq!(deps, vec!["b".to_string()]);
    }

    #[test]
    fn builds_we_cannot_describe_are_not_offered() {
        // A build whose `linkingto` names something the package does not depend
        // on, and one for a version the source metadata does not know: neither is
        // a candidate, so the solve falls back to the source tarball.
        let (_reg, solution) = solve(
            StubSource {
                packages: vec![("a", "1.0.0", "")],
            },
            Some(StubBinaries {
                builds: vec![("a", "1.0.0", 1, "b=1.0.0"), ("a", "9.9.9", 2, "")],
                ..Default::default()
            }),
            "a",
        );
        assert_eq!(solution["a"], source("a", "1.0.0"));
    }

    // ---------------------------------------------------------------------
    // Preferring binaries over newer versions (`--prefer-binary`)

    /// The `a_newer_version_beats_a_binary_of_an_older_one` case, with the
    /// preference turned on: `2.0.0` has no binary, `1.0.0` does, and the
    /// preference is what makes the older version win.
    fn solve_preferring(lookback: Option<usize>) -> (RPackageRegistry, RegistryPackageVersion) {
        let (reg, solution) = solve_preferring_binaries(
            StubSource {
                packages: vec![("a", "1.0.0", ""), ("a", "2.0.0", "")],
            },
            Some(StubBinaries {
                builds: vec![("a", "1.0.0", 1, "")],
                ..Default::default()
            }),
            "a",
            lookback,
        );
        let chosen = solution["a"].clone();
        (reg, chosen)
    }

    #[test]
    fn a_binary_can_win_against_a_newer_version() {
        let (reg, chosen) = solve_preferring(Some(3));
        assert_eq!(chosen, binary("a", "1.0.0", 1));
        // And the version it was chosen over is reported.
        assert_eq!(
            reg.held_back_from(&"a".to_string(), &chosen),
            Some(version("2.0.0"))
        );
    }

    #[test]
    fn a_binary_outside_the_window_does_not_win() {
        let (reg, solution) = solve_preferring_binaries(
            StubSource {
                packages: vec![
                    ("a", "1.0.0", ""),
                    ("a", "2.0.0", ""),
                    ("a", "3.0.0", ""),
                    ("a", "4.0.0", ""),
                ],
            },
            Some(StubBinaries {
                // Three versions back, so outside a window of three.
                builds: vec![("a", "1.0.0", 1, "")],
                ..Default::default()
            }),
            "a",
            Some(3),
        );
        assert_eq!(solution["a"], source("a", "4.0.0"));
        assert_eq!(reg.held_back_from(&"a".to_string(), &solution["a"]), None);
    }

    #[test]
    fn a_window_of_one_is_the_default_behaviour() {
        // Only the newest version is eligible, so there is nothing to trade.
        let (reg, chosen) = solve_preferring(Some(1));
        assert_eq!(chosen, source("a", "2.0.0"));
        assert_eq!(reg.held_back_from(&"a".to_string(), &chosen), None);
    }

    #[test]
    fn the_preference_never_holds_a_version_back_for_nothing() {
        let (reg, solution) = solve_preferring_binaries(
            StubSource {
                packages: vec![("a", "1.0.0", ""), ("a", "2.0.0", "")],
            },
            Some(StubBinaries::default()),
            "a",
            Some(3),
        );
        assert_eq!(solution["a"], source("a", "2.0.0"));
        assert_eq!(reg.held_back_from(&"a".to_string(), &solution["a"]), None);
    }

    #[test]
    fn a_version_a_constraint_ruled_out_is_not_reported_as_held_back() {
        // The project asks for the older version itself, so its binary is the
        // newest candidate there is: the preference did not trade anything.
        let (reg, solution) = solve_preferring_binaries(
            StubSource {
                packages: vec![("a", "1.0.0", ""), ("a", "2.0.0", "")],
            },
            Some(StubBinaries {
                builds: vec![("a", "1.0.0", 1, "")],
                ..Default::default()
            }),
            "a (<= 1.0.0)",
            Some(3),
        );
        assert_eq!(solution["a"], binary("a", "1.0.0", 1));
        assert_eq!(reg.held_back_from(&"a".to_string(), &solution["a"]), None);
    }

    #[test]
    fn a_preferred_binary_still_pins_what_it_was_built_against() {
        let (_reg, solution) = solve_preferring_binaries(
            StubSource {
                packages: vec![
                    ("a", "1.0.0", "b (>= 1.0.0)"),
                    ("a", "2.0.0", "b (>= 1.0.0)"),
                    ("b", "1.0.0", ""),
                    ("b", "2.0.0", ""),
                ],
            },
            Some(StubBinaries {
                builds: vec![("a", "1.0.0", 1, "b=1.0.0")],
                ..Default::default()
            }),
            "a",
            Some(3),
        );
        // Trading a version for a binary drags its `LinkingTo` dependencies back
        // with it: this is why the window is bounded.
        assert_eq!(solution["a"], binary("a", "1.0.0", 1));
        assert_eq!(solution["b"], source("b", "1.0.0"));
    }

    #[test]
    fn an_unsatisfiable_preferred_binary_backtracks_to_the_newest_version() {
        let (reg, solution) = solve_preferring_binaries(
            StubSource {
                packages: vec![
                    ("a", "1.0.0", "b (>= 1.0.0)"),
                    ("a", "2.0.0", "b (>= 1.0.0)"),
                    // Something else in the project needs the newer `b`, which
                    // the binary build of `a` 1.0.0 was not compiled against.
                    ("c", "1.0.0", "b (== 2.0.0)"),
                    ("b", "1.0.0", ""),
                    ("b", "2.0.0", ""),
                ],
            },
            Some(StubBinaries {
                builds: vec![("a", "1.0.0", 1, "b=1.0.0")],
                ..Default::default()
            }),
            "a, c",
            Some(3),
        );
        // The preference is a heuristic, so the solve still finds the answer that
        // works, and the abandoned choice is not reported as held back.
        assert_eq!(solution["a"], source("a", "2.0.0"));
        assert_eq!(solution["b"], source("b", "2.0.0"));
        assert_eq!(reg.held_back_from(&"a".to_string(), &solution["a"]), None);
    }

    /// Build a registry on the stubs, and hand back the list `prefetch` sees.
    fn registry(
        source: StubSource,
        binaries: StubBinaries,
    ) -> (RPackageRegistry, Rc<RefCell<Vec<String>>>) {
        let prefetched = binaries.prefetched.clone();
        let reg = RPackageRegistry::with_loaders(
            Box::new(source),
            Some(Box::new(binaries) as Box<dyn BinaryIndexLoader>),
        );
        (reg, prefetched)
    }

    fn prefetched_for(source: StubSource, roots: &[&str]) -> Vec<String> {
        let (reg, prefetched) = registry(source, StubBinaries::default());
        let roots: Vec<String> = roots.iter().map(|r| r.to_string()).collect();
        reg.prefetch_binaries(&roots);
        let mut names = prefetched.borrow().clone();
        names.sort();
        names
    }

    #[test]
    fn prefetching_walks_the_whole_dependency_closure() {
        // A diamond, so a package reached twice is still prefetched once, plus a
        // package nothing depends on, which is not prefetched at all.
        let names = prefetched_for(
            StubSource {
                packages: vec![
                    ("a", "1.0.0", "b, c"),
                    ("b", "1.0.0", "d"),
                    ("c", "1.0.0", "d"),
                    ("d", "1.0.0", ""),
                    ("unrelated", "1.0.0", ""),
                ],
            },
            &["a"],
        );
        assert_eq!(names, ["a", "b", "c", "d"]);
    }

    #[test]
    fn prefetching_skips_what_the_solver_never_downloads() {
        let names = prefetched_for(
            StubSource {
                packages: vec![
                    // The newest version is what the closure follows: 2.0.0
                    // needs `c`, and the `b` that only 1.0.0 needed is left to
                    // the lazy path in case the solve backtracks to it.
                    ("a", "1.0.0", "b"),
                    ("a", "2.0.0", "R (>= 4.0.0), stats, c | suggested"),
                    ("b", "1.0.0", ""),
                    ("c", "1.0.0", ""),
                    ("suggested", "1.0.0", ""),
                ],
            },
            // R and the base packages are not downloadable, whether they are
            // roots or dependencies.
            &["a", "utils"],
        );
        assert_eq!(names, ["a", "c"]);
    }

    #[test]
    fn without_a_binary_loader_there_is_nothing_to_prefetch() {
        // No loader to prefetch into, and nothing that could fail: a
        // source-only solve never asks about binaries.
        let reg = RPackageRegistry::with_loaders(
            Box::new(StubSource {
                packages: vec![("a", "1.0.0", "b"), ("b", "1.0.0", "")],
            }),
            None,
        );
        reg.prefetch_binaries(&["a".to_string()]);
        assert!(reg.binary_target().is_none());
    }

    #[test]
    fn without_a_binary_loader_only_source_is_offered() {
        let (reg, solution) = solve(
            StubSource {
                packages: vec![("a", "1.0.0", "")],
            },
            None,
            "a",
        );
        assert_eq!(solution["a"], source("a", "1.0.0"));
        assert!(reg.binary_target().is_none());
        assert!(reg.artifact_url(&"a".to_string(), &solution["a"]).is_none());
    }
}
