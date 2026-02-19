use std::collections::HashMap;
use std::error::Error;

use bitcode::{Decode, Encode};
use semver;
use simple_error::*;

// ------------------------------------------------------------------------
// A version constraint type, e.g. >= or >>, etc.

#[derive(Debug, Hash, Clone, PartialEq, Eq, Encode, Decode)]
pub enum VersionConstraintType {
    Less,
    LessOrEqual,
    Equal,
    Greater,
    GreaterOrEqual,
}

impl std::fmt::Display for VersionConstraintType {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            VersionConstraintType::GreaterOrEqual => f.write_str(">="),
            VersionConstraintType::LessOrEqual => f.write_str("<="),
            VersionConstraintType::Equal => f.write_str("="),
            VersionConstraintType::Greater => f.write_str(">>"),
            VersionConstraintType::Less => f.write_str("<<"),
        }
    }
}

// ------------------------------------------------------------------------
// This is a version constraint that also includes the version number,
// e.g. ">= 4.0.0"

#[derive(Debug, Hash, Clone, PartialEq, Eq, Encode, Decode)]
pub struct VersionConstraint {
    pub constraint_type: VersionConstraintType,
    pub version: String,
}

impl VersionConstraint {
    /// Parse a version constraint specification (e.g., ">= 4.0.0")
    /// The spec should NOT include surrounding parentheses
    pub fn from_str(spec: &str) -> Result<Self, Box<dyn Error>> {
        let (constraint_type, version) = if spec.starts_with(">=") {
            let ver = spec[2..].trim().to_string();
            (VersionConstraintType::GreaterOrEqual, ver)
        } else if spec.starts_with("<=") {
            let ver = spec[2..].trim().to_string();
            (VersionConstraintType::LessOrEqual, ver)
        } else if spec.starts_with("==") {
            let ver = spec[2..].trim().to_string();
            (VersionConstraintType::Equal, ver)
        } else if spec.starts_with('=') {
            let ver = spec[1..].trim().to_string();
            (VersionConstraintType::Equal, ver)
        } else if spec.starts_with(">>") {
            let ver = spec[2..].trim().to_string();
            (VersionConstraintType::Greater, ver)
        } else if spec.starts_with('>') {
            let ver = spec[1..].trim().to_string();
            (VersionConstraintType::Greater, ver)
        } else if spec.starts_with("<<") {
            let ver = spec[2..].trim().to_string();
            (VersionConstraintType::Less, ver)
        } else if spec.starts_with('<') {
            let ver = spec[1..].trim().to_string();
            (VersionConstraintType::Less, ver)
        } else {
            bail!("Invalid version constraint: {}", spec)
        };

        Ok(VersionConstraint {
            constraint_type,
            version,
        })
    }
}

// ------------------------------------------------------------------------
// This is a single package dependency spec, including the package name,
// the dependency types, and a list of version constraints,
// which can also be empty

#[derive(Debug, Clone, PartialEq, Eq, Hash, Encode, Decode)]
pub struct DepVersionSpec {
    /// Package name.
    pub name: String,
    /// Dependency Type(s)
    pub types: Vec<String>,
    /// Version constraints.
    pub constraints: Vec<VersionConstraint>,
}

impl DepVersionSpec {
    /// Parse a single dependency specification, i.e. a package in a dependency field
    pub fn parse(dep: &str, dep_type: &str) -> Result<Self, Box<dyn Error>> {
        let (name, spec) = match dep.find('(') {
            Some(i) => (&dep[..i], &dep[i..]),
            None => (dep, ""),
        };
        let name = name.trim();
        let types: Vec<String> = vec![dep_type.to_string()];
        let mut constraints = Vec::new();

        if spec.len() > 0 {
            let specbytes = spec.as_bytes();
            if specbytes.first() != Some(&b'(') || specbytes.last() != Some(&b')') {
                bail!("Invalid dependency version: {}", dep);
            }
            let spec = &spec[1..spec.len() - 1];
            constraints.push(VersionConstraint::from_str(spec)?);
        }
        Ok(DepVersionSpec {
            name: name.to_string(),
            types,
            constraints,
        })
    }

    /// Check if a version string satisfies all constraints in this DepVersionSpec
    pub fn satisfies(&self, version: &str) -> Result<bool, Box<dyn Error>> {
        // Parse the version string
        let ver = match semver::Version::parse(version) {
            Ok(v) => v,
            Err(e) => bail!("Invalid version '{}': {}", version, e),
        };

        // Check all constraints
        for constraint in self.constraints.iter() {
            let constraint_ver = match semver::Version::parse(&constraint.version) {
                Ok(v) => v,
                Err(e) => bail!("Invalid constraint version '{}': {}", constraint.version, e),
            };

            let satisfied = match constraint.constraint_type {
                VersionConstraintType::Less => ver < constraint_ver,
                VersionConstraintType::LessOrEqual => ver <= constraint_ver,
                VersionConstraintType::Equal => ver == constraint_ver,
                VersionConstraintType::Greater => ver > constraint_ver,
                VersionConstraintType::GreaterOrEqual => ver >= constraint_ver,
            };

            if !satisfied {
                return Ok(false);
            }
        }

        Ok(true)
    }
}

impl std::fmt::Display for DepVersionSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.name)?;
        Ok(())
    }
}

// ------------------------------------------------------------------------
// This is a set of package dependencies. It can be used for a single field,
// e.g. Depends, or it can be used for the combined dependencies of a
// package

#[derive(Debug, Clone, Encode, Decode)]
pub struct PackageDependencies {
    pub dependencies: Vec<DepVersionSpec>,
}

impl PackageDependencies {
    pub fn new() -> Self {
        PackageDependencies {
            dependencies: Vec::new(),
        }
    }

    pub fn append(&mut self, other: &mut PackageDependencies) {
        self.dependencies.append(&mut other.dependencies);
    }

    /// Parse a single dependency field
    pub fn from_str(deps: &str, dep_type: &str) -> Result<Self, Box<dyn Error>> {
        let mut result: Vec<DepVersionSpec> = Vec::new();
        for dep in deps.split(',') {
            let dep = dep.trim();
            if dep.len() == 0 {
                continue;
            }
            result.push(DepVersionSpec::parse(dep, dep_type)?);
        }

        // need to merge constraints for the same package
        let mut pkg_deps = PackageDependencies { dependencies: result };
        pkg_deps.simplify();
        Ok(pkg_deps)
    }

    /// Merge constraints for the same package
    pub fn simplify(&mut self) {
        let mut pkgmap: HashMap<&str, usize> = HashMap::new();
        let mut deps2 = Vec::new();
        for dep in self.dependencies.iter() {
            if let Some(idx) = pkgmap.get(dep.name.as_str()) {
                let existing: &mut DepVersionSpec = &mut deps2[*idx];
                for c in dep.constraints.iter() {
                    if !existing.constraints.contains(c) {
                        existing.constraints.push(c.clone());
                    }
                }
            } else {
                pkgmap.insert(dep.name.as_str(), deps2.len());
                deps2.push(dep.clone());
            }
        }
        self.dependencies = deps2;
    }
}

// ------------------------------------------------------------------------

#[derive(Debug, Clone, Encode, Decode)]
pub struct DCFBuilt {
    pub r: String,
    pub platform: Option<String>,
    pub timestamp: String,
    pub os_type: String
}

impl DCFBuilt {
    pub fn from_str(s: &str) -> Result<Self, Box<dyn Error>> {
        let parts: Vec<&str> = s.split(';').collect();

        if parts.len() != 4 {
            bail!("Invalid Built field format: expected 4 parts, got {}", parts.len());
        }

        // First part: R version (e.g., "R 4.3.0") - strip the "R" prefix and any whitespace
        let r_part = parts[0].trim();
        let r = if r_part.starts_with('R') {
            r_part[1..].trim().to_string()
        } else {
            r_part.to_string()
        };

        // Second part: platform (can be empty)
        let platform = if parts[1].trim().is_empty() {
            None
        } else {
            Some(parts[1].trim().to_string())
        };

        // Third part: timestamp
        let timestamp = parts[2].trim().to_string();

        // Fourth part: os_type
        let os_type = parts[3].trim().to_string();

        Ok(DCFBuilt {
            r: r,
            platform,
            timestamp,
            os_type,
        })
    }
}

// ------------------------------------------------------------------------


#[derive(Debug, Clone, Encode, Decode)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub dependencies: PackageDependencies,
    // with pak, it is possible to store the package at a custom URL
    // instead of the normal CRAN-like repository structure.
    pub url: Option<String>,
    // sometimes the package is at a special path in the repository
    // if url is not None, then this should be None
    pub path: Option<String>,
    // newer repos have a Built field, so we can update binaries when
    // CRAN rebuilds them.
    pub built: Option<DCFBuilt>,
}

// ------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dcf_built_from_str_with_platform() {
        let input = "R 4.3.0; x86_64-pc-linux-gnu; 2024-01-15 10:30:00 UTC; unix";
        let result = DCFBuilt::from_str(input);

        assert!(result.is_ok());
        let built = result.unwrap();
        assert_eq!(built.r, "4.3.0");
        assert_eq!(built.platform, Some("x86_64-pc-linux-gnu".to_string()));
        assert_eq!(built.timestamp, "2024-01-15 10:30:00 UTC");
        assert_eq!(built.os_type, "unix");
    }

    #[test]
    fn test_dcf_built_from_str_empty_platform() {
        let input = "R 4.3.0; ; 2024-01-15 10:30:00 UTC; unix";
        let result = DCFBuilt::from_str(input);

        assert!(result.is_ok());
        let built = result.unwrap();
        assert_eq!(built.r, "4.3.0");
        assert_eq!(built.platform, None);
        assert_eq!(built.timestamp, "2024-01-15 10:30:00 UTC");
        assert_eq!(built.os_type, "unix");
    }

    #[test]
    fn test_dcf_built_from_str_flexible_whitespace() {
        // Test with multiple spaces between R and version
        let input = "R  4.3.0; ; 2024-01-15 10:30:00 UTC; unix";
        let result = DCFBuilt::from_str(input);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().r, "4.3.0");

        // Test with tab between R and version
        let input = "R\t4.3.0; ; 2024-01-15 10:30:00 UTC; unix";
        let result = DCFBuilt::from_str(input);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().r, "4.3.0");

        // Test with no space (just R prefix)
        let input = "R4.3.0; ; 2024-01-15 10:30:00 UTC; unix";
        let result = DCFBuilt::from_str(input);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().r, "4.3.0");
    }

    #[test]
    fn test_dcf_built_from_str_no_r_prefix() {
        // Should still work if R prefix is missing
        let input = "4.3.0; ; 2024-01-15 10:30:00 UTC; unix";
        let result = DCFBuilt::from_str(input);

        assert!(result.is_ok());
        let built = result.unwrap();
        assert_eq!(built.r, "4.3.0");
    }

    #[test]
    fn test_dcf_built_from_str_invalid_parts() {
        // Too few parts
        let input = "R 4.3.0; ; 2024-01-15 10:30:00 UTC";
        let result = DCFBuilt::from_str(input);
        assert!(result.is_err());

        // Too many parts
        let input = "R 4.3.0; ; 2024-01-15 10:30:00 UTC; unix; extra";
        let result = DCFBuilt::from_str(input);
        assert!(result.is_err());
    }
}
