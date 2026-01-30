#[cfg(target_os = "windows")]
use once_cell::sync::Lazy;
#[cfg(target_os = "windows")]
use serde_json::Value;

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
