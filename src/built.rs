//! The cache of packages rig built itself.
//!
//! Installing a source package means compiling it, which is the expensive part
//! of installing anything at all. The result of that compilation is exactly what
//! a repository would have served as a binary package, so rig keeps it: after a
//! successful `R CMD INSTALL` the installed directory is archived into
//! `<cache>/built`, and the next install of the same build unpacks that archive
//! instead of compiling again.
//!
//! What "the same build" means is the cache key, see [`BuiltCache::path`]. What
//! it does *not* mean is the compiler, the system libraries `configure` found,
//! or the `--configure-args` a package was given; a machine whose toolchain
//! changed under it can hold a stale entry, which is the same bet renv's cache
//! makes.

use std::error::Error;
use std::path::{Path, PathBuf};

use log::debug;
use simple_error::bail;

use crate::cache::get_cache_dir;
use crate::install::{format_linkingto, PackageInfo};
use crate::pak::artifact_cache_key;
use crate::platform::detect_platform;
use crate::repos::cranlike_metadata::minor_r_version;
use crate::rversion::OsVersion;

/// Which archive `R CMD INSTALL --build` produces, which is what rig produces
/// too, down to the file name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BuiltKind {
    /// macOS: `<pkg>_<version>.tgz`.
    Tgz,
    /// Windows: `<pkg>_<version>.zip`.
    Zip,
    /// Every other Unix: `<pkg>_<version>_R_<R_PLATFORM>.tar.gz`.
    Tarball,
}

fn built_kind() -> BuiltKind {
    if cfg!(target_os = "windows") {
        BuiltKind::Zip
    } else if cfg!(target_os = "macos") {
        BuiltKind::Tgz
    } else {
        BuiltKind::Tarball
    }
}

/// Where rig puts the packages it builds, for one machine and one R version.
///
/// Created once per install run, because everything in it except the per-package
/// key is the same for every package: the platform, the R version, the R
/// platform triple that goes into a Linux file name, and the user's `Makevars`.
#[derive(Debug, Clone)]
pub struct BuiltCache {
    /// `<cache>/built/<platform-id>/<r-minor>`.
    dir: PathBuf,
    /// `R_PLATFORM`, for the file name on the platforms that put it there.
    r_platform: Option<String>,
    /// The user's `Makevars` files, folded into every key: they change how a
    /// package is compiled, and they are the one such knob a user actually
    /// turns.
    makevars: String,
    kind: BuiltKind,
}

impl BuiltCache {
    /// The built-package cache for this machine, `r_version` and `r_binary`, or
    /// `None` when there is no usable cache directory.
    ///
    /// Never an error: not caching a build is a missed optimization, not a
    /// failure, so everything that can go wrong here is logged and dropped.
    pub fn new(r_version: &str, r_binary: &str) -> Option<BuiltCache> {
        let cache = match get_cache_dir() {
            Ok(cache) => cache,
            Err(err) => {
                debug!("Not caching built packages, no cache directory: {}", err);
                return None;
            }
        };
        let platform = match detect_platform() {
            Ok(platform) => platform,
            Err(err) => {
                debug!("Not caching built packages, unknown platform: {}", err);
                return None;
            }
        };
        let minor = match minor_r_version(r_version) {
            Ok(minor) => minor,
            Err(err) => {
                debug!("Not caching built packages, unknown R version: {}", err);
                return None;
            }
        };
        Some(BuiltCache {
            dir: cache
                .join("built")
                .join(platform_id(&platform))
                .join(sanitize(&minor)),
            r_platform: r_platform(r_binary),
            makevars: makevars_fingerprint(),
            kind: built_kind(),
        })
    }

    /// Where `pkg`'s built binary is cached, or `None` when the build cannot be
    /// identified.
    ///
    /// The key is the source tarball's sha256, what the package is compiled
    /// against (its `LinkingTo` provenance, which the solve records for source
    /// packages too), and the user's `Makevars`. It is a *directory* component
    /// rather than part of the file name, so that the file name stays exactly
    /// the one `R CMD INSTALL --build` would have given the archive.
    ///
    /// A package with no recorded hash has no entry: without it there is nothing
    /// to tell one build of a version from another.
    pub fn path(&self, pkg: &PackageInfo) -> Option<PathBuf> {
        let ingredients = format!("{}\n{}", format_linkingto(&pkg.linkingto), self.makevars);
        let key = artifact_cache_key(pkg.hash.as_deref(), Some(&ingredients))?;
        Some(self.dir.join(key).join(built_file_name(
            self.kind,
            &pkg.name,
            &pkg.version,
            self.r_platform.as_deref(),
        )))
    }
}

/// A directory name for the platform a build belongs to.
///
/// Not [`crate::repos::binaries::loader::BinaryTarget`], which is P3M's
/// vocabulary and has no name at all for a platform P3M does not build for. A
/// package rig compiled here works here either way.
fn platform_id(platform: &OsVersion) -> String {
    let id = match (&platform.distro, &platform.version) {
        (Some(distro), Some(version)) => format!("{}-{}{}", platform.arch, distro, version),
        _ => format!("{}-{}", platform.arch, platform.os),
    };
    sanitize(&id)
}

/// `s` with everything that is not safe in a path component replaced.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// The file name `R CMD INSTALL --build` gives a built package.
///
/// From R's own `tools:::.install_packages`: a `.tgz` on macOS, a `.zip` on
/// Windows, and `<pkg>_<version>_R_<R_PLATFORM>.tar.gz` everywhere else. Not
/// knowing `R_PLATFORM` only costs the platform part of the name: what matters
/// is that storing and looking up agree, and they both come through here.
fn built_file_name(kind: BuiltKind, name: &str, version: &str, r_platform: Option<&str>) -> String {
    match kind {
        BuiltKind::Zip => format!("{}_{}.zip", name, version),
        BuiltKind::Tgz => format!("{}_{}.tgz", name, version),
        BuiltKind::Tarball => match r_platform {
            Some(platform) => format!("{}_{}_R_{}.tar.gz", name, version, sanitize(platform)),
            None => format!("{}_{}.tar.gz", name, version),
        },
    }
}

/// `R_PLATFORM` for an R installation, without starting R.
///
/// `$R_HOME/etc/Renviron` sets it, as `R_PLATFORM=${R_PLATFORM-'<triple>'}`, and
/// `$R_HOME` is where the `R` binary is: `<R_HOME>/bin/R`.
fn r_platform(r_binary: &str) -> Option<String> {
    let bin = Path::new(r_binary).parent()?;
    let renviron = bin.parent()?.join("etc").join("Renviron");
    let text = std::fs::read_to_string(&renviron).ok()?;
    parse_r_platform(&text)
}

fn parse_r_platform(renviron: &str) -> Option<String> {
    for line in renviron.lines() {
        let rest = match line.trim().strip_prefix("R_PLATFORM=") {
            Some(rest) => rest.trim(),
            None => continue,
        };
        let value = match rest
            .strip_prefix("${R_PLATFORM-")
            .and_then(|x| x.strip_suffix('}'))
        {
            Some(inner) => inner.trim(),
            None => rest,
        };
        let value = crate::utils::unquote(value);
        if !value.is_empty() {
            return Some(value);
        }
    }
    None
}

/// The contents of every `Makevars` file R would read, as one string.
///
/// `~/.R/` holds `Makevars` and its per-platform variants, and
/// `R_MAKEVARS_USER` overrides where they are looked for. All of them change how
/// a package is compiled, so all of them are part of the key; the file names are
/// sorted so that the result does not depend on directory order.
fn makevars_fingerprint() -> String {
    let mut parts: Vec<String> = vec![];

    if let Ok(path) = std::env::var("R_MAKEVARS_USER") {
        if let Ok(text) = std::fs::read_to_string(&path) {
            parts.push(format!("{}\n{}", path, text));
        }
    }

    if let Some(home) = home_dir() {
        let dir = home.join(".R");
        if let Ok(entries) = std::fs::read_dir(&dir) {
            let mut names: Vec<String> = entries
                .filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
                .filter(|name| name.starts_with("Makevars"))
                .collect();
            names.sort();
            for name in names {
                if let Ok(text) = std::fs::read_to_string(dir.join(&name)) {
                    parts.push(format!("{}\n{}", name, text));
                }
            }
        }
    }

    parts.join("\n")
}

fn home_dir() -> Option<PathBuf> {
    let home = if cfg!(target_os = "windows") {
        std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME"))
    } else {
        std::env::var("HOME")
    };
    home.ok().map(PathBuf::from)
}

/// Archive the installed `pkg` into `dest`, as `R CMD INSTALL --build` would.
///
/// The archive holds a single top level directory named after the package, which
/// is both what a built package is and what rig's own binary installer requires,
/// so the result round trips back into a library.
///
/// It is written to a temporary file in `dest`'s directory and renamed into
/// place, so that another rig reading the cache sees either no entry or a
/// complete one. Two rigs building the same package both write, and the last
/// rename wins with the same contents.
///
/// R also writes a sums file into the installed tree before archiving. rig does
/// not: nothing reads it, and writing it would mean modifying an installed
/// package.
pub fn store(pkg: &PackageInfo, library_path: &Path, dest: &Path) -> Result<(), Box<dyn Error>> {
    let source = library_path.join(&pkg.name);
    if !source.join("DESCRIPTION").exists() {
        bail!("{} is not an installed package", source.display());
    }

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut tmp = dest.as_os_str().to_os_string();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);

    let is_zip = dest
        .extension()
        .and_then(|x| x.to_str())
        .map(|x| x.eq_ignore_ascii_case("zip"))
        .unwrap_or(false);

    let written = if is_zip {
        write_zip(&tmp, &source, &pkg.name)
    } else {
        write_tar_gz(&tmp, &source, &pkg.name)
    };
    if let Err(err) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }

    if let Err(err) = std::fs::rename(&tmp, dest) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err.into());
    }

    Ok(())
}

fn write_tar_gz(dest: &Path, source: &Path, name: &str) -> Result<(), Box<dyn Error>> {
    let file = std::fs::File::create(dest)?;
    // R compresses a built package with gzip at level 9.
    let enc = flate2::write::GzEncoder::new(file, flate2::Compression::new(9));
    let mut ar = tar::Builder::new(enc);
    // A symlink in an installed package stays a symlink, rather than being
    // stored again as the file it points at.
    ar.follow_symlinks(false);
    ar.mode(tar::HeaderMode::Complete);
    ar.append_dir_all(name, source)?;
    ar.into_inner()?.finish()?;
    Ok(())
}

fn write_zip(dest: &Path, source: &Path, name: &str) -> Result<(), Box<dyn Error>> {
    let file = std::fs::File::create(dest)?;
    let mut zw = zip::ZipWriter::new(file);
    zip_dir(&mut zw, source, name)?;
    zw.finish()?;
    Ok(())
}

/// Add `source` and everything below it to `zw`, under `prefix`.
fn zip_dir<W: std::io::Write + std::io::Seek>(
    zw: &mut zip::ZipWriter<W>,
    source: &Path,
    prefix: &str,
) -> Result<(), Box<dyn Error>> {
    use std::io::Write;

    let options = zip::write::SimpleFileOptions::default();
    zw.add_directory(format!("{}/", prefix), options)?;

    let mut entries: Vec<std::fs::DirEntry> =
        std::fs::read_dir(source)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let name = match entry.file_name().to_str() {
            Some(name) => name.to_string(),
            // A name that is not valid Unicode cannot go into a zip entry, and a
            // package that has one is not something we can cache.
            None => bail!("{} has a non-Unicode file name", source.display()),
        };
        let path = entry.path();
        let inner = format!("{}/{}", prefix, name);
        // `file_type()` does not follow symlinks, so a symlink to a directory is
        // stored as the file it is, not walked into.
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            zip_dir(zw, &path, &inner)?;
        } else {
            zw.start_file(inner, options)?;
            let bytes = std::fs::read(&path)?;
            zw.write_all(&bytes)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: &str, hash: Option<&str>, linkingto: &[(&str, &str, &str)]) -> PackageInfo {
        PackageInfo {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            binary: false,
            file_path: PathBuf::from("/nowhere"),
            dependencies: vec![],
            hash: hash.map(|x| x.to_string()),
            linkingto: linkingto
                .iter()
                .map(|(p, v, s)| (p.to_string(), v.to_string(), s.to_string()))
                .collect(),
            built: None,
        }
    }

    fn cache(dir: &Path) -> BuiltCache {
        BuiltCache {
            dir: dir.to_path_buf(),
            r_platform: Some("x86_64-pc-linux-gnu".to_string()),
            makevars: "".to_string(),
            kind: BuiltKind::Tarball,
        }
    }

    #[test]
    fn the_file_name_is_the_one_r_would_use() {
        assert_eq!(
            built_file_name(BuiltKind::Tgz, "foo", "1.0.0", None),
            "foo_1.0.0.tgz"
        );
        assert_eq!(
            built_file_name(BuiltKind::Zip, "foo", "1.0.0", None),
            "foo_1.0.0.zip"
        );
        assert_eq!(
            built_file_name(
                BuiltKind::Tarball,
                "foo",
                "1.0.0",
                Some("x86_64-pc-linux-gnu")
            ),
            "foo_1.0.0_R_x86_64-pc-linux-gnu.tar.gz"
        );
    }

    /// Not knowing `R_PLATFORM` still gives a usable name, because storing and
    /// looking up compute it the same way.
    #[test]
    fn an_unknown_r_platform_falls_back_to_a_plain_tarball_name() {
        assert_eq!(
            built_file_name(BuiltKind::Tarball, "foo", "1.0.0", None),
            "foo_1.0.0.tar.gz"
        );
    }

    #[test]
    fn r_platform_is_read_out_of_renviron() {
        assert_eq!(
            parse_r_platform("R_LIBS=\nR_PLATFORM=${R_PLATFORM-'aarch64-apple-darwin23'}\n"),
            Some("aarch64-apple-darwin23".to_string())
        );
        assert_eq!(
            parse_r_platform("R_PLATFORM=x86_64-pc-linux-gnu\n"),
            Some("x86_64-pc-linux-gnu".to_string())
        );
        assert_eq!(parse_r_platform("R_LIBS=\n"), None);
    }

    #[test]
    fn the_platform_id_names_the_distro_when_there_is_one() {
        let linux = OsVersion {
            rig_platform: None,
            arch: "x86_64".to_string(),
            vendor: "unknown".to_string(),
            os: "linux".to_string(),
            distro: Some("ubuntu".to_string()),
            version: Some("24.04".to_string()),
        };
        assert_eq!(platform_id(&linux), "x86_64-ubuntu24.04");

        let macos = OsVersion {
            rig_platform: None,
            arch: "aarch64".to_string(),
            vendor: "apple".to_string(),
            os: "darwin24".to_string(),
            distro: None,
            version: None,
        };
        assert_eq!(platform_id(&macos), "aarch64-darwin24");
    }

    #[test]
    fn a_package_without_a_hash_is_not_cached() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(cache(tmp.path()).path(&info("foo", None, &[])).is_none());
    }

    #[test]
    fn the_key_changes_with_what_the_package_is_compiled_against() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = cache(tmp.path());
        let plain = cache.path(&info("foo", Some("abc"), &[])).unwrap();
        let linked = cache
            .path(&info("foo", Some("abc"), &[("Rcpp", "1.0.0", "def")]))
            .unwrap();
        assert_ne!(plain, linked);
        // The file name is R's either way; only the key directory differs.
        assert_eq!(plain.file_name(), linked.file_name());
        assert_ne!(plain.parent(), linked.parent());
    }

    #[test]
    fn the_key_changes_with_makevars() {
        let tmp = tempfile::tempdir().unwrap();
        let mut with = cache(tmp.path());
        with.makevars = "CFLAGS=-O0\n".to_string();
        assert_ne!(
            cache(tmp.path()).path(&info("foo", Some("abc"), &[])),
            with.path(&info("foo", Some("abc"), &[]))
        );
    }

    #[test]
    fn storing_an_installed_package_makes_an_archive_that_unpacks_again() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = tmp.path().join("lib");
        std::fs::create_dir_all(lib.join("foo/libs")).unwrap();
        std::fs::write(
            lib.join("foo/DESCRIPTION"),
            "Package: foo\nVersion: 1.0.0\n",
        )
        .unwrap();
        std::fs::write(lib.join("foo/libs/foo.so"), "binary\n").unwrap();

        let dest = tmp.path().join("cache/foo_1.0.0.tar.gz");
        store(&info("foo", Some("abc"), &[]), &lib, &dest).unwrap();
        assert!(dest.exists());
        // No temporary file left behind.
        assert_eq!(
            std::fs::read_dir(dest.parent().unwrap()).unwrap().count(),
            1
        );

        let out = tmp.path().join("out");
        let file = std::fs::File::open(&dest).unwrap();
        let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(file));
        ar.unpack(&out).unwrap();
        // A single top level directory named after the package, which is what
        // `install_binary_package` requires.
        assert_eq!(std::fs::read_dir(&out).unwrap().count(), 1);
        assert!(out.join("foo/DESCRIPTION").exists());
        assert_eq!(
            std::fs::read_to_string(out.join("foo/libs/foo.so")).unwrap(),
            "binary\n"
        );
    }

    #[test]
    fn storing_something_that_is_not_installed_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = tmp.path().join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        let dest = tmp.path().join("cache/foo_1.0.0.tar.gz");
        assert!(store(&info("foo", Some("abc"), &[]), &lib, &dest).is_err());
        assert!(!dest.exists());
    }
}
