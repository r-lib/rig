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

use reqwest::StatusCode;
use simple_error::bail;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use simplelog::info;

#[cfg(target_os = "windows")]
use crate::resolve::get_resolve;
#[cfg(target_os = "windows")]
use crate::rversion::Rversion;
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
    update_older: Option<Duration>,
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
            return Ok(false);
        }
    }

    let etag_path = add_suffix(local_path, ".etag");
    let etag = fs::read_to_string(&etag_path).ok();
    let mut req = client.get(url);
    if let Some(etag) = etag.as_deref() {
        req = req.header("If-None-Match", etag);
    }
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
pub async fn download_if_newer_(
    url: &str,
    local_path: &PathBuf,
    update_older: Option<Duration>,
) -> Result<bool, Box<dyn Error>> {
    let client = reqwest::Client::new();
    let client = &client;
    let updated = download_if_newer(client, url, local_path, update_older).await?;
    Ok(updated)
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
