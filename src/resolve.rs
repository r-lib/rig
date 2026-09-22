use futures::future;
use regex::Regex;
use std::error::Error;

use clap::ArgMatches;
use log::error;
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
use crate::output::OUTPUT;
use crate::rversion::*;
use crate::utils::http_client;
use crate::utils::*;

const API_URI: &str = "https://api.r-hub.io/rversions/resolve/";
#[cfg(target_os = "windows")]
const API_ROOT: &str = "https://api.r-hub.io/rversions/"; // must end with '/'

pub fn get_resolve(args: &ArgMatches) -> Result<Rversion, Box<dyn Error>> {
    let platform = get_platform(args)?;
    get_resolve_for(args, &platform)
}

pub fn get_resolve_for(args: &ArgMatches, platform: &str) -> Result<Rversion, Box<dyn Error>> {
    let mut arch = get_arch(platform, args);
    let arg_str: &String = args.get_one("str").unwrap();
    let resolved_ver = if is_renv_lockfile_path(arg_str) {
        Some(crate::renv::parse_r_version(std::path::PathBuf::from(
            arg_str,
        ))?)
    } else {
        None
    };
    let str: &str = resolved_ver.as_deref().unwrap_or(arg_str);
    let eps = vec![str.to_string()];

    validate_version_arg(str)?;

    // R only has native arm64 macOS builds from 4.1.0 onward. If the user
    // did not explicitly ask for arm64, and requested a plain version number
    // older than that, use the x86_64 build instead (it runs fine via
    // Rosetta). See https://github.com/r-lib/rig/issues/133.
    if platform == "macos" && arch == "aarch64" {
        let arch_explicit = args.try_contains_id("arch").is_ok()
            && args.value_source("arch") == Some(clap::parser::ValueSource::CommandLine);
        if !arch_explicit {
            if let Some(ver) = parse_plain_version(str) {
                if ver < semver::Version::new(4, 1, 0) {
                    OUTPUT.status(&format!(
                        "R {} has no native arm64 build, using the x86_64 build instead (runs via Rosetta).",
                        str
                    ));
                    arch = "x86_64".to_string();
                }
            }
        }
    }

    if is_url(str) {
        Ok(Rversion {
            version: None,
            url: Some(str.to_string()),
            arch: None,
            ppm: false,
            ppmurl: None,
        })
    } else {
        Ok(resolve_versions(eps, platform, &arch)?[0].to_owned())
    }
}

// Checked before escalating to admin, so a bad value fails once, without
// prompting for a password first. See https://github.com/r-lib/rig/issues/371.
pub fn validate_version_arg(str: &str) -> Result<(), Box<dyn Error>> {
    if is_url(str) || is_valid_version_string(str) {
        return Ok(());
    }
    // Actually read the file here (not just check the basename), so a
    // missing/malformed renv.lock fails now, before escalating to admin.
    if is_renv_lockfile_path(str) {
        crate::renv::parse_r_version(std::path::PathBuf::from(str))?;
        return Ok(());
    }
    let msg = format!(
        "Unknown value \"{}\". Accepted values: version numbers, \"devel\", \"next\", \"release\", \"oldrel/n\", a URL, or a path to an renv.lock file.",
        str
    );
    OUTPUT.error(&msg);
    error!("{}", msg);
    bail!(msg)
}

fn is_url(str: &str) -> bool {
    str.len() > 8 && (&str[..7] == "http://" || &str[..8] == "https://")
}

fn is_valid_version_string(str: &str) -> bool {
    let re = Regex::new(r"^(devel|next|release|oldrel(/\d+)?|\d+(\.\d+){0,2})$").unwrap();
    re.is_match(str)
}

// A fully pinned version, e.g. "4.3.3": all three components given, so the
// exact version is already known without resolving anything.
pub fn is_pinned_version_string(str: &str) -> bool {
    let re = Regex::new(r"^\d+\.\d+\.\d+$").unwrap();
    re.is_match(str)
}

// Lets `rig add renv.lock` / `rig add path/to/renv.lock` install the R
// version an renv.lock file requires. See
// https://github.com/r-lib/rig/issues/294.
fn is_renv_lockfile_path(str: &str) -> bool {
    std::path::Path::new(str)
        .file_name()
        .is_some_and(|n| n == "renv.lock")
}

// Parses a plain version string ("4", "4.0", "4.0.5"), padding missing
// components with 0. Returns None for symbolic versions (release, devel,
// oldrel/N) and URLs.
pub(crate) fn parse_plain_version(str: &str) -> Option<semver::Version> {
    let re = Regex::new(r"^(\d+)(?:\.(\d+))?(?:\.(\d+))?$").unwrap();
    let caps = re.captures(str)?;
    let major = caps.get(1)?.as_str().parse().ok()?;
    let minor = caps.get(2).map_or("0", |m| m.as_str()).parse().ok()?;
    let patch = caps.get(3).map_or("0", |m| m.as_str()).parse().ok()?;
    Some(semver::Version::new(major, minor, patch))
}

#[tokio::main]
pub async fn resolve_versions(
    vers: Vec<String>,
    platform: &str,
    arch: &str,
) -> Result<Vec<Rversion>, Box<dyn Error>> {
    let client = http_client();
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
            Err(x) => {
                OUTPUT.error(&format!("Failed to resolve R version: {}", x));
                error!("Failed to resolve R version: {}", x);
                bail!("Failed to resolve R version: {}", x.to_string())
            }
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
    let ppm = resp["ppm-binaries"].as_bool().unwrap_or_default();
    let ppmurl = resp["ppm-binary-url"].as_str().map(|v| v.to_string());
    Ok(Rversion {
        version: Some(version),
        url: dlurl,
        arch: Some(arch.to_string()),
        ppm,
        ppmurl,
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
    if crate::cache::no_cache() {
        return None;
    }
    let map = API_CACHE.read().unwrap();
    map.get(key).cloned()
}

#[cfg(target_os = "windows")]
pub fn get_available_rtools_versions(arch: &str) -> serde_json::Value {
    let cache_key = "rtools".to_string() + arch;
    let value = match cache_get_value(&cache_key) {
        Some(cached) => cached,
        None => {
            let url = API_ROOT.to_string() + "rtools-versions/" + arch;
            let val = match download_json_sync(vec![url]) {
                Ok(dl) => dl[0].clone(),
                Err(err) => {
                    OUTPUT.warn(&format!(
                        "Failed to download Rtools version data: {}, will use hardcoded data.",
                        err
                    ));
                    warn!(
                        "Failed to download Rtools version data: {}, will use hardcoded data.",
                        err
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
        None => {
            OUTPUT.error(msg);
            error!("{}", msg);
            bail!(msg)
        }
    };

    for ver in value {
        let versionx: String = ver["version"].as_str().ok_or(msg)?.to_string();
        if versionx == version {
            let url: String = ver["url"].as_str().ok_or(msg)?.to_string();
            return Ok(RtoolsVersion { url });
        }
    }

    let msg = format!(
        "Cannot find Rtools version {} for architecture {}",
        version, arch
    );
    OUTPUT.error(&msg);
    error!("{}", msg);
    bail!(msg)
}
