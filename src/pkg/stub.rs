//! Test fixtures shared by the `rig pkg deps` and `rig pkg tree` unit tests: a
//! [`PackageVersionLoader`] over a hand-written set of packages, so the walks
//! can be exercised without a metadata database.

use std::error::Error;

use crate::dcf::{Package, PackageDependencies, RPackageVersion};
use crate::solver::PackageVersionLoader;

/// A [`PackageVersionLoader`] over a fixed set of `(name, version, deps)`
/// triples, `deps` being DCF-ish fields, e.g.
/// `"Imports: b (>= 1.0.0), c; Suggests: d"`.
pub(super) struct Stub {
    pub(super) packages: Vec<(&'static str, &'static str, &'static str)>,
}

impl PackageVersionLoader for Stub {
    fn load_versions(&self, package: &str) -> Result<Vec<Package>, Box<dyn Error>> {
        Ok(self
            .packages
            .iter()
            .filter(|(name, _, _)| *name == package)
            .map(|(name, version, deps)| {
                Package::from_crandb(
                    name.to_string(),
                    RPackageVersion::from_str(version).unwrap(),
                    stub_deps(deps).dependencies,
                )
            })
            .collect())
    }
}

/// Parse the DCF-ish dependency spec of a [`Stub`] package, e.g.
/// `"Imports: b (>= 1.0.0), c; Suggests: d"`.
pub(super) fn stub_deps(spec: &str) -> PackageDependencies {
    let mut deps = PackageDependencies::new();
    for field in spec.split(';') {
        let field = field.trim();
        if field.is_empty() {
            continue;
        }
        let (dep_type, list) = field.split_once(':').unwrap();
        deps.append(&mut PackageDependencies::from_str(list, dep_type.trim()).unwrap());
    }
    deps.simplify();
    deps
}
