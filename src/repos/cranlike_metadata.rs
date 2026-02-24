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
use rusqlite::{params, Connection};
use simple_error::bail;
use xz2::read::XzDecoder;

use crate::dcf::*;
use crate::download::download_first_available_;
use crate::rds::*;
use crate::utils::{calculate_hash, create_parent_dir_if_needed};

pub fn repos_get_packages(repo_url: &str) -> Result<Vec<Package>, Box<dyn Error>> {
    let repo_url_gz = repo_url.to_string() + "/PACKAGES.gz";
    let repo_url_rds = repo_url.to_string() + "/PACKAGES.rds";
    let repo_url_plain = repo_url.to_string() + "/PACKAGES";
    let repo_urls: Vec<&str> = vec![
        repo_url_gz.as_str(),
        repo_url_rds.as_str(),
        repo_url_plain.as_str(),
    ];
    let repo_local = repo_local_file(repo_url)?;
    let repo_db = repo_db_file(&repo_local)?;

    create_parent_dir_if_needed(&repo_local)?;
    info!("Updating repo metadata from {}", repo_url);
    let dl_status = download_first_available_(&repo_urls, &repo_local, None, None)?;

    if dl_status {
        info!("Updated repo metadata at {}", repo_local.display());
        // Parse DCF/RDS file and save to database
        let packages = parse_packages(&repo_local)?;
        save_packages_to_db(&packages, &repo_db)?;
        info!("Saved database cache to {}", repo_db.display());
        Ok(packages)
    } else {
        info!("Repo metadata is up to date at {}", repo_local.display());
        // Try to load from database cache
        match load_packages_from_db(&repo_db) {
            Ok(packages) => {
                info!("Loaded {} packages from database cache", packages.len());
                Ok(packages)
            }
            Err(_) => {
                // Database file doesn't exist or is corrupted, parse DCF/RDS file
                info!("Database cache not available, parsing DCF/RDS file");
                let packages = parse_packages(&repo_local)?;
                save_packages_to_db(&packages, &repo_db)?;
                Ok(packages)
            }
        }
    }
}

fn parse_packages(dcf_path: &PathBuf) -> Result<Vec<Package>, Box<dyn Error>> {
    let mut file = File::open(dcf_path)?;

    // Peek at first 6 bytes to check for compression magic numbers
    // gzip: 0x1f, 0x8b (2 bytes)
    // xz: 0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00 (6 bytes: 0xFD, '7', 'z', 'X', 'Z', 0x00)
    let mut magic = [0u8; 6];
    let bytes_read = file.read(&mut magic)?;

    // Rewind to start
    file.seek(SeekFrom::Start(0))?;

    info!("Parsing repo metadata from {}", dcf_path.display());

    // Decompress if needed and read into memory to check format
    let data: Vec<u8> = if bytes_read >= 2 && magic[0..2] == [0x1f, 0x8b] {
        // Gzip compressed
        let mut decoder = GzDecoder::new(file);
        let mut data = Vec::new();
        decoder.read_to_end(&mut data)?;
        data
    } else if bytes_read >= 6 && magic == [0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00] {
        // XZ compressed
        let mut decoder = XzDecoder::new(file);
        let mut data = Vec::new();
        decoder.read_to_end(&mut data)?;
        data
    } else {
        // Uncompressed - read entire file
        let mut data = Vec::new();
        file.read_to_end(&mut data)?;
        data
    };

    // Check if decompressed data is RDS format
    // RDS files start with: 0x58 0x00 (X), 0x41 0x00 (A), or 0x42 0x00 (B)
    if data.len() >= 2 {
        let is_rds = (data[0] == 0x58 && data[1] == 0x00)  // X format
                  || (data[0] == 0x41 && data[1] == 0x00)  // A format
                  || (data[0] == 0x42 && data[1] == 0x00); // B format

        if is_rds {
            info!("Detected RDS format, parsing as RDS");
            let robj = read_rds(&data)?;
            return parse_packages_from_rds_object(robj);
        }
    }

    // Parse as DCF format
    info!("Parsing as DCF format");
    let desc = Deb822::from_reader(&data[..])?;
    info!("Parsed {} packages from repo metadata", desc.len());

    let mut packages: Vec<Package> = vec![];

    for pkg in desc.iter() {
        packages.push(Package::from_dcf_paragraph(pkg)?);
    }

    Ok(packages)
}

fn parse_packages_from_rds_object(robj: RObject) -> Result<Vec<Package>, Box<dyn Error>> {
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

pub fn parse_packages_from_rds(rds_path: &PathBuf) -> Result<Vec<Package>, Box<dyn Error>> {
    let robj = read_rds_file(rds_path)?;
    parse_packages_from_rds_object(robj)
}

fn load_packages_from_db(db_path: &PathBuf) -> Result<Vec<Package>, Box<dyn Error>> {
    let conn = Connection::open(db_path)?;

    let mut stmt = conn.prepare(
        "SELECT name, version, dependencies, download_url, file, path, built,
                license, platform, arch, graphics_api_version, internals_id, filesize
         FROM packages"
    )?;

    let packages = stmt.query_map([], |row| {
        Ok(Package {
            name: row.get(0)?,
            version: RPackageVersion::from_str(&row.get::<_, String>(1)?).unwrap(),
            dependencies: serde_json::from_str(&row.get::<_, String>(2)?).unwrap(),
            download_url: row.get(3)?,
            file: row.get(4)?,
            path: row.get(5)?,
            built: row.get::<_, Option<String>>(6)?
                .map(|s| serde_json::from_str(&s).ok())
                .flatten(),
            license: row.get(7)?,
            platform: row.get(8)?,
            arch: row.get(9)?,
            graphics_api_version: row.get(10)?,
            internals_id: row.get(11)?,
            filesize: row.get(12)?,
        })
    })?
    .collect::<Result<Vec<_>, _>>()?;

    Ok(packages)
}

fn save_packages_to_db(
    packages: &Vec<Package>,
    db_path: &PathBuf,
) -> Result<(), Box<dyn Error>> {
    let conn = Connection::open(db_path)?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS packages (
            name TEXT NOT NULL,
            version TEXT NOT NULL,
            dependencies TEXT NOT NULL,
            download_url TEXT,
            file TEXT,
            path TEXT,
            built TEXT,
            license TEXT,
            platform TEXT,
            arch TEXT,
            graphics_api_version TEXT,
            internals_id TEXT,
            filesize INTEGER
        )",
        [],
    )?;

    // Clear existing data
    conn.execute("DELETE FROM packages", [])?;

    // Insert packages
    let mut stmt = conn.prepare(
        "INSERT INTO packages
         (name, version, dependencies, download_url, file, path, built,
          license, platform, arch, graphics_api_version, internals_id, filesize)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"
    )?;

    for pkg in packages {
        let deps_json = serde_json::to_string(&pkg.dependencies)?;
        let built_json = pkg.built.as_ref()
            .map(|b| serde_json::to_string(b).ok())
            .flatten();

        stmt.execute(params![
            &pkg.name,
            pkg.version.to_string(),
            deps_json,
            &pkg.download_url,
            &pkg.file,
            &pkg.path,
            built_json,
            &pkg.license,
            &pkg.platform,
            &pkg.arch,
            &pkg.graphics_api_version,
            &pkg.internals_id,
            pkg.filesize,
        ])?;
    }

    Ok(())
}

fn repo_db_file(dcf_path: &PathBuf) -> Result<PathBuf, Box<dyn Error>> {
    let mut db_path = dcf_path.clone();
    db_path.set_extension("db");
    Ok(db_path)
}

fn repo_local_file(url: &str) -> Result<PathBuf, Box<dyn Error>> {
    let mut cache = ProjectDirs::from("com", "gaborcsardi", "rig")
        .ok_or("Cannot determine cache directory")?
        .cache_dir()
        .to_path_buf();
    let urlhash = "repo-".to_string() + &calculate_hash(url) + ".data";

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
