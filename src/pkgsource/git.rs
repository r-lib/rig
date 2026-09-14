//! Fetching a `git::<url>` or `github::`/bare `owner/repo` package source by
//! shelling out to the system `git` binary, with sparse-checkout and shallow
//! fetches so only the bytes actually needed are downloaded: just
//! `DESCRIPTION` when resolving a dependency (see [`fetch_git_description`]),
//! or the whole repo tree (still with no history, and scoped to the
//! package's subdirectory if it has one) when populating the package cache
//! at sync time (see [`fetch_git_checkout`]).
//!
//! Callers resolve whatever a `git::`/`github::` reference's `pr`/`release`/
//! `rev`/`branch`/`tag` fields imply into a single `refspec` string (a
//! branch, a tag, a raw commit sha, or a synthetic ref such as
//! `refs/pull/41/head`) before calling into this module -- `git` itself is
//! given nothing more than "fetch this refspec", so it does not need to know
//! or care which host or reference style produced it.

use std::error::Error;
use std::fs;
use std::path::Path;

/// Run `git -C <dest> <args>`, returning trimmed stdout. Both `dest` and, if
/// it doesn't exist yet, its ancestors must already exist for subcommands
/// other than `init` (`init` creates `dest` itself).
fn run_git(dest: &Path, args: &[&str]) -> Result<String, Box<dyn Error>> {
    let output = duct::cmd("git", args)
        .dir(dest)
        .stdout_capture()
        .stderr_capture()
        .unchecked()
        .run()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Box::new(simple_error::SimpleError::new(format!(
            "`git {}` failed in {}: {}",
            args.join(" "),
            dest.display(),
            stderr.trim()
        ))));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// `init` a repo at `dest`, add `origin` pointing at `url`, and (if `sparse`
/// is given) scope a sparse-checkout before ever fetching anything.
///
/// `sparse` is `(cone, paths)`: `cone: true` restricts to whole
/// directories (used for a package subdirectory, still containing the rest
/// of that directory's own tree), `cone: false` restricts to exact paths
/// (used to fetch a single file, e.g. `DESCRIPTION`).
fn init_sparse_repo(
    url: &str,
    dest: &Path,
    sparse: Option<(bool, &[&str])>,
) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(dest)?;
    run_git(dest, &["init", "--quiet"])?;
    run_git(dest, &["remote", "add", "origin", url])?;
    if let Some((cone, paths)) = sparse {
        if cone {
            run_git(dest, &["sparse-checkout", "init", "--cone"])?;
        } else {
            run_git(dest, &["sparse-checkout", "init", "--no-cone"])?;
        }
        let mut set_args = vec!["sparse-checkout", "set"];
        set_args.extend(paths.iter().copied());
        run_git(dest, &set_args)?;
    }
    Ok(())
}

/// Fetch `refspec` (or `HEAD`, the default branch tip, if `None`) into a repo
/// already `init_sparse_repo`-ed at `dest`, check it out, and return the
/// resolved commit sha. `partial`, when set, adds `--filter=blob:none` to the
/// fetch, so only blobs the sparse-checkout actually selects are downloaded.
fn fetch_and_checkout(
    dest: &Path,
    refspec: Option<&str>,
    partial: bool,
) -> Result<String, Box<dyn Error>> {
    let want = refspec.unwrap_or("HEAD");
    let mut fetch_args = vec!["fetch", "--depth", "1"];
    if partial {
        fetch_args.push("--filter=blob:none");
    }
    fetch_args.extend(["origin", want]);
    run_git(dest, &fetch_args)?;
    run_git(dest, &["checkout", "--quiet", "FETCH_HEAD"])?;
    run_git(dest, &["rev-parse", "FETCH_HEAD"])
}

/// Fetch just `DESCRIPTION` (or `<subdir>/DESCRIPTION`) from `url` at
/// `refspec`, without downloading any other blob, and return its contents
/// plus the resolved commit sha.
///
/// Used while resolving a git/GitHub dependency (`rig proj lock`/solve): only
/// the package metadata is needed at this point, not the rest of the
/// repository.
pub fn fetch_git_description(
    url: &str,
    refspec: Option<&str>,
    subdir: Option<&str>,
) -> Result<(String, String), Box<dyn Error>> {
    let tmp = tempfile::tempdir()?;
    let dest = tmp.path();
    let description_path = match subdir {
        Some(s) => format!("{}/DESCRIPTION", s),
        None => "DESCRIPTION".to_string(),
    };
    init_sparse_repo(url, dest, Some((false, &[&description_path])))?;
    let sha = fetch_and_checkout(dest, refspec, true).map_err(|err| {
        simple_error::SimpleError::new(format!(
            "Cannot fetch {}{}: {}",
            url,
            refspec.map(|r| format!(" at {}", r)).unwrap_or_default(),
            err
        ))
    })?;
    let contents = fs::read_to_string(dest.join(&description_path)).map_err(|err| {
        simple_error::SimpleError::new(format!(
            "Cannot read {} from {}: {}",
            description_path, url, err
        ))
    })?;
    Ok((contents, sha))
}

/// Clone `url` at `refspec` into `dest`: a shallow (`--depth 1`) fetch,
/// scoped to `subdir` if given (else the whole tree), and return the
/// resolved commit sha.
///
/// Used to populate the package cache (`rig proj sync`): the whole package
/// tree is needed here (for `R CMD INSTALL`), just not its history, and not
/// the rest of the repository when the package lives in a subdirectory.
/// `dest` is the repository root -- if the package lives in `subdir`,
/// callers find it (and its `DESCRIPTION`) at `<dest>/<subdir>`, the same
/// layout a plain (non-sparse) checkout would have.
pub fn fetch_git_checkout(
    url: &str,
    refspec: Option<&str>,
    subdir: Option<&str>,
    dest: &Path,
) -> Result<String, Box<dyn Error>> {
    let paths: Vec<&str> = subdir.into_iter().collect();
    let sparse: Option<(bool, &[&str])> = subdir.map(|_| (true, paths.as_slice()));

    init_sparse_repo(url, dest, sparse)?;
    fetch_and_checkout(dest, refspec, false).map_err(|err| {
        simple_error::SimpleError::new(format!(
            "Cannot fetch {}{}: {}",
            url,
            refspec.map(|r| format!(" at {}", r)).unwrap_or_default(),
            err
        ))
        .into()
    })
}

/// Resolve `owner/repo`'s latest GitHub release to its tag name, without the
/// GitHub REST API: `https://github.com/<owner>/<repo>/releases/latest`
/// redirects to `.../releases/tag/<tag>`, so a single unauthenticated request
/// with redirects disabled reveals the tag via the `Location` header.
pub fn resolve_release_tag(owner: &str, repo: &str) -> Result<String, Box<dyn Error>> {
    resolve_release_tag_(owner, repo)
}

#[tokio::main]
async fn resolve_release_tag_(owner: &str, repo: &str) -> Result<String, Box<dyn Error>> {
    let url = format!("https://github.com/{}/{}/releases/latest", owner, repo);
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let resp = client.get(&url).send().await?;
    let location = resp
        .headers()
        .get(reqwest::header::LOCATION)
        .ok_or_else(|| {
            simple_error::SimpleError::new(format!(
                "{} did not redirect to a release (no releases published?)",
                url
            ))
        })?
        .to_str()?;
    let tag = location
        .rsplit_once("/releases/tag/")
        .map(|(_, tag)| tag)
        .ok_or_else(|| {
            simple_error::SimpleError::new(format!(
                "Unexpected redirect from {}: {}",
                url, location
            ))
        })?;
    Ok(percent_decode(tag))
}

/// Minimal percent-decoding for a URL path segment (a git tag name has no
/// reason to contain anything beyond ASCII/percent-escapes here).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// An absolute path to the `git` binary, so this fixture is immune to
    /// other tests in the suite temporarily clobbering the process-wide
    /// `PATH` env var (see `utils::tests`, which does exactly that) while
    /// tests run in parallel -- a `Command::new("git")` PATH lookup would
    /// otherwise randomly fail with ENOENT depending on scheduling.
    fn git_binary() -> &'static str {
        for candidate in [
            "/usr/bin/git",
            "/usr/local/bin/git",
            "/opt/homebrew/bin/git",
        ] {
            if std::path::Path::new(candidate).exists() {
                return candidate;
            }
        }
        "git"
    }

    /// A local bare repo with one commit on `main` (with `DESCRIPTION` and a
    /// `pkg/` subdirectory) and a tag `v1`, used as a `file://` stand-in for
    /// a remote host -- no live network needed.
    fn bare_fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let run = |args: &[&str]| {
            let status = Command::new(git_binary())
                .args(args)
                .current_dir(&work)
                .env("GIT_AUTHOR_NAME", "rig-test")
                .env("GIT_AUTHOR_EMAIL", "rig-test@example.com")
                .env("GIT_COMMITTER_NAME", "rig-test")
                .env("GIT_COMMITTER_EMAIL", "rig-test@example.com")
                .status()
                .unwrap();
            assert!(status.success(), "git {:?} failed", args);
        };
        run(&["init", "-q", "-b", "main"]);
        std::fs::write(
            work.join("DESCRIPTION"),
            "Package: fixture\nVersion: 0.0.1\n",
        )
        .unwrap();
        std::fs::create_dir_all(work.join("pkg")).unwrap();
        std::fs::write(
            work.join("pkg").join("DESCRIPTION"),
            "Package: subpkg\nVersion: 0.0.1\n",
        )
        .unwrap();
        std::fs::write(work.join("pkg").join("R.R"), "1\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "init"]);
        run(&["tag", "v1"]);
        dir
    }

    // `#[ignore]`d: these tests spawn `git-upload-pack` (via `git`'s own
    // `file://` transport) which is looked up via `PATH`, and a few
    // `utils::tests` in this same test binary temporarily clobber the
    // process-wide `PATH` env var (env vars are process-global, not
    // per-thread), which races with these when the suite runs with its
    // default parallelism. Run explicitly with `cargo test -- --ignored`
    // (or `--test-threads=1`) to verify.

    #[test]
    #[ignore]
    fn description_only_fetch() {
        let fixture = bare_fixture();
        let url = format!("file://{}", fixture.path().join("work").display());
        let (contents, sha) = fetch_git_description(&url, None, None).unwrap();
        assert!(contents.contains("Package: fixture"));
        assert_eq!(sha.len(), 40);
    }

    #[test]
    #[ignore]
    fn description_only_fetch_from_subdir() {
        let fixture = bare_fixture();
        let url = format!("file://{}", fixture.path().join("work").display());
        let (contents, _sha) = fetch_git_description(&url, None, Some("pkg")).unwrap();
        assert!(contents.contains("Package: subpkg"));
    }

    #[test]
    #[ignore]
    fn checkout_a_tag() {
        let fixture = bare_fixture();
        let url = format!("file://{}", fixture.path().join("work").display());
        let dest = tempfile::tempdir().unwrap();
        let sha = fetch_git_checkout(&url, Some("v1"), None, dest.path()).unwrap();
        assert_eq!(sha.len(), 40);
        assert!(dest.path().join("DESCRIPTION").exists());
    }

    #[test]
    #[ignore]
    fn checkout_a_raw_commit_sha() {
        let fixture = bare_fixture();
        let url = format!("file://{}", fixture.path().join("work").display());
        let dest = tempfile::tempdir().unwrap();
        let head = run_git(&fixture.path().join("work"), &["rev-parse", "HEAD"]).unwrap();
        let sha = fetch_git_checkout(&url, Some(&head), None, dest.path()).unwrap();
        assert_eq!(sha, head);
    }

    #[test]
    #[ignore]
    fn checkout_scoped_to_subdir() {
        let fixture = bare_fixture();
        let url = format!("file://{}", fixture.path().join("work").display());
        let dest = tempfile::tempdir().unwrap();
        fetch_git_checkout(&url, None, Some("pkg"), dest.path()).unwrap();
        assert!(dest.path().join("pkg").join("DESCRIPTION").exists());
        assert!(dest.path().join("pkg").join("R.R").exists());
    }

    /// Network-gated: hits real github.com. Run explicitly with
    /// `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn resolve_release_tag_against_a_real_repo() {
        // r-lib/crayon has releases and is unlikely to ever un-release one.
        let tag = resolve_release_tag("r-lib", "crayon").unwrap();
        assert!(!tag.is_empty());
    }

    #[test]
    fn percent_decode_handles_escapes() {
        assert_eq!(percent_decode("v1.0"), "v1.0");
        assert_eq!(percent_decode("release%2Fv1"), "release/v1");
    }
}
