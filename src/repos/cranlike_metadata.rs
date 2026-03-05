use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;

use deb822_fast::Deb822;
use flate2::read::GzDecoder;
use log::info;
use rds2rust::RObject;
use rds2rust::RObject::*;
use rds2rust::VectorData;
use rusqlite::{params, Connection};
use simple_error::bail;
use xz2::read::XzDecoder;

use crate::cache::get_cache_dir;
use crate::dcf::*;
use crate::download::download_first_available_;
use crate::rds::*;
use crate::utils::{calculate_hash, create_parent_dir_if_needed};

fn package_type_to_path(pkg_type: &str, r_version: &str) -> Result<String, Box<dyn Error>> {
    use regex::Regex;

    if pkg_type == "source" {
        return Ok("src/contrib".to_string());
    }

    // Pattern: ^([[:lower:]]+)[.]binary(|[.]([[:alnum:]_-]+))$
    // In Rust regex: ^([a-z]+)\.binary(|\.([a-zA-Z0-9_-]+))$
    let re = Regex::new(r"^([a-z]+)\.binary(|\.([a-zA-Z0-9_-]+))$")?;

    if let Some(caps) = re.captures(pkg_type) {
        let os_raw = caps.get(1).map(|m| m.as_str()).unwrap_or("");

        // Switch "mac" -> "macosx", "win" -> "windows"
        let os = match os_raw {
            "mac" => "macosx",
            "win" => "windows",
            other => other,
        };

        // Check if there's a subtype (group 3)
        if let Some(subtype) = caps.get(3) {
            // bin/{os}/{subtype}/contrib/{ver}
            Ok(format!(
                "bin/{}/{}/contrib/{}",
                os,
                subtype.as_str(),
                r_version
            ))
        } else {
            // bin/{os}/contrib/{ver}
            Ok(format!("bin/{}/contrib/{}", os, r_version))
        }
    } else {
        bail!("invalid 'type': {}", pkg_type)
    }
}

fn minor_r_version(r_version: &str) -> Result<String, Box<dyn Error>> {
    // If version has only 2 parts (e.g., "4.3"), append ".0" for semver parsing
    let version_str = if r_version.matches('.').count() == 1 {
        format!("{}.0", r_version)
    } else {
        r_version.to_string()
    };

    let version = semver::Version::parse(&version_str)
        .map_err(|e| format!("Invalid R version format '{}': {}", r_version, e))?;
    Ok(format!("{}.{}", version.major, version.minor))
}

pub fn repos_get_packages(
    repo_url: &str,
    pkg_type: &str,
    r_version: &str,
) -> Result<Vec<Package>, Box<dyn Error>> {
    let r_version = minor_r_version(r_version)?;
    let path = package_type_to_path(pkg_type, &r_version)?;
    let repo_url_gz = repo_url.to_string() + "/" + &path + "/PACKAGES.gz";
    let repo_url_rds = repo_url.to_string() + "/" + &path + "/PACKAGES.rds";
    let repo_url_plain = repo_url.to_string() + "/" + &path + "/PACKAGES";
    let repo_urls: Vec<&str> = vec![
        repo_url_gz.as_str(),
        repo_url_rds.as_str(),
        repo_url_plain.as_str(),
    ];

    // Use a temporary file for downloads (will be deleted after parsing)
    let repo_local = repo_local_file(&repo_url_plain)?;
    let repo_db = repo_db_file(&repo_local)?;

    // Ensure database schema exists early
    ensure_db_schema(&repo_db)?;

    // Check if we have recent data in the database (less than 24 hours old)
    let should_download = match is_repo_cache_recent(&repo_db, repo_url, pkg_type) {
        Ok(is_recent) => {
            if is_recent {
                info!("Database cache is recent (less than 24 hours old), skipping download");
            }
            !is_recent
        }
        Err(_) => {
            // No database entry, need to download
            info!("No database cache found, will download");
            true
        }
    };

    let (dl_status, new_etag) = if should_download {
        create_parent_dir_if_needed(&repo_local)?;
        info!("Checking for repo metadata updates from {}", repo_url_plain);

        // Download with etag (will return false if 304 Not Modified or file is cached)
        download_first_available_(&repo_urls, &repo_local, None, None)?
    } else {
        // Skip download, database is recent
        (false, None)
    };

    if dl_status {
        info!("Downloaded new repo metadata to {}", repo_local.display());
        // Parse DCF/RDS file and save to database
        let packages = parse_packages(&repo_local)?;

        // Save to database with the etag from the download
        save_packages_to_db(
            &packages,
            &repo_db,
            repo_url,
            Some(&r_version),
            pkg_type,
            &path,
            new_etag.as_deref(),
        )?;

        // Delete the temporary data file after saving to database
        if let Err(e) = std::fs::remove_file(&repo_local) {
            info!(
                "Could not delete temporary file {}: {}",
                repo_local.display(),
                e
            );
        }

        info!("Saved {} packages to database cache", packages.len());
        Ok(packages)
    } else {
        info!("Repo metadata is up to date (cached)");
        // Load from database cache
        match load_packages_from_db(&repo_db, repo_url, pkg_type) {
            Ok(packages) => {
                info!("Loaded {} packages from database cache", packages.len());
                Ok(packages)
            }
            Err(e) => {
                // Database file doesn't exist or is corrupted
                // If we have a cached file, try parsing it
                if repo_local.exists() {
                    info!("Database cache error, parsing cached file: {}", e);
                    let packages = parse_packages(&repo_local)?;
                    save_packages_to_db(
                        &packages,
                        &repo_db,
                        repo_url,
                        Some(&r_version),
                        pkg_type,
                        &path,
                        None,
                    )?;
                    Ok(packages)
                } else {
                    bail!("No cached data available and database error: {}", e)
                }
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

fn ensure_db_schema(db_path: &PathBuf) -> Result<(), Box<dyn Error>> {
    let conn = Connection::open(db_path)?;

    // Create repos table
    conn.execute(
        "CREATE TABLE IF NOT EXISTS repos (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            url TEXT NOT NULL,
            pkg_type TEXT NOT NULL,
            r_version TEXT,
            path TEXT NOT NULL,
            etag TEXT,
            last_updated TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
        )",
        [],
    )?;

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
            filesize INTEGER,
            repo_id INTEGER NOT NULL,
            FOREIGN KEY (repo_id) REFERENCES repos(id)
        )",
        [],
    )?;

    // Create index for fast lookups by name, version, platform, arch
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_packages_lookup
         ON packages (name, version, platform, arch)",
        [],
    )?;

    // Create index for fast lookups by repo_id
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_packages_repo_id
         ON packages (repo_id)",
        [],
    )?;

    Ok(())
}

/// Get the stored etag for a repository from the database
fn get_repo_etag(
    db_path: &PathBuf,
    repo_url: &str,
    pkg_type: &str,
) -> Result<String, Box<dyn Error>> {
    let conn = Connection::open(db_path)?;

    let etag: String = conn.query_row(
        "SELECT etag FROM repos WHERE url = ?1 AND pkg_type = ?2 AND etag IS NOT NULL",
        params![repo_url, pkg_type],
        |row| row.get(0),
    )?;

    Ok(etag)
}

fn is_repo_cache_recent(
    db_path: &PathBuf,
    repo_url: &str,
    pkg_type: &str,
) -> Result<bool, Box<dyn Error>> {
    let conn = Connection::open(db_path)?;

    // Check if last_updated is within the last 24 hours using SQLite's datetime functions
    let is_recent: bool = conn.query_row(
        "SELECT
            CASE
                WHEN (julianday('now') - julianday(last_updated)) * 24 < 24 THEN 1
                ELSE 0
            END as is_recent
         FROM repos
         WHERE url = ?1 AND pkg_type = ?2",
        params![repo_url, pkg_type],
        |row| row.get(0),
    )?;

    Ok(is_recent)
}

fn load_packages_from_db(
    db_path: &PathBuf,
    repo_url: &str,
    pkg_type: &str,
) -> Result<Vec<Package>, Box<dyn Error>> {
    let conn = Connection::open(db_path)?;

    // Get the repo_id for this URL
    let repo_id: i64 = conn.query_row(
        "SELECT id FROM repos WHERE url = ?1 AND pkg_type = ?2",
        params![repo_url, pkg_type],
        |row| row.get(0),
    )?;

    let mut stmt = conn.prepare(
        "SELECT name, version, dependencies, download_url, file, path, built,
                license, platform, arch, graphics_api_version, internals_id, filesize
         FROM packages WHERE repo_id = ?1",
    )?;

    let packages = stmt
        .query_map(params![repo_id], |row| {
            Ok(Package {
                name: row.get(0)?,
                version: RPackageVersion::from_str(&row.get::<_, String>(1)?).unwrap(),
                dependencies: serde_json::from_str(&row.get::<_, String>(2)?).unwrap(),
                download_url: row.get(3)?,
                file: row.get(4)?,
                path: row.get(5)?,
                built: row
                    .get::<_, Option<String>>(6)?
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
    repo_url: &str,
    r_version: Option<&str>,
    pkg_type: &str,
    path: &str,
    etag: Option<&str>,
) -> Result<(), Box<dyn Error>> {
    let mut conn = Connection::open(db_path)?;

    // For source packages, we don't store r_version (use NULL)
    let r_version_to_store = if pkg_type == "source" {
        None
    } else {
        r_version
    };

    // Use a single transaction for all inserts - much faster!
    let tx = conn.transaction()?;

    // Insert or get the repo_id
    tx.execute(
        "INSERT OR IGNORE INTO repos (url, pkg_type, r_version, path, etag) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![repo_url, pkg_type, r_version_to_store, path, etag],
    )?;

    // Update etag and last_updated timestamp for existing repos
    tx.execute(
        "UPDATE repos SET etag = ?1, last_updated = CURRENT_TIMESTAMP
         WHERE url = ?2 AND pkg_type = ?3 AND r_version IS ?4 AND path = ?5",
        params![etag, repo_url, pkg_type, r_version_to_store, path],
    )?;

    let repo_id: i64 = tx.query_row(
        "SELECT id FROM repos WHERE url = ?1 AND pkg_type = ?2 AND r_version IS ?3 AND path = ?4",
        params![repo_url, pkg_type, r_version_to_store, path],
        |row| row.get(0),
    )?;

    // Clear existing data for this repository only
    tx.execute("DELETE FROM packages WHERE repo_id = ?1", params![repo_id])?;

    // Insert packages
    let mut stmt = tx.prepare(
        "INSERT INTO packages
         (name, version, dependencies, download_url, file, path, built,
          license, platform, arch, graphics_api_version, internals_id, filesize, repo_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
    )?;

    for pkg in packages {
        let deps_json = serde_json::to_string(&pkg.dependencies)?;
        let built_json = pkg
            .built
            .as_ref()
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
            repo_id,
        ])?;
    }

    drop(stmt); // Drop statement before committing
    tx.commit()?;

    Ok(())
}

fn repo_db_file(dcf_path: &PathBuf) -> Result<PathBuf, Box<dyn Error>> {
    let parent = dcf_path
        .parent()
        .ok_or("Cannot determine parent directory for database file")?;
    let db_path = parent.join("packages.db");
    Ok(db_path)
}

fn repo_local_file(url: &str) -> Result<PathBuf, Box<dyn Error>> {
    let mut cache = get_cache_dir()?;
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

    #[test]
    fn test_package_type_to_path_source() {
        let result = package_type_to_path("source", "4.3").unwrap();
        assert_eq!(result, "src/contrib");
    }

    #[test]
    fn test_package_type_to_path_mac_binary() {
        let result = package_type_to_path("mac.binary", "4.3").unwrap();
        assert_eq!(result, "bin/macosx/contrib/4.3");
    }

    #[test]
    fn test_package_type_to_path_mac_binary_with_subtype() {
        let result = package_type_to_path("mac.binary.big-sur-arm64", "4.3").unwrap();
        assert_eq!(result, "bin/macosx/big-sur-arm64/contrib/4.3");
    }

    #[test]
    fn test_package_type_to_path_mac_binary_el_capitan() {
        let result = package_type_to_path("mac.binary.el-capitan", "3.6").unwrap();
        assert_eq!(result, "bin/macosx/el-capitan/contrib/3.6");
    }

    #[test]
    fn test_package_type_to_path_win_binary() {
        let result = package_type_to_path("win.binary", "4.3").unwrap();
        assert_eq!(result, "bin/windows/contrib/4.3");
    }

    #[test]
    fn test_package_type_to_path_different_versions() {
        let result1 = package_type_to_path("mac.binary", "4.1").unwrap();
        assert_eq!(result1, "bin/macosx/contrib/4.1");

        let result2 = package_type_to_path("mac.binary", "3.5").unwrap();
        assert_eq!(result2, "bin/macosx/contrib/3.5");
    }

    #[test]
    fn test_package_type_to_path_invalid() {
        let result = package_type_to_path("invalid", "4.3");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid 'type'"));
    }

    #[test]
    fn test_package_type_to_path_no_version_in_source() {
        // Source should work with any version string (it's ignored)
        let result1 = package_type_to_path("source", "4.3").unwrap();
        let result2 = package_type_to_path("source", "3.0").unwrap();
        assert_eq!(result1, result2);
    }

    // Tests for minor_r_version

    #[test]
    fn test_minor_r_version_basic() {
        let result = minor_r_version("4.3.2").unwrap();
        assert_eq!(result, "4.3");
    }

    #[test]
    fn test_minor_r_version_patch_zero() {
        let result = minor_r_version("3.6.0").unwrap();
        assert_eq!(result, "3.6");
    }

    #[test]
    fn test_minor_r_version_different_versions() {
        assert_eq!(minor_r_version("4.1.3").unwrap(), "4.1");
        assert_eq!(minor_r_version("3.5.1").unwrap(), "3.5");
        assert_eq!(minor_r_version("4.0.0").unwrap(), "4.0");
    }

    #[test]
    fn test_minor_r_version_two_parts() {
        // Two-part versions should work (we append .0 internally)
        let result = minor_r_version("4.3").unwrap();
        assert_eq!(result, "4.3");

        // Test multiple two-part versions
        assert_eq!(minor_r_version("3.5").unwrap(), "3.5");
        assert_eq!(minor_r_version("4.0").unwrap(), "4.0");
    }

    #[test]
    fn test_minor_r_version_invalid() {
        let result = minor_r_version("invalid");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid R version format"));
    }

    #[test]
    fn test_minor_r_version_empty() {
        let result = minor_r_version("");
        assert!(result.is_err());
    }
}
