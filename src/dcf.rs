use std::collections::HashMap;
use std::error::Error;

use simple_error::*;

/// Parse a single dependency field
pub fn parse_deps(deps: &str, dep_type: &str)
            -> Result<Vec<DepVersionSpec>, Box<dyn Error>> {
    let mut result = Vec::new();
    for dep in deps.split(',') {
        let dep = dep.trim();
        if dep.len() == 0 {
            continue;
        }
        result.push(parse_dep(dep, dep_type)?);
    }

    // need to merge constraints for the same package
    let result2 = simplify_constraints(result);
    Ok(result2)
}

/// Merge constraints for the same package
pub fn simplify_constraints(deps: Vec<DepVersionSpec>) -> Vec<DepVersionSpec> {
    let mut pkgmap: HashMap<&str, usize> = HashMap::new();
    let mut deps2 = Vec::new();
    for dep in deps.iter() {
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
    deps2
}

/// Parse a single dependency specification, i.e. a package in a
/// dependency field
fn parse_dep(dep: &str, dep_type: &str)
            -> Result<DepVersionSpec, Box<dyn Error>> {
    let (name, spec) = match dep.find('(') {
        Some(i) => (&dep[..i], &dep[i..]),
        None => (dep, ""),
    };
    let name = name.trim();
    let sepc = spec.trim();
    let types: Vec<String> =
      vec![dep_type.to_string()];
    let mut constraints = Vec::new();

    if spec.len() > 0 {
        let specbytes = spec.as_bytes();
        if specbytes.first() != Some(&b'(') || specbytes.last() != Some(&b')') {
            bail!("Invalid dependency version: {}", dep);
        }
        let spec = &spec[1..spec.len()-1];
        if spec.starts_with(">=") {
            let ver = spec[2..].trim().to_string();
            constraints.push((VersionConstraint::GreaterOrEqual, ver));
        } else if spec.starts_with("<=") {
            let ver = spec[2..].trim().to_string();
            constraints.push((VersionConstraint::LessOrEqual, ver));
        } else if spec.starts_with("==") {
            let ver = spec[2..].trim().to_string();
            constraints.push((VersionConstraint::Equal, ver));
        } else if spec.starts_with('=') {
            let ver = spec[1..].trim().to_string();
            constraints.push((VersionConstraint::Equal, ver));
        } else if spec.starts_with(">>") {
            let ver = spec[2..].trim().to_string();
            constraints.push((VersionConstraint::Greater, ver));
        } else if spec.starts_with('>') {
            let ver = spec[1..].trim().to_string();
            constraints.push((VersionConstraint::Greater, ver));
        } else if spec.starts_with("<<") {
            let ver = spec[2..].trim().to_string();
            constraints.push((VersionConstraint::Less, ver));
        } else if spec.starts_with('<') {
            let ver = spec[1..].trim().to_string();
            constraints.push((VersionConstraint::Less, ver));
        } else {
            bail!("Invalid dependency version: {}", dep);
        }
    }
    Ok(DepVersionSpec { name: name.to_string(), types, constraints })
}

#[derive(Debug, Hash, Clone, PartialEq, Eq)]
pub enum VersionConstraint {
    Less,
    LessOrEqual,
    Equal,
    Greater,
    GreaterOrEqual,
}

impl std::fmt::Display for VersionConstraint {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            VersionConstraint::GreaterOrEqual => f.write_str(">="),
            VersionConstraint::LessOrEqual => f.write_str("<="),
            VersionConstraint::Equal => f.write_str("="),
            VersionConstraint::Greater => f.write_str(">>"),
            VersionConstraint::Less => f.write_str("<<"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DepVersionSpec {
    /// Package name.
    pub name: String,
    /// Dependency Type(s)
    pub types: Vec<String>,
    /// Version constraints.
    pub constraints: Vec<(VersionConstraint, String)>,
}

impl std::fmt::Display for DepVersionSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.name)?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub dependencies: Vec<DepVersionSpec>,
}
