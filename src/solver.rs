use std::collections::HashMap;
use std::cmp::Reverse;
use std::fmt;

use pubgrub::*;
use rustc_hash::FxBuildHasher;

use crate::dcf::*;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RPackageVersion {
    pub components: Vec<u32>,
}

impl RPackageVersion {
    pub fn from_str(s: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let comps: Result<Vec<u32>, _> = s
            .split(['.', '-'])
            .map(|part| part.parse::<u32>())
            .collect();
        Ok(RPackageVersion { components: comps? })
    }
}

impl fmt::Display for RPackageVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, comp) in self.components.iter().enumerate() {
            write!(f, "{}{}", if i != 0 { "." } else { "" }, comp)?;
        }
        Ok(())
    }
}

type RPackageName = String;
pub type RPackageVersionRanges = version_ranges::Ranges<RPackageVersion>;

pub fn rpackage_version_ranges_from_constraints(
    constraints: &Vec<DepVersionSpec>
)
    -> HashMap<RPackageName, RPackageVersionRanges, rustc_hash::FxBuildHasher>
{
    let mut vranges=
        HashMap::with_hasher(rustc_hash::FxBuildHasher::default());
    for dep in constraints.iter() {
      let mut vs = RPackageVersionRanges::full();
      for cs in dep.constraints.iter() {
        let ver = RPackageVersion::from_str(&cs.1).unwrap();
        match cs.0 {
            VersionConstraint::Less => {
                vs = vs.intersection(&Range::strictly_lower_than(ver));
            },
            VersionConstraint::LessOrEqual => {
                vs = vs.intersection(&Range::lower_than(ver));
            },
            VersionConstraint::Equal => {
                vs = vs.intersection(&Range::singleton(ver));
            },
            VersionConstraint::Greater => {
                vs = vs.intersection(&Range::strictly_higher_than(ver));
            },
            VersionConstraint::GreaterOrEqual => {
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
    versions: HashMap<RPackageName, Vec<RPackageVersion>>,
    deps: HashMap<
        (RPackageName, RPackageVersion),
        HashMap<RPackageName, RPackageVersionRanges, rustc_hash::FxBuildHasher>
    >,
}

impl RPackageRegistry {
    pub fn add_package_version(
        &mut self,
        pkg: RPackageName,
        ver: RPackageVersion,
        deps: HashMap<RPackageName, RPackageVersionRanges, rustc_hash::FxBuildHasher>)
    {
      if self.versions.contains_key(&pkg) {
          self.versions.get_mut(&pkg).unwrap().push(ver.clone());
      } else {
          self.versions.insert(pkg.clone(), vec![ver.clone()]);
      }
      self.deps.insert((pkg, ver), deps);
    }
}

#[derive(Debug)]
pub enum ProviderError {
    UnknownPackage,
    UnknownVersion,
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
            .get(package)
            .map(|vs| {
                vs.iter().filter(|v| range.contains(v)).count()
            })
            .unwrap_or(0);
        Reverse(count)
    }

    fn choose_version(
        &self,
        package: &Self::P,
        range: &Self::VS,
    ) -> Result<Option<Self::V>, Self::Err> {
        let best = self
            .versions
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
        let deps = self.deps.get(&key).cloned().unwrap_or_default();
        Ok(Dependencies::Available(deps))
    }
}
