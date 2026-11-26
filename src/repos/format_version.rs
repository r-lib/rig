//! The format version of the extended metadata, see Tidyup 12.
//!
//! A feed gives its version in the `Format-Version` field of `METADATA`, and
//! in a `# Format-Version: <major>.<minor>` comment on the first line of
//! `ALLPACKAGES`, `ARCHIVEDPACKAGES` and every binary index. rig reads major
//! version 1. A newer minor version only adds fields, columns and values,
//! which rig ignores, so it reads those as well. A file without a version is
//! the format P3M served before Tidyup 12, which rig no longer reads.

use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

/// The major version rig reads.
pub const KNOWN_MAJOR: u32 = 1;

/// The newest minor version of [`KNOWN_MAJOR`] rig knows about.
pub const KNOWN_MINOR: u32 = 0;

/// A `<major>.<minor>` format version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FormatVersion {
    pub major: u32,
    pub minor: u32,
}

impl fmt::Display for FormatVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

impl FormatVersion {
    /// Parse `<major>.<minor>`, both non-negative integers.
    pub fn parse(text: &str) -> Option<FormatVersion> {
        let (major, minor) = text.trim().split_once('.')?;
        Some(FormatVersion {
            major: major.parse().ok()?,
            minor: minor.parse().ok()?,
        })
    }

    /// Parse a `# Format-Version: <major>.<minor>` comment line.
    pub fn from_comment(line: &str) -> Option<FormatVersion> {
        let rest = line.trim().strip_prefix('#')?.trim_start();
        let value = rest.strip_prefix("Format-Version:")?;
        FormatVersion::parse(value)
    }

    /// The version from the first line of a data file, if it has one.
    pub fn from_first_line(bytes: &[u8]) -> Option<FormatVersion> {
        let end = bytes
            .iter()
            .position(|b| *b == b'\n')
            .unwrap_or(bytes.len());
        let line = std::str::from_utf8(&bytes[..end]).ok()?;
        FormatVersion::from_comment(line)
    }
}

/// Whether we already noted a newer minor version, to say it only once.
static NEWER_MINOR_NOTED: AtomicBool = AtomicBool::new(false);

/// Fail unless `found` is a version rig reads. `what` names the file, e.g. a
/// URL, for the error message.
pub fn check(found: Option<FormatVersion>, what: &str) -> Result<(), Box<dyn Error>> {
    let Some(found) = found else {
        bail!(
            "The extended metadata at {} has no format version. rig reads \
             format {}.x, served under a `/v{}` URL, e.g. \
             `https://ppm.r-pkg.org/v{}`.",
            what,
            KNOWN_MAJOR,
            KNOWN_MAJOR,
            KNOWN_MAJOR
        );
    };
    if found.major != KNOWN_MAJOR {
        bail!(
            "The extended metadata at {} is format {}, and this rig reads \
             format {}.x. {}",
            what,
            found,
            KNOWN_MAJOR,
            if found.major > KNOWN_MAJOR {
                "Update rig to use it."
            } else {
                "It is too old for this rig."
            }
        );
    }
    if found.minor > KNOWN_MINOR && !NEWER_MINOR_NOTED.swap(true, Ordering::Relaxed) {
        log::debug!(
            "The extended metadata at {} is format {}, newer than {}.{}, \
             ignoring the fields rig does not know",
            what,
            found,
            KNOWN_MAJOR,
            KNOWN_MINOR
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions() {
        assert_eq!(
            FormatVersion::parse("1.10"),
            Some(FormatVersion {
                major: 1,
                minor: 10
            })
        );
        assert!(FormatVersion::parse("1.10") > FormatVersion::parse("1.9"));
        assert_eq!(FormatVersion::parse("1"), None);
        assert_eq!(FormatVersion::parse("a.b"), None);
    }

    #[test]
    fn parses_comments() {
        let v1 = Some(FormatVersion { major: 1, minor: 0 });
        assert_eq!(FormatVersion::from_comment("# Format-Version: 1.0"), v1);
        assert_eq!(FormatVersion::from_comment("#Format-Version:1.0\r"), v1);
        assert_eq!(FormatVersion::from_comment("Format-Version: 1.0"), None);
        assert_eq!(FormatVersion::from_comment("# other"), None);
        assert_eq!(
            FormatVersion::from_first_line(b"# Format-Version: 1.0\n\nPackage: a\n"),
            v1
        );
        assert_eq!(FormatVersion::from_first_line(b"Package: a\n"), None);
    }

    #[test]
    fn checks_versions() {
        let v = |major, minor| Some(FormatVersion { major, minor });
        assert!(check(v(1, 0), "x").is_ok());
        assert!(check(v(1, 7), "x").is_ok());
        let err = check(v(2, 0), "x").unwrap_err().to_string();
        assert!(err.contains("Update rig"), "{}", err);
        assert!(check(v(0, 0), "x").is_err());
        let err = check(None, "x").unwrap_err().to_string();
        assert!(err.contains("/v1"), "{}", err);
    }
}
