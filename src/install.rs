use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use futures::stream::{FuturesUnordered, StreamExt};
use log::{debug, error, info};
use simple_error::bail;
use tokio::fs::create_dir_all;
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::output::OUTPUT;

#[derive(Debug, Clone)]
pub struct PackageInfo {
    pub name: String,
    pub file_path: PathBuf,
    pub dependencies: Vec<String>,
}

/// Install an R package from a given path into a specified library
///
/// # Arguments
/// * `package_name` - Name of the package being installed
/// * `package_path` - Path to the package tarball (e.g., "package_1.0.0.tar.gz")
/// * `library_path` - Path to the R library directory where the package should be installed
/// * `r_binary` - Path to the R binary to use for installation
/// * `print_fn` - Optional custom print function (e.g., for progress bars). If None, uses OUTPUT.
///
/// # Returns
/// * `Ok(())` if installation succeeded
/// * `Err` if installation failed
pub async fn install_package<F>(
    package_name: &str,
    package_path: &Path,
    library_path: &Path,
    r_binary: &str,
    print_fn: Option<Arc<F>>,
) -> Result<(), Box<dyn Error>>
where
    F: Fn(&str) + Send + Sync + 'static,
{
    info!(
        "Installing package {} from {} to {}",
        package_name,
        package_path.display(),
        library_path.display()
    );

    let logs_dir = library_path.join("_logs");
    create_dir_all(&logs_dir).await?;

    let log_file_path = logs_dir.join(format!("{}-install.log", package_name));
    let log_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&log_file_path)?;

    let log_file_stderr = log_file.try_clone()?;

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
        // User output: Use custom print function if provided, otherwise use OUTPUT
        if let Some(ref print) = print_fn {
            print(&format!("Installed {}", package_name));
        } else {
            OUTPUT.success(&format!("Installed {}", package_name));
        }

        info!(
            "Successfully installed package {} to {} (log: {})",
            package_name,
            library_path.display(),
            log_file_path.display()
        );

        Ok(())
    } else {
        // User output: Always use OUTPUT for errors (they should be visible)
        OUTPUT.error(&format!(
            "Failed to install {}\n  See log: {}",
            package_name,
            log_file_path.display()
        ));

        error!(
            "Installation failed for {} from {}: exit code {}",
            package_name,
            package_path.display(),
            status.code().unwrap_or(-1)
        );

        bail!("Installation failed for {}", package_name);
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
/// * `print_fn` - Optional print function for success messages (e.g., progress bar's println)
/// * `progress_callback` - Optional callback called when each package completes installation
///
/// # Returns
/// * `Ok(())` if all installations succeeded
/// * `Err` if any installation failed
pub async fn install_package_tree_with_progress<P, F>(
    packages: Vec<PackageInfo>,
    library_path: &Path,
    r_binary: &str,
    max_concurrent: usize,
    print_fn: Option<Arc<P>>,
    mut progress_callback: Option<F>,
) -> Result<(), Box<dyn Error>>
where
    P: Fn(&str) + Send + Sync + 'static,
    F: FnMut(&str, bool),
{
    let package_count = packages.len();

    OUTPUT.status(&format!("Installing {} packages.", package_count));

    info!(
        "Installing {} packages in dependency order with max_concurrent={}",
        package_count, max_concurrent
    );

    let package_map: Arc<HashMap<String, PackageInfo>> = Arc::new(
        packages
            .into_iter()
            .map(|pkg| (pkg.name.clone(), pkg))
            .collect(),
    );

    let installed = Arc::new(Mutex::new(HashSet::new()));
    let failed: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    let installing = Arc::new(Mutex::new(HashSet::new()));

    let mut running_tasks = FuturesUnordered::new();

    let library_path = library_path.to_path_buf();
    let r_binary = r_binary.to_string();

    async fn try_start_packages<P>(
        package_map: Arc<HashMap<String, PackageInfo>>,
        installed: Arc<Mutex<HashSet<String>>>,
        failed: Arc<Mutex<HashSet<String>>>,
        installing: Arc<Mutex<HashSet<String>>>,
        library_path: PathBuf,
        r_binary: String,
        max_to_start: usize,
        print_fn: Option<Arc<P>>,
    ) -> Vec<tokio::task::JoinHandle<Result<String, String>>>
    where
        P: Fn(&str) + Send + Sync + 'static,
    {
        let installed_set = installed.lock().await.clone();
        let failed_set = failed.lock().await.clone();
        let mut installing_set = installing.lock().await;

        let mut new_tasks = Vec::new();
        let mut started = 0;

        for (name, pkg) in package_map.iter() {
            if started >= max_to_start {
                break;
            }

            if installed_set.contains(name)
                || failed_set.contains(name)
                || installing_set.contains(name)
            {
                continue;
            }

            let all_deps_installed = pkg
                .dependencies
                .iter()
                .all(|dep| installed_set.contains(dep));

            let any_dep_failed = pkg.dependencies.iter().any(|dep| failed_set.contains(dep));

            if any_dep_failed {
                drop(installing_set);
                failed.lock().await.insert(name.clone());
                installing_set = installing.lock().await;
                // TODO: can this happen? We quit on the first failure, no?
                error!("Skipping package {} because a dependency failed", name);
            } else if all_deps_installed {
                installing_set.insert(name.clone());
                started += 1;
                let name_clone = name.clone();
                let file_path = pkg.file_path.clone();
                let library_path_clone = library_path.clone();
                let r_binary_clone = r_binary.clone();
                let installed_clone = Arc::clone(&installed);
                let failed_clone = Arc::clone(&failed);
                let installing_clone = Arc::clone(&installing);
                let print_fn_clone = print_fn.clone();

                let task = tokio::spawn(async move {
                    let result = match install_package(
                        &name_clone,
                        &file_path,
                        &library_path_clone,
                        &r_binary_clone,
                        print_fn_clone,
                    )
                    .await
                    {
                        Ok(()) => Ok(()),
                        Err(e) => Err(e.to_string()),
                    };

                    installing_clone.lock().await.remove(&name_clone);

                    match result {
                        Ok(()) => {
                            installed_clone.lock().await.insert(name_clone.clone());
                            Ok(name_clone)
                        }
                        Err(err_msg) => {
                            debug!(
                                "Install task completed with error for {}: {}",
                                name_clone, err_msg
                            );
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

    let initial_tasks = try_start_packages(
        Arc::clone(&package_map),
        Arc::clone(&installed),
        Arc::clone(&failed),
        Arc::clone(&installing),
        library_path.clone(),
        r_binary.clone(),
        max_concurrent,
        print_fn.clone(),
    )
    .await;

    for task in initial_tasks {
        running_tasks.push(task);
    }

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

        let currently_running = installing.lock().await.len();
        let can_start = if currently_running < max_concurrent {
            max_concurrent - currently_running
        } else {
            0
        };

        if can_start > 0 {
            let new_tasks = try_start_packages(
                Arc::clone(&package_map),
                Arc::clone(&installed),
                Arc::clone(&failed),
                Arc::clone(&installing),
                library_path.clone(),
                r_binary.clone(),
                can_start,
                print_fn.clone(),
            )
            .await;

            for task in new_tasks {
                running_tasks.push(task);
            }
        }
    }

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

        let err_msg = format!(
            "Unable to install remaining packages (possible circular dependency): {}",
            remaining.join(", ")
        );

        OUTPUT.error(&err_msg);
        error!("{}: {:?}", err_msg, remaining);

        return Err(err_msg.into());
    }

    if final_failed > 0 {
        let err_msg = format!(
            "Installation completed with {} failures ({}  succeeded)",
            final_failed, final_installed
        );

        OUTPUT.error(&err_msg);
        error!(
            "Installation completed: {} succeeded, {} failed",
            final_installed, final_failed
        );

        return Err(err_msg.into());
    }

    OUTPUT.success(&format!(
        "Installed all {} packages successfully",
        final_installed
    ));

    info!("Successfully installed all {} packages", final_installed);

    Ok(())
}

/// Install multiple packages respecting dependency order (without progress callback)
///
/// This is a convenience wrapper around `install_package_tree_with_progress` that doesn't
/// provide progress callbacks.
#[allow(dead_code)]
pub async fn install_package_tree(
    packages: Vec<PackageInfo>,
    library_path: &Path,
    r_binary: &str,
    max_concurrent: usize,
) -> Result<(), Box<dyn Error>> {
    install_package_tree_with_progress::<fn(&str), _>(
        packages,
        library_path,
        r_binary,
        max_concurrent,
        None,
        None::<fn(&str, bool)>,
    )
    .await
}
