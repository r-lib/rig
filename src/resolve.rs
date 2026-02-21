use futures::future;
use std::error::Error;

use clap::ArgMatches;
#[cfg(target_os = "windows")]
use log::warn;
#[cfg(target_os = "windows")]
use serde_json::{Map, Value};
use simple_error::bail;
#[cfg(target_os = "windows")]
use std::sync::{LazyLock, RwLock};

use crate::common::*;
use crate::download::*;
#[cfg(target_os = "windows")]
use crate::hardcoded::*;
use crate::rversion::*;
use crate::utils::*;

const API_URI: &str = "https://api.r-hub.io/rversions/resolve/";
#[cfg(target_os = "windows")]
const API_ROOT: &str = "https://api.r-hub.io/rversions/";

pub fn get_resolve(args: &ArgMatches) -> Result<Rversion, Box<dyn Error>> {
    let platform = get_platform(args)?;
    let arch = get_arch(&platform, args);
    let str: &String = args.get_one("str").unwrap();
    let eps = vec![str.to_string()];

    if str.len() > 8 && (&str[..7] == "http://" || &str[..8] == "https://") {
        Ok(Rversion {
            version: None,
            url: Some(str.to_string()),
            arch: None,
            ppm: false,
            ppmurl: None,
        })
    } else {
        Ok(resolve_versions(eps, &platform, &arch)?[0].to_owned())
    }
}

#[tokio::main]
pub async fn resolve_versions(
    vers: Vec<String>,
    platform: &str,
    arch: &str,
) -> Result<Vec<Rversion>, Box<dyn Error>> {
    let client = reqwest::Client::new();
    let client = &client;
    let out: Vec<Result<Rversion, Box<dyn Error>>> = future::join_all(
        vers.into_iter()
            .map(move |ver| async move { resolve_version(client, &ver, platform, arch).await }),
    )
    .await;

    // We quit with the first error we found
    let mut out2: Vec<Rversion> = vec![];
    for o in out {
        match o {
            Ok(x) => out2.push(x),
            Err(x) => bail!("Failed to resolve R version: {}", x.to_string()),
        };
    }

    Ok(out2)
}

async fn resolve_version(
    client: &reqwest::Client,
    ver: &str,
    platform: &str,
    arch: &str,
) -> Result<Rversion, Box<dyn Error>> {
    let mut url = API_URI.to_string() + ver + "/" + platform;

    if arch != "default" {
        url = url + "/" + arch;
    }

    let resp = download_json(client, vec![url]).await?;
    let resp = &resp[0];

    let version: String = unquote(&resp["version"].to_string());
    let dlurl = Some(unquote(&resp["url"].to_string()));
    let ppm = match resp["ppm-binaries"].as_bool() {
        Some(v) => v,
        None => false,
    };
    let ppmurl = match resp["ppm-binary-url"].as_str() {
        Some(v) => Some(v.to_string()),
        None => None,
    };
    Ok(Rversion {
        version: Some(version),
        url: dlurl,
        arch: Some(arch.to_string()),
        ppm: ppm,
        ppmurl: ppmurl,
    })
}

#[cfg(target_os = "windows")]
static API_CACHE: LazyLock<RwLock<Map<String, Value>>> = LazyLock::new(|| RwLock::new(Map::new()));

#[cfg(target_os = "windows")]
fn cache_set_value(key: &str, value: Value) {
    let mut map = API_CACHE.write().unwrap();
    map.insert(key.to_string(), value);
}

#[cfg(target_os = "windows")]
fn cache_get_value(key: &str) -> Option<Value> {
    let map = API_CACHE.read().unwrap();
    map.get(key).cloned()
}

#[cfg(target_os = "windows")]
pub fn get_available_rtools_versions(arch: &str) -> serde_json::Value {
    let cache_key = "rtools".to_string() + arch;
    let value = match cache_get_value(&cache_key) {
        Some(cached) => cached,
        None => {
            let url = API_ROOT.to_string() + "/rtools-versions/" + arch;
            let val = match download_json_sync(vec![url]) {
                Ok(dl) => dl[0].clone(),
                Err(err) => {
                    warn!("Download error: {}.", err);
                    warn!(
                        "Failed to download Rtools version data, will use \
			   hardcoded data instead."
                    );
                    if arch == "aarch64" {
                        HC_RTOOLS_AARCH64.clone()
                    } else {
                        HC_RTOOLS.clone()
                    }
                }
            };
            cache_set_value(&cache_key, val.clone());
            val
        }
    };

    value
}

#[cfg(target_os = "windows")]
pub fn get_rtools_version(version: &str, arch: &str) -> Result<RtoolsVersion, Box<dyn Error>> {
    let value = get_available_rtools_versions(arch);

    let msg = "Cannot parse response from the R version API to learn about \
	       Rtools versions";

    let value = match value.as_array() {
        Some(x) => x,
        None => bail!(msg),
    };

    for ver in value {
        let versionx: String = ver["version"].as_str().ok_or(msg)?.to_string();
        if &versionx == version {
            let url: String = ver["url"].as_str().ok_or(msg)?.to_string();
            return Ok(RtoolsVersion { url });
        }
    }

    bail!(
        "Cannot find Rtools version {} for architecture {}",
        version,
        arch
    );
}
