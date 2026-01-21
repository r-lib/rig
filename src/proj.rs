use std::error::Error;
use std::fs::File;

use clap::ArgMatches;
use deb822_fast::Deb822;

use simple_error::*;

pub fn sc_proj(args: &ArgMatches, mainargs: &ArgMatches)
              -> Result<(), Box<dyn Error>> {

    match args.subcommand() {
        Some(("deps", s)) => sc_proj_deps(s, args, mainargs),
        _ => Ok(()), // unreachable
    }
}

fn sc_proj_deps(
    args: &ArgMatches,
    libargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {

    let df = File::open("DESCRIPTION")?;
    let desc = Deb822::from_reader(df)?;

    if desc.len() == 0 {
      bail!("Empty DESCRIPTION file");
    }

    if desc.len() > 1 {
      bail!("Invalid DESCRIPTION file, empty lines are not allowed");
    }

    for desc0 in desc.iter() {
      println!("Dependencies for project:");
      if let Some(deps) = desc0.get("Depends") {
          println!("  Depends: {:?}", parse_deps(deps)?);
      }
      if let Some(deps) = desc0.get("Imports") {
          println!("  Imports: {:?}", parse_deps(deps)?);
      }
      if let Some(deps) = desc0.get("Suggests") {
          println!("  Suggests: {:?}", parse_deps(deps)?);
      }
      if let Some(deps) = desc0.get("Enhances") {
          println!("  Enhances: {:?}", parse_deps(deps)?);
      }
      if let Some(deps) = desc0.get("LinkingTo") {
          println!("  LinkingTo: {:?}", parse_deps(deps)?);
      }
    }

    Ok(())
}

fn parse_deps(deps: &str) -> Result<Vec<DepVersionSpec>, Box<dyn Error>> {
    let mut result = Vec::new();
    for dep in deps.split(',') {
        let dep = dep.trim();
        if dep.len() == 0 {
            continue;
        }
        result.push(parse_dep(dep)?);
    }
    // TODO: need to merge constraints for the same package
    Ok(result)
}

fn parse_dep(dep: &str) -> Result<DepVersionSpec, Box<dyn Error>> {
    let parts: Vec<&str> = dep.split_whitespace().collect();
    if parts.len() == 0 || parts[0].len() == 0 {
        bail!("Invalid dependency version: {}", dep);
    }
    let name = parts[0].to_string();
    let mut constraints = Vec::new();

    if parts.len() > 1 {
        let trimmed = parts.iter()
            .map(|s| s.trim())
            .collect::<Vec<&str>>();
        let spec = trimmed[1..].join("");
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
    Ok(DepVersionSpec { name, constraints })
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
    /// Version constraints.
    pub constraints: Vec<(VersionConstraint, String)>,
}
