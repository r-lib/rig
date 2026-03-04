use std::error::Error;
use std::path::PathBuf;

use directories::ProjectDirs;

/// Get the project cache directory
///
/// Returns the cache directory for the rig application.
/// This is used for storing temporary data like downloaded packages.
pub fn get_cache_dir() -> Result<PathBuf, Box<dyn Error>> {
    let cache_dir = ProjectDirs::from("com", "gaborcsardi", "rig")
        .ok_or("Cannot determine cache directory")?
        .cache_dir()
        .to_path_buf();
    Ok(cache_dir)
}

/// Get the project data directory
///
/// Returns the data directory for the rig application.
/// This is used for storing persistent application data like configuration files.
pub fn get_data_dir() -> Result<PathBuf, Box<dyn Error>> {
    let data_dir = ProjectDirs::from("com", "gaborcsardi", "rig")
        .ok_or("Cannot determine data directory")?
        .data_dir()
        .to_path_buf();
    Ok(data_dir)
}
