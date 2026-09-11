//! Parsing for `git`/`github` package sources, the `pak`-compatible syntax
//! accepted by `rig proj add` (and, recursively, a fetched package's own
//! `Remotes:` DESCRIPTION field):
//!
//!   - `[<name>=][github::]<owner>/<repo>[/<subdir>][@<ref>|#<pr>|@*release]`
//!   - bare `<owner>/<repo>...` (same suffixes) auto-detects as GitHub
//!   - `[<name>=]git::<https-url>[.git][@<ref>]`
//!
//! A CRAN-style spec (`dplyr`, `dplyr@1.1.0`, `dplyr@>= 1.1`) never contains
//! `/` or `::`, which is what tells the two apart: anything with a `/` or a
//! `::` before its `@`/`#` suffix is a remote reference, everything else falls
//! through unchanged to the existing `parse_add_spec` (CRAN) path.

use std::error::Error;

use simple_error::bail;

pub mod git;
pub mod github;

/// A parsed, not yet fetched, package source. `Cran` means "not a
/// git/github reference at all", so the caller can fall back to the existing
/// `parse_add_spec` handling unchanged.
#[derive(Debug, Clone, PartialEq)]
pub enum PkgSource {
    Cran,
    Remote(RemoteSource),
}

/// A `git`/`github` reference, already normalized to a plain git URL plus
/// whichever of `branch`/`tag`/`rev`/`pr`/`release` selects a commit, and an
/// optional subdirectory within the repository. This is the shape written
/// into `rproj.toml`'s `DepTable` (`git`, `branch`, `tag`, `rev`, `pr`,
/// `release`, `subdir`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RemoteSource {
    /// An explicit `<name>=` override, if the spec had one.
    pub name_override: Option<String>,
    /// The git remote URL, e.g. `https://github.com/r-lib/crayon.git` or
    /// `https://gitlab.com/x/y.git`.
    pub git: String,
    /// A branch name, if the ref was recognized as one at fetch time. Never
    /// set by the parser itself -- see [`RemoteSource::rev`].
    pub branch: Option<String>,
    /// A tag name, if the ref was recognized as one at fetch time. Never set
    /// by the parser itself -- see [`RemoteSource::rev`].
    pub tag: Option<String>,
    /// The raw `@<commitish>` the user wrote (a branch, tag, or commit
    /// prefix): the parser cannot tell which without asking the remote, so it
    /// always goes here first; fetching may reclassify it into `branch`/`tag`.
    pub rev: Option<String>,
    /// A GitHub pull request number (`#<pr>`), GitHub sources only.
    pub pr: Option<u32>,
    /// `@*release`: resolve to the repository's latest release tag, GitHub
    /// sources only.
    pub release: bool,
    /// `<owner>/<repo>/<subdir>`: the package lives in a subdirectory of the
    /// repository, not at its root.
    pub subdir: Option<String>,
}

/// Parse a `rig proj add`-style package specification into a [`PkgSource`].
///
/// A spec with no `/` and no `::` is always [`PkgSource::Cran`] -- the
/// existing `<package>`/`<package>@<version>` syntax, untouched.
pub fn parse_pkg_source(spec: &str) -> Result<PkgSource, Box<dyn Error>> {
    let spec = spec.trim();
    let (name_override, body) = strip_name_override(spec);

    if let Some(url) = body.strip_prefix("git::") {
        return Ok(PkgSource::Remote(parse_git_url(name_override, url)?));
    }

    if let Some(rest) = body.strip_prefix("github::") {
        return Ok(PkgSource::Remote(parse_github_ref(name_override, rest)?));
    }

    if looks_like_owner_repo(body) {
        return Ok(PkgSource::Remote(parse_github_ref(name_override, body)?));
    }

    if body.contains("::") || body.contains('/') {
        bail!(
            "Cannot parse package reference `{}`: expected `<owner>/<repo>`, \
             `github::...` or `git::<url>`",
            spec
        );
    }

    Ok(PkgSource::Cran)
}

/// Split off an optional `<name>=` override prefix, the way `pak` refs allow
/// naming the package explicitly (`mypkg=owner/repo`). The name has to look
/// like a real R package name, so a version requirement's `>=`/`<=` (which
/// also contains `=`) is never misread as a name override.
fn strip_name_override(spec: &str) -> (Option<String>, &str) {
    if let Some((name, rest)) = spec.split_once('=') {
        let name = name.trim();
        if is_r_package_name(name) {
            return (Some(name.to_string()), rest.trim());
        }
    }
    (None, spec)
}

fn is_r_package_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '.')
}

/// Whether `body` (already stripped of any `<name>=` override) looks like a
/// bare `<owner>/<repo>` GitHub shorthand: a `/`-separated path, appearing
/// before any `@`/`#` suffix, whose first two segments are valid-looking
/// GitHub owner/repo names.
fn looks_like_owner_repo(body: &str) -> bool {
    let path = body.split(['@', '#']).next().unwrap_or(body);
    let mut segments = path.split('/');
    let owner = segments.next().unwrap_or("");
    let repo = segments.next().unwrap_or("");
    !owner.is_empty()
        && !repo.is_empty()
        && is_github_path_segment(owner)
        && is_github_path_segment(repo)
}

fn is_github_path_segment(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// Parse `<owner>/<repo>[/<subdir>][@<ref>|#<pr>|@*release]`.
fn parse_github_ref(
    name_override: Option<String>,
    body: &str,
) -> Result<RemoteSource, Box<dyn Error>> {
    // `#<pr>` and `@<detail>` cannot both appear, and whichever is present
    // marks the end of the `<owner>/<repo>/<subdir>` path.
    let (path, pr) = match body.split_once('#') {
        Some((path, pr)) => {
            let pr: u32 = pr.trim().parse().map_err(|_| {
                simple_error::SimpleError::new(format!(
                    "Invalid pull request number `{}` in `{}`",
                    pr, body
                ))
            })?;
            (path, Some(pr))
        }
        None => (body, None),
    };
    let (path, detail) = match path.split_once('@') {
        Some((path, detail)) => (path, Some(detail)),
        None => (path, None),
    };

    let mut segments = path.trim_matches('/').splitn(3, '/');
    let owner = segments.next().unwrap_or("");
    let repo = segments.next().unwrap_or("");
    let subdir = segments.next().map(|s| s.to_string());

    if owner.is_empty() || repo.is_empty() {
        bail!(
            "Invalid GitHub reference `{}`, expected `<owner>/<repo>`",
            path
        );
    }

    let (rev, release) = match detail {
        Some("*release") => (None, true),
        Some(other) => (Some(other.to_string()), false),
        None => (None, false),
    };

    Ok(RemoteSource {
        name_override,
        git: format!("https://github.com/{}/{}.git", owner, repo),
        branch: None,
        tag: None,
        rev,
        pr,
        release,
        subdir,
    })
}

/// Parse `<https-url>[.git][@<ref>]` (the body after a `git::` prefix has
/// already been stripped).
fn parse_git_url(
    name_override: Option<String>,
    body: &str,
) -> Result<RemoteSource, Box<dyn Error>> {
    if !body.starts_with("https://") && !body.starts_with("http://") {
        bail!(
            "Invalid `git::` package reference `{}`: expected an http(s) URL",
            body
        );
    }
    // The scheme's own `://` has no `@`, so the *last* `@` (if any) is the
    // ref separator; a URL with embedded credentials (`https://user@host/...`)
    // is not supported in v1 (public repositories only).
    let (url, rev) = match body.rsplit_once('@') {
        Some((url, rev)) if !url.is_empty() => (url, Some(rev.to_string())),
        _ => (body, None),
    };

    Ok(RemoteSource {
        name_override,
        git: url.to_string(),
        branch: None,
        tag: None,
        rev,
        pr: None,
        release: false,
        subdir: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(spec: &str) -> RemoteSource {
        match parse_pkg_source(spec).unwrap() {
            PkgSource::Remote(r) => r,
            PkgSource::Cran => panic!("expected a remote source for `{}`", spec),
        }
    }

    #[test]
    fn cran_specs_are_unaffected() {
        for spec in [
            "dplyr",
            "dplyr@1.1.0",
            "dplyr@>= 1.1",
            "rlang@>= 1.0, < 2.0",
        ] {
            assert_eq!(parse_pkg_source(spec).unwrap(), PkgSource::Cran, "{}", spec);
        }
    }

    #[test]
    fn bare_owner_repo() {
        let r = remote("r-lib/crayon");
        assert_eq!(r.git, "https://github.com/r-lib/crayon.git");
        assert_eq!(r.rev, None);
        assert_eq!(r.pr, None);
        assert!(!r.release);
        assert_eq!(r.subdir, None);
    }

    #[test]
    fn owner_repo_with_ref() {
        let r = remote("r-lib/crayon@84be6207");
        assert_eq!(r.rev.as_deref(), Some("84be6207"));
    }

    #[test]
    fn owner_repo_with_branch() {
        let r = remote("r-lib/crayon@branch");
        assert_eq!(r.rev.as_deref(), Some("branch"));
    }

    #[test]
    fn owner_repo_with_pull_request() {
        let r = remote("r-lib/crayon#41");
        assert_eq!(r.pr, Some(41));
        assert_eq!(r.rev, None);
    }

    #[test]
    fn owner_repo_with_release() {
        let r = remote("r-lib/crayon@*release");
        assert!(r.release);
        assert_eq!(r.rev, None);
    }

    #[test]
    fn owner_repo_with_subdir() {
        let r = remote("r-lib/usethis/subdir");
        assert_eq!(r.subdir.as_deref(), Some("subdir"));
    }

    #[test]
    fn owner_repo_with_subdir_and_ref() {
        let r = remote("r-lib/usethis/subdir@main");
        assert_eq!(r.subdir.as_deref(), Some("subdir"));
        assert_eq!(r.rev.as_deref(), Some("main"));
    }

    #[test]
    fn explicit_github_prefix() {
        let r = remote("github::r-lib/crayon");
        assert_eq!(r.git, "https://github.com/r-lib/crayon.git");
    }

    #[test]
    fn name_override() {
        let r = remote("mycrayon=r-lib/crayon@main");
        assert_eq!(r.name_override.as_deref(), Some("mycrayon"));
        assert_eq!(r.rev.as_deref(), Some("main"));
    }

    #[test]
    fn git_url_plain() {
        let r = remote("git::https://gitlab.com/x/y.git");
        assert_eq!(r.git, "https://gitlab.com/x/y.git");
        assert_eq!(r.rev, None);
    }

    #[test]
    fn git_url_with_ref() {
        let r = remote("git::https://github.com/r-lib/crayon.git@branch");
        assert_eq!(r.git, "https://github.com/r-lib/crayon.git");
        assert_eq!(r.rev.as_deref(), Some("branch"));
    }

    #[test]
    fn git_url_with_rev_and_no_dot_git() {
        let r = remote("git::https://github.com/r-lib/crayon@84be6207");
        assert_eq!(r.git, "https://github.com/r-lib/crayon");
        assert_eq!(r.rev.as_deref(), Some("84be6207"));
    }

    #[test]
    fn rejects_malformed_git_url() {
        assert!(parse_pkg_source("git::not-a-url").is_err());
    }

    #[test]
    fn rejects_bad_pull_request_number() {
        assert!(parse_pkg_source("r-lib/crayon#not-a-number").is_err());
    }
}
