use std::error::Error;
use std::fs::File;
use std::path::PathBuf;

use deb822_fast::Deb822;
use directories::ProjectDirs;
use flate2::read::GzDecoder;
use log::info;

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
    let df = File::open(dcf_path)?;
    let decoder = GzDecoder::new(df);
    info!("Parsing repo metadata from {}", dcf_path.display());
    let desc = Deb822::from_reader(decoder)?;
    info!("Parsed {} packages from repo metadata", desc.len());

    let mut packages: Vec<Package> = vec![];

    for pkg in desc.iter() {
        let name = match pkg.get("Package") {
            Some(n) => n.to_string(),
            None => continue,
        };
        let version = match pkg.get("Version") {
            Some(v) => v.to_string(),
            None => continue,
        };
        let mut dependencies = PackageDependencies::new();

        let dep_types = vec!["Depends", "Imports", "LinkingTo", "Suggests", "Enhances"];
        for dep_type in dep_types {
            if let Some(deps) = pkg.get(dep_type) {
                dependencies.append(&mut PackageDependencies::from_str(deps, dep_type)?);
            }
        }
        dependencies.simplify();
        let path = pkg.get("Path").map(|p| p.to_string());
        let url = pkg.get("URL").map(|u| u.to_string());
        let built = pkg.get("Built")
            .map(|b| DCFBuilt::from_str(b))
            .transpose()?;

        packages.push(Package {
            name,
            version,
            dependencies: dependencies,
            url: url,
            path: path,
            built: built
        });
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
