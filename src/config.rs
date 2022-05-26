
use std::collections::HashMap;
use std::error::Error;
use std::path::PathBuf;

use directories::ProjectDirs;
use simple_error::bail;

use serde_derive::Serialize;
use serde_derive::Deserialize;

use crate::utils::*;

#[derive(Serialize,Deserialize,Debug)]
struct Config {
    #[serde(default = "empty_stringmap")]
    userlibrary: HashMap<String, String>,
}

fn empty_stringmap() -> HashMap<String, String> {
    HashMap::<String, String>::new()
}

fn rig_config_file() -> Result<PathBuf, Box<dyn Error>> {
    let proj_dirs = match ProjectDirs::from("com", "gaborcsardi", "rig") {
        Some(x) => x,
        None => bail!("Config file if not supported on this system")
    };
    let config_dir = proj_dirs.data_dir();
    let config_file = config_dir.join("config.json");
    Ok(config_file)
}

impl Config {

    fn load() -> Result<Config, Box<dyn Error>> {
        let config_file = rig_config_file()?;
        let config: Config = if config_file.exists() {
            let contents = read_file_string(&config_file)?;
            serde_json::from_str(&contents)?
        } else {
            serde_json::from_str::<Config>("{}")?
        };

        Ok(config)
    }

    fn save(&self) -> Result<(), Box<dyn Error>> {
        let str = serde_json::to_string_pretty(self)?;
        let config_file = rig_config_file()?;
        std::fs::write(config_file, str)?;
        Ok(())
    }

    fn get_userlibrary(&self, rver: &str) -> Option<String> {
        self.userlibrary.get(rver).and_then(|x| Some(x.to_string()))
    }

    fn set_userlibrary(&mut self, rver: &str, value: Option<&str>)
                       -> Result<(), Box<dyn Error>> {
        match value {
            None => {
                self.userlibrary.remove(&rver.to_string())
            },
            Some(str) => {
                self.userlibrary.insert(rver.to_string(), str.to_string())
            },
        };
        self.save()?;

        Ok(())
    }
}

pub fn save_config(rver: &str, key: &str, value: Option<&str>)
                   -> Result<(), Box<dyn Error>> {

    let mut config = Config::load()?;
    match key {
        "userlibrary" => config.set_userlibrary(rver, value)?,
        _ => bail!("Unknown config key: {}, internal error", key)
    };

    Ok(())
}

pub fn get_config(rver: &str, key: &str)
                  -> Result<Option<String>, Box<dyn Error>> {

    let config = Config::load()?;
    match key {
        "userlibrary" => Ok(config.get_userlibrary(rver)),
        _ => bail!("Unknown config key: {}, internal error", key)
    }
}
