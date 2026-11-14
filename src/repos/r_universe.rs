//! The git commit of the packages of an r-universe repository.
//!
//! r-universe builds every package from a git repository, and replaces the
//! files of a package when the repository changes, even if the version stays
//! the same. So a locked r-universe file may be gone later. The lock file
//! records the commit the file was built from, too, and `rig proj sync`
//! builds the package from git at that commit if the file is gone or
//! changed.
//!
//! The `PACKAGES` files of r-universe do not have the commit, its API does,
//! at `<universe>/api/packages/<package>`: `RemoteSha`, `RemoteUrl`, etc. of
//! the current source package, and the commit of every binary build, in
//! `_binaries`.

use std::collections::HashMap;
use std::sync::Mutex;

use futures::future;
use log::debug;
use serde::Deserialize;

use crate::utils::http_client;

/// Where an r-universe package was built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RUniverseRemote {
    /// The git commit.
    pub sha: String,
    /// The git repository.
    pub url: String,
    /// The directory of the package in the git repository, if not the root.
    pub subdir: Option<String>,
    /// The branch or tag the package was built from, e.g. `HEAD`.
    pub ref_: Option<String>,
}

/// The fields of `api/packages/<package>` that rig needs.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ApiPackage {
    #[serde(rename = "Version")]
    pub version: String,
    #[serde(rename = "RemoteSha")]
    pub remote_sha: Option<String>,
    #[serde(rename = "RemoteUrl")]
    pub remote_url: Option<String>,
    #[serde(rename = "RemoteSubdir")]
    pub remote_subdir: Option<String>,
    #[serde(rename = "RemoteRef")]
    pub remote_ref: Option<String>,
    /// The sha256 of the source package file.
    #[serde(rename = "_sha256")]
    pub sha256: Option<String>,
    #[serde(rename = "_binaries", default)]
    pub binaries: Vec<ApiBinary>,
}

/// One binary build of an r-universe package.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ApiBinary {
    pub version: Option<String>,
    pub commit: Option<String>,
    /// The URL of the file, ending in its sha256.
    pub fileid: Option<String>,
}

/// Where the file with sha256 `sha256` of version `version` was built from,
/// according to `api`. The file is the source package, or one of the binary
/// builds. `None` if `api` is about another version, or no file of it has
/// that sha256: r-universe has rebuilt the package since its `PACKAGES` file
/// was read.
pub fn remote_for(api: &ApiPackage, version: &str, sha256: &str) -> Option<RUniverseRemote> {
    if api.version != version {
        return None;
    }
    let sha = if api.sha256.as_deref() == Some(sha256) {
        api.remote_sha.clone()?
    } else {
        api.binaries
            .iter()
            .find(|b| {
                b.version.as_deref() == Some(version)
                    && b.fileid.as_deref().is_some_and(|f| f.ends_with(sha256))
            })
            .and_then(|b| b.commit.clone())?
    };
    Some(RUniverseRemote {
        sha,
        url: api.remote_url.clone()?,
        subdir: api.remote_subdir.clone().filter(|s| !s.is_empty()),
        ref_: api.remote_ref.clone(),
    })
}

/// The sha256 of an r-universe file, from the `sha256` parameter of its URL,
/// e.g. `cli_3.6.6.9000.tgz?sha256=22b2...&file=...`. For a binary package
/// this is the sha256 of the binary, while rig's `RemoteHash` is the sha256 of
/// the source package it was built from.
pub fn url_sha256(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    parsed
        .query_pairs()
        .find(|(k, _)| k == "sha256")
        .map(|(_, v)| v.to_string())
        .filter(|v| !v.is_empty())
}

/// A package file to look up, see [`lookup_remotes`].
#[derive(Debug, Clone)]
pub struct RemoteRequest {
    /// The URL of the r-universe repository, without a trailing `/`.
    pub repo_url: String,
    pub package: String,
    pub version: String,
    /// The sha256 of the file.
    pub sha256: String,
}

lazy_static::lazy_static! {
    /// The API answers of this process, by URL, so that the several targets
    /// of a lock ask about each package once.
    static ref API_CACHE: Mutex<HashMap<String, Option<ApiPackage>>> = Mutex::new(HashMap::new());
}

/// What one request to the API found.
enum ApiAnswer {
    Found(ApiPackage),
    /// The API does not know the package, or answered something else. Asking
    /// again would not help.
    Missing,
    /// No answer, e.g. a network error. Asking again might help.
    Failed,
}

async fn ask_api(client: &reqwest::Client, url: &str) -> ApiAnswer {
    let resp = match crate::auth::get(client, url).send().await {
        Ok(resp) => resp,
        Err(e) => {
            debug!("Cannot get {}: {}", url, e);
            return ApiAnswer::Failed;
        }
    };
    let status = resp.status();
    if crate::download::is_transient_status(status) {
        debug!("{} returned HTTP status {}", url, status);
        return ApiAnswer::Failed;
    }
    if !status.is_success() {
        debug!("{} returned HTTP status {}", url, status);
        return ApiAnswer::Missing;
    }
    let body = match resp.bytes().await {
        Ok(body) => body,
        Err(e) => {
            debug!("Cannot read the answer from {}: {}", url, e);
            return ApiAnswer::Failed;
        }
    };
    match serde_json::from_slice::<ApiPackage>(&body) {
        Ok(api) => ApiAnswer::Found(api),
        Err(e) => {
            debug!("Invalid answer from {}: {}", url, e);
            ApiAnswer::Missing
        }
    }
}

/// The API's answer about a package, at `url`. A request that fails is
/// tried again, a few times. Answers are remembered for the process, except
/// failed requests.
async fn fetch_api_package(client: &reqwest::Client, url: &str) -> Option<ApiPackage> {
    if let Some(cached) = API_CACHE.lock().unwrap().get(url) {
        return cached.clone();
    }
    use crate::download::{retry_delay, RETRY_DELAYS};
    let mut delays = RETRY_DELAYS.iter();
    loop {
        let result = match ask_api(client, url).await {
            ApiAnswer::Found(api) => Some(api),
            ApiAnswer::Missing => None,
            ApiAnswer::Failed => match delays.next() {
                Some(base) => {
                    tokio::time::sleep(retry_delay(*base)).await;
                    continue;
                }
                None => return None,
            },
        };
        API_CACHE
            .lock()
            .unwrap()
            .insert(url.to_string(), result.clone());
        return result;
    }
}

/// Where each file of `requests` was built from, in the same order. `None`
/// for the ones the API does not know about.
#[tokio::main]
pub async fn lookup_remotes(requests: &[RemoteRequest]) -> Vec<Option<RUniverseRemote>> {
    let client = http_client();
    let client = &client;
    let url = |req: &RemoteRequest| format!("{}/api/packages/{}", req.repo_url, req.package);
    // Each package once, also if it is asked about for several files.
    let mut urls: Vec<String> = requests.iter().map(url).collect();
    urls.sort();
    urls.dedup();
    let answers = future::join_all(urls.iter().map(|u| fetch_api_package(client, u))).await;
    let answers: HashMap<&String, Option<ApiPackage>> = urls.iter().zip(answers).collect();
    requests
        .iter()
        .map(|req| {
            let api = answers.get(&url(req))?.as_ref()?;
            remote_for(api, &req.version, &req.sha256)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli() -> ApiPackage {
        ApiPackage {
            version: "3.6.6.9000".to_string(),
            remote_sha: Some("ceadbf".to_string()),
            remote_url: Some("https://github.com/r-lib/cli".to_string()),
            remote_subdir: None,
            remote_ref: Some("HEAD".to_string()),
            sha256: Some("src-sha".to_string()),
            binaries: vec![ApiBinary {
                version: Some("3.6.6.9000".to_string()),
                commit: Some("ceadbf".to_string()),
                fileid: Some("https://r2.ropensci.org/bin-sha".to_string()),
            }],
        }
    }

    #[test]
    fn the_source_package_has_the_remote_sha() {
        let remote = remote_for(&cli(), "3.6.6.9000", "src-sha").unwrap();
        assert_eq!(
            remote,
            RUniverseRemote {
                sha: "ceadbf".to_string(),
                url: "https://github.com/r-lib/cli".to_string(),
                subdir: None,
                ref_: Some("HEAD".to_string()),
            }
        );
    }

    #[test]
    fn a_binary_has_its_own_commit() {
        let mut api = cli();
        api.binaries[0].commit = Some("older".to_string());
        let remote = remote_for(&api, "3.6.6.9000", "bin-sha").unwrap();
        assert_eq!(remote.sha, "older");
    }

    #[test]
    fn another_version_or_file_has_no_remote() {
        assert_eq!(remote_for(&cli(), "3.6.5", "src-sha"), None);
        assert_eq!(remote_for(&cli(), "3.6.6.9000", "other-sha"), None);
        let mut api = cli();
        api.remote_url = None;
        assert_eq!(remote_for(&api, "3.6.6.9000", "src-sha"), None);
    }

    #[test]
    fn remotes_are_looked_up_in_the_api() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let rt = tokio::runtime::Runtime::new().unwrap();
        let server = rt.block_on(MockServer::start());
        let body = r#"{"Version": "1.0", "RemoteSha": "abc",
            "RemoteUrl": "https://github.com/o/a", "_sha256": "s"}"#;
        rt.block_on(
            Mock::given(method("GET"))
                .and(path("/api/packages/a"))
                .respond_with(ResponseTemplate::new(200).set_body_string(body))
                .expect(1)
                .mount(&server),
        );
        let req = |package: &str, sha256: &str| RemoteRequest {
            repo_url: server.uri(),
            package: package.to_string(),
            version: "1.0".to_string(),
            sha256: sha256.to_string(),
        };
        let remotes = lookup_remotes(&[req("a", "s"), req("a", "other"), req("b", "s")]);
        assert_eq!(remotes[0].as_ref().map(|r| r.sha.as_str()), Some("abc"));
        // Another build of `a`, and a package the API does not know.
        assert_eq!(remotes[1], None);
        assert_eq!(remotes[2], None);
        // Asked once, the second answer is remembered.
        let again = lookup_remotes(&[req("a", "s")]);
        assert!(again[0].is_some());
        rt.block_on(server.verify());
    }

    #[test]
    fn sha256_from_the_url() {
        assert_eq!(
            url_sha256("https://x.r-universe.dev/bin/a_1.0.tgz?sha256=abc&file=/a_1.0.tgz")
                .as_deref(),
            Some("abc")
        );
        assert_eq!(
            url_sha256("https://x.r-universe.dev/src/contrib/a_1.0.tar.gz"),
            None
        );
        assert_eq!(url_sha256("not a url"), None);
    }

    #[test]
    fn api_answer_parses() {
        let json = r#"{
            "Package": "cli", "Version": "1.0", "RemoteSha": "abc",
            "RemoteUrl": "https://github.com/r-lib/cli", "RemoteSubdir": "pkg",
            "_sha256": "s", "_binaries": [{"version": "1.0", "commit": "abc",
            "fileid": "https://x/b", "os": "mac"}], "_other": 1
        }"#;
        let api: ApiPackage = serde_json::from_str(json).unwrap();
        let remote = remote_for(&api, "1.0", "s").unwrap();
        assert_eq!(remote.subdir.as_deref(), Some("pkg"));
        assert_eq!(remote.ref_, None);
    }
}
