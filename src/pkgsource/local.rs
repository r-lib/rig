//! Reading a `local::<path>` package source: a package directory, a source
//! tarball, or a built binary package file that is already on this machine.
//!
//! Nothing is downloaded and nothing is cached. Resolving a local dependency
//! only needs its `DESCRIPTION`, which is read out of the directory, or out of
//! a temporary extraction of the archive; the install itself then points
//! `R CMD INSTALL` (or, for a binary, rig's unpacker) at the original path.

use std::error::Error;
use std::path::{Path, PathBuf};

use simple_error::{bail, SimpleError};

/// What a local path holds: everything the solver and the installer need to
/// know about it before anything is installed.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalPackageFiles {
    /// The `DESCRIPTION` contents.
    pub description: String,
    /// Whether this is a built binary package rather than a source directory
    /// or source tarball -- a binary's `DESCRIPTION` has a `Built:` field.
    pub binary: bool,
}

/// Turn a path as the user wrote it into an absolute path that exists.
///
/// `~` is expanded here rather than left to the shell, since a quoted
/// `'~/pkg'` (and a path that came from a config file) never reaches one.
pub fn resolve_local_path(path: &str) -> Result<PathBuf, Box<dyn Error>> {
    let expanded = expand_tilde(path);

    // Canonicalized here and not at install time, because a relative path is
    // relative to rig's working directory, and `R CMD INSTALL` is run from a
    // throwaway one (see `crate::install::r_cmd_install`).
    expanded.canonicalize().map_err(|err| {
        SimpleError::new(format!(
            "Cannot use local package at `{}`: {}",
            expanded.display(),
            err
        ))
        .into()
    })
}

/// `~` or `~/...` relative to the user's home directory. Anything else, and a
/// `~user/...` form rig cannot resolve, is left alone.
fn expand_tilde(path: &str) -> PathBuf {
    let home = if cfg!(target_os = "windows") {
        std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME"))
    } else {
        std::env::var("HOME")
    };
    let Ok(home) = home else {
        return PathBuf::from(path);
    };

    if path == "~" {
        return PathBuf::from(home);
    }
    for prefix in ["~/", "~\\"] {
        if let Some(rest) = path.strip_prefix(prefix) {
            return PathBuf::from(home).join(rest);
        }
    }

    PathBuf::from(path)
}

/// Read a local package's `DESCRIPTION`, whether `path` is a package source
/// directory or a package file.
///
/// A package file is extracted into a temporary directory that is thrown away
/// again right after: the archive itself is what gets installed later, so
/// there is nothing worth keeping. [`crate::pkgsource::url::locate_package_dir`]
/// finds the package inside it, the same way a `url::` source's archive is
/// handled.
pub fn read_local_package_files(path: &Path) -> Result<LocalPackageFiles, Box<dyn Error>> {
    let description = if path.is_dir() {
        std::fs::read_to_string(path.join("DESCRIPTION")).map_err(|err| {
            SimpleError::new(format!(
                "Cannot read DESCRIPTION from `{}`: {}. Is it an R package?",
                path.display(),
                err
            ))
        })?
    } else if is_package_file(path) {
        read_description_from_archive(path)?
    } else {
        bail!(
            "`{}` is not an R package directory or package file (`.tar.gz`, \
             `.tgz`, `.zip`)",
            path.display()
        );
    };

    // `Built:` is written by `R CMD INSTALL`, so only a built binary package
    // has one; a source directory or source tarball never does.
    let binary = crate::proj::parse_description_paragraph(description.as_bytes())?
        .get("Built")
        .is_some();

    Ok(LocalPackageFiles {
        description,
        binary,
    })
}

fn is_package_file(path: &Path) -> bool {
    path.is_file()
        && path
            .to_str()
            .map(crate::pkgsource::is_package_file_name)
            .unwrap_or(false)
}

fn read_description_from_archive(path: &Path) -> Result<String, Box<dyn Error>> {
    let extract_dir = tempfile::tempdir()?;
    crate::install::unpack_package(path, extract_dir.path())
        .map_err(|err| SimpleError::new(format!("Cannot extract `{}`: {}", path.display(), err)))?;

    let (package_dir, _subdir) =
        crate::pkgsource::url::locate_package_dir(extract_dir.path(), None);

    let description = std::fs::read_to_string(package_dir.join("DESCRIPTION")).map_err(|err| {
        SimpleError::new(format!(
            "Cannot read DESCRIPTION from `{}`: {}. Is it an R package file?",
            path.display(),
            err
        ))
    })?;

    Ok(description)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_tar_gz(dest: &Path, entries: &[(&str, &[u8])]) {
        let file = std::fs::File::create(dest).unwrap();
        let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut ar = tar::Builder::new(enc);
        for (name, contents) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            ar.append_data(&mut header, name, *contents).unwrap();
        }
        ar.finish().unwrap();
    }

    #[test]
    fn reads_a_package_directory() {
        let dir = tempfile::tempdir().unwrap();
        let pkg = dir.path().join("mypkg");
        std::fs::create_dir(&pkg).unwrap();
        std::fs::write(pkg.join("DESCRIPTION"), "Package: mypkg\nVersion: 1.0.0\n").unwrap();

        let files = read_local_package_files(&pkg).unwrap();
        assert!(files.description.contains("Package: mypkg"));
        assert!(!files.binary);
    }

    #[test]
    fn a_directory_without_a_description_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = read_local_package_files(dir.path())
            .unwrap_err()
            .to_string();
        assert!(err.contains("Cannot read DESCRIPTION"), "{}", err);
    }

    #[test]
    fn reads_a_source_tarball() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("mypkg_1.0.0.tar.gz");
        write_tar_gz(
            &archive,
            &[("mypkg/DESCRIPTION", b"Package: mypkg\nVersion: 1.0.0\n")],
        );

        let files = read_local_package_files(&archive).unwrap();
        assert!(files.description.contains("Package: mypkg"));
        assert!(!files.binary);
    }

    #[test]
    fn a_built_field_marks_an_archive_as_binary() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("mypkg_1.0.0.tgz");
        write_tar_gz(
            &archive,
            &[(
                "mypkg/DESCRIPTION",
                b"Package: mypkg\nVersion: 1.0.0\nBuilt: R 4.5.1; aarch64-apple-darwin20; 2025-01-01 00:00:00 UTC; unix\n",
            )],
        );

        let files = read_local_package_files(&archive).unwrap();
        assert!(files.binary);
    }

    #[test]
    fn a_file_that_is_not_a_package_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes.txt");
        std::fs::write(&file, "hello").unwrap();

        let err = read_local_package_files(&file).unwrap_err().to_string();
        assert!(err.contains("not an R package directory"), "{}", err);
    }

    #[test]
    fn a_missing_path_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope");
        let err = resolve_local_path(&missing.display().to_string())
            .unwrap_err()
            .to_string();
        assert!(err.contains("Cannot use local package"), "{}", err);
    }

    #[test]
    fn a_tilde_path_expands_to_the_home_directory() {
        let home = match std::env::var("HOME") {
            Ok(home) => home,
            Err(_) => return,
        };
        assert_eq!(expand_tilde("~"), PathBuf::from(&home));
        assert_eq!(expand_tilde("~/pkg"), PathBuf::from(&home).join("pkg"));
        assert_eq!(expand_tilde("./pkg"), PathBuf::from("./pkg"));
    }
}
