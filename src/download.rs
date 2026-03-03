use futures::future;
use futures_util::StreamExt;
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
use log::info;
use reqwest::StatusCode;
use simple_error::bail;

#[cfg(target_os = "windows")]
use crate::resolve::get_resolve;
#[cfg(target_os = "windows")]
use crate::rversion::Rversion;
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
        None => bail!(
            "Cannot find a download url for R version {}",
            ver.unwrap_or("???".to_string())
        ),
    };
    let mut filename = OsString::new();
    filename.push(version2.arch.unwrap_or("".to_string()));
    filename.push("-");
    filename.push(basename(&url).unwrap_or("foo"));
    let filename_path = Path::new(&filename);
    let tmp_dir = std::env::temp_dir().join("rig");
    let target = tmp_dir.join(&filename);
    if target.exists() && not_too_old(&target) {
        info!(
            "{} is cached at {}",
            filename_path.display(),
            target.display()
        );
    } else {
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
    let tmp_dir = std::env::temp_dir().join("rig");
    let target = tmp_dir.join(&filename);
    if target.exists() && (infinite_cache || not_too_old(&target)) {
        info!("{} is cached at {}", filename, target.display());
    } else {
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
        Err(err) => bail!("HTTP error at {}: {}", url, err.to_string()),
    };
    let resp = match resp {
        Ok(resp) => resp,
        Err(err) => bail!("HTTP error at {}: {}", url, err.to_string()),
    };

    // If dirname(path) is / then this is None
    let dir = Path::new(&path).parent();
    match dir {
        Some(dir) => {
            match std::fs::create_dir_all(dir) {
                Err(err) => {
                    let dir = dir.to_str().unwrap_or_else(|| "???");
                    bail!("Cannot create directory {}: {}", dir, err.to_string())
                }
                _ => {}
            };
        }
        None => {}
    };
    let file = File::create(&path);
    let mut file = match file {
        Ok(file) => file,
        Err(err) => bail!(
            "Cannot create file '{}': {}",
            path.display(),
            err.to_string()
        ),
    };
    let mut stream = resp.bytes_stream();

    while let Some(item) = stream.next().await {
        let chunk = match item {
            Ok(chunk) => chunk,
            Err(err) => bail!("HTTP error at {}: {}", url, err.to_string()),
        };
        match file.write(&chunk) {
            Err(err) => bail!(
                "Failed to write to file {}: {}",
                path.display(),
                err.to_string()
            ),
            _ => {}
        };
    }

    match std::fs::rename(Path::new(&path), Path::new(&opath)) {
        Err(err) => bail!("Failed to rename downloaded file: {}", err.to_string()),
        _ => {}
    };

    Ok(())
}

pub fn download_json_sync(urls: Vec<String>) -> Result<Vec<serde_json::Value>, Box<dyn Error>> {
    let client = reqwest::Client::new();
    let client = &client;
    let resp = download_json_(client, urls)?;
    return Ok(resp);
}

async fn download_if_newer(
    client: &reqwest::Client,
    url: &str,
    local_path: &PathBuf,
) -> Result<bool, Box<dyn Error>> {
    let etag_path = add_suffix(local_path, ".etag");
    let etag = fs::read_to_string(&etag_path).ok();
    let mut req = client.get(url);
    if let Some(etag) = etag.as_deref() {
        req = req.header("If-None-Match", etag);
    }
    info!("Checking for updates for {}", local_path.display());
    let resp = req.send().await?;

    match resp.status() {
        StatusCode::NOT_MODIFIED => {
            filetime::set_file_mtime(local_path, FileTime::now())?;
            Ok(false)
        }

        StatusCode::OK => {
            // 200 → new content
            // Save new ETag if present
            if let Some(etag) = resp.headers().get("ETag") {
                fs::write(etag_path, etag.to_str()?)?;
            }
            let bytes = resp.bytes().await?;
            fs::write(local_path, &bytes)?;
            Ok(true)
        }

        status => {
            bail!("Failed to download package metadata, status: {}", status);
        }
    }
}

#[tokio::main]
pub async fn download_if_newer__(
    url: &str,
    local_path: &PathBuf,
    client: Option<&reqwest::Client>,
) -> Result<bool, Box<dyn Error>> {
    let client_ = match client {
        Some(c) => c,
        None => &reqwest::Client::new(),
    };
    download_if_newer(client_, url, local_path).await
}

pub fn download_if_newer_(
    url: &str,
    local_path: &PathBuf,
    update_older: Option<Duration>,
    client: Option<&reqwest::Client>,
) -> Result<bool, Box<dyn Error>> {
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
            return Ok(false);
        }
    }

    download_if_newer__(url, local_path, client)
}

/// Try to download from multiple URLs, using the first one that succeeds (async).
async fn download_first_available(
    client: &reqwest::Client,
    urls: &[&str],
    local_path: &PathBuf,
) -> Result<bool, Box<dyn Error>> {
    let mut last_error = None;

    for url in urls {
        info!("Trying to download from {}", url);
        match download_if_newer(client, url, local_path).await {
            Ok(result) => {
                info!("Successfully downloaded from {}", url);
                return Ok(result);
            }
            Err(e) => {
                info!("Failed to download from {}: {}", url, e);
                last_error = Some(e);
            }
        }
    }

    match last_error {
        Some(e) => bail!("All download URLs failed. Last error: {}", e),
        None => bail!("No URLs provided"),
    }
}

/// Try to download from multiple URLs, using the first one that succeeds (sync wrapper).
/// Returns Ok(true) if a new file was downloaded, Ok(false) if existing file is up to date,
/// or Err if all URLs failed.
pub fn download_first_available_(
    urls: &[&str],
    local_path: &PathBuf,
    update_older: Option<Duration>,
    client: Option<&reqwest::Client>,
) -> Result<bool, Box<dyn Error>> {
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
            return Ok(false);
        }
    }

    let client_ = match client {
        Some(c) => c,
        None => &reqwest::Client::new(),
    };

    download_first_available__(client_, urls, local_path)
}

/// Download multiple files concurrently, each from a list of candidate URLs.
/// Each download will try its URLs in order until one succeeds.
/// Returns a vector of results, one for each download request.
/// Each result is Ok(true) if downloaded, Ok(false) if cached, or Err if all URLs failed.
pub fn download_multiple_first_available_(
    downloads: Vec<(Vec<String>, PathBuf)>,
    update_older: Option<Duration>,
    client: Option<&reqwest::Client>,
) -> Vec<Result<bool, Box<dyn Error>>> {
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

#[tokio::main]
async fn download_multiple_first_available__(
    client: &reqwest::Client,
    downloads: Vec<(Vec<String>, PathBuf)>,
    update_older: Duration,
) -> Vec<Result<bool, Box<dyn Error>>> {
    download_multiple_first_available(client, downloads, update_older).await
}

/// Async implementation: download multiple files concurrently.
async fn download_multiple_first_available(
    client: &reqwest::Client,
    downloads: Vec<(Vec<String>, PathBuf)>,
    update_older: Duration,
) -> Vec<Result<bool, Box<dyn Error>>> {
    future::join_all(downloads.into_iter().map(|(urls, local_path)| async move {
        // Check if file is up to date before attempting download
        if local_path.exists() {
            let metadata = fs::metadata(&local_path)?;
            let modified = metadata.modified()?;
            let elapsed = SystemTime::now().duration_since(modified)?;

            if elapsed < update_older {
                info!("{} is up to date, skipping download", local_path.display());
                return Ok(false);
            }
        }

        // Convert Vec<String> to Vec<&str> for download_first_available
        let url_refs: Vec<&str> = urls.iter().map(|s| s.as_str()).collect();
        download_first_available(client, &url_refs, &local_path).await
    }))
    .await
}

#[tokio::main]
async fn download_first_available__(
    client: &reqwest::Client,
    urls: &[&str],
    local_path: &PathBuf,
) -> Result<bool, Box<dyn Error>> {
    download_first_available(client, urls, local_path).await
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
            Err(e) => bail!("Cannot download JSON: {}", e.to_string()),
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
        let results = download_multiple_first_available(
            &client,
            downloads,
            Duration::from_hours(24),
        )
        .await;

        assert_eq!(results.len(), 2);
        assert!(results[0].is_ok());
        assert!(results[1].is_ok());
        assert_eq!(results[0].as_ref().unwrap(), &true); // Downloaded
        assert_eq!(results[1].as_ref().unwrap(), &true); // Downloaded

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
        let results = download_multiple_first_available(
            &client,
            downloads,
            Duration::from_hours(24),
        )
        .await;

        assert_eq!(results.len(), 1);
        assert!(results[0].is_ok());
        assert_eq!(results[0].as_ref().unwrap(), &true); // Downloaded from fallback URL

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
        let results = download_multiple_first_available(
            &client,
            downloads,
            Duration::from_hours(24),
        )
        .await;

        assert_eq!(results.len(), 2);
        assert!(results[0].is_ok());
        assert_eq!(results[0].as_ref().unwrap(), &true); // Success
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
        assert_eq!(results[0].as_ref().unwrap(), &false); // Cached, not downloaded

        // Clean up
        let _ = std::fs::remove_file(&file1_path);
    }
}
