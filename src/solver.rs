use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::HashMap;
use std::error::Error;
use std::fmt;

use log::info;
use pubgrub::*;
use simple_error::bail;

use crate::dcf::*;
use crate::repos::*;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RPackageVersion {
    pub components: Vec<u32>,
    pub original: String,
}

impl RPackageVersion {
    pub fn from_str(s: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let comps: Result<Vec<u32>, _> = s
            .split(['.', '-'])
            .map(|part| part.parse::<u32>())
            .collect();
        Ok(RPackageVersion {
            components: comps?,
            original: s.to_string(),
        })
    }
}

impl fmt::Display for RPackageVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.original)?;
        Ok(())
    }
}

type RPackageName = String;
pub type RPackageVersionRanges = version_ranges::Ranges<RPackageVersion>;

pub fn rpackage_version_ranges_from_constraints(
    constraints: &Vec<DepVersionSpec>,
) -> HashMap<RPackageName, RPackageVersionRanges, rustc_hash::FxBuildHasher> {
    let mut vranges = HashMap::with_hasher(rustc_hash::FxBuildHasher::default());
    for dep in constraints.iter() {
        let mut vs = RPackageVersionRanges::full();
        for cs in dep.constraints.iter() {
            let ver = match RPackageVersion::from_str(&cs.version) {
                Ok(v) => v,
                Err(_) => {
                    info!(
                        "Invalid version in constraint for package {}: {}",
                        dep.name, &cs.version
                    );
                    continue;
                }
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
    versions: RefCell<HashMap<RPackageName, Vec<RPackageVersion>>>,
    deps: RefCell<
        HashMap<
            (RPackageName, RPackageVersion),
            HashMap<RPackageName, RPackageVersionRanges, rustc_hash::FxBuildHasher>,
        >,
    >,
    client: RefCell<Option<reqwest::Client>>,
}

impl RPackageRegistry {
    pub fn add_package_version(
        &self,
        pkg: RPackageName,
        ver: RPackageVersion,
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
        // TODO: PACKAGES has multiple copies of the same version for Recommended packages,
        // but that does not matter for now, they should have the same dependencies.
        if !self.deps.borrow().contains_key(&(pkg.clone(), ver.clone())) {
            self.deps.borrow_mut().insert((pkg, ver), deps);
        }
    }

    fn get_all_versions(&self, pkg: &RPackageName) -> Result<(), Box<dyn Error>> {
        if self.client.borrow().is_none() {
            self.client.replace(Some(reqwest::Client::new()));
        }
        let vers = get_all_cran_package_versions(pkg, self.client.borrow().as_ref())?;
        for (ver, deps) in vers {
            let vranges = rpackage_version_ranges_from_constraints(&deps);
            self.add_package_version(pkg.clone(), ver, vranges);
        }
        Ok(())
    }
    pub fn get_dependency_summary(
        &self,
        package: &RPackageName,
        version: &RPackageVersion,
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
    type V = RPackageVersion;
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
        if !self.versions.borrow().contains_key(package) {
            match self.get_all_versions(package) {
                Err(_) => return Err(ProviderError::UnknownPackage),
                _ => {}
            };
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
        // Look up explicitly stored dependencies
        let key = (package.clone(), version.clone());
        if let Some(deps) = self.deps.borrow().get(&key) {
            return Ok(Dependencies::Available(deps.clone()));
        }
        match self.get_all_versions(package) {
            Err(_) => return Err(ProviderError::UnknownPackage),
            _ => {}
        };
        match self.deps.borrow().get(&key) {
            Some(res) => Ok(Dependencies::Available(res.clone())),
            None => Err(ProviderError::UnknownPackage),
        }
    }
}
