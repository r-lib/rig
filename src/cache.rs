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

/// Get the project logs directory
///
/// Returns the appropriate logs directory for each platform:
/// - macOS: ~/Library/Logs/com.gaborcsardi.rig/
/// - Linux: ~/.cache/rig/logs/
/// - Windows: %LOCALAPPDATA%\gaborcsardi\rig\cache\logs\
pub fn get_logs_dir() -> Result<PathBuf, Box<dyn Error>> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME")
            .map_err(|_| "Cannot determine home directory")?;
        Ok(PathBuf::from(home).join("Library/Logs/com.gaborcsardi.rig"))
    }

    #[cfg(not(target_os = "macos"))]
    {
        // Use cache_dir for Linux/Windows
        let logs_dir = ProjectDirs::from("com", "gaborcsardi", "rig")
            .ok_or("Cannot determine logs directory")?
            .cache_dir()
            .join("logs");
        Ok(logs_dir.to_path_buf())
    }
}
