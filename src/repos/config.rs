use std::collections::HashMap;
use std::error::Error;

use serde::{Deserialize, Serialize};

use crate::config::{get_global_config_json, set_global_config_json};
use crate::hardcoded::*;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum Enabled {
    Always(bool),
    OnPlatforms { platforms: Vec<String> },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RepoEntry {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub url: String,
    /// Base URL of the extended metadata of this repository
    /// (`ALLPACKAGES.zst`, `ARCHIVEDPACKAGES.zst` and `binaries/`), if it has
    /// one. May contain `%v`, the Bioconductor version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<String>,
    pub platforms: Option<Vec<String>>,
    pub archs: Option<Vec<String>>,
    pub rversions: Option<Vec<String>>,
    pub enabled: Option<Enabled>,
    /// Use this URL only if no other URL with the same `metadata` is set up,
    /// from any repository. E.g. P3M's source package URL is for the
    /// platforms that P3M has no binary packages for.
    #[serde(default, skip_serializing_if = "is_false")]
    pub fallback: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Repository {
    // E.g. CRAN, BioCsoft, PPPM, etc.
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub enabled: Enabled,
    pub repos: Vec<RepoEntry>,
    /// Added by the user with `rig repos add`, not built into rig.
    #[serde(default)]
    pub custom: bool,
}

/// A repository added with `rig repos add`, as stored in the `repos` entry
/// of the rig configuration file.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct CustomRepo {
    pub name: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Enabled for R versions installed later, too. Set by
    /// `rig repos add --enable --all-versions`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub default: bool,
}

fn is_false(x: &bool) -> bool {
    !*x
}

impl CustomRepo {
    fn to_repository(&self) -> Repository {
        Repository {
            name: self.name.clone(),
            title: self.title.clone(),
            description: self.description.clone(),
            enabled: Enabled::Always(self.default),
            repos: vec![RepoEntry {
                name: self.name.clone(),
                title: self.title.clone(),
                description: self.description.clone(),
                url: self.url.clone(),
                metadata: None,
                platforms: None,
                archs: None,
                rversions: None,
                enabled: None,
                fallback: false,
            }],
            custom: true,
        }
    }
}

const CUSTOM_REPOS_KEY: &str = "repos";

/// The built-in repositories, followed by the ones added with `rig repos add`.
pub fn get_repos_config() -> Result<Vec<Repository>, Box<dyn Error>> {
    let mut config = HC_REPOS.to_vec();
    config.extend(get_custom_repos()?.iter().map(|r| r.to_repository()));
    Ok(config)
}

/// The base URLs of the extended metadata of the repositories that have one,
/// by lowercase repository entry name, e.g. `p3m` and `biocsoft`. This is
/// how an entry of an R installation's `repositories` file is matched to its
/// extended metadata.
pub fn repo_metadata_urls() -> Result<HashMap<String, String>, Box<dyn Error>> {
    let mut metadata = HashMap::new();
    for entry in get_repos_config()?.iter().flat_map(|r| r.repos.iter()) {
        if let Some(m) = &entry.metadata {
            metadata.insert(entry.name.to_lowercase(), m.clone());
        }
    }
    Ok(metadata)
}

pub fn builtin_repo_names() -> Vec<String> {
    HC_REPOS.iter().map(|r| r.name.clone()).collect()
}

pub fn get_custom_repos() -> Result<Vec<CustomRepo>, Box<dyn Error>> {
    match get_global_config_json(CUSTOM_REPOS_KEY)? {
        None => Ok(vec![]),
        Some(value) => match serde_json::from_value(value) {
            Ok(repos) => Ok(repos),
            Err(e) => bail!(
                "Invalid '{}' entry in rig config file: {}",
                CUSTOM_REPOS_KEY,
                e
            ),
        },
    }
}

pub fn save_custom_repos(repos: &[CustomRepo]) -> Result<(), Box<dyn Error>> {
    let value = if repos.is_empty() {
        None
    } else {
        Some(serde_json::to_value(repos)?)
    };
    set_global_config_json(CUSTOM_REPOS_KEY, value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acme(default: bool) -> CustomRepo {
        CustomRepo {
            name: "acme".to_string(),
            url: "https://cran.acme.com".to_string(),
            title: None,
            description: None,
            default,
        }
    }

    #[test]
    fn custom_repo_default_follows_the_flag() {
        assert!(matches!(
            acme(false).to_repository().enabled,
            Enabled::Always(false)
        ));
        assert!(matches!(
            acme(true).to_repository().enabled,
            Enabled::Always(true)
        ));
    }

    #[test]
    fn default_is_only_stored_if_set() {
        let json = serde_json::to_string(&acme(false)).unwrap();
        assert_eq!(json, r#"{"name":"acme","url":"https://cran.acme.com"}"#);
        let json = serde_json::to_string(&acme(true)).unwrap();
        assert!(json.contains(r#""default":true"#));
        let back: CustomRepo =
            serde_json::from_str(r#"{"name":"acme","url":"https://x"}"#).unwrap();
        assert!(!back.default);
    }
}
