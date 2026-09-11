//! Fetching a `github::`/bare `owner/repo` package source: resolve a
//! branch/tag/commit/PR/release to a commit sha via the GitHub REST API, then
//! download the repository tarball at that sha from `codeload.github.com`.
//! No `git` clone is involved, so this needs neither a system `git` binary
//! nor `gix`.

use std::error::Error;
use std::path::Path;
use std::time::Duration;

use simple_error::bail;

use crate::download::download_first_available_;

/// What to resolve a GitHub reference to a commit from -- mirrors
/// [`crate::pkgsource::RemoteSource`]'s `rev`/`pr`/`release` fields.
pub enum GithubDetail<'a> {
    /// The repository's default branch.
    Default,
    /// A branch, tag, or commit prefix.
    Ref(&'a str),
    /// A pull request number.
    PullRequest(u32),
    /// The latest release.
    Release,
}

pub struct ResolvedGithub {
    pub sha: String,
    /// The ref actually used, for the manifest/lockfile's `RemoteRef`.
    pub resolved_ref: String,
}

fn api_get(url: &str) -> Result<serde_json::Value, Box<dyn Error>> {
    api_get_(url)
}

#[tokio::main]
async fn api_get_(url: &str) -> Result<serde_json::Value, Box<dyn Error>> {
    let client = reqwest::Client::builder().user_agent("rig").build()?;
    let resp = client.get(url).send().await?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        bail!("GitHub repository or ref not found: {}", url);
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        bail!(
            "GitHub API request to {} was refused ({}). rig only supports unauthenticated \
             access to public repositories in this version, so this may be GitHub's \
             anonymous rate limit or a private repository.",
            url,
            status
        );
    }
    if !status.is_success() {
        bail!("GitHub API request to {} failed: {}", url, status);
    }
    Ok(resp.json().await?)
}

fn field<'a>(
    json: &'a serde_json::Value,
    path: &[&str],
    url: &str,
) -> Result<&'a str, Box<dyn Error>> {
    let mut v = json;
    for key in path {
        v = v.get(key).ok_or_else(|| {
            simple_error::SimpleError::new(format!(
                "Unexpected GitHub API response for {} (missing `{}`)",
                url, key
            ))
        })?;
    }
    v.as_str().ok_or_else(|| {
        simple_error::SimpleError::new(format!("Unexpected GitHub API response for {}", url)).into()
    })
}

/// Resolve `detail` on `owner/repo` to a commit sha.
pub fn resolve_github_ref(
    owner: &str,
    repo: &str,
    detail: &GithubDetail,
) -> Result<ResolvedGithub, Box<dyn Error>> {
    match detail {
        GithubDetail::PullRequest(n) => {
            let url = format!(
                "https://api.github.com/repos/{}/{}/pulls/{}",
                owner, repo, n
            );
            let json = api_get(&url)?;
            let sha = field(&json, &["head", "sha"], &url)?.to_string();
            Ok(ResolvedGithub {
                sha,
                resolved_ref: format!("#{}", n),
            })
        }
        GithubDetail::Release => {
            let url = format!(
                "https://api.github.com/repos/{}/{}/releases/latest",
                owner, repo
            );
            let json = api_get(&url)?;
            let tag = field(&json, &["tag_name"], &url)?.to_string();

            let commit_url = format!(
                "https://api.github.com/repos/{}/{}/commits/{}",
                owner, repo, tag
            );
            let commit = api_get(&commit_url)?;
            let sha = field(&commit, &["sha"], &commit_url)?.to_string();
            Ok(ResolvedGithub {
                sha,
                resolved_ref: tag,
            })
        }
        GithubDetail::Ref(r) => {
            let url = format!(
                "https://api.github.com/repos/{}/{}/commits/{}",
                owner, repo, r
            );
            let json = api_get(&url)?;
            let sha = field(&json, &["sha"], &url)?.to_string();
            Ok(ResolvedGithub {
                sha,
                resolved_ref: r.to_string(),
            })
        }
        // GitHub's single-commit endpoint (`/commits/<ref>`) needs an actual ref
        // -- an empty one is not "the default branch", it is an invalid path and
        // the API answers 422. So the default branch's name is looked up first
        // (one extra request, only for this case) and used as the ref.
        GithubDetail::Default => {
            let repo_url = format!("https://api.github.com/repos/{}/{}", owner, repo);
            let repo_json = api_get(&repo_url)?;
            let branch = field(&repo_json, &["default_branch"], &repo_url)?.to_string();

            let url = format!(
                "https://api.github.com/repos/{}/{}/commits/{}",
                owner, repo, branch
            );
            let json = api_get(&url)?;
            let sha = field(&json, &["sha"], &url)?.to_string();
            Ok(ResolvedGithub {
                sha,
                resolved_ref: branch,
            })
        }
    }
}

/// Download the repository tarball at `sha` to `dest`, content-addressed so
/// callers can treat an existing file at `dest` as an unconditional cache hit.
pub fn download_tarball(
    owner: &str,
    repo: &str,
    sha: &str,
    dest: &Path,
) -> Result<(), Box<dyn Error>> {
    let url = format!(
        "https://codeload.github.com/{}/{}/tar.gz/{}",
        owner, repo, sha
    );
    download_first_available_(
        &[&url],
        &dest.to_path_buf(),
        Some(Duration::MAX),
        None,
        None,
    )?;
    Ok(())
}
