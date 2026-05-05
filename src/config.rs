use std::collections::HashMap;
use std::error::Error;
use std::path::PathBuf;

use clap::ArgMatches;

use simple_error::{bail, SimpleError};

use serde_derive::Deserialize;
use serde_derive::Serialize;

use crate::cache::get_data_dir;
use crate::utils::*;

#[derive(Serialize, Deserialize, Debug)]
struct Config {
    #[serde(default = "empty_stringmap")]
    userlibrary: HashMap<String, String>,
    #[serde(flatten)]
    extra: HashMap<String, serde_json::Value>,
}

fn empty_stringmap() -> HashMap<String, String> {
    HashMap::<String, String>::new()
}

fn rig_config_dir() -> Result<PathBuf, Box<dyn Error>> {
    get_data_dir()
}

fn rig_config_file() -> Result<PathBuf, Box<dyn Error>> {
    let config_dir = rig_config_dir()?;
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
        let parent = config_file
            .parent()
            .ok_or(SimpleError::new("Invalid config file directory"))?;
        std::fs::create_dir_all(&parent)?;
        std::fs::write(config_file, str)?;
        Ok(())
    }

    fn get_userlibrary(&self, rver: &str) -> Option<String> {
        self.userlibrary.get(rver).and_then(|x| Some(x.to_string()))
    }

    fn set_userlibrary(&mut self, rver: &str, value: Option<&str>) -> Result<(), Box<dyn Error>> {
        match value {
            None => self.userlibrary.remove(&rver.to_string()),
            Some(str) => self.userlibrary.insert(rver.to_string(), str.to_string()),
        };
        self.save()?;

        Ok(())
    }
}

pub fn save_config(rver: &str, key: &str, value: Option<&str>) -> Result<(), Box<dyn Error>> {
    let mut config = Config::load()?;
    match key {
        "userlibrary" => config.set_userlibrary(rver, value)?,
        _ => bail!("Unknown config key: {}, internal error", key),
    };

    Ok(())
}

pub fn get_config(rver: &str, key: &str) -> Result<Option<String>, Box<dyn Error>> {
    let config = Config::load()?;
    match key {
        "userlibrary" => Ok(config.get_userlibrary(rver)),
        _ => bail!("Unknown config key: {}, internal error", key),
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
pub fn sc_config(_args: &ArgMatches, _mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    // Cannot be called
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn sc_config(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        Some(("config-file-path", _)) => sc_config_config_file_path(),
        Some(("list", s)) => sc_config_list(s, mainargs),
        Some(("get", s)) => sc_config_get(s, mainargs),
        Some(("set", s)) => sc_config_set(s),
        _ => Ok(()),
    }
}

#[cfg(target_os = "macos")]
fn sc_config_config_file_path() -> Result<(), Box<dyn Error>> {
    let path = rig_config_file()?;
    println!("{}", path.display());
    Ok(())
}

#[cfg(target_os = "macos")]
fn sc_config_get(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let key = args.get_one::<String>("key").unwrap();
    let json = args.get_flag("json") || mainargs.get_flag("json");

    let config_file = rig_config_file()?;
    let root: serde_json::Value = if config_file.exists() {
        let contents = read_file_string(&config_file)?;
        serde_json::from_str(&contents)?
    } else {
        serde_json::Value::Object(serde_json::Map::new())
    };

    let value = &root[key.as_str()];
    match value {
        serde_json::Value::Null => {
            if json {
                println!("null");
            }
        }
        serde_json::Value::String(s) => {
            if json {
                println!("{}", serde_json::to_string(s)?);
            } else {
                println!("{}", s);
            }
        }
        scalar @ (serde_json::Value::Bool(_) | serde_json::Value::Number(_)) => {
            println!("{}", scalar);
        }
        complex => {
            println!("{}", serde_json::to_string_pretty(complex)?);
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn load_raw_config() -> Result<serde_json::Map<String, serde_json::Value>, Box<dyn Error>> {
    let config_file = rig_config_file()?;
    if config_file.exists() {
        let contents = read_file_string(&config_file)?;
        let value: serde_json::Value = serde_json::from_str(&contents)?;
        match value {
            serde_json::Value::Object(map) => Ok(map),
            _ => bail!("Config file is not a JSON object"),
        }
    } else {
        Ok(serde_json::Map::new())
    }
}

#[cfg(target_os = "macos")]
fn save_raw_config(map: &serde_json::Map<String, serde_json::Value>) -> Result<(), Box<dyn Error>> {
    let config_file = rig_config_file()?;
    let parent = config_file
        .parent()
        .ok_or(SimpleError::new("Invalid config file directory"))?;
    std::fs::create_dir_all(parent)?;
    std::fs::write(config_file, serde_json::to_string_pretty(map)?)?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn get_global_config_value(key: &str) -> Result<Option<String>, Box<dyn Error>> {
    let map = load_raw_config()?;
    match map.get(key) {
        Some(serde_json::Value::String(s)) => Ok(Some(s.clone())),
        _ => Ok(None),
    }
}

#[cfg(target_os = "macos")]
fn sc_config_set(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let keyvalue = args.get_one::<String>("keyvalue").unwrap();
    let (key, value) = keyvalue
        .split_once('=')
        .ok_or_else(|| SimpleError::new(format!("Invalid key=value format: '{}'", keyvalue)))?;
    let mut map = load_raw_config()?;
    map.insert(key.to_string(), serde_json::Value::String(value.to_string()));
    save_raw_config(&map)
}

#[cfg(target_os = "macos")]
fn sc_config_list(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let config_file = rig_config_file()?;
    let keys: Vec<String> = if config_file.exists() {
        let contents = read_file_string(&config_file)?;
        let value: serde_json::Value = serde_json::from_str(&contents)?;
        match value.as_object() {
            Some(obj) => obj.keys().cloned().collect(),
            None => vec![],
        }
    } else {
        vec![]
    };

    if args.get_flag("json") || mainargs.get_flag("json") {
        #[derive(serde::Serialize)]
        struct Entry { key: String }
        let entries: Vec<Entry> = keys.into_iter().map(|k| Entry { key: k }).collect();
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else {
        for key in keys {
            println!("{}", key);
        }
    }
    Ok(())
}
