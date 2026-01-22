use std::collections::HashMap;
use std::error::Error;
use std::fs::File;

use clap::ArgMatches;
use deb822_fast::Deb822;
use simple_error::*;
use tabular::*;

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

    let mut deps: Vec<DepVersionSpec> = vec![];

    for desc0 in desc.iter() {
      if let Some(dd) = desc0.get("Depends") {
          deps.append(&mut parse_deps(dd, "Depends")?)
      }
      if let Some(di) = desc0.get("Imports") {
          deps.append(&mut parse_deps(di, "Imports")?)
      }
      if let Some(dl) = desc0.get("LinkingTo") {
          deps.append(&mut parse_deps(dl, "LinkingTo")?);
      }
      // if let Some(ds) = desc0.get("Suggests") {
      //     deps.append(&mut parse_deps(ds)?);
      // }
      // if let Some(de) = desc0.get("Enhances") {
      //     deps.append(&mut parse_deps(de)?);
      // }
    }

    let deps = simplify_constraints(deps);

    if args.get_flag("json") || mainargs.get_flag("json") {
      println!("[");
      let num = deps.len();
      for (i, pkg) in deps.iter().enumerate() {
          let mut cst: String = "".to_string();
          for (i, cs) in pkg.constraints.iter().enumerate() {
            if i > 0 {
              cst += ", ";
            }
            cst += &format!("{} {}", cs.0, cs.1);
          }
          println!(" {{");
          let comma = if cst == "" { "" } else { ", " };
          // TODO: should this be an array? Probably
          println!("     \"types\": \"{}\",", pkg.types.join(", "));
          println!("     \"package\": \"{}\"{}", pkg.name, comma);
          if cst != "" {
            println!("     \"version\": \"{}\"", cst)
          }
          println!("  }}{}", if i == num - 1 { "" } else { "," });
      }
      println!("]");

    } else {
        let mut tab: Table = Table::new("{:<}   {:<}   {:<}");
        tab.add_row(row!["package", "constraints", "types"]);
        tab.add_heading("------------------------------------------");
        for pkg in deps {
          let mut cst: String = "".to_string();
          for (i, cs) in pkg.constraints.iter().enumerate() {
            if i > 0 {
              cst += ", ";
            }
            cst += &format!("{} {}", cs.0, cs.1);
          }
          tab.add_row(row!(pkg.name, cst, pkg.types.join(", ")));
        }

        print!("{}", tab);
    }

  Ok(())
}

fn parse_deps(deps: &str, dep_type: &str)
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

fn simplify_constraints(deps: Vec<DepVersionSpec>) -> Vec<DepVersionSpec> {
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

fn parse_dep(dep: &str, dep_type: &str)
            -> Result<DepVersionSpec, Box<dyn Error>> {
    let parts: Vec<&str> = dep.split_whitespace().collect();
    if parts.len() == 0 || parts[0].len() == 0 {
        bail!("Invalid dependency version: {}", dep);
    }
    let name = parts[0].to_string();
    let types: Vec<String> =
      vec![dep_type.to_string()];
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
    Ok(DepVersionSpec { name, types, constraints })
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
