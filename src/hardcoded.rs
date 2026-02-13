use once_cell::sync::Lazy;
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
