use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;

use deb822_fast::Deb822;
use directories::ProjectDirs;
use flate2::read::GzDecoder;
use log::info;
use rds2rust::RObject;
use rds2rust::RObject::*;
use rds2rust::VectorData;
use simple_error::bail;
use xz2::read::XzDecoder;

use crate::dcf::*;
use crate::download::download_if_newer_;
use crate::rds::*;
use crate::utils::{calculate_hash, create_parent_dir_if_needed};

pub fn repos_get_packages() -> Result<Vec<Package>, Box<dyn Error>> {
    // TODO: do not hardcode repo URL
    let repo_url = "https://cloud.r-project.org/src/contrib/PACKAGES.gz";
    let repo_local = repo_local_file(repo_url)?;
    let repo_bitcode = repo_bitcode_file(&repo_local)?;

    create_parent_dir_if_needed(&repo_local)?;
    info!("Updating repo metadata from {}", repo_url);
    let dl_status = download_if_newer_(repo_url, &repo_local, None, None)?;

    if dl_status {
        info!("Updated repo metadata at {}", repo_local.display());
        // Parse DCF/RDS file and save to bitcode
        let packages = parse_packages(&repo_local)?;
        save_packages_to_bitcode(&packages, &repo_bitcode)?;
        info!("Saved bitcode cache to {}", repo_bitcode.display());
        Ok(packages)
    } else {
        info!("Repo metadata is up to date at {}", repo_local.display());
        // Try to load from bitcode cache
        match load_packages_from_bitcode(&repo_bitcode) {
            Ok(packages) => {
                info!("Loaded {} packages from bitcode cache", packages.len());
                Ok(packages)
            }
            Err(_) => {
                // Bitcode file doesn't exist or is corrupted, parse DCF/RDS file
                info!("Bitcode cache not available, parsing DCF/RDS file");
                let packages = parse_packages(&repo_local)?;
                save_packages_to_bitcode(&packages, &repo_bitcode)?;
                Ok(packages)
            }
        }
    }
}

fn parse_packages(path: &PathBuf) -> Result<Vec<Package>, Box<dyn Error>> {
    if path.extension().and_then(|s| s.to_str()) == Some("rds") {
        parse_packages_from_rds(path)
    } else {
        parse_packages_from_dcf(path)
    }
}

fn parse_packages_from_dcf(dcf_path: &PathBuf) -> Result<Vec<Package>, Box<dyn Error>> {
    let mut file = File::open(dcf_path)?;

    // Peek at first 6 bytes to check for compression magic numbers
    // gzip: 0x1f, 0x8b (2 bytes)
    // xz: 0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00 (6 bytes: 0xFD, '7', 'z', 'X', 'Z', 0x00)
    let mut magic = [0u8; 6];
    let bytes_read = file.read(&mut magic)?;

    // Rewind to start
    file.seek(SeekFrom::Start(0))?;

    info!("Parsing repo metadata from {}", dcf_path.display());

    let desc = if bytes_read >= 2 && magic[0..2] == [0x1f, 0x8b] {
        // Gzip compressed
        let decoder = GzDecoder::new(file);
        Deb822::from_reader(decoder)?
    } else if bytes_read >= 6 && magic == [0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00] {
        // XZ compressed
        let decoder = XzDecoder::new(file);
        Deb822::from_reader(decoder)?
    } else {
        // Uncompressed
        Deb822::from_reader(file)?
    };

    info!("Parsed {} packages from repo metadata", desc.len());

    let mut packages: Vec<Package> = vec![];

    for pkg in desc.iter() {
        packages.push(Package::from_dcf_paragraph(pkg)?);
    }

    Ok(packages)
}

pub fn parse_packages_from_rds(rds_path: &PathBuf) -> Result<Vec<Package>, Box<dyn Error>> {
    let robj = read_rds(rds_path)?;
    let (data, attr) = match robj {
        WithAttributes { object, attributes } => (object, attributes),
        _ => bail!("Expected R object with attributes when reading PACKAGES.rds"),
    };

    let data = match *data {
        Character(vd) => {
            if let VectorData::Owned(v) = vd {
                v
            } else {
                bail!("Expected data to be owned character vector in PACKAGES.rds")
            }
        }
        _ => bail!("Expected data to be a character vector in PACKAGES.rds"),
    };

    let dim = attr
        .get("dim")
        .ok_or("Missing 'dim' attribute in PACKAGES.rds")?;
    let dim = match dim {
        Integer(vd) => {
            if let VectorData::Owned(v) = vd {
                if let [nrow, ncol] = &v[..] {
                    (*nrow as usize, *ncol as usize)
                } else {
                    bail!("Expected 'dim' to have length 2 in PACKAGES.rds")
                }
            } else {
                bail!("Expected 'dim' to be owned integer vector in PACKAGES.rds")
            }
        }
        _ => bail!("Expected 'dim' to be an integer vector in PACKAGES.rds"),
    };
    let dimnames = attr
        .get("dimnames")
        .ok_or("Missing 'dimnames' attribute in PACKAGES.rds")?;
    let names = match dimnames {
        RObject::List(dn) => {
            if dn.len() != 2 {
                bail!("Expected 'dimnames' to have length 2 in PACKAGES.rds")
            }
            if let Character(vd) = &dn[1] {
                if let VectorData::Owned(v) = vd {
                    v
                } else {
                    bail!("Expected 'dimnames' second element to be owned character vector in PACKAGES.rds")
                }
            } else {
                bail!("Expected 'dimnames' second element to be character vector in PACKAGES.rds")
            }
        }
        _ => bail!("Expected 'dimnames' to be a list in PACKAGES.rds"),
    };
    let mut col_idx = HashMap::new();
    for (idx, nm) in names.iter().enumerate() {
        col_idx.insert(nm.clone(), idx);
    }
    let selected_col_names = vec![
        "Package",
        "Version",
        "Depends",
        "Imports",
        "Suggests",
        "Enhances",
        "LinkingTo",
        "File",
        "Path",
        "DownloadURL",
        "Built",
        "License",
        "Platform",
        "Arch",
        "GraphicsAPIVersion",
        "InternalsID",
        "Filesize",
    ];
    let mut cols: HashMap<&str, Vec<Arc<str>>> = HashMap::new();
    let nacol: Vec<Arc<str>> = vec!["NA".into(); dim.0];
    for nm in selected_col_names.iter() {
        let idx = col_idx.get(*nm);
        let col = match idx {
            Some(i) => {
                let start = i * dim.0;
                let end = start + dim.0;
                data[start..end].to_vec()
            }
            None => nacol.clone(),
        };
        cols.insert(*nm, col);
    }

    fn na_to_none(s: &str) -> Option<String> {
        if s == "NA" {
            None
        } else {
            Some(s.to_string())
        }
    }

    let mut packages: Vec<Package> = vec![];
    for i in 0..dim.0 {
        let mut dependencies = PackageDependencies::new();
        for dep_type in RDepType::all() {
            let dep_type_str = dep_type.to_string();
            let dep_str = cols.get(dep_type_str.as_str()).unwrap()[i].clone();
            if dep_str != "NA".into() {
                dependencies.append(&mut PackageDependencies::from_str(&dep_str, &dep_type_str)?);
            }
        }
        let name = cols.get("Package").unwrap()[i].clone();
        let version = RPackageVersion::from_str(&cols.get("Version").unwrap()[i])?;
        let file = cols.get("File").unwrap()[i].clone();
        let path = cols.get("Path").unwrap()[i].clone();
        let download_url = cols.get("DownloadURL").unwrap()[i].clone();
        let built = cols.get("Built").unwrap()[i].clone();
        let license = cols.get("License").unwrap()[i].clone();
        let platform = cols.get("Platform").unwrap()[i].clone();
        let arch = cols.get("Arch").unwrap()[i].clone();
        let graphics_api_version = cols.get("GraphicsAPIVersion").unwrap()[i].clone();
        let internals_id = cols.get("InternalsID").unwrap()[i].clone();
        let filesize = cols.get("Filesize").unwrap()[i].clone();

        let pkg = Package {
            name: name.to_string(),
            version,
            dependencies,
            file: na_to_none(&file),
            path: na_to_none(&path),
            download_url: na_to_none(&download_url),
            built: na_to_none(&built)
                .map(|b| DCFBuilt::from_str(&b))
                .transpose()?,
            license: na_to_none(&license),
            platform: na_to_none(&platform),
            arch: na_to_none(&arch),
            graphics_api_version: na_to_none(&graphics_api_version),
            internals_id: na_to_none(&internals_id),
            filesize: na_to_none(&filesize).and_then(|s| s.parse::<u64>().ok()),
        };
        packages.push(pkg);
    }

    Ok(packages)
}

fn load_packages_from_bitcode(bitcode_path: &PathBuf) -> Result<Vec<Package>, Box<dyn Error>> {
    let bytes = std::fs::read(bitcode_path)?;
    let packages: Vec<Package> = bitcode::decode(&bytes)?;
    Ok(packages)
}

fn save_packages_to_bitcode(
    packages: &Vec<Package>,
    bitcode_path: &PathBuf,
) -> Result<(), Box<dyn Error>> {
    let bytes = bitcode::encode(packages);
    std::fs::write(bitcode_path, bytes)?;
    Ok(())
}

fn repo_bitcode_file(dcf_path: &PathBuf) -> Result<PathBuf, Box<dyn Error>> {
    let mut bitcode_path = dcf_path.clone();
    bitcode_path.set_extension("bitcode");
    Ok(bitcode_path)
}

fn repo_local_file(url: &str) -> Result<PathBuf, Box<dyn Error>> {
    let mut cache = ProjectDirs::from("com", "gaborcsardi", "rig")
        .ok_or("Cannot determine cache directory")?
        .cache_dir()
        .to_path_buf();
    let extension = if url.ends_with(".rds") {
        ".rds"
    } else if url.ends_with(".gz") {
        ".gz"
    } else if url.ends_with(".xz") {
        ".xz"
    } else {
        ""
    };
    let urlhash = "repo-".to_string() + &calculate_hash(url) + extension;

    cache.push(urlhash);

    Ok(cache)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_packages_from_rds_src() {
        let path = PathBuf::from("tests/fixtures/cran-metadata/src/PACKAGES.rds");
        let result = parse_packages_from_rds(&path);

        assert!(
            result.is_ok(),
            "Failed to parse PACKAGES.rds: {:?}",
            result.err()
        );

        let packages = result.unwrap();
        assert!(packages.len() > 0, "Expected at least one package");

        // Snapshot test the parsed packages
        insta::assert_debug_snapshot!(packages);
    }

    #[test]
    fn test_parse_packages_from_rds_binary() {
        let path =
            PathBuf::from("tests/fixtures/cran-metadata/bin/macosx/sonoma-arm64/PACKAGES.rds");
        let result = parse_packages_from_rds(&path);

        assert!(
            result.is_ok(),
            "Failed to parse binary PACKAGES.rds: {:?}",
            result.err()
        );

        let packages = result.unwrap();
        assert!(packages.len() > 0, "Expected at least one package");

        // Snapshot test the parsed binary packages
        insta::assert_debug_snapshot!(packages);
    }

    #[test]
    fn test_parse_packages_from_rds_validates_structure() {
        let path = PathBuf::from("tests/fixtures/cran-metadata/src/PACKAGES.rds");
        let result = parse_packages_from_rds(&path);

        assert!(result.is_ok());
        let packages = result.unwrap();

        // Validate structure and snapshot the first package
        let first_pkg = &packages[0];

        // Name and version are required
        assert!(!first_pkg.name.is_empty());
        assert!(first_pkg.version.to_string().len() > 0);

        // Snapshot the first package to validate its structure
        insta::assert_debug_snapshot!(first_pkg);
    }
}
