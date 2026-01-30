use once_cell::sync::Lazy;
use serde_json::Value;

pub static HC_RTOOLS_AARCH64: Lazy<Value> = Lazy::new(|| {
    let data = include_str!("json/rtools-versions-aarch64.json");
    serde_json::from_str(data).expect("Invalid JSON in json/rtools-versions-aarch64.json")
});

pub static HC_RTOOLS: Lazy<Value> = Lazy::new(|| {
    let data = include_str!("json/rtools-versions.json");
    serde_json::from_str(data).expect("Invalid JSON in json/rtools-versions.json")
});
