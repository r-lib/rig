use std::error::Error;

use serde::{Deserialize, Serialize};

use crate::hardcoded::*;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum Enabled {
    Always(bool),
    OnPlatforms { platforms: Vec<String> },
}

impl Enabled {
    /// A short human-readable description of the default-enabled state, used by
    /// `rig repos list`.
    pub fn describe(&self) -> String {
        match self {
            Enabled::Always(true) => "Yes".to_string(),
            Enabled::Always(false) => "No".to_string(),
            Enabled::OnPlatforms { platforms } => {
                format!("On platforms: {}", platforms.join(" | "))
            }
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RepoEntry {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub url: String,
    pub platforms: Option<Vec<String>>,
    pub archs: Option<Vec<String>>,
    pub rversions: Option<Vec<String>>,
    pub enabled: Option<Enabled>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Repository {
    // E.g. CRAN, BioCsoft, PPPM, etc.
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub enabled: Enabled,
    pub repos: Vec<RepoEntry>,
}

pub fn get_repos_config() -> Result<Vec<Repository>, Box<dyn Error>> {
    Ok(HC_REPOS.to_vec())
}
