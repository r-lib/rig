//! Fetching a `git::<url>` or `github::`/bare `owner/repo` package source by
//! shelling out to the system `git` binary.
//!
//! Resolving a dependency (`rig proj lock`/solve) only ever needs
//! `DESCRIPTION` at one commit, see [`fetch_git_description`]: it reuses a
//! persistent, per-URL bare mirror under rig's cache directory (see
//! [`crate::cache::git_mirror_dir`]) across separate `rig proj lock` runs,
//! so a repeat resolution of an unchanged ref is a cheap ref check rather
//! than a fresh clone, and reads the file straight out of the object store
//! with `git show` instead of materializing a working tree. Populating the
//! package cache at sync time (see [`fetch_git_checkout`]) still needs the
//! whole package tree (for `R CMD INSTALL`) checked out fresh into a
//! caller-supplied, sha-keyed destination -- that path is unchanged.
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

use fs4::fs_std::FileExt;

use crate::utils::http_client_builder;

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
/// resolved commit sha. No `--filter=blob:none`: the sparse-checkout already
/// narrows the fetch to a handful of blobs, so a filtered fetch just defers
/// them to a second network round trip at `checkout` instead of saving one.
fn fetch_and_checkout(dest: &Path, refspec: Option<&str>) -> Result<String, Box<dyn Error>> {
    let want = refspec.unwrap_or("HEAD");
    run_git(dest, &["fetch", "--depth", "1", "origin", want])?;
    run_git(dest, &["checkout", "--quiet", "FETCH_HEAD"])?;
    run_git(dest, &["rev-parse", "FETCH_HEAD"])
}

/// `init --bare` a repo at `dest` and add `origin`, unless it already looks
/// like one (has a `HEAD` file) -- called on every cached fetch, so this is
/// the idempotent, cheap path once the mirror exists.
fn ensure_bare_mirror(url: &str, dest: &Path) -> Result<(), Box<dyn Error>> {
    if dest.join("HEAD").exists() {
        return Ok(());
    }
    fs::create_dir_all(dest)?;
    run_git(dest, &["init", "--quiet", "--bare"])?;
    run_git(dest, &["remote", "add", "origin", url])?;
    Ok(())
}

/// Fetch `refspec` (or `HEAD`) into a bare mirror already `ensure_bare_mirror`
/// -ed at `dest`, and return the resolved commit sha, without checking
/// anything out.
///
/// Tries a partial-clone fetch (`--filter=blob:none`) first: on a big repo
/// where only one file is ever read back out (see [`read_blob`]), this skips
/// downloading every other blob at that commit, deferring them to a lazy
/// per-blob fetch if they're ever actually requested. Not every git host
/// supports partial-clone filters, so a filtered fetch that fails is retried
/// once without `--filter`.
fn fetch_into_mirror(dest: &Path, refspec: Option<&str>) -> Result<String, Box<dyn Error>> {
    let want = refspec.unwrap_or("HEAD");
    let filtered = run_git(
        dest,
        &[
            "fetch",
            "--depth",
            "1",
            "--filter=blob:none",
            "origin",
            want,
        ],
    );
    if filtered.is_err() {
        run_git(dest, &["fetch", "--depth", "1", "origin", want])?;
    }
    run_git(dest, &["rev-parse", "FETCH_HEAD"])
}

/// Read `path` at `sha` out of a bare mirror at `dest`, via `git show`
/// (which lazily fetches the blob first if `fetch_into_mirror` deferred it).
fn read_blob(dest: &Path, sha: &str, path: &str) -> Result<String, Box<dyn Error>> {
    run_git(dest, &["show", &format!("{}:{}", sha, path)])
}

/// Make sure `sha` is present in a bare mirror already `ensure_bare_mirror`
/// -ed at `dest`, and return it unchanged -- the "trust a known commit"
/// counterpart to [`fetch_into_mirror`]'s "resolve a possibly-moved ref".
///
/// If `sha` was already fetched by an earlier `rig proj lock` (the common
/// case: an unchanged git dependency, resolved before on this machine), this
/// touches the network not at all -- `cat-file -e` only looks at local
/// objects. Only a genuine cache miss (a fresh machine, a cleared cache, or
/// a `sha` from a lockfile older than this mirror) falls through to fetching
/// that one exact commit.
fn ensure_commit(dest: &Path, sha: &str) -> Result<String, Box<dyn Error>> {
    if run_git(dest, &["cat-file", "-e", &format!("{}^{{commit}}", sha)]).is_ok() {
        return Ok(sha.to_string());
    }
    let filtered = run_git(
        dest,
        &["fetch", "--depth", "1", "--filter=blob:none", "origin", sha],
    );
    if filtered.is_err() {
        run_git(dest, &["fetch", "--depth", "1", "origin", sha])?;
    }
    Ok(sha.to_string())
}

/// An exclusive, blocking lock on `<mirror>.lock`, held for as long as the
/// returned `File` stays alive (released on drop) -- guards one mirror
/// directory against concurrent `git fetch`es, both from other threads in
/// this process (`resolve_git_sources` resolves dependencies in parallel)
/// and from other `rig` processes racing on the same cache.
fn lock_mirror(mirror: &Path) -> Result<fs::File, Box<dyn Error>> {
    let lock_path = mirror.with_extension("lock");
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let lock_file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)?;
    lock_file.lock_exclusive()?;
    Ok(lock_file)
}

/// Fetch just `DESCRIPTION` (or `<subdir>/DESCRIPTION`) from `url` at
/// `refspec`, and return its contents plus the resolved commit sha.
///
/// Used while resolving a git/GitHub dependency (`rig proj lock`/solve): only
/// the package metadata is needed at this point, not the rest of the
/// repository. Backed by a persistent, per-URL mirror when rig's cache is
/// available (see [`crate::cache::git_mirror_dir`]), falling back to a
/// throwaway clone otherwise -- caching a git fetch is a missed
/// optimization when it's unavailable, never a hard failure.
///
/// `known_sha` is a commit an earlier `rig proj lock` already pinned this
/// exact request (URL/refspec/subdir) to, if any -- see
/// `crate::proj::existing_git_shas`. When given, it's trusted outright
/// instead of re-resolving `refspec` against the remote (the same "a
/// lockfile is sticky until asked to upgrade" behavior `Cargo.lock`/
/// `uv.lock` have), and only costs a network round trip at all if the
/// mirror doesn't already have that commit locally.
pub fn fetch_git_description(
    url: &str,
    refspec: Option<&str>,
    subdir: Option<&str>,
    known_sha: Option<&str>,
) -> Result<(String, String), Box<dyn Error>> {
    let description_path = match subdir {
        Some(s) => format!("{}/DESCRIPTION", s),
        None => "DESCRIPTION".to_string(),
    };
    match crate::cache::git_mirror_dir(url) {
        Some(mirror) => {
            fetch_git_description_cached(url, refspec, known_sha, &description_path, &mirror)
        }
        None => fetch_git_description_tempdir(url, refspec, known_sha, &description_path),
    }
}

/// The persistent-mirror path for [`fetch_git_description`]: reuses (or
/// creates) the bare mirror at `mirror`, makes sure the wanted commit is
/// present in it (trusting `known_sha` if given, else resolving `refspec`
/// against the remote), and reads `path` back out with `git show` -- no
/// working tree, so no sparse-checkout and no race between concurrent calls
/// checking out different refs into the same directory.
fn fetch_git_description_cached(
    url: &str,
    refspec: Option<&str>,
    known_sha: Option<&str>,
    path: &str,
    mirror: &Path,
) -> Result<(String, String), Box<dyn Error>> {
    let _lock = lock_mirror(mirror)?;
    ensure_bare_mirror(url, mirror)?;
    let sha = match known_sha {
        Some(sha) => ensure_commit(mirror, sha),
        None => fetch_into_mirror(mirror, refspec),
    }
    .map_err(|err| {
        simple_error::SimpleError::new(format!(
            "Cannot fetch {}{}: {}",
            url,
            refspec.map(|r| format!(" at {}", r)).unwrap_or_default(),
            err
        ))
    })?;
    let contents = read_blob(mirror, &sha, path).map_err(|err| {
        simple_error::SimpleError::new(format!("Cannot read {} from {}: {}", path, url, err))
    })?;
    Ok((contents, sha))
}

/// The no-cache fallback for [`fetch_git_description`]: today's original
/// throwaway-clone behavior, used when `--no-cache`/`RIG_NO_CACHE` is set or
/// rig's cache directory can't be determined. `known_sha`, when given, is
/// fetched in place of `refspec` -- there's no persistent mirror to check it
/// against, but a lockfile-pinned commit still shouldn't drift just because
/// caching happens to be off.
fn fetch_git_description_tempdir(
    url: &str,
    refspec: Option<&str>,
    known_sha: Option<&str>,
    path: &str,
) -> Result<(String, String), Box<dyn Error>> {
    let tmp = tempfile::tempdir()?;
    let dest = tmp.path();
    let want = known_sha.or(refspec);
    init_sparse_repo(url, dest, Some((false, &[path])))?;
    let sha = fetch_and_checkout(dest, want).map_err(|err| {
        simple_error::SimpleError::new(format!(
            "Cannot fetch {}{}: {}",
            url,
            want.map(|r| format!(" at {}", r)).unwrap_or_default(),
            err
        ))
    })?;
    let contents = fs::read_to_string(dest.join(path)).map_err(|err| {
        simple_error::SimpleError::new(format!("Cannot read {} from {}: {}", path, url, err))
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
    fetch_and_checkout(dest, refspec).map_err(|err| {
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
    let client = http_client_builder()
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

    // These exercise `fetch_git_description_tempdir` directly (the
    // no-cache fallback) rather than the public `fetch_git_description`, so
    // the test doesn't depend on -- or write into -- whatever the real OS
    // cache directory happens to be on the machine running the suite. The
    // cached path has its own tests below, each pointed at an explicit
    // tempdir `mirror` instead of `crate::cache::git_mirror_dir`'s real
    // cache directory, for the same reason.

    #[test]
    #[ignore]
    fn description_only_fetch() {
        let fixture = bare_fixture();
        let url = format!("file://{}", fixture.path().join("work").display());
        let (contents, sha) =
            fetch_git_description_tempdir(&url, None, None, "DESCRIPTION").unwrap();
        assert!(contents.contains("Package: fixture"));
        assert_eq!(sha.len(), 40);
    }

    #[test]
    #[ignore]
    fn description_only_fetch_from_subdir() {
        let fixture = bare_fixture();
        let url = format!("file://{}", fixture.path().join("work").display());
        let (contents, _sha) =
            fetch_git_description_tempdir(&url, None, None, "pkg/DESCRIPTION").unwrap();
        assert!(contents.contains("Package: subpkg"));
    }

    #[test]
    #[ignore]
    fn cached_fetch_reuses_mirror() {
        let fixture = bare_fixture();
        let url = format!("file://{}", fixture.path().join("work").display());
        let mirror = tempfile::tempdir().unwrap();

        let (first, first_sha) =
            fetch_git_description_cached(&url, None, None, "DESCRIPTION", mirror.path()).unwrap();
        assert!(first.contains("Package: fixture"));

        // A second call against the same mirror must succeed and agree --
        // it should reuse the existing bare repo (`ensure_bare_mirror` is a
        // no-op once `HEAD` exists) rather than fail trying to re-`init`/
        // `remote add` over it.
        let (second, second_sha) =
            fetch_git_description_cached(&url, None, None, "DESCRIPTION", mirror.path()).unwrap();
        assert_eq!(first, second);
        assert_eq!(first_sha, second_sha);
    }

    #[test]
    #[ignore]
    fn cached_fetch_with_known_sha_skips_resolving_refspec() {
        let fixture = bare_fixture();
        let url = format!("file://{}", fixture.path().join("work").display());
        let mirror = tempfile::tempdir().unwrap();

        // Warm the mirror the ordinary way first, to learn the fixture's
        // commit sha and make sure the mirror actually has it locally.
        let (_, sha) =
            fetch_git_description_cached(&url, None, None, "DESCRIPTION", mirror.path()).unwrap();

        // A bogus `refspec` would fail to resolve against the remote -- if
        // this succeeds, `known_sha` was trusted outright and `refspec` was
        // never consulted, exactly as `ensure_commit` intends.
        let (contents, resolved_sha) = fetch_git_description_cached(
            &url,
            Some("no-such-branch"),
            Some(&sha),
            "DESCRIPTION",
            mirror.path(),
        )
        .unwrap();
        assert!(contents.contains("Package: fixture"));
        assert_eq!(resolved_sha, sha);
    }

    #[test]
    #[ignore]
    fn cached_fetch_from_two_subdirs_reuses_one_mirror() {
        let fixture = bare_fixture();
        let url = format!("file://{}", fixture.path().join("work").display());
        let mirror = tempfile::tempdir().unwrap();

        let (root, _) =
            fetch_git_description_cached(&url, None, None, "DESCRIPTION", mirror.path()).unwrap();
        assert!(root.contains("Package: fixture"));

        // No working tree/sparse-checkout state to reset between calls: a
        // different path from the same URL just reads a different blob out
        // of the same already-fetched mirror.
        let (subdir, _) =
            fetch_git_description_cached(&url, None, None, "pkg/DESCRIPTION", mirror.path())
                .unwrap();
        assert!(subdir.contains("Package: subpkg"));
    }

    #[test]
    #[ignore]
    fn concurrent_cached_fetches_dont_corrupt() {
        let fixture = bare_fixture();
        let url = format!("file://{}", fixture.path().join("work").display());
        let mirror = tempfile::tempdir().unwrap();
        let mirror_path = mirror.path().to_path_buf();

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let url = url.clone();
                let mirror_path = mirror_path.clone();
                std::thread::spawn(move || {
                    fetch_git_description_cached(&url, None, None, "DESCRIPTION", &mirror_path)
                        .map_err(|err| err.to_string())
                })
            })
            .collect();

        for handle in handles {
            let (contents, sha) = handle.join().unwrap().unwrap();
            assert!(contents.contains("Package: fixture"));
            assert_eq!(sha.len(), 40);
        }
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
