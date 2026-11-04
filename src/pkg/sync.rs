//! `rig pkg sync`: install the packages of a lock file into a library.
//!
//! This is `rig proj sync --frozen --inexact`, with a lock file and a library
//! of your choosing, and without the project environment: see
//! [`crate::proj::ProjSyncOptions::pkg_sync`].

use std::error::Error;
use std::path::Path;

use clap::ArgMatches;

#[cfg(target_os = "linux")]
use crate::linux::sc_get_default;
#[cfg(target_os = "macos")]
use crate::macos::sc_get_default;
#[cfg(target_os = "windows")]
use crate::windows::sc_get_default;

use crate::common::{find_installed, sc_get_list_details};
use crate::proj::{proj_sync, PkgSyncR, PkgSyncTarget, ProjSyncOptions};
use crate::rvenv::RPROJ_LOCK_FILE;

pub fn sc_pkg_sync(
    args: &ArgMatches,
    _pkgargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let lock_path = std::path::absolute(
        args.get_one::<String>("lockfile")
            .map(|x| x.as_str())
            .unwrap_or(RPROJ_LOCK_FILE),
    )?;
    // Without `..` components, for the messages. A missing lock file is
    // reported later.
    let lock_path = lock_path.canonicalize().unwrap_or(lock_path);
    // Local packages in the lock file, including the project's own package,
    // are relative to the lock file.
    let root = lock_path.parent().unwrap_or(Path::new(".")).to_path_buf();

    // `--r-version` names an installed R version, like for the other `rig
    // pkg` commands, or else one of the lock file's R versions, which may
    // not be installed yet. Without it, the default R version.
    let (r_install, r_version) = match args.get_one::<String>("r-version") {
        Some(rver) => match find_installed(rver)? {
            Some(name) => (pkg_sync_r(&name, true)?, None),
            None => (None, Some(rver.clone())),
        },
        None => match sc_get_default().ok().flatten() {
            Some(name) => (pkg_sync_r(&name, false)?, None),
            None => (None, None),
        },
    };

    let opts = ProjSyncOptions {
        dev: !args.get_flag("no-dev"),
        groups: args
            .get_many::<String>("group")
            .map(|vs| vs.cloned().collect())
            .unwrap_or_default(),
        all_groups: args.get_flag("all-groups"),
        extras: args
            .get_many::<String>("extra")
            .map(|vs| vs.cloned().collect())
            .unwrap_or_default(),
        all_extras: args.get_flag("all-extras"),
        install_r: !args.get_flag("no-install-r"),
        install_project: !args.get_flag("no-install-project"),
        max_concurrent: args.get_one::<usize>("max-concurrent").copied(),
        r_version,
        platform: args.get_one::<String>("platform").cloned(),
        // Never remove anything from the library, and never lock.
        inexact: true,
        frozen: true,
        dry_run: args.get_flag("dry-run"),
        sysreqs: crate::sysreqs::cli_flag(args),
        pkg_sync: Some(PkgSyncTarget {
            lock_path,
            library: args.get_one::<String>("library").cloned(),
            r_install,
        }),
        ..Default::default()
    };

    proj_sync(&root, &opts, args)
}

/// The installed R version `name`, with its version number. `None` for an
/// installation whose version rig cannot tell.
fn pkg_sync_r(name: &str, explicit: bool) -> Result<Option<PkgSyncR>, Box<dyn Error>> {
    let version = sc_get_list_details()?
        .into_iter()
        .find(|v| v.name == name)
        .and_then(|v| v.version);
    Ok(version.map(|version| PkgSyncR {
        name: name.to_string(),
        version,
        explicit,
    }))
}
