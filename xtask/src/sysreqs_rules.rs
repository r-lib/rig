//! `gen-sysreqs-rules <dir>`: merge the rules of an r-system-requirements
//! checkout (https://github.com/r-hub/r-system-requirements) into the
//! committed `src/data/sysreqs-rules.json`.
//!
//! rig embeds this file and uses it when it cannot download a fresh copy of
//! the rules. The format is one JSON object, keyed by rule name (the file name
//! of `rules/<name>.json`, without the extension), with each rule unchanged.
//! rig writes its downloaded copy in the same format.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::ExitCode;

pub fn gen_sysreqs_rules(root: &Path, src: &Path) -> ExitCode {
    let rules_dir = src.join("rules");
    let entries = match fs::read_dir(&rules_dir) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("cannot read {}: {}", rules_dir.display(), e);
            return ExitCode::FAILURE;
        }
    };

    let mut rules: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.extension().map(|e| e != "json").unwrap_or(true) {
            continue;
        }
        let name = path
            .file_stem()
            .expect("rule file name")
            .to_string_lossy()
            .to_string();
        let text = fs::read_to_string(&path).expect("read rule file");
        match serde_json::from_str(&text) {
            Ok(rule) => {
                rules.insert(name, rule);
            }
            Err(e) => {
                eprintln!("invalid rule {}: {}", path.display(), e);
                return ExitCode::FAILURE;
            }
        }
    }

    let out = root.join("src/data/sysreqs-rules.json");
    let mut text = serde_json::to_string_pretty(&rules).expect("serialize rules");
    text.push('\n');
    fs::write(&out, text).expect("write sysreqs-rules.json");
    eprintln!("wrote {} rules to {}", rules.len(), out.display());
    ExitCode::SUCCESS
}
