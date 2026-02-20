use std::error::Error;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use deb822_fast::Deb822;
use directories::ProjectDirs;
use flate2::read::GzDecoder;
use log::info;
use xz2::read::XzDecoder;

use crate::dcf::*;
use crate::download::download_if_newer_;
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
        // Parse DCF file and save to bitcode
        let packages = parse_packages_from_dcf(&repo_local)?;
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
                // Bitcode file doesn't exist or is corrupted, parse DCF
                info!("Bitcode cache not available, parsing DCF file");
                let packages = parse_packages_from_dcf(&repo_local)?;
                save_packages_to_bitcode(&packages, &repo_bitcode)?;
                Ok(packages)
            }
        }
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
    let urlhash = "repo-".to_string() + &calculate_hash(url) + ".dcf.gz";

    cache.push(urlhash);

    Ok(cache)
}
