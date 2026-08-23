use futures::future;
use futures::stream::{FuturesUnordered, StreamExt};
use std::error::Error;
use std::ffi::OsStr;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::ffi::OsString;
use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::SystemTime;

#[cfg(target_os = "windows")]
use clap::ArgMatches;

use filetime::FileTime;
use log::*;
use reqwest::StatusCode;
use simple_error::bail;

use crate::output::OUTPUT;
#[cfg(target_os = "windows")]
use crate::resolve::get_resolve;
#[cfg(target_os = "windows")]
use crate::rversion::Rversion;
use crate::utils::write_atomically;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use crate::utils::*;

// ------------------------------------------------------------------------
// synchronous API
// ------------------------------------------------------------------------

#[cfg(target_os = "windows")]
pub fn download_r(args: &ArgMatches) -> Result<(Rversion, OsString), Box<dyn Error>> {
    let version = get_resolve(args)?;
    let version2 = version.to_owned();
    let ver = version2.version;
    let url: String = match &version.url {
        Some(s) => s.to_string(),
        None => {
            OUTPUT.error(&format!(
                "Cannot find a download url for R version {}",
                ver.as_ref().unwrap_or(&"???".to_string())
            ));
            error!(
                "Cannot find a download url for R version {}",
                ver.as_ref().unwrap_or(&"???".to_string())
            );
            bail!(
                "Cannot find a download url for R version {}",
                ver.unwrap_or("???".to_string())
            )
        }
    };
    let mut filename = OsString::new();
    filename.push(version2.arch.unwrap_or("".to_string()));
    filename.push("-");
    filename.push(basename(&url).unwrap_or("foo"));
    let filename_path = Path::new(&filename);
    let tmp_dir = crate::cache::ensure_download_dir()?;
    let target = tmp_dir.join(&filename);
    if target.exists() && not_too_old(&target) {
        OUTPUT.success(&format!(
            "{} is cached at {}",
            filename_path.display(),
            target.display()
        ));
        info!(
            "{} is cached at {}",
            filename_path.display(),
            target.display()
        );
    } else {
        OUTPUT.status(&format!("Downloading {} -> {}", url, target.display()));
        info!("Downloading {} -> {}", url, target.display());
        let client = &reqwest::Client::new();
        download_file(client, &url, target.as_os_str())?;
    }

    Ok((version, target.into_os_string()))
}

#[cfg(target_os = "macos")]
pub fn download_file_sync(
    url: &str,
    filename: &str,
    infinite_cache: bool,
) -> Result<OsString, Box<dyn Error>> {
    let tmp_dir = crate::cache::ensure_download_dir()?;
    let target = tmp_dir.join(filename);
    // `infinite_cache` goes around `not_too_old()`, so `--no-cache` has to be
    // checked here as well and not only there.
    let cached =
        target.exists() && !crate::cache::no_cache() && (infinite_cache || not_too_old(&target));
    if cached {
        OUTPUT.success(&format!("{} is cached at {}", filename, target.display()));
        info!("{} is cached at {}", filename, target.display());
    } else {
        OUTPUT.status(&format!("Downloading {} -> {}", url, target.display()));
        info!("Downloading {} -> {}", url, target.display());
        let client = &reqwest::Client::new();
        download_file(client, url, target.as_os_str())?;
    }

    Ok(target.into_os_string())
}

#[tokio::main]
pub async fn download_file(
    client: &reqwest::Client,
    url: &str,
    opath: &OsStr,
) -> Result<(), Box<dyn Error>> {
    let mut path = opath.to_os_string();
    path.push(".tmp");
    let path = Path::new(&path);
    let resp = client.get(url).send().await;
    let resp = match resp {
        Ok(resp) => resp.error_for_status(),
        Err(err) => {
            OUTPUT.error(&format!("HTTP error at {}: {}", url, err));
            error!("HTTP error at {}: {}", url, err);
            bail!("HTTP error at {}: {}", url, err.to_string())
        }
    };
    let resp = match resp {
        Ok(resp) => resp,
        Err(err) => {
            OUTPUT.error(&format!("HTTP error at {}: {}", url, err));
            error!("HTTP error at {}: {}", url, err);
            bail!("HTTP error at {}: {}", url, err.to_string())
        }
    };

    // If dirname(path) is / then this is None
    let dir = Path::new(&path).parent();
    if let Some(dir) = dir {
        if let Err(err) = std::fs::create_dir_all(dir) {
            let dir = dir.to_str().unwrap_or("???");
            OUTPUT.error(&format!("Cannot create directory {}: {}", dir, err));
            error!("Cannot create directory {}: {}", dir, err);
            bail!("Cannot create directory {}: {}", dir, err.to_string())
        };
    };
    let file = File::create(path);
    let mut file = match file {
        Ok(file) => file,
        Err(err) => {
            OUTPUT.error(&format!("Cannot create file '{}': {}", path.display(), err));
            error!("Cannot create file '{}': {}", path.display(), err);
            bail!(
                "Cannot create file '{}': {}",
                path.display(),
                err.to_string()
            )
        }
    };
    let mut stream = resp.bytes_stream();

    while let Some(item) = stream.next().await {
        let chunk = match item {
            Ok(chunk) => chunk,
            Err(err) => {
                OUTPUT.error(&format!("HTTP error at {}: {}", url, err));
                error!("HTTP error at {}: {}", url, err);
                bail!("HTTP error at {}: {}", url, err.to_string())
            }
        };
        if let Err(err) = file.write(&chunk) {
            OUTPUT.error(&format!(
                "Failed to write to file {}: {}",
                path.display(),
                err
            ));
            error!("Failed to write to file {}: {}", path.display(), err);
            bail!(
                "Failed to write to file {}: {}",
                path.display(),
                err.to_string()
            )
        };
    }

    if let Err(err) = std::fs::rename(Path::new(&path), Path::new(&opath)) {
        OUTPUT.error(&format!("Failed to rename downloaded file: {}", err));
        error!("Failed to rename downloaded file: {}", err);
        bail!("Failed to rename downloaded file: {}", err.to_string())
    };

    Ok(())
}

pub fn download_json_sync(urls: Vec<String>) -> Result<Vec<serde_json::Value>, Box<dyn Error>> {
    let client = reqwest::Client::new();
    let client = &client;
    let resp = download_json_(client, urls)?;
    Ok(resp)
}

async fn download_if_newer(
    client: &reqwest::Client,
    url: &str,
    local_path: &PathBuf,
    etag: Option<&str>,
) -> Result<(bool, Option<String>), Box<dyn Error>> {
    let mut req = client.get(url);
    if local_path.exists() {
        if let Some(etag_value) = etag {
            req = req.header("If-None-Match", etag_value);
        }
    }
    info!("Checking for updates for {}", local_path.display());
    let resp = req.send().await?;

    match resp.status() {
        StatusCode::NOT_MODIFIED => {
            filetime::set_file_mtime(local_path, FileTime::now())?;
            Ok((false, None))
        }

        StatusCode::OK => {
            // 200 → new content
            // Extract etag from response headers
            let new_etag = resp
                .headers()
                .get("etag")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());

            let bytes = resp.bytes().await?;
            write_atomically(local_path, &bytes)?;
            Ok((true, new_etag))
        }

        status => {
            OUTPUT.error(&format!("Failed to download {}, status: {}", url, status));
            error!("Failed to download {}, status: {}", url, status);
            bail!("Failed to download {}, status: {}", url, status);
        }
    }
}

#[tokio::main]
pub async fn download_if_newer__(
    url: &str,
    local_path: &PathBuf,
    client: Option<&reqwest::Client>,
) -> Result<(bool, Option<String>), Box<dyn Error>> {
    let client_ = match client {
        Some(c) => c,
        None => &reqwest::Client::new(),
    };
    download_if_newer(client_, url, local_path, None).await
}

pub fn download_if_newer_(
    url: &str,
    local_path: &PathBuf,
    update_older: Option<Duration>,
    client: Option<&reqwest::Client>,
) -> Result<(bool, Option<String>), Box<dyn Error>> {
    let update_older = match update_older {
        Some(dur) => dur,
        None => Duration::from_hours(24),
    };

    if local_path.exists() {
        let metadata = fs::metadata(local_path)?;
        let modified = metadata.modified()?;
        let elapsed = SystemTime::now().duration_since(modified)?;

        if elapsed < update_older {
            // File is newer than the threshold, skip update
            info!("{} is up to date, skipping download", local_path.display());
            return Ok((false, None));
        }
    }

    download_if_newer__(url, local_path, client)
}

/// Try to download from multiple URLs, using the first one that succeeds (async).
async fn download_first_available(
    client: &reqwest::Client,
    urls: &[&str],
    local_path: &PathBuf,
    etag: Option<&str>,
) -> Result<(bool, Option<String>), Box<dyn Error>> {
    let mut last_error = None;

    for url in urls {
        info!("Trying to download from {}", url);
        match download_if_newer(client, url, local_path, etag).await {
            Ok((downloaded, etag)) => {
                info!("Successfully downloaded from {}", url);
                return Ok((downloaded, etag));
            }
            Err(e) => {
                warn!("Failed to download from {}: {}", url, e);
                last_error = Some(e);
            }
        }
    }

    match last_error {
        Some(e) => {
            OUTPUT.error("All download URLs failed.");
            error!("All download URLs failed. Last error: {}", e);
            bail!("All download URLs failed. Last error: {}", e)
        }
        None => {
            OUTPUT.error("No URLs provided.");
            error!("No URLs provided.");
            bail!("No URLs provided")
        }
    }
}

/// Try to download from multiple URLs, using the first one that succeeds (sync wrapper).
/// Returns Ok((true, etag)) if a new file was downloaded, Ok((false, None)) if existing file is up to date,
/// or Err if all URLs failed.
pub fn download_first_available_(
    urls: &[&str],
    local_path: &PathBuf,
    update_older: Option<Duration>,
    client: Option<&reqwest::Client>,
    etag: Option<&str>,
) -> Result<(bool, Option<String>), Box<dyn Error>> {
    let update_older = match update_older {
        Some(dur) => dur,
        None => Duration::from_hours(24),
    };

    if local_path.exists() {
        let metadata = fs::metadata(local_path)?;
        let modified = metadata.modified()?;
        let elapsed = SystemTime::now().duration_since(modified)?;

        if elapsed < update_older {
            // File is newer than the threshold, skip update
            info!("{} is up to date, skipping download", local_path.display());
            return Ok((false, None));
        }
    }

    let client_ = match client {
        Some(c) => c,
        None => &reqwest::Client::new(),
    };

    download_first_available__(client_, urls, local_path, etag)
}

/// What a conditional fetch found. See [`fetch_optional_if_modified_`].
pub enum ConditionalFetch {
    /// The server has no such resource (404).
    NotFound,
    /// The server confirmed the caller's copy is still current (304). There is
    /// no body to go with this.
    NotModified,
    /// New content, with the `ETag` identifying it if the server sent one.
    Fetched {
        bytes: Vec<u8>,
        etag: Option<String>,
    },
}

/// Conditionally fetch a resource into memory.
///
/// Like `download_optional_if_newer_`, but nothing is written to disk and
/// there is no TTL check: the caller decides when to call this and what to
/// keep. That is what you want when the downloaded bytes are not the artifact
/// being cached — the binary indices are parsed and stored in a different
/// format entirely, so writing the response out only to delete it again would
/// be pure overhead.
///
/// Passing `etag` sends `If-None-Match`. Only do that while you still hold the
/// content it describes: a 304 comes with no body, so asking for one you
/// cannot use leaves you with nothing.
pub fn fetch_optional_if_modified_(
    url: &str,
    etag: Option<&str>,
    client: Option<&reqwest::Client>,
) -> Result<ConditionalFetch, Box<dyn Error>> {
    let client_ = match client {
        Some(c) => c,
        None => &reqwest::Client::new(),
    };
    fetch_optional_if_modified__(client_, url, etag)
}

#[tokio::main]
async fn fetch_optional_if_modified__(
    client: &reqwest::Client,
    url: &str,
    etag: Option<&str>,
) -> Result<ConditionalFetch, Box<dyn Error>> {
    fetch_optional_if_modified(client, url, etag).await
}

/// The async form of [`fetch_optional_if_modified_`], for callers that already
/// have a runtime and want several of these in flight at once.
///
/// An unexpected status is returned as an error and only logged, never printed:
/// every caller of this decides for itself whether a failed metadata fetch is
/// worth telling the user about, and for the speculative ones it is not.
pub async fn fetch_optional_if_modified(
    client: &reqwest::Client,
    url: &str,
    etag: Option<&str>,
) -> Result<ConditionalFetch, Box<dyn Error>> {
    let mut req = client.get(url);
    if let Some(etag) = etag {
        req = req.header("If-None-Match", etag);
    }
    info!("Checking for updates for {}", url);
    let resp = req.send().await?;

    match resp.status() {
        StatusCode::NOT_MODIFIED => Ok(ConditionalFetch::NotModified),

        StatusCode::OK => {
            let etag = resp
                .headers()
                .get("etag")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let bytes = resp.bytes().await?.to_vec();
            Ok(ConditionalFetch::Fetched { bytes, etag })
        }

        StatusCode::NOT_FOUND => {
            debug!("No such resource (404): {}", url);
            Ok(ConditionalFetch::NotFound)
        }

        status => {
            error!("Failed to download {}, status: {}", url, status);
            bail!("Failed to download {}, status: {}", url, status);
        }
    }
}

// ------------------------------------------------------------------------
// probing URLs
// ------------------------------------------------------------------------

/// How long a single probe may take before it is reported as a timeout.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// What a URL probe found. See [`probe_url`].
///
/// A probe never fails: an unreachable host, a TLS error or a timeout is a
/// result to report, not an error to propagate, because the caller is asking
/// about the state of the server in the first place.
#[derive(Debug, Clone)]
pub struct UrlProbe {
    pub url: String,
    /// The HTTP status code, or `None` if the request never got a response.
    pub status: Option<u16>,
    /// The transport error, if the request never got a response.
    pub error: Option<String>,
    /// Time from sending the request to having the response headers. The body
    /// is never read, so this does not include the size of the resource.
    pub elapsed_ms: u128,
    /// The `Last-Modified` header, verbatim, if the server sent one.
    pub last_modified: Option<String>,
}

/// Ask a server about a resource, without downloading it: a `HEAD` request,
/// timed, keeping the status and the `Last-Modified` header.
///
/// Some servers do not implement `HEAD` (405, 501); those are retried once as a
/// single-byte ranged `GET`, which every static file server answers.
pub async fn probe_url(client: &reqwest::Client, url: &str) -> UrlProbe {
    info!("Probing {}", url);
    let start = std::time::Instant::now();
    let mut resp = client.head(url).timeout(PROBE_TIMEOUT).send().await;

    if let Ok(r) = &resp {
        if r.status() == StatusCode::METHOD_NOT_ALLOWED || r.status() == StatusCode::NOT_IMPLEMENTED
        {
            debug!("HEAD not supported by {}, retrying with a ranged GET", url);
            resp = client
                .get(url)
                .header("Range", "bytes=0-0")
                .timeout(PROBE_TIMEOUT)
                .send()
                .await;
        }
    }

    let elapsed_ms = start.elapsed().as_millis();

    match resp {
        Ok(resp) => {
            let last_modified = resp
                .headers()
                .get("last-modified")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            UrlProbe {
                url: url.to_string(),
                status: Some(resp.status().as_u16()),
                error: None,
                elapsed_ms,
                last_modified,
            }
        }
        Err(err) => {
            debug!("Failed to probe {}: {}", url, err);
            UrlProbe {
                url: url.to_string(),
                status: None,
                error: Some(probe_error_message(&err)),
                elapsed_ms,
                last_modified: None,
            }
        }
    }
}

/// A short, printable description of why a probe got no response.
///
/// `reqwest`'s own `Display` is a chain of wrapper types
/// ("error sending request for url (...): ..."), which is too long for a table
/// cell, so the interesting cases get their own word and everything else falls
/// back to the innermost source.
fn probe_error_message(err: &reqwest::Error) -> String {
    if err.is_timeout() {
        return "timeout".to_string();
    }
    if err.is_connect() {
        return "cannot connect".to_string();
    }
    if err.is_redirect() {
        return "too many redirects".to_string();
    }

    let mut src: &dyn Error = err;
    while let Some(next) = src.source() {
        src = next;
    }
    src.to_string()
}

/// Probe several URLs at once, returning the results in the order of the input.
#[tokio::main]
pub async fn probe_urls_(urls: &[String]) -> Vec<UrlProbe> {
    // `reqwest::Client::new()` has no timeout of its own, and one shared client
    // means one connection pool for the repositories that share a host.
    let client = match reqwest::Client::builder().timeout(PROBE_TIMEOUT).build() {
        Ok(c) => c,
        Err(_) => reqwest::Client::new(),
    };
    let client = &client;
    future::join_all(
        urls.iter()
            .map(|url| async move { probe_url(client, url).await }),
    )
    .await
}

/// Like `download_if_newer`, but a 404 is a normal outcome rather than an
/// error: it returns `Ok(None)` instead of failing and printing to the
/// terminal. Used for optional per-package metadata that simply may not exist
/// for a given package.
///
/// Nothing is written to `local_path` unless the server answers 200, which
/// matters because the 404 response body is a ~27 KB HTML error page.
#[allow(clippy::type_complexity)]
async fn download_optional_if_newer(
    client: &reqwest::Client,
    url: &str,
    local_path: &PathBuf,
    etag: Option<&str>,
) -> Result<Option<(bool, Option<String>)>, Box<dyn Error>> {
    let mut req = client.get(url);
    if local_path.exists() {
        if let Some(etag_value) = etag {
            req = req.header("If-None-Match", etag_value);
        }
    }
    info!("Checking for updates for {}", local_path.display());
    let resp = req.send().await?;

    match resp.status() {
        StatusCode::NOT_MODIFIED => {
            filetime::set_file_mtime(local_path, FileTime::now())?;
            Ok(Some((false, None)))
        }

        StatusCode::OK => {
            let new_etag = resp
                .headers()
                .get("etag")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());

            let bytes = resp.bytes().await?;
            write_atomically(local_path, &bytes)?;
            Ok(Some((true, new_etag)))
        }

        StatusCode::NOT_FOUND => {
            debug!("No such resource (404): {}", url);
            Ok(None)
        }

        status => {
            OUTPUT.error(&format!("Failed to download {}, status: {}", url, status));
            error!("Failed to download {}, status: {}", url, status);
            bail!("Failed to download {}, status: {}", url, status);
        }
    }
}

#[tokio::main]
#[allow(clippy::type_complexity)]
async fn download_optional_if_newer__(
    client: &reqwest::Client,
    url: &str,
    local_path: &PathBuf,
    etag: Option<&str>,
) -> Result<Option<(bool, Option<String>)>, Box<dyn Error>> {
    download_optional_if_newer(client, url, local_path, etag).await
}

/// Sync wrapper for `download_optional_if_newer`.
///
/// Returns `Ok(None)` if the server answered 404, `Ok(Some((true, etag)))` if a
/// new file was downloaded, and `Ok(Some((false, None)))` if the local copy is
/// still considered up to date (either younger than `update_older`, or
/// revalidated with a 304).
#[allow(clippy::type_complexity)]
pub fn download_optional_if_newer_(
    url: &str,
    local_path: &PathBuf,
    update_older: Option<Duration>,
    client: Option<&reqwest::Client>,
    etag: Option<&str>,
) -> Result<Option<(bool, Option<String>)>, Box<dyn Error>> {
    let update_older = match update_older {
        Some(dur) => dur,
        None => Duration::from_hours(24),
    };

    if local_path.exists() {
        let metadata = fs::metadata(local_path)?;
        let modified = metadata.modified()?;
        let elapsed = SystemTime::now().duration_since(modified)?;

        if elapsed < update_older {
            // File is newer than the threshold, skip update
            info!("{} is up to date, skipping download", local_path.display());
            return Ok(Some((false, None)));
        }
    }

    let client_ = match client {
        Some(c) => c,
        None => &reqwest::Client::new(),
    };

    download_optional_if_newer__(client_, url, local_path, etag)
}

/// Download multiple files concurrently, each from a list of candidate URLs.
/// Each download will try its URLs in order until one succeeds.
/// Returns a vector of results, one for each download request.
/// Each result is Ok((true, etag)) if downloaded, Ok((false, None)) if cached, or Err if all URLs failed.
#[cfg(test)]
#[allow(clippy::type_complexity)]
pub fn download_multiple_first_available_(
    downloads: Vec<(Vec<String>, PathBuf)>,
    update_older: Option<Duration>,
    client: Option<&reqwest::Client>,
) -> Vec<Result<(bool, Option<String>), Box<dyn Error>>> {
    let update_older = match update_older {
        Some(dur) => dur,
        None => Duration::from_hours(24),
    };

    let client_ = match client {
        Some(c) => c,
        None => &reqwest::Client::new(),
    };

    download_multiple_first_available__(client_, downloads, update_older)
}

#[cfg(test)]
#[tokio::main]
#[allow(clippy::type_complexity)]
async fn download_multiple_first_available__(
    client: &reqwest::Client,
    downloads: Vec<(Vec<String>, PathBuf)>,
    update_older: Duration,
) -> Vec<Result<(bool, Option<String>), Box<dyn Error>>> {
    download_multiple_first_available(client, downloads, update_older).await
}

/// Async implementation: download multiple files concurrently.
#[cfg(test)]
async fn download_multiple_first_available(
    client: &reqwest::Client,
    downloads: Vec<(Vec<String>, PathBuf)>,
    update_older: Duration,
) -> Vec<Result<(bool, Option<String>), Box<dyn Error>>> {
    future::join_all(downloads.into_iter().map(|(urls, local_path)| async move {
        // Check if file is up to date before attempting download
        if local_path.exists() {
            let metadata = fs::metadata(&local_path)?;
            let modified = metadata.modified()?;
            let elapsed = SystemTime::now().duration_since(modified)?;

            if elapsed < update_older {
                info!("{} is up to date, skipping download", local_path.display());
                return Ok((false, None));
            }
        }

        // Convert Vec<String> to Vec<&str> for download_first_available
        let url_refs: Vec<&str> = urls.iter().map(|s| s.as_str()).collect();
        download_first_available(client, &url_refs, &local_path, None).await
    }))
    .await
}

/// Download multiple files with a progress callback.
/// The callback is called with (index, result) as each download completes.
pub fn download_multiple_first_available_with_progress<F>(
    downloads: Vec<(Vec<String>, PathBuf)>,
    update_older: Option<Duration>,
    client: Option<&reqwest::Client>,
    progress_callback: F,
) where
    F: FnMut(usize, &Result<(bool, Option<String>), Box<dyn Error>>),
{
    let update_older = match update_older {
        Some(dur) => dur,
        None => Duration::from_hours(24),
    };

    let client_ = match client {
        Some(c) => c,
        None => &reqwest::Client::new(),
    };

    download_multiple_with_progress_async(client_, downloads, update_older, progress_callback);
}

#[tokio::main]
async fn download_multiple_with_progress_async<F>(
    client: &reqwest::Client,
    downloads: Vec<(Vec<String>, PathBuf)>,
    update_older: Duration,
    mut progress_callback: F,
) where
    F: FnMut(usize, &Result<(bool, Option<String>), Box<dyn Error>>),
{
    let mut futures = FuturesUnordered::new();

    for (idx, (urls, local_path)) in downloads.into_iter().enumerate() {
        let client = client.clone();
        futures.push(async move {
            let result: Result<(bool, Option<String>), Box<dyn Error>> = async {
                // Check if file is up to date before attempting download
                if local_path.exists() {
                    let metadata = fs::metadata(&local_path)?;
                    let modified = metadata.modified()?;
                    let elapsed = SystemTime::now().duration_since(modified)?;

                    if elapsed < update_older {
                        info!("{} is up to date, skipping download", local_path.display());
                        return Ok((false, None));
                    }
                }

                // Convert Vec<String> to Vec<&str> for download_first_available
                let url_refs: Vec<&str> = urls.iter().map(|s| s.as_str()).collect();
                download_first_available(&client, &url_refs, &local_path, None).await
            }
            .await;

            (idx, result)
        });
    }

    // Process results as they complete
    while let Some((idx, result)) = futures.next().await {
        progress_callback(idx, &result);
    }
}

#[tokio::main]
async fn download_first_available__(
    client: &reqwest::Client,
    urls: &[&str],
    local_path: &PathBuf,
    etag: Option<&str>,
) -> Result<(bool, Option<String>), Box<dyn Error>> {
    download_first_available(client, urls, local_path, etag).await
}

#[tokio::main]
async fn download_json_(
    client: &reqwest::Client,
    urls: Vec<String>,
) -> Result<Vec<serde_json::Value>, Box<dyn Error>> {
    let resp = download_json(client, urls).await?;
    return Ok(resp);
}

// ------------------------------------------------------------------------
// asynchronous API
// ------------------------------------------------------------------------

pub async fn download_json(
    client: &reqwest::Client,
    urls: Vec<String>,
) -> Result<Vec<serde_json::Value>, Box<dyn Error>> {
    let vers: Vec<Result<serde_json::Value, Box<dyn Error>>> =
        future::join_all(urls.into_iter().map(|url| async move {
            let json = client
                .get(url)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            Ok(json)
        }))
        .await;

    let mut vers2: Vec<serde_json::Value> = vec![];

    for v in vers {
        match v {
            Ok(v) => vers2.push(v),
            Err(e) => {
                OUTPUT.error(&format!("Cannot download JSON: {}", e));
                error!("Cannot download JSON: {}", e);
                bail!("Cannot download JSON: {}", e.to_string())
            }
        };
    }

    Ok(vers2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn test_download_multiple_first_available_no_downloads() {
        let downloads: Vec<(Vec<String>, PathBuf)> = vec![];
        let results = download_multiple_first_available_(downloads, None, None);
        assert_eq!(results.len(), 0);
    }

    #[tokio::test]
    async fn test_download_multiple_first_available_all_success() {
        let mock_server = MockServer::start().await;

        // Mock responses for two files
        Mock::given(method("GET"))
            .and(path("/file1.txt"))
            .respond_with(ResponseTemplate::new(200).set_body_string("content1"))
            .mount(&mock_server)
            .await;

        Mock::given(method("GET"))
            .and(path("/file2.txt"))
            .respond_with(ResponseTemplate::new(200).set_body_string("content2"))
            .mount(&mock_server)
            .await;

        let tmp_dir = std::env::temp_dir();
        let file1_path = tmp_dir.join("test_download_concurrent_file1.txt");
        let file2_path = tmp_dir.join("test_download_concurrent_file2.txt");

        // Clean up any existing files
        let _ = std::fs::remove_file(&file1_path);
        let _ = std::fs::remove_file(&file2_path);

        let downloads = vec![
            (
                vec![format!("{}/file1.txt", mock_server.uri())],
                file1_path.clone(),
            ),
            (
                vec![format!("{}/file2.txt", mock_server.uri())],
                file2_path.clone(),
            ),
        ];

        let client = reqwest::Client::new();
        let results =
            download_multiple_first_available(&client, downloads, Duration::from_hours(24)).await;

        assert_eq!(results.len(), 2);
        assert!(results[0].is_ok());
        assert!(results[1].is_ok());
        assert!(results[0].as_ref().unwrap().0); // Downloaded
        assert!(results[1].as_ref().unwrap().0); // Downloaded

        // Verify files exist
        assert!(file1_path.exists());
        assert!(file2_path.exists());

        // Clean up
        let _ = std::fs::remove_file(&file1_path);
        let _ = std::fs::remove_file(&file2_path);
    }

    #[tokio::test]
    async fn test_download_multiple_first_available_with_fallback() {
        let mock_server = MockServer::start().await;

        // File1: only respond on /mirror path (first URL will fail)
        Mock::given(method("GET"))
            .and(path("/mirror/file1.txt"))
            .respond_with(ResponseTemplate::new(200).set_body_string("content1"))
            .mount(&mock_server)
            .await;

        let tmp_dir = std::env::temp_dir();
        let file1_path = tmp_dir.join("test_download_fallback_file1.txt");

        // Clean up any existing file
        let _ = std::fs::remove_file(&file1_path);

        let downloads = vec![(
            vec![
                format!("{}/nonexistent/file1.txt", mock_server.uri()),
                format!("{}/mirror/file1.txt", mock_server.uri()),
            ],
            file1_path.clone(),
        )];

        let client = reqwest::Client::new();
        let results =
            download_multiple_first_available(&client, downloads, Duration::from_hours(24)).await;

        assert_eq!(results.len(), 1);
        assert!(results[0].is_ok());
        assert!(results[0].as_ref().unwrap().0); // Downloaded from fallback URL

        // Verify file exists
        assert!(file1_path.exists());

        // Clean up
        let _ = std::fs::remove_file(&file1_path);
    }

    #[tokio::test]
    async fn test_download_multiple_first_available_mixed_results() {
        let mock_server = MockServer::start().await;

        // File1: success
        Mock::given(method("GET"))
            .and(path("/file1.txt"))
            .respond_with(ResponseTemplate::new(200).set_body_string("content1"))
            .mount(&mock_server)
            .await;

        // File2: all URLs will fail (404)
        Mock::given(method("GET"))
            .and(path("/file2.txt"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock_server)
            .await;

        let tmp_dir = std::env::temp_dir();
        let file1_path = tmp_dir.join("test_download_mixed_file1.txt");
        let file2_path = tmp_dir.join("test_download_mixed_file2.txt");

        // Clean up any existing files
        let _ = std::fs::remove_file(&file1_path);
        let _ = std::fs::remove_file(&file2_path);

        let downloads = vec![
            (
                vec![format!("{}/file1.txt", mock_server.uri())],
                file1_path.clone(),
            ),
            (
                vec![format!("{}/file2.txt", mock_server.uri())],
                file2_path.clone(),
            ),
        ];

        let client = reqwest::Client::new();
        let results =
            download_multiple_first_available(&client, downloads, Duration::from_hours(24)).await;

        assert_eq!(results.len(), 2);
        assert!(results[0].is_ok());
        assert!(results[0].as_ref().unwrap().0); // Success
        assert!(results[1].is_err()); // Failed

        // Verify only file1 exists
        assert!(file1_path.exists());
        assert!(!file2_path.exists());

        // Clean up
        let _ = std::fs::remove_file(&file1_path);
    }

    #[tokio::test]
    async fn test_download_multiple_first_available_all_cached() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/file1.txt"))
            .respond_with(
                ResponseTemplate::new(304), // Not Modified
            )
            .mount(&mock_server)
            .await;

        let tmp_dir = std::env::temp_dir();
        let file1_path = tmp_dir.join("test_download_cached_file1.txt");

        // Create a file that already exists
        std::fs::write(&file1_path, "existing content").unwrap();

        let downloads = vec![(
            vec![format!("{}/file1.txt", mock_server.uri())],
            file1_path.clone(),
        )];

        let client = reqwest::Client::new();
        // Set update_older to a very long time so the file is considered up-to-date
        let results = download_multiple_first_available(
            &client,
            downloads,
            Duration::from_secs(86400 * 365), // 1 year
        )
        .await;

        assert_eq!(results.len(), 1);
        assert!(results[0].is_ok());
        assert!(!results[0].as_ref().unwrap().0); // Cached, not downloaded

        // Clean up
        let _ = std::fs::remove_file(&file1_path);
    }
}
