
#[cfg(target_os = "windows")]
use clap::ArgMatches;

use futures::future;
use futures_util::StreamExt;
use std::fs::File;
use std::io::Write;
use std::path::Path;

#[cfg(target_os = "windows")]
use crate::windows::*;
#[cfg(target_os = "windows")]
use crate::rversion::Rversion;
#[cfg(target_os = "windows")]
use crate::utils::*;

// ------------------------------------------------------------------------
// synchronous API
// ------------------------------------------------------------------------

#[cfg(target_os = "windows")]
pub fn download_r(args: &ArgMatches) -> (Rversion, String) {
    let version = get_resolve(args);
    let ver = version.version.to_owned();
    let url: String = match &version.url {
        Some(s) => s.to_string(),
        None => panic!("Cannot find a download url for R version {}", ver.unwrap()),
    };
    let filename = version.arch.to_owned().unwrap() + "-" + basename(&url).unwrap();
    let tmp_dir = std::env::temp_dir().join("rim");
    let target = tmp_dir.join(&filename);
    let target_str;
    if target.exists() {
        target_str = target.into_os_string().into_string().unwrap();
        println!("{} is cached at\n    {}", filename, target_str);
    } else {
        target_str = target.into_os_string().into_string().unwrap();
        println!("Downloading {} ->\n    {}", url, target_str);
        let client = &reqwest::Client::new();
        download_file(client, url, &target_str);
    }

    (version, target_str)
}

#[tokio::main]
pub async fn download_file(client: &reqwest::Client, url: String, opath: &str) {
    let path = opath.to_string() + ".tmp";
    let resp = client.get(&url).send().await;
    let resp = match resp {
        Ok(resp) => resp.error_for_status(),
        Err(err) => panic!("HTTP error at {}: {}", url, err.to_string()),
    };
    let resp = match resp {
        Ok(resp) => resp,
        Err(err) => panic!("HTTP error at {}: {}", url, err.to_string()),
    };

    // If dirname(path) is / then this is None
    let dir = Path::new(&path).parent();
    match dir {
        Some(dir) => {
            match std::fs::create_dir_all(dir) {
                Err(err) => {
                    let dir = dir.to_str().unwrap_or_else(|| "???");
                    panic!("Cannot create directory {}: {}", dir, err.to_string())
                }
                _ => {}
            };
        }
        None => {}
    };
    let file = File::create(&path);
    let mut file = match file {
        Ok(file) => file,
        Err(err) => panic!("Cannot create file '{}': {}", path, err.to_string()),
    };
    let mut stream = resp.bytes_stream();

    while let Some(item) = stream.next().await {
        let chunk = match item {
            Ok(chunk) => chunk,
            Err(err) => panic!("HTTP error at {}: {}", url, err.to_string()),
        };
        match file.write(&chunk) {
            Err(err) => panic!("Failed to write to file {}: {}", path, err.to_string()),
            _ => {}
        };
    }

    match std::fs::rename(Path::new(&path), Path::new(&opath)) {
        Err(err) => panic!("Failed to rename downloaded file: {}", err.to_string()),
        _ => {}
    };
}

// ------------------------------------------------------------------------
// asynchronous API
// ------------------------------------------------------------------------

pub async fn download_text(client: &reqwest::Client, url: String) -> String {
    let resp = client.get(&url).send().await;
    let body = match resp {
        Ok(resp) => resp.error_for_status(),
        Err(err) => panic!("HTTP error at {}: {}", url, err.to_string()),
    };
    let body = match body {
        Ok(content) => content,
        Err(err) => panic!("HTTP error at {}: {}", url, err.to_string()),
    };
    let body = body.text().await;
    match body {
        Ok(txt) => txt,
        Err(err) => panic!("HTTP error at {}: {}", url, err.to_string()),
    }
}

pub async fn download_json(client: &reqwest::Client, urls: Vec<String>) -> Vec<serde_json::Value> {
    let vers: Vec<serde_json::Value> = future::join_all(urls.into_iter().map(|url| async move {
        let resp = client
            .get(url)
            .send()
            .await
            .expect("Cannot query R versions API")
            .error_for_status()
            .expect("HTTP error on the R versions API");
        let json: serde_json::Value = resp.json().await.expect("Cannot parse JSON response");
        json
    }))
    .await;

    vers
}
