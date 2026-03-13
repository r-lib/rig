use std::collections::HashMap;

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
#[cfg(target_os = "windows")]
use serde_json::Value;

use crate::repos::Repository;

pub static HC_REPOS: Lazy<Vec<Repository>> = Lazy::new(|| {
    let data = include_str!("data/repos.json");
    serde_json::from_str(data).expect("Invalid JSON in data/repos.json")
});

pub static HC_PROFILE_REPOS: Lazy<String> = Lazy::new(|| {
    let data = include_str!("data/repositories.R");
    data.to_string()
});

pub struct ProfileReposMarkers {
    pub generic_start: String,
    pub current_start: String,
    pub end: String,
}

pub static HC_PROFILE_REPOS_MARKERS: Lazy<ProfileReposMarkers> = Lazy::new(|| {
    let data = include_str!("data/repositories.R");
    let lines = data.trim().split('\n').collect::<Vec<&str>>();
    ProfileReposMarkers {
        generic_start: lines[0].to_string(),
        current_start: lines[1].to_string(),
        end: lines[lines.len() - 1].to_string(),
    }
});

#[derive(Debug, Serialize, Deserialize)]
pub struct BiocVersionMapping {
    pub r_version: String,
    pub bioc_version: String,
}

pub static HC_R_VERSION_TO_BIOC_VERSION: Lazy<HashMap<String, String>> = Lazy::new(|| {
    let data = include_str!("data/r-version-to-bioc-version.json");
    let mappings: Vec<BiocVersionMapping> =
        serde_json::from_str(data).expect("Invalid JSON in data/r-version-to-bioc-version.json");
    let mut map = HashMap::new();
    for mapping in mappings {
        map.insert(mapping.r_version.clone(), mapping.bioc_version.clone());
    }
    map
});

#[allow(dead_code)]
pub static HC_BIOC_VERSION_TO_R_VERSION: Lazy<HashMap<String, String>> = Lazy::new(|| {
    let data = include_str!("data/r-version-to-bioc-version.json");
    let mappings: Vec<BiocVersionMapping> =
        serde_json::from_str(data).expect("Invalid JSON in data/r-version-to-bioc-version.json");
    let mut map = HashMap::new();
    for mapping in mappings {
        map.insert(mapping.bioc_version.clone(), mapping.r_version.clone());
    }
    map
});

#[cfg(target_os = "windows")]
pub static HC_RTOOLS_AARCH64: Lazy<Value> = Lazy::new(|| {
    let data = include_str!("data/rtools-versions-aarch64.json");
    serde_json::from_str(data).expect("Invalid JSON in data/rtools-versions-aarch64.json")
});

#[cfg(target_os = "windows")]
pub static HC_RTOOLS: Lazy<Value> = Lazy::new(|| {
    let data = include_str!("data/rtools-versions.json");
    serde_json::from_str(data).expect("Invalid JSON in data/rtools-versions.json")
});
