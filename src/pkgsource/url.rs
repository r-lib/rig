//! Fetching a `url::<url>` package source: downloading a direct link to a
//! package archive (`.tar.gz`/`.tgz`/`.zip`), extracting it, and reading its
//! `DESCRIPTION`.
//!
//! Unlike a git dependency, there is no cheap way to read just `DESCRIPTION`
//! out of an arbitrary HTTP resource -- the whole archive has to be
//! downloaded either way, so resolving a `url` dependency
//! ([`fetch_url_description`]) and fetching it for install
//! ([`fetch_url_checkout`]) both start from the same cached download (see
//! [`crate::cache::url_pkg_dir`]), reusing
//! [`crate::download::download_if_newer_`]'s own ETag/mtime freshness check
//! instead of new cache-invalidation logic.

use std::error::Error;
use std::path::{Path, PathBuf};

use simple_error::bail;

/// sha256 of a file's contents, hex-encoded.
fn sha256_file(path: &Path) -> Result<String, Box<dyn Error>> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect())
}

/// The archive file name a `url` dependency's URL implies, for the cached
/// copy's own file name -- so [`crate::install::unpack_package`]'s
/// extension-based zip/tar.gz dispatch still works. Falls back to
/// `archive.tar.gz` when the URL has no usable last path segment.
fn archive_file_name(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    match path.rsplit('/').next().filter(|s| !s.is_empty()) {
        Some(name) => name.to_string(),
        None => "archive.tar.gz".to_string(),
    }
}

/// A downloaded, verified archive: its local path and sha256. Holds on to the
/// throwaway tempdir it lives in, when caching is off, for as long as the
/// archive itself needs to stay on disk.
struct DownloadedArchive {
    path: PathBuf,
    sha256: String,
    _tmp: Option<tempfile::TempDir>,
}

/// Download `url`'s archive into its cache directory (or a throwaway tempdir
/// when caching is off), and verify it against `expected_sha256` if given.
fn download_and_verify(
    url: &str,
    expected_sha256: Option<&str>,
) -> Result<DownloadedArchive, Box<dyn Error>> {
    let cache_dir = crate::cache::url_pkg_dir(url);
    let (dir, tmp) = match cache_dir {
        Some(dir) => (dir, None),
        None => {
            let tmp = tempfile::tempdir()?;
            (tmp.path().to_path_buf(), Some(tmp))
        }
    };
    std::fs::create_dir_all(&dir)?;

    let archive_path = dir.join(archive_file_name(url));
    crate::download::download_if_newer_(url, &archive_path, None, None).map_err(|err| {
        simple_error::SimpleError::new(format!("Cannot download {}: {}", url, err))
    })?;

    let sha256 = sha256_file(&archive_path)?;
    if let Some(expected) = expected_sha256 {
        if !expected.eq_ignore_ascii_case(&sha256) {
            bail!(
                "Checksum mismatch for {}: expected {}, got {}",
                url,
                expected,
                sha256
            );
        }
    }

    Ok(DownloadedArchive {
        path: archive_path,
        sha256,
        _tmp: tmp,
    })
}

/// Where the package lives inside an extracted archive: `<extracted>/<subdir>`
/// when `subdir` is given, otherwise the archive's single top-level directory
/// (the CRAN tarball convention, `pkgname/DESCRIPTION`) if it has one -- that
/// directory's name becomes the effective subdir, returned so the lockfile
/// can record where the package actually lives -- or the archive's root
/// itself, if it has no such single wrapping directory.
pub(crate) fn locate_package_dir(
    extracted: &Path,
    subdir: Option<&str>,
) -> (PathBuf, Option<String>) {
    match subdir {
        Some(s) => (extracted.join(s), Some(s.to_string())),
        None => match crate::install::single_subdir(extracted) {
            Ok(dir) => {
                let name = dir
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                (dir, Some(name))
            }
            Err(_) => (extracted.to_path_buf(), None),
        },
    }
}

/// Fetch a `url` dependency's `DESCRIPTION`, for resolving a dependency
/// (`rig proj lock`/solve). Returns `(DESCRIPTION contents, archive sha256,
/// effective subdir)` -- see [`locate_package_dir`] for what "effective
/// subdir" means.
pub fn fetch_url_description(
    url: &str,
    subdir: Option<&str>,
    expected_sha256: Option<&str>,
) -> Result<(String, String, Option<String>), Box<dyn Error>> {
    let archive = download_and_verify(url, expected_sha256)?;

    let extract_dir = tempfile::tempdir()?;
    crate::install::unpack_package(&archive.path, extract_dir.path()).map_err(|err| {
        simple_error::SimpleError::new(format!("Cannot extract {}: {}", url, err))
    })?;

    let (package_dir, effective_subdir) = locate_package_dir(extract_dir.path(), subdir);

    let description = std::fs::read_to_string(package_dir.join("DESCRIPTION")).map_err(|err| {
        simple_error::SimpleError::new(format!(
            "Cannot read DESCRIPTION from {} (in {}): {}",
            url,
            package_dir.display(),
            err
        ))
    })?;

    Ok((description, archive.sha256, effective_subdir))
}

/// Fetch a `url` dependency's whole archive into `dest`, unmodified -- like
/// [`crate::pkgsource::git::fetch_git_checkout`], `dest` is the archive
/// root, and a subdirectory source lives at `<dest>/<subdir>` within it;
/// callers already know `subdir` from the lockfile (see
/// [`fetch_url_description`]'s return value), so it plays no part here.
/// Returns the archive's sha256.
pub fn fetch_url_checkout(
    url: &str,
    expected_sha256: Option<&str>,
    dest: &Path,
) -> Result<String, Box<dyn Error>> {
    let archive = download_and_verify(url, expected_sha256)?;
    std::fs::create_dir_all(dest)?;
    crate::install::unpack_package(&archive.path, dest).map_err(|err| {
        simple_error::SimpleError::new(format!("Cannot extract {}: {}", url, err))
    })?;
    Ok(archive.sha256)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_file_name_uses_the_last_path_segment() {
        assert_eq!(
            archive_file_name("https://example.com/pkgs/mypkg_1.0.0.tar.gz"),
            "mypkg_1.0.0.tar.gz"
        );
        assert_eq!(
            archive_file_name("https://example.com/download?file=mypkg_1.0.0.zip"),
            "download"
        );
    }

    #[test]
    fn archive_file_name_falls_back_with_no_path_segment() {
        assert_eq!(archive_file_name("https://example.com/"), "archive.tar.gz");
    }

    #[test]
    fn sha256_file_matches_a_known_hash() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), b"hello world").unwrap();
        assert_eq!(
            sha256_file(tmp.path()).unwrap(),
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

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
    fn locates_a_wrapped_package_by_its_single_top_level_directory() {
        let archive_dir = tempfile::tempdir().unwrap();
        let archive = archive_dir.path().join("mypkg_1.0.0.tar.gz");
        write_tar_gz(
            &archive,
            &[("mypkg/DESCRIPTION", b"Package: mypkg\nVersion: 1.0.0\n")],
        );

        let extract_dir = tempfile::tempdir().unwrap();
        crate::install::unpack_package(&archive, extract_dir.path()).unwrap();
        let (package_dir, effective_subdir) = locate_package_dir(extract_dir.path(), None);
        assert_eq!(effective_subdir.as_deref(), Some("mypkg"));
        let description = std::fs::read_to_string(package_dir.join("DESCRIPTION")).unwrap();
        assert!(description.contains("Package: mypkg"));
    }

    #[test]
    fn locates_a_package_at_an_explicit_subdir() {
        let archive_dir = tempfile::tempdir().unwrap();
        let archive = archive_dir.path().join("archive.tar.gz");
        write_tar_gz(
            &archive,
            &[(
                "repo-main/pkgs/mypkg/DESCRIPTION",
                b"Package: mypkg\nVersion: 1.0.0\n",
            )],
        );

        let extract_dir = tempfile::tempdir().unwrap();
        crate::install::unpack_package(&archive, extract_dir.path()).unwrap();
        let (package_dir, effective_subdir) =
            locate_package_dir(extract_dir.path(), Some("repo-main/pkgs/mypkg"));
        assert_eq!(effective_subdir.as_deref(), Some("repo-main/pkgs/mypkg"));
        assert!(package_dir.join("DESCRIPTION").exists());
    }

    #[test]
    fn locates_a_package_at_the_archive_root_when_unwrapped() {
        let archive_dir = tempfile::tempdir().unwrap();
        let archive = archive_dir.path().join("archive.tar.gz");
        write_tar_gz(
            &archive,
            &[
                ("DESCRIPTION", b"Package: mypkg\nVersion: 1.0.0\n"),
                ("R/mypkg.R", b"f <- function() 1\n"),
            ],
        );

        let extract_dir = tempfile::tempdir().unwrap();
        crate::install::unpack_package(&archive, extract_dir.path()).unwrap();
        let (package_dir, effective_subdir) = locate_package_dir(extract_dir.path(), None);
        assert_eq!(effective_subdir, None);
        assert_eq!(package_dir, extract_dir.path());
        assert!(package_dir.join("DESCRIPTION").exists());
    }
}
