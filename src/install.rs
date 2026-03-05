use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fs::{remove_file, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use futures::stream::{FuturesUnordered, StreamExt};
use log::{error, info};
use tokio::fs::create_dir_all;
use tokio::process::Command;
use tokio::sync::Mutex;

/// Information about a package to be installed
#[derive(Debug, Clone)]
pub struct PackageInfo {
    /// Package name
    pub name: String,
    /// Path to the package file on disk
    pub file_path: PathBuf,
    /// Names of packages this package depends on
    pub dependencies: Vec<String>,
}

/// Install an R package from a given path into a specified library
///
/// # Arguments
/// * `package_name` - Name of the package being installed
/// * `package_path` - Path to the package tarball (e.g., "package_1.0.0.tar.gz")
/// * `library_path` - Path to the R library directory where the package should be installed
/// * `r_binary` - Path to the R binary to use for installation
///
/// # Returns
/// * `Ok(())` if installation succeeded
/// * `Err` if installation failed
pub async fn install_package(
    package_name: &str,
    package_path: &Path,
    library_path: &Path,
    r_binary: &str,
) -> Result<(), Box<dyn Error>> {
    info!(
        "Installing package {} from {} to {}",
        package_name,
        package_path.display(),
        library_path.display()
    );

    // Create _logs directory and log file
    let logs_dir = library_path.join("_logs");
    create_dir_all(&logs_dir).await?;

    let log_file_path = logs_dir.join(format!("{}-install.log", package_name));
    let log_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&log_file_path)?;

    // Clone the file handle for stderr
    let log_file_stderr = log_file.try_clone()?;

    // Spawn the R CMD INSTALL process with stdout and stderr redirected to the log file
    let status = Command::new(r_binary)
        .arg("CMD")
        .arg("INSTALL")
        .arg("-l")
        .arg(library_path)
        .arg(package_path)
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_file_stderr))
        .status()
        .await?;

    if status.success() {
        // Remove the log file after successful installation
        let _ = remove_file(&log_file_path);
        info!("Successfully installed package {}", package_name);
        Ok(())
    } else {
        Err(format!(
            "Failed to install package {} (see log: {})",
            package_name,
            log_file_path.display()
        )
        .into())
    }
}

/// Install multiple packages respecting dependency order
///
/// Packages are installed concurrently when possible, but dependencies
/// are always installed before packages that depend on them.
///
/// # Arguments
/// * `packages` - List of packages with their file paths and dependencies
/// * `library_path` - Path to the R library directory
/// * `r_binary` - Path to the R binary to use for installation
/// * `max_concurrent` - Maximum number of packages to install concurrently
/// * `progress_callback` - Optional callback called when each package completes installation
///
/// # Returns
/// * `Ok(())` if all installations succeeded
/// * `Err` if any installation failed
pub async fn install_package_tree_with_progress<F>(
    packages: Vec<PackageInfo>,
    library_path: &Path,
    r_binary: &str,
    max_concurrent: usize,
    mut progress_callback: Option<F>,
) -> Result<(), Box<dyn Error>>
where
    F: FnMut(&str, bool),
{
    let package_count = packages.len();
    info!("Installing {} packages in dependency order", package_count);

    // Create a map of package name to package info
    let package_map: Arc<HashMap<String, PackageInfo>> = Arc::new(
        packages
            .into_iter()
            .map(|pkg| (pkg.name.clone(), pkg))
            .collect(),
    );

    // Track installed packages
    let installed = Arc::new(Mutex::new(HashSet::new()));
    // Track failed packages
    let failed = Arc::new(Mutex::new(HashSet::new()));
    // Track packages currently being installed
    let installing = Arc::new(Mutex::new(HashSet::new()));

    // Collection of running installation tasks
    let mut running_tasks = FuturesUnordered::new();

    let library_path = library_path.to_path_buf();
    let r_binary = r_binary.to_string();

    // Helper function to find and start ready packages
    async fn try_start_packages(
        package_map: Arc<HashMap<String, PackageInfo>>,
        installed: Arc<Mutex<HashSet<String>>>,
        failed: Arc<Mutex<HashSet<String>>>,
        installing: Arc<Mutex<HashSet<String>>>,
        library_path: PathBuf,
        r_binary: String,
        max_to_start: usize,
    ) -> Vec<tokio::task::JoinHandle<Result<String, String>>> {
        let installed_set = installed.lock().await.clone();
        let failed_set = failed.lock().await.clone();
        let mut installing_set = installing.lock().await;

        let mut new_tasks = Vec::new();
        let mut started = 0;

        for (name, pkg) in package_map.iter() {
            // Stop if we've reached the maximum number of tasks to start
            if started >= max_to_start {
                break;
            }

            // Skip if already installed, failed, or currently installing
            if installed_set.contains(name)
                || failed_set.contains(name)
                || installing_set.contains(name)
            {
                continue;
            }

            // Check if all dependencies are installed
            let all_deps_installed = pkg
                .dependencies
                .iter()
                .all(|dep| installed_set.contains(dep));

            // Check if any dependency failed
            let any_dep_failed = pkg.dependencies.iter().any(|dep| failed_set.contains(dep));

            if any_dep_failed {
                // Mark as failed if any dependency failed
                drop(installing_set);
                failed.lock().await.insert(name.clone());
                installing_set = installing.lock().await;
                error!("Skipping package {} because a dependency failed", name);
            } else if all_deps_installed {
                // Mark as currently installing
                installing_set.insert(name.clone());
                started += 1;

                let name_clone = name.clone();
                let file_path = pkg.file_path.clone();
                let library_path_clone = library_path.clone();
                let r_binary_clone = r_binary.clone();
                let installed_clone = Arc::clone(&installed);
                let failed_clone = Arc::clone(&failed);
                let installing_clone = Arc::clone(&installing);

                let task = tokio::spawn(async move {
                    // Convert result to String immediately to avoid Send issues
                    let result = match install_package(
                        &name_clone,
                        &file_path,
                        &library_path_clone,
                        &r_binary_clone,
                    )
                    .await
                    {
                        Ok(()) => Ok(()),
                        Err(e) => Err(e.to_string()),
                    };

                    // Remove from installing set
                    installing_clone.lock().await.remove(&name_clone);

                    match result {
                        Ok(()) => {
                            installed_clone.lock().await.insert(name_clone.clone());
                            Ok(name_clone)
                        }
                        Err(err_msg) => {
                            error!("Failed to install {}: {}", name_clone, err_msg);
                            failed_clone.lock().await.insert(name_clone.clone());
                            Err(err_msg)
                        }
                    }
                });

                new_tasks.push(task);
            }
        }

        new_tasks
    }

    // Start initial batch of packages
    let initial_tasks = try_start_packages(
        Arc::clone(&package_map),
        Arc::clone(&installed),
        Arc::clone(&failed),
        Arc::clone(&installing),
        library_path.clone(),
        r_binary.clone(),
        max_concurrent,
    )
    .await;

    for task in initial_tasks {
        running_tasks.push(task);
    }

    // Process tasks as they complete
    while let Some(result) = running_tasks.next().await {
        match result? {
            Ok(pkg_name) => {
                if let Some(ref mut callback) = progress_callback {
                    callback(&pkg_name, true);
                }
            }
            Err(_err_msg) => {
                // Error already logged in the task
                // We can't get the package name easily here since it's in the error context
                // The error will be caught at the end anyway
            }
        }

        // After each completion, check how many tasks are currently running
        let currently_running = installing.lock().await.len();
        let can_start = if currently_running < max_concurrent {
            max_concurrent - currently_running
        } else {
            0
        };

        // Try to start new tasks if we have capacity
        if can_start > 0 {
            let new_tasks = try_start_packages(
                Arc::clone(&package_map),
                Arc::clone(&installed),
                Arc::clone(&failed),
                Arc::clone(&installing),
                library_path.clone(),
                r_binary.clone(),
                can_start,
            )
            .await;

            for task in new_tasks {
                running_tasks.push(task);
            }
        }
    }

    // Check for remaining packages
    let final_installed = installed.lock().await.len();
    let final_failed = failed.lock().await.len();

    if final_installed + final_failed < package_count {
        let installed_set = installed.lock().await.clone();
        let failed_set = failed.lock().await.clone();
        let remaining: Vec<String> = package_map
            .keys()
            .filter(|k| !installed_set.contains(*k) && !failed_set.contains(*k))
            .cloned()
            .collect();

        return Err(format!(
            "Unable to install remaining packages (possible circular dependency): {:?}",
            remaining
        )
        .into());
    }

    if final_failed > 0 {
        return Err(format!(
            "Installation completed with {} successes and {} failures",
            final_installed, final_failed
        )
        .into());
    }

    info!("Successfully installed all {} packages", final_installed);
    Ok(())
}

/// Install multiple packages respecting dependency order (without progress callback)
///
/// This is a convenience wrapper around `install_package_tree_with_progress` that doesn't
/// provide progress callbacks.
pub async fn install_package_tree(
    packages: Vec<PackageInfo>,
    library_path: &Path,
    r_binary: &str,
    max_concurrent: usize,
) -> Result<(), Box<dyn Error>> {
    install_package_tree_with_progress(
        packages,
        library_path,
        r_binary,
        max_concurrent,
        None::<fn(&str, bool)>,
    )
    .await
}
