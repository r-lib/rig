use once_cell::sync::Lazy;
#[cfg(target_os = "windows")]
use serde_json::Value;

use crate::repos::Repository;

pub static HC_REPOS: Lazy<Vec<Repository>> = Lazy::new(|| {
    let data = include_str!("json/repos.json");
    serde_json::from_str(data).expect("Invalid JSON in json/repos.json")
});

#[cfg(target_os = "windows")]
pub static HC_RTOOLS_AARCH64: Lazy<Value> = Lazy::new(|| {
    let data = include_str!("json/rtools-versions-aarch64.json");
    serde_json::from_str(data).expect("Invalid JSON in json/rtools-versions-aarch64.json")
});

#[cfg(target_os = "windows")]
pub static HC_RTOOLS: Lazy<Value> = Lazy::new(|| {
    let data = include_str!("json/rtools-versions.json");
    serde_json::from_str(data).expect("Invalid JSON in json/rtools-versions.json")
});
