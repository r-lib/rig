use std::error::Error;

use serde::{Deserialize, Serialize};

use crate::hardcoded::*;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RepoEntry {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub url: String,
    pub platforms: Option<Vec<String>>,
    pub archs: Option<Vec<String>>,
    pub rversions: Option<Vec<String>>,
    pub enabled: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Repository {
    // E.g. CRAN, BioCsoft, PPPM, etc.
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub enabled: bool,
    pub repos: Vec<RepoEntry>,
}

pub fn get_repos_config() -> Result<Vec<Repository>, Box<dyn Error>> {
    Ok(HC_REPOS.to_vec())
}
