use std::collections::HashMap;
use std::error::Error;
use std::path::PathBuf;

use clap::ArgMatches;

use simple_error::SimpleError;

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
    if let Some(dir) = escalated_config_dir() {
        return Ok(dir);
    }
    get_data_dir()
}

// When rig re-runs itself with `sudo` (see `escalate()`), HOME may point to
// root's home directory, but the configuration belongs to the user who ran
// rig. `escalate()` records that user's home in RIG_HOME, use it.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn escalated_config_dir() -> Option<PathBuf> {
    if sudo::check() != sudo::RunningAs::Root {
        return None;
    }
    let home = PathBuf::from(std::env::var_os("RIG_HOME")?);
    #[cfg(target_os = "macos")]
    let dir = home.join("Library/Application Support/com.gaborcsardi.rig");
    #[cfg(target_os = "linux")]
    let dir = home.join(".local/share/rig");
    Some(dir)
}

#[cfg(target_os = "windows")]
fn escalated_config_dir() -> Option<PathBuf> {
    None
}

fn rig_config_file() -> Result<PathBuf, Box<dyn Error>> {
    let config_dir = rig_config_dir()?;
    let config_file = config_dir.join("config.json");
    Ok(config_file)
}

// The path of the rig configuration file. It does not need to exist.
// Used by `rig config config-file-path` and `rig system dirs`.
pub fn config_file_path() -> Result<PathBuf, Box<dyn Error>> {
    rig_config_file()
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
        std::fs::create_dir_all(parent)?;
        std::fs::write(&config_file, str)?;
        give_back_to_user(&config_file)?;
        Ok(())
    }

    fn get_userlibrary(&self, rver: &str) -> Option<String> {
        self.userlibrary.get(rver).map(|x| x.to_string())
    }

    fn set_userlibrary(&mut self, rver: &str, value: Option<&str>) -> Result<(), Box<dyn Error>> {
        match value {
            None => self.userlibrary.remove(rver),
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

pub fn sc_config(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        Some(("config-file-path", _)) => sc_config_config_file_path(),
        Some(("list", s)) => sc_config_list(s, mainargs),
        Some(("get", s)) => sc_config_get(s, mainargs),
        Some(("set", s)) => sc_config_set(s),
        _ => Ok(()),
    }
}

fn sc_config_config_file_path() -> Result<(), Box<dyn Error>> {
    let path = rig_config_file()?;
    println!("{}", path.display());
    Ok(())
}

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

fn save_raw_config(map: &serde_json::Map<String, serde_json::Value>) -> Result<(), Box<dyn Error>> {
    let config_file = rig_config_file()?;
    let parent = config_file
        .parent()
        .ok_or(SimpleError::new("Invalid config file directory"))?;
    std::fs::create_dir_all(parent)?;
    std::fs::write(&config_file, serde_json::to_string_pretty(map)?)?;
    give_back_to_user(&config_file)?;
    Ok(())
}

// After `sudo` the config file is written by root, into the user's home
// directory. Give it, and the directories created for it, back to the user,
// so rig can still update it without `sudo`.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn give_back_to_user(config_file: &std::path::Path) -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::MetadataExt;
    if escalated_config_dir().is_none() {
        return Ok(());
    }
    let home = match std::env::var_os("RIG_HOME") {
        Some(h) => PathBuf::from(h),
        None => return Ok(()),
    };
    let meta = std::fs::metadata(&home)?;
    let (uid, gid) = (meta.uid(), meta.gid());
    let mut path = Some(config_file);
    while let Some(p) = path {
        if p == home || !p.starts_with(&home) {
            break;
        }
        std::os::unix::fs::chown(p, Some(uid), Some(gid))?;
        path = p.parent();
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn give_back_to_user(_config_file: &std::path::Path) -> Result<(), Box<dyn Error>> {
    Ok(())
}

pub fn get_global_config_value(key: &str) -> Result<Option<String>, Box<dyn Error>> {
    let map = load_raw_config()?;
    match map.get(key) {
        Some(serde_json::Value::String(s)) => Ok(Some(s.clone())),
        _ => Ok(None),
    }
}

/// A configuration entry, as JSON, e.g. a list or an object.
pub fn get_global_config_json(key: &str) -> Result<Option<serde_json::Value>, Box<dyn Error>> {
    let map = load_raw_config()?;
    Ok(map.get(key).cloned())
}

/// Set (or with `None` remove) a JSON configuration entry. The file is only
/// written if the entry changes.
pub fn set_global_config_json(
    key: &str,
    value: Option<serde_json::Value>,
) -> Result<(), Box<dyn Error>> {
    let mut map = load_raw_config()?;
    if map.get(key) == value.as_ref() {
        return Ok(());
    }
    match value {
        Some(v) => map.insert(key.to_string(), v),
        None => map.remove(key),
    };
    save_raw_config(&map)
}

/// A boolean configuration entry.
pub fn get_global_config_bool(key: &str) -> Result<Option<bool>, Box<dyn Error>> {
    let map = load_raw_config()?;
    match map.get(key) {
        None => Ok(None),
        Some(serde_json::Value::Bool(b)) => Ok(Some(*b)),
        Some(serde_json::Value::String(s)) => match s.as_str() {
            "true" => Ok(Some(true)),
            "false" => Ok(Some(false)),
            _ => bail!(
                "Invalid '{}' in rig config: '{}', expected 'true' or 'false'",
                key,
                s
            ),
        },
        Some(other) => bail!(
            "Invalid '{}' in rig config: {}, expected 'true' or 'false'",
            key,
            other
        ),
    }
}

pub fn set_global_config_value(key: &str, value: &str) -> Result<(), Box<dyn Error>> {
    let mut map = load_raw_config()?;
    map.insert(
        key.to_string(),
        serde_json::Value::String(value.to_string()),
    );
    save_raw_config(&map)
}

fn sc_config_set(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let keyvalue = args.get_one::<String>("keyvalue").unwrap();
    let (key, value) = keyvalue
        .split_once('=')
        .ok_or_else(|| SimpleError::new(format!("Invalid key=value format: '{}'", keyvalue)))?;
    let mut map = load_raw_config()?;
    map.insert(
        key.to_string(),
        serde_json::Value::String(value.to_string()),
    );
    save_raw_config(&map)
}

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
        struct Entry {
            key: String,
        }
        let entries: Vec<Entry> = keys.into_iter().map(|k| Entry { key: k }).collect();
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else {
        for key in keys {
            println!("{}", key);
        }
    }
    Ok(())
}
