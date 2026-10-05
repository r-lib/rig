//! The `sources` entry of a git/GitHub/url/local package in `rproj.lock`.
//!
//! A lockfile package's `metadata` is provenance (the `Remote*` fields written
//! into the installed `DESCRIPTION`); everything needed to fetch and install a
//! remote package is in its `sources` entry instead, in a compact syntax:
//!
//!   - `git+<url>#commit=<sha>[&subdir=<path>]` for a git or GitHub source
//!   - `<http(s)-url>#sha256=<hex>[&subdir=<path>]` for a `url` archive
//!   - a `file://` URL for a package directory or file on this machine
//!   - a plain URL, with no `#`, for a CRAN-like repository download
//!
//! Everything after the last `#` belongs to rig, as `key=value` pairs joined
//! by `&`, like a pip direct reference. Values are percent-encoded, so a
//! subdir may contain `&`, `#` or `=`.

use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockSource {
    /// A git (or GitHub) repository, pinned to one commit. The package lives
    /// at `<checkout>/<subdir>` if `subdir` is given.
    Git {
        url: String,
        commit: String,
        subdir: Option<String>,
    },
    /// A package archive, verified against its sha256. The package lives at
    /// `<extracted>/<subdir>` if `subdir` is given.
    Archive {
        url: String,
        sha256: String,
        subdir: Option<String>,
    },
    /// A package directory or file on this machine.
    Local { path: String },
    /// A file downloaded as is, from a CRAN-like repository.
    Http { url: String },
}

impl LockSource {
    pub fn parse(source: &str) -> Result<LockSource, Box<dyn Error>> {
        if source.starts_with("file://") {
            let path = reqwest::Url::parse(source)
                .ok()
                .and_then(|url| url.to_file_path().ok())
                .ok_or_else(|| {
                    simple_error::SimpleError::new(format!(
                        "Invalid lockfile source `{}`: not a file path",
                        source
                    ))
                })?;
            return Ok(LockSource::Local {
                path: path.display().to_string(),
            });
        }
        let (git, rest) = match source.strip_prefix("git+") {
            Some(rest) => (true, rest),
            None => (false, source),
        };
        let Some((url, fragment)) = rest.rsplit_once('#') else {
            if git {
                bail!("Invalid lockfile source `{}`: no commit", source);
            }
            return Ok(LockSource::Http {
                url: source.to_string(),
            });
        };

        let mut id: Option<String> = None;
        let mut subdir: Option<String> = None;
        let id_key = if git { "commit" } else { "sha256" };
        for pair in fragment.split('&').filter(|p| !p.is_empty()) {
            let (key, value) = pair.split_once('=').ok_or_else(|| {
                simple_error::SimpleError::new(format!(
                    "Invalid lockfile source `{}`: `{}` is not `key=value`",
                    source, pair
                ))
            })?;
            let value = percent_decode(value)?;
            match key {
                "subdir" => subdir = Some(value),
                k if k == id_key => id = Some(value),
                _ => bail!(
                    "Invalid lockfile source `{}`: unknown key `{}`",
                    source,
                    key
                ),
            }
        }
        let Some(id) = id else {
            bail!("Invalid lockfile source `{}`: no `{}`", source, id_key);
        };
        let url = url.to_string();
        Ok(if git {
            LockSource::Git {
                url,
                commit: id,
                subdir,
            }
        } else {
            LockSource::Archive {
                url,
                sha256: id,
                subdir,
            }
        })
    }

    /// The source of a remote package, from its `RemoteType`, its URL (a path
    /// for `local`), its commit (archive sha256 for `url`) and its subdir.
    /// `None` for an unknown `remote_type`.
    pub fn from_remote(
        remote_type: &str,
        url: &str,
        sha: &str,
        subdir: Option<&str>,
    ) -> Option<LockSource> {
        let subdir = subdir.map(|s| s.to_string());
        match remote_type {
            "git" | "github" => Some(LockSource::Git {
                url: url.to_string(),
                commit: sha.to_string(),
                subdir,
            }),
            "url" => Some(LockSource::Archive {
                url: url.to_string(),
                sha256: sha.to_string(),
                subdir,
            }),
            "local" => Some(LockSource::Local {
                path: url.to_string(),
            }),
            _ => None,
        }
    }

    /// Where the package lives within the fetched directory, if not at its
    /// root.
    pub fn subdir(&self) -> Option<&str> {
        match self {
            LockSource::Git { subdir, .. } | LockSource::Archive { subdir, .. } => {
                subdir.as_deref()
            }
            _ => None,
        }
    }
}

/// Drop a URL's own fragment: it means nothing to the server, and the
/// fragment of a lockfile source is rig's.
fn strip_fragment(url: &str) -> &str {
    url.split_once('#').map(|(u, _)| u).unwrap_or(url)
}

impl fmt::Display for LockSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let subdir_part = |subdir: &Option<String>| match subdir {
            Some(s) => format!("&subdir={}", percent_encode(s)),
            None => String::new(),
        };
        match self {
            LockSource::Git {
                url,
                commit,
                subdir,
            } => write!(
                f,
                "git+{}#commit={}{}",
                strip_fragment(url),
                percent_encode(commit),
                subdir_part(subdir)
            ),
            LockSource::Archive {
                url,
                sha256,
                subdir,
            } => write!(
                f,
                "{}#sha256={}{}",
                strip_fragment(url),
                percent_encode(sha256),
                subdir_part(subdir)
            ),
            LockSource::Local { path } => match reqwest::Url::from_file_path(path) {
                Ok(url) => write!(f, "{}", url),
                Err(_) => write!(f, "file://{}", path),
            },
            LockSource::Http { url } => write!(f, "{}", url),
        }
    }
}

fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '%' | '&' | '#' | '=' | ' ' => out.push_str(&format!("%{:02X}", c as u8)),
            _ => out.push(c),
        }
    }
    out
}

fn percent_decode(value: &str) -> Result<String, Box<dyn Error>> {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = value
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            match hex {
                Some(b) => {
                    out.push(b);
                    i += 3;
                    continue;
                }
                None => bail!("Invalid percent-encoding in `{}`", value),
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    Ok(String::from_utf8(out)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(source: &LockSource) {
        let text = source.to_string();
        assert_eq!(&LockSource::parse(&text).unwrap(), source, "{}", text);
    }

    #[test]
    fn git_source_with_subdir() {
        let s = LockSource::Git {
            url: "https://example.com/repo.git".to_string(),
            commit: "3f2a".to_string(),
            subdir: Some("pkgs/mypkg".to_string()),
        };
        assert_eq!(
            s.to_string(),
            "git+https://example.com/repo.git#commit=3f2a&subdir=pkgs/mypkg"
        );
        round_trip(&s);
    }

    #[test]
    fn git_source_without_subdir() {
        let s = LockSource::Git {
            url: "https://example.com/repo.git".to_string(),
            commit: "3f2a".to_string(),
            subdir: None,
        };
        assert_eq!(
            s.to_string(),
            "git+https://example.com/repo.git#commit=3f2a"
        );
        round_trip(&s);
    }

    #[test]
    fn archive_source_keeps_the_url_query() {
        let s = LockSource::Archive {
            url: "https://example.com/download?file=mypkg.zip".to_string(),
            sha256: "abcd".to_string(),
            subdir: Some("mypkg-1.0".to_string()),
        };
        assert_eq!(
            s.to_string(),
            "https://example.com/download?file=mypkg.zip#sha256=abcd&subdir=mypkg-1.0"
        );
        round_trip(&s);
    }

    #[test]
    fn special_characters_in_subdir_are_escaped() {
        let s = LockSource::Git {
            url: "https://example.com/repo.git".to_string(),
            commit: "3f2a".to_string(),
            subdir: Some("a&b=c#d e%f/ü".to_string()),
        };
        assert_eq!(
            s.to_string(),
            "git+https://example.com/repo.git#commit=3f2a&subdir=a%26b%3Dc%23d%20e%25f/ü"
        );
        round_trip(&s);
    }

    #[test]
    fn url_fragment_is_dropped() {
        let s = LockSource::Archive {
            url: "https://example.com/mypkg.tar.gz#top".to_string(),
            sha256: "abcd".to_string(),
            subdir: None,
        };
        assert_eq!(
            s.to_string(),
            "https://example.com/mypkg.tar.gz#sha256=abcd"
        );
    }

    #[test]
    fn local_and_http_sources() {
        let local = LockSource::Local {
            path: if cfg!(windows) {
                "C:\\Users\\me\\my pkg"
            } else {
                "/home/me/my pkg"
            }
            .to_string(),
        };
        round_trip(&local);
        assert!(local.to_string().starts_with("file:///"));
        // The project's own entry is a directory URL, with a trailing slash.
        assert!(matches!(
            LockSource::parse(if cfg!(windows) {
                "file:///C:/me/pkg/"
            } else {
                "file:///me/pkg/"
            })
            .unwrap(),
            LockSource::Local { .. }
        ));
        assert_eq!(
            LockSource::parse("https://cran.r-project.org/src/contrib/cli_3.6.3.tar.gz").unwrap(),
            LockSource::Http {
                url: "https://cran.r-project.org/src/contrib/cli_3.6.3.tar.gz".to_string()
            }
        );
    }

    #[test]
    fn invalid_sources_are_errors() {
        assert!(LockSource::parse("git+https://example.com/repo.git").is_err());
        assert!(LockSource::parse("git+https://example.com/repo.git#subdir=x").is_err());
        assert!(LockSource::parse("git+https://example.com/repo.git#3f2a").is_err());
        assert!(LockSource::parse("https://example.com/x.tar.gz#commit=3f2a").is_err());
        assert!(LockSource::parse("git+https://example.com/r.git#commit=%zz").is_err());
    }
}
