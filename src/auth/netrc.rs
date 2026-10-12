//! Reading credentials from a netrc file, the way curl does.

use std::path::PathBuf;

/// One `machine` (or `default`) entry of a netrc file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetrcEntry {
    /// `None` for the `default` entry.
    pub machine: Option<String>,
    pub login: Option<String>,
    pub password: Option<String>,
}

/// The netrc file to read: `PKG_NETRC_PATH` (as pak), then `NETRC` (as curl),
/// then `~/.netrc`, and on Windows `~/_netrc` if `~/.netrc` does not exist.
pub fn netrc_path() -> Option<PathBuf> {
    for var in ["PKG_NETRC_PATH", "NETRC"] {
        if let Ok(p) = std::env::var(var) {
            if !p.is_empty() {
                return Some(PathBuf::from(p));
            }
        }
    }
    let home = super::home_dir()?;
    let path = home.join(".netrc");
    #[cfg(target_os = "windows")]
    if !path.exists() {
        return Some(home.join("_netrc"));
    }
    Some(path)
}

/// Split a netrc file into tokens. Like curl, a token may be quoted with
/// `"`, and then `\"`, `\\`, `\n`, `\r` and `\t` are escapes.
fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = vec![];
    let mut chars = text.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let Some(&first) = chars.peek() else {
            break;
        };
        let mut tok = String::new();
        if first == '"' {
            chars.next();
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => match chars.next() {
                        Some('n') => tok.push('\n'),
                        Some('r') => tok.push('\r'),
                        Some('t') => tok.push('\t'),
                        Some(c) => tok.push(c),
                        None => break,
                    },
                    c => tok.push(c),
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                tok.push(c);
                chars.next();
            }
        }
        tokens.push(tok);
    }
    tokens
}

/// Parse the contents of a netrc file. `macdef` definitions are skipped, and
/// so is anything rig does not know, e.g. `account`.
pub fn parse_netrc(text: &str) -> Vec<NetrcEntry> {
    // `macdef` runs until an empty line, so drop those before tokenizing.
    let mut cleaned = String::new();
    let mut in_macdef = false;
    for line in text.lines() {
        if in_macdef {
            if line.trim().is_empty() {
                in_macdef = false;
            }
            continue;
        }
        for w in line.split_whitespace() {
            if w == "macdef" {
                in_macdef = true;
                break;
            }
            cleaned.push_str(w);
            cleaned.push(' ');
        }
        cleaned.push('\n');
    }

    let tokens = tokenize(&cleaned);
    let mut entries: Vec<NetrcEntry> = vec![];
    let mut i = 0;
    while i < tokens.len() {
        match tokens[i].as_str() {
            "machine" => {
                entries.push(NetrcEntry {
                    machine: tokens.get(i + 1).cloned(),
                    login: None,
                    password: None,
                });
                i += 2;
            }
            "default" => {
                entries.push(NetrcEntry {
                    machine: None,
                    login: None,
                    password: None,
                });
                i += 1;
            }
            "login" | "password" | "account" => {
                let value = tokens.get(i + 1).cloned();
                if let Some(entry) = entries.last_mut() {
                    match tokens[i].as_str() {
                        "login" => entry.login = value,
                        "password" => entry.password = value,
                        _ => {}
                    }
                }
                i += 2;
            }
            _ => i += 1,
        }
    }
    entries
}

/// The login and password for `host` in `entries`. If `username` is given,
/// the entry's login must match it. A `machine` entry wins over `default`.
pub fn netrc_lookup(
    entries: &[NetrcEntry],
    host: &str,
    username: Option<&str>,
) -> Option<(String, String)> {
    let matches = |e: &&NetrcEntry| {
        e.password.is_some()
            && match (username, &e.login) {
                (None, _) => true,
                (Some(u), Some(l)) => u == l,
                (Some(_), None) => false,
            }
    };
    let hit = entries
        .iter()
        .filter(|e| {
            e.machine
                .as_deref()
                .is_some_and(|m| m.eq_ignore_ascii_case(host))
        })
        .find(matches)
        .or_else(|| entries.iter().filter(|e| e.machine.is_none()).find(matches))?;
    let login = hit
        .login
        .clone()
        .or_else(|| username.map(|u| u.to_string()))
        .unwrap_or_default();
    Some((login, hit.password.clone().unwrap_or_default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NETRC: &str = r#"
machine repo.example.com login alice password s3cret
machine repo.example.com
  login bob
  password "with space\"quote"

macdef init
machine evil.example.com login x password y

machine other.example.com login carol password c
default login anon password anonpw
"#;

    #[test]
    fn test_parse_netrc() {
        let e = parse_netrc(NETRC);
        assert_eq!(e.len(), 4);
        assert_eq!(e[1].password.as_deref(), Some("with space\"quote"));
        assert!(e
            .iter()
            .all(|e| e.machine.as_deref() != Some("evil.example.com")));
        assert_eq!(e[3].machine, None);
    }

    #[test]
    fn test_netrc_lookup() {
        let e = parse_netrc(NETRC);
        assert_eq!(
            netrc_lookup(&e, "repo.example.com", None),
            Some(("alice".to_string(), "s3cret".to_string()))
        );
        assert_eq!(
            netrc_lookup(&e, "REPO.example.com", Some("bob")),
            Some(("bob".to_string(), "with space\"quote".to_string()))
        );
        // login mismatch for the machine falls back to `default`, which has
        // a different login as well
        assert_eq!(netrc_lookup(&e, "other.example.com", Some("dave")), None);
        assert_eq!(
            netrc_lookup(&e, "unknown.example.com", None),
            Some(("anon".to_string(), "anonpw".to_string()))
        );
        assert_eq!(netrc_lookup(&parse_netrc(""), "x", None), None);
    }
}
