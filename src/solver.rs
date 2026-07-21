use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RegistryPackageVersion {
    pub name: RPackageName,
    pub version: RPackageVersion,
}

impl RegistryPackageVersion {
    pub fn new(name: &str, version_str: &str) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(RegistryPackageVersion {
            name: name.to_string(),
            version: RPackageVersion::from_str(version_str)?,
        })
    }
}

impl Ord for RegistryPackageVersion {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.version.cmp(&other.version)
    }
}

impl PartialOrd for RegistryPackageVersion {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for RegistryPackageVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.version)?;
        Ok(())
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
            let ver = RegistryPackageVersion {
                name: dep.name.clone(),
                version: cs.version.clone(),
            };
            match cs.constraint_type {
                VersionConstraintType::Less => {
                    vs = vs.intersection(&Range::strictly_lower_than(ver));
                }
                VersionConstraintType::LessOrEqual => {
                    vs = vs.intersection(&Range::lower_than(ver));
                }
                VersionConstraintType::Equal => {
                    vs = vs.intersection(&Range::singleton(ver));
                }
                VersionConstraintType::Greater => {
                    vs = vs.intersection(&Range::strictly_higher_than(ver));
                }
                VersionConstraintType::GreaterOrEqual => {
                    vs = vs.intersection(&Range::higher_than(ver));
                }
            }
        }
        vranges.insert(dep.name.clone(), vs);
    }
    vranges
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
}

impl RPackageRegistry {
    /// A registry that lazily loads package versions from `loader` on demand.
    pub fn with_loader(loader: Box<dyn PackageVersionLoader>) -> Self {
        RPackageRegistry {
            loader: Some(loader),
            ..Default::default()
        }
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
    fn ensure_loaded(&self, pkg: &RPackageName) {
        if self.loaded.borrow().contains(pkg) {
            return;
        }
        if let Some(loader) = &self.loader {
            match loader.load_versions(pkg) {
                Ok(packages) => {
                    for package in packages {
                        let ranges =
                            rpackage_version_ranges_from_constraints(&package.dependencies, false);
                        let v = RegistryPackageVersion {
                            name: pkg.clone(),
                            version: package.version.clone(),
                        };
                        self.add_package_version(pkg.clone(), v, ranges);
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
        if !self.versions.borrow().contains_key(package) {
            return Err(ProviderError::UnknownPackage);
        }

        let best = self
            .versions
            .borrow()
            .get(package)
            .into_iter()
            .flat_map(|vlist| vlist.iter())
            .filter(|v| range.contains(v))
            .cloned()
            .max();
        Ok(best)
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
