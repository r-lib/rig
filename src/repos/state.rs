use std::collections::BTreeMap;
use std::error::Error;

use serde::{Deserialize, Serialize};

use crate::config::{get_global_config_json, set_global_config_json};

use super::interpret_repos_args::ReposSetupArgs;

/// Where the repository choices of an R installation start from.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SetupBase {
    /// The default repositories.
    Default,
    /// No repositories, from `--without-repos`.
    Empty,
}

/// The repository choices of an R installation, as stored in the
/// `repos-setup` entry of the rig configuration file, so `rig repos setup`,
/// `rig repos enable` and `rig repos disable` can re-apply them. Repository
/// names are lowercase.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct SetupState {
    pub base: SetupBase,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enable: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disable: Vec<String>,
}

impl Default for SetupState {
    fn default() -> Self {
        SetupState {
            base: SetupBase::Default,
            enable: vec![],
            disable: vec![],
        }
    }
}

impl SetupState {
    pub fn to_args(&self) -> ReposSetupArgs {
        match self.base {
            SetupBase::Default => ReposSetupArgs::Default {
                whitelist: self.enable.clone(),
                blacklist: self.disable.clone(),
            },
            SetupBase::Empty => ReposSetupArgs::Empty {
                whitelist: self.enable.clone(),
            },
        }
    }

    /// Apply the repository arguments of a command on top of the stored
    /// choices. `--with-repos` / `rig repos enable` add to the enabled
    /// repositories, `--without-repos=<names>` / `rig repos disable` add to
    /// the disabled ones, and the later choice wins for the same repository.
    /// `--without-repos` without names starts over from no repositories.
    pub fn merge(&self, args: &ReposSetupArgs) -> SetupState {
        match args {
            ReposSetupArgs::Empty { whitelist } => SetupState {
                base: SetupBase::Empty,
                enable: dedup(whitelist.clone()),
                disable: vec![],
            },
            ReposSetupArgs::Default {
                whitelist,
                blacklist,
            } => {
                let mut enable: Vec<String> = self
                    .enable
                    .iter()
                    .chain(whitelist.iter())
                    .filter(|x| !blacklist.contains(x))
                    .cloned()
                    .collect();
                let mut disable: Vec<String> = match self.base {
                    SetupBase::Default => self
                        .disable
                        .iter()
                        .chain(blacklist.iter())
                        .filter(|x| !whitelist.contains(x))
                        .cloned()
                        .collect(),
                    // Nothing is enabled to start with, nothing to disable.
                    SetupBase::Empty => vec![],
                };
                enable = dedup(enable);
                disable = dedup(disable);
                SetupState {
                    base: self.base,
                    enable,
                    disable,
                }
            }
        }
    }

    /// Forget a repository, e.g. after `rig repos rm`. Returns whether it was
    /// mentioned at all.
    pub fn forget(&mut self, name: &str) -> bool {
        let before = self.enable.len() + self.disable.len();
        self.enable.retain(|x| x != name);
        self.disable.retain(|x| x != name);
        before != self.enable.len() + self.disable.len()
    }
}

fn dedup(mut x: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    x.retain(|e| seen.insert(e.clone()));
    x
}

const SETUP_STATE_KEY: &str = "repos-setup";

/// The stored repository choices of all R installations, by installation
/// name.
pub fn get_setup_states() -> Result<BTreeMap<String, SetupState>, Box<dyn Error>> {
    match get_global_config_json(SETUP_STATE_KEY)? {
        None => Ok(BTreeMap::new()),
        Some(value) => match serde_json::from_value(value) {
            Ok(states) => Ok(states),
            Err(e) => bail!(
                "Invalid '{}' entry in rig config file: {}",
                SETUP_STATE_KEY,
                e
            ),
        },
    }
}

pub fn save_setup_states(states: &BTreeMap<String, SetupState>) -> Result<(), Box<dyn Error>> {
    let value = if states.is_empty() {
        None
    } else {
        Some(serde_json::to_value(states)?)
    };
    set_global_config_json(SETUP_STATE_KEY, value)
}

pub fn get_setup_state(rver: &str) -> Result<Option<SetupState>, Box<dyn Error>> {
    Ok(get_setup_states()?.remove(rver))
}

/// Store (or with `None` forget) the repository choices of an installation.
pub fn save_setup_state(rver: &str, state: Option<SetupState>) -> Result<(), Box<dyn Error>> {
    let mut states = get_setup_states()?;
    match state {
        Some(s) => states.insert(rver.to_string(), s),
        None => states.remove(rver),
    };
    save_setup_states(&states)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: &[&str]) -> Vec<String> {
        x.iter().map(|x| x.to_string()).collect()
    }

    fn default_args(w: &[&str], b: &[&str]) -> ReposSetupArgs {
        ReposSetupArgs::Default {
            whitelist: s(w),
            blacklist: s(b),
        }
    }

    #[test]
    fn no_arguments_keep_the_stored_choices() {
        let state = SetupState {
            base: SetupBase::Default,
            enable: s(&["bioconductor"]),
            disable: s(&["p3m"]),
        };
        assert_eq!(state.merge(&default_args(&[], &[])), state);
    }

    #[test]
    fn enable_adds_and_undoes_disable() {
        let state = SetupState {
            base: SetupBase::Default,
            enable: s(&["bioconductor"]),
            disable: s(&["p3m"]),
        };
        let new = state.merge(&default_args(&["p3m", "acme"], &[]));
        assert_eq!(new.enable, s(&["bioconductor", "p3m", "acme"]));
        assert!(new.disable.is_empty());
    }

    #[test]
    fn disable_adds_and_undoes_enable() {
        let state = SetupState {
            base: SetupBase::Default,
            enable: s(&["bioconductor", "acme"]),
            disable: vec![],
        };
        let new = state.merge(&default_args(&[], &["acme", "cran"]));
        assert_eq!(new.enable, s(&["bioconductor"]));
        assert_eq!(new.disable, s(&["acme", "cran"]));
    }

    #[test]
    fn merge_does_not_duplicate() {
        let state = SetupState {
            base: SetupBase::Default,
            enable: s(&["acme"]),
            disable: vec![],
        };
        assert_eq!(state.merge(&default_args(&["acme"], &[])), state);
    }

    #[test]
    fn without_repos_starts_over() {
        let state = SetupState {
            base: SetupBase::Default,
            enable: s(&["bioconductor"]),
            disable: s(&["p3m"]),
        };
        let new = state.merge(&ReposSetupArgs::Empty {
            whitelist: s(&["cran"]),
        });
        assert_eq!(
            new,
            SetupState {
                base: SetupBase::Empty,
                enable: s(&["cran"]),
                disable: vec![],
            }
        );
    }

    #[test]
    fn empty_base_is_kept_by_enable_and_disable() {
        let state = SetupState {
            base: SetupBase::Empty,
            enable: s(&["cran"]),
            disable: vec![],
        };
        let new = state.merge(&default_args(&["acme"], &["cran"]));
        assert_eq!(
            new,
            SetupState {
                base: SetupBase::Empty,
                enable: s(&["acme"]),
                disable: vec![],
            }
        );
    }

    #[test]
    fn state_round_trips_through_json() {
        let state = SetupState {
            base: SetupBase::Default,
            enable: s(&["acme"]),
            disable: vec![],
        };
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(json, r#"{"base":"default","enable":["acme"]}"#);
        let back: SetupState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, state);
    }

    #[test]
    fn forget_removes_a_repo() {
        let mut state = SetupState {
            base: SetupBase::Default,
            enable: s(&["acme"]),
            disable: s(&["p3m"]),
        };
        assert!(state.forget("acme"));
        assert!(!state.forget("acme"));
        assert_eq!(state.enable, Vec::<String>::new());
        assert_eq!(state.disable, s(&["p3m"]));
    }
}
