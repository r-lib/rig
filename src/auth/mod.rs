//! Credentials for authenticated repositories and Package Manager instances.
//!
//! Every HTTP request of rig goes through [`request`], which strips any
//! userinfo from the URL and adds a basic `Authorization` header if it finds
//! credentials for it. This follows pak (pkgcache), with some additions from
//! uv. The credentials of a URL are looked up in this order:
//!
//! 1. A password in the URL itself. These are only ever kept in memory, see
//!    [`register_repo`].
//! 2. `RIG_REPO_<NAME>_USERNAME` and `RIG_REPO_<NAME>_PASSWORD` environment
//!    variables, for a repository named `<NAME>`.
//! 3. A Posit Package Manager SSO token, from `~/.ppm/tokens.toml` (the file
//!    pak uses, too), or an identity token in
//!    `PACKAGEMANAGER_IDENTITY_TOKEN_FILE`.
//! 4. A netrc file.
//! 5. The system credential store, for URLs that include a username, with the
//!    same keys as pak: repository URL with and without the username, then
//!    host URL with and without the username, then the host name.
//!
//! Lookups are cached for the lifetime of the process, misses as well.

pub mod cmd;
pub mod netrc;
pub mod ppm_sso;
pub mod store;

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use log::*;

use crate::output::OUTPUT;

/// The home directory of the user. Under `sudo` this is the home of the user
/// who ran rig, as for the rig config file.
pub fn home_dir() -> Option<PathBuf> {
    #[cfg(unix)]
    if sudo::check() == sudo::RunningAs::Root {
        if let Some(h) = std::env::var_os("RIG_HOME") {
            return Some(PathBuf::from(h));
        }
    }
    directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf())
}

#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

// Never print the password, not even in debug logs.
impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Credentials {{ username: {:?}, password: *** }}",
            self.username
        )
    }
}

/// Where credentials were found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Url,
    Env(String),
    PpmSso(String),
    Netrc(PathBuf),
    Keyring(String),
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Url => write!(f, "url"),
            Source::Env(var) => write!(f, "env:{}", var),
            Source::PpmSso(from) => write!(f, "ppm-sso:{}", from),
            Source::Netrc(path) => write!(f, "netrc:{}", path.display()),
            Source::Keyring(service) => write!(f, "keyring:{}:{}", store::BACKEND, service),
        }
    }
}

/// The result of a credential lookup.
#[derive(Debug, Clone, Default)]
pub struct Lookup {
    /// Username in the URL, if any.
    pub username: Option<String>,
    pub creds: Option<Credentials>,
    pub source: Option<Source>,
    /// Name of the registered repository the URL belongs to.
    pub repo: Option<String>,
    /// Problems found during the lookup, e.g. an expired SSO token, or a
    /// credential store that is not available.
    pub notes: Vec<String>,
}

// ------------------------------------------------------------------------
// URLs

/// The parts of an `http(s)` URL that matter for authentication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlParts {
    /// `scheme://host[:port]`
    pub origin: String,
    /// Host name, without the port.
    pub host: String,
    /// Percent-decoded.
    pub username: Option<String>,
    /// Percent-decoded.
    pub password: Option<String>,
    /// The URL without userinfo.
    pub clean: String,
    /// Username as it appears in the URL, percent-encoded.
    raw_username: String,
    path: String,
}

fn pct_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

pub fn parse_url(url: &str) -> Option<UrlParts> {
    let mut u = reqwest::Url::parse(url).ok()?;
    if u.scheme() != "http" && u.scheme() != "https" {
        return None;
    }
    let host = u.host_str()?.to_string();
    let origin = match u.port() {
        Some(p) => format!("{}://{}:{}", u.scheme(), host, p),
        None => format!("{}://{}", u.scheme(), host),
    };
    let raw_username = u.username().to_string();
    let username = if raw_username.is_empty() {
        None
    } else {
        Some(pct_decode(&raw_username))
    };
    let password = u.password().map(pct_decode);
    let path = u.path().to_string();
    let _ = u.set_username("");
    let _ = u.set_password(None);
    let mut clean = u.to_string();
    // `Url` adds a `/` to a bare host, keep the URL as it was given
    if !url.ends_with('/') && u.path() == "/" && u.query().is_none() && u.fragment().is_none() {
        clean.pop();
    }
    Some(UrlParts {
        origin,
        host,
        username,
        password,
        clean,
        raw_username,
        path,
    })
}

impl UrlParts {
    /// The repository part of the path: everything before `/src/` or `/bin/`,
    /// without a `/__linux__/<distro>` component, as in pkgcache.
    fn repo_path(&self) -> String {
        let mut path = self.path.clone();
        for marker in ["/src/", "/bin/"] {
            if let Some(pos) = path.find(marker) {
                path.truncate(pos);
            }
        }
        if let Some(pos) = path.find("/__linux__/") {
            let rest = &path[pos + "/__linux__/".len()..];
            let after = rest
                .find('/')
                .map(|i| rest[i..].to_string())
                .unwrap_or_default();
            path = format!("{}{}", &path[..pos], after);
        }
        path.trim_end_matches('/').to_string()
    }

    fn with_user(&self, origin: &str) -> String {
        if self.raw_username.is_empty() {
            origin.to_string()
        } else {
            origin.replacen("://", &format!("://{}@", self.raw_username), 1)
        }
    }

    /// Repository URL without and with the username.
    pub fn repo_url(&self) -> String {
        format!("{}{}", self.origin, self.repo_path())
    }

    /// The keys to look for in the credential store, most specific first.
    pub fn auth_domains(&self) -> Vec<String> {
        let repo = self.repo_url();
        let mut domains = vec![
            self.with_user(&repo),
            repo,
            self.with_user(&self.origin),
            self.origin.clone(),
            self.host.clone(),
        ];
        let mut seen = HashSet::new();
        domains.retain(|d| seen.insert(d.clone()));
        domains
    }
}

/// `url` without a username and password.
pub fn strip_userinfo(url: &str) -> String {
    match parse_url(url) {
        Some(p) if p.username.is_some() || p.password.is_some() => p.clean,
        _ => url.to_string(),
    }
}

/// `url` without a password, but with the username, if any. Use this before
/// storing or printing a URL.
pub fn redact_url(url: &str) -> String {
    match parse_url(url) {
        Some(p) if p.password.is_some() => match p.username {
            Some(_) => p.with_user(&p.clean),
            None => p.clean,
        },
        _ => url.to_string(),
    }
}

pub fn url_has_password(url: &str) -> bool {
    parse_url(url).is_some_and(|p| p.password.is_some())
}

/// `RIG_REPO_<NAME>_` for repository `name`: upper case, every character that
/// is not a letter or a digit replaced by `_`.
pub fn env_prefix(name: &str) -> String {
    let name: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("RIG_REPO_{}_", name)
}

/// Whether `url` is `base` or below it.
fn under(url: &str, base: &str) -> bool {
    let base = base.trim_end_matches('/');
    url == base
        || (url.starts_with(base)
            && matches!(url.as_bytes().get(base.len()), Some(b'/') | Some(b'?')))
}

// ------------------------------------------------------------------------
// Registry of repositories

struct Registered {
    name: String,
    /// Without userinfo and trailing `/`.
    base: String,
    creds: Option<Credentials>,
}

fn registry() -> &'static Mutex<Vec<Registered>> {
    static REG: OnceLock<Mutex<Vec<Registered>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(vec![]))
}

/// Make rig aware of a repository, so the `RIG_REPO_<NAME>_*` environment
/// variables and a password in its URL apply to it. Returns the URL without
/// the password, which is the only form that may be stored or printed.
pub fn register_repo(name: &str, url: &str) -> String {
    let Some(p) = parse_url(url) else {
        return url.to_string();
    };
    let creds = p.password.clone().map(|password| Credentials {
        username: p.username.clone().unwrap_or_default(),
        password,
    });
    let base = p.clean.trim_end_matches('/').to_string();
    let mut reg = registry().lock().unwrap();
    match reg.iter_mut().find(|r| r.name == name && r.base == base) {
        Some(r) => {
            if creds.is_some() {
                r.creds = creds;
            }
        }
        None => reg.push(Registered {
            name: name.to_string(),
            base,
            creds,
        }),
    }
    drop(reg);
    redact_url(url)
}

/// The registered repository `clean` belongs to, the most specific one if
/// there are several.
fn registered_for(clean: &str) -> Option<(String, Option<Credentials>)> {
    let reg = registry().lock().unwrap();
    reg.iter()
        .filter(|r| under(clean, &r.base))
        .max_by_key(|r| r.base.len())
        .map(|r| (r.name.clone(), r.creds.clone()))
}

// ------------------------------------------------------------------------
// Lookup

fn netrc_entries() -> &'static Option<(PathBuf, Vec<netrc::NetrcEntry>)> {
    static NETRC: OnceLock<Option<(PathBuf, Vec<netrc::NetrcEntry>)>> = OnceLock::new();
    NETRC.get_or_init(|| {
        let path = netrc::netrc_path()?;
        let text = std::fs::read_to_string(&path).ok()?;
        Some((path, netrc::parse_netrc(&text)))
    })
}

/// PPM SSO tokens obtained from identity tokens in this process.
fn identity_cache() -> &'static Mutex<HashMap<String, Option<String>>> {
    static C: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn sso_lookup(p: &UrlParts, res: &mut Lookup) -> Option<Credentials> {
    if p.username.as_deref().is_some_and(|u| u != "__token__") {
        return None;
    }
    let explicit = p.username.is_some();

    // A stored token for an instance this URL belongs to
    let conn = ppm_sso::read_connections()
        .into_iter()
        .filter(|c| under(&p.clean, &c.address))
        .max_by_key(|c| c.address.len());
    if let Some(conn) = conn {
        if !ppm_sso::is_expired(&conn.token) {
            let file = ppm_sso::token_file_path()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            res.source = Some(Source::PpmSso(file));
            return Some(Credentials {
                username: "__token__".to_string(),
                password: conn.token,
            });
        }
        res.notes.push(format!(
            "The PPM SSO token of {} has expired, run `rig auth login {}`",
            conn.address, conn.address
        ));
    }

    // An identity token, from CI, for PACKAGEMANAGER_ADDRESS
    let addr = ppm_sso::ppm_address()?;
    if !under(&p.clean, &addr) {
        return None;
    }
    if std::env::var_os("PACKAGEMANAGER_IDENTITY_TOKEN_FILE").is_some() {
        let mut cache = identity_cache().lock().unwrap();
        let token = cache
            .entry(addr.clone())
            .or_insert_with(|| {
                let identity = ppm_sso::identity_token_from_file()?;
                let addr = addr.clone();
                let res = std::thread::scope(|s| {
                    s.spawn(move || {
                        ppm_sso::identity_to_ppm_token(&addr, &identity).map_err(|e| e.to_string())
                    })
                    .join()
                    .unwrap_or_else(|_| Err("panic".to_string()))
                });
                match res {
                    Ok(t) => Some(t),
                    Err(e) => {
                        OUTPUT.warn(&format!("PPM SSO: {}", e));
                        None
                    }
                }
            })
            .clone();
        if let Some(token) = token {
            res.source = Some(Source::PpmSso(
                "PACKAGEMANAGER_IDENTITY_TOKEN_FILE".to_string(),
            ));
            return Some(Credentials {
                username: "__token__".to_string(),
                password: token,
            });
        }
    } else if explicit && conn_missing(&addr) {
        res.notes.push(format!(
            "No PPM SSO token for {}, run `rig auth login`",
            addr
        ));
    }
    None
}

fn conn_missing(addr: &str) -> bool {
    ppm_sso::stored_token(addr).is_none()
}

/// Look up the credentials of `url`, without the cache and without warnings.
pub fn lookup_uncached(url: &str) -> Lookup {
    let mut res = Lookup::default();
    let Some(p) = parse_url(url) else {
        return res;
    };
    res.username = p.username.clone();

    // 1. password in the URL
    let reg = registered_for(&p.clean);
    if let Some(password) = p.password.clone() {
        res.creds = Some(Credentials {
            username: p.username.clone().unwrap_or_default(),
            password,
        });
        res.source = Some(Source::Url);
        return res;
    }
    if let Some((name, creds)) = &reg {
        res.repo = Some(name.clone());
        if let Some(c) = creds {
            if p.username.is_none() || p.username.as_deref() == Some(&c.username) {
                res.creds = Some(c.clone());
                res.source = Some(Source::Url);
                return res;
            }
        }
    }

    // 2. environment variables
    if let Some((name, _)) = &reg {
        let prefix = env_prefix(name);
        let pw_var = format!("{}PASSWORD", prefix);
        if let Ok(password) = std::env::var(&pw_var) {
            let username = std::env::var(format!("{}USERNAME", prefix))
                .ok()
                .or_else(|| p.username.clone())
                .unwrap_or_default();
            res.creds = Some(Credentials { username, password });
            res.source = Some(Source::Env(pw_var));
            return res;
        }
    }

    // 3. PPM SSO
    if let Some(c) = sso_lookup(&p, &mut res) {
        res.creds = Some(c);
        return res;
    }

    // 4. netrc. The `default` entry only applies to URLs with a username, as
    //    otherwise it would apply to every host rig talks to.
    if let Some((path, entries)) = netrc_entries() {
        let hit = netrc::netrc_lookup(entries, &p.host, p.username.as_deref());
        let is_machine = entries.iter().any(|e| {
            e.machine
                .as_deref()
                .is_some_and(|m| m.eq_ignore_ascii_case(&p.host))
        });
        if let Some((username, password)) = hit {
            if is_machine || p.username.is_some() {
                res.creds = Some(Credentials { username, password });
                res.source = Some(Source::Netrc(path.clone()));
                return res;
            }
        }
    }

    // 5. credential store, only with a username
    if let Some(user) = p.username.clone() {
        for domain in p.auth_domains() {
            match store::get(&domain, &user) {
                Ok(Some(password)) => {
                    res.creds = Some(Credentials {
                        username: user,
                        password,
                    });
                    res.source = Some(Source::Keyring(domain));
                    return res;
                }
                Ok(None) => {}
                Err(e) => {
                    res.notes
                        .push(format!("credential store ({}): {}", store::BACKEND, e));
                    break;
                }
            }
        }
    }

    res
}

fn cache() -> &'static Mutex<HashMap<String, Lookup>> {
    static C: OnceLock<Mutex<HashMap<String, Lookup>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Look up the credentials of `url`, cached per repository (or per path
/// for URLs that are not in a repository). Warns once if the URL has a
/// username but no credentials were found.
pub fn lookup(url: &str) -> Lookup {
    let Some(p) = parse_url(url) else {
        return Lookup::default();
    };
    // a password in the URL needs no lookup, and must not end up in the cache
    if p.password.is_some() {
        return lookup_uncached(url);
    }
    let key = format!("{}|{}", p.raw_username, p.repo_url());
    if let Some(l) = cache().lock().unwrap().get(&key) {
        return l.clone();
    }
    let res = lookup_uncached(url);
    match &res.source {
        Some(src) => debug!("Credentials for {} from {}", redact_url(url), src),
        None => {
            for note in &res.notes {
                debug!("Credential lookup for {}: {}", p.repo_url(), note);
            }
            if p.username.is_some() {
                let mut msg = format!(
                    "Did not find credentials for {}",
                    p.with_user(&p.repo_url())
                );
                if let Some(note) = res.notes.first() {
                    msg.push_str(&format!(" ({})", note));
                }
                msg.push_str(". See `rig auth status`.");
                OUTPUT.warn(&msg);
            }
        }
    }
    cache().lock().unwrap().insert(key, res.clone());
    res
}

/// A request for `url`, with credentials if rig has any for it. The URL that
/// is actually requested never has userinfo, so it cannot leak into error
/// messages.
pub fn request(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: &str,
) -> reqwest::RequestBuilder {
    let creds = lookup(url).creds;
    let req = client.request(method, strip_userinfo(url));
    match creds {
        Some(c) => req.basic_auth(c.username, Some(c.password)),
        None => req,
    }
}

pub fn get(client: &reqwest::Client, url: &str) -> reqwest::RequestBuilder {
    request(client, reqwest::Method::GET, url)
}

pub fn head(client: &reqwest::Client, url: &str) -> reqwest::RequestBuilder {
    request(client, reqwest::Method::HEAD, url)
}

/// Extra text for an HTTP error message, for statuses that mean missing or
/// wrong credentials.
pub fn status_hint(status: reqwest::StatusCode) -> &'static str {
    match status.as_u16() {
        401 | 403 => " (authentication needed? See `rig auth status`)",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_url() {
        let p = parse_url("https://us%40er:p%3Ass@Repo.example.com:8443/cran/latest").unwrap();
        assert_eq!(p.username.as_deref(), Some("us@er"));
        assert_eq!(p.password.as_deref(), Some("p:ss"));
        assert_eq!(p.host, "repo.example.com");
        assert_eq!(p.origin, "https://repo.example.com:8443");
        assert_eq!(p.clean, "https://repo.example.com:8443/cran/latest");
        assert!(parse_url("file:///tmp/repo").is_none());
        assert_eq!(
            parse_url("https://example.com").unwrap().clean,
            "https://example.com"
        );
        assert_eq!(
            parse_url("https://example.com/").unwrap().clean,
            "https://example.com/"
        );
    }

    #[test]
    fn test_auth_domains() {
        let p = parse_url(
            "https://__token__@ppm.example.com/cran/__linux__/jammy/latest/src/contrib/PACKAGES.gz",
        )
        .unwrap();
        assert_eq!(
            p.auth_domains(),
            vec![
                "https://__token__@ppm.example.com/cran/latest",
                "https://ppm.example.com/cran/latest",
                "https://__token__@ppm.example.com",
                "https://ppm.example.com",
                "ppm.example.com",
            ]
        );
        let p =
            parse_url("https://ppm.example.com/cran/latest/bin/windows/contrib/4.5/x.zip").unwrap();
        assert_eq!(p.repo_url(), "https://ppm.example.com/cran/latest");
        assert_eq!(
            p.auth_domains(),
            vec![
                "https://ppm.example.com/cran/latest",
                "https://ppm.example.com",
                "ppm.example.com",
            ]
        );
    }

    #[test]
    fn test_redact_url() {
        assert_eq!(
            redact_url("https://user:secret@example.com/repo"),
            "https://user@example.com/repo"
        );
        assert_eq!(
            redact_url("https://:secret@example.com/repo"),
            "https://example.com/repo"
        );
        assert_eq!(
            redact_url("https://user@example.com/repo"),
            "https://user@example.com/repo"
        );
        assert_eq!(
            strip_userinfo("https://user:pw@example.com/repo"),
            "https://example.com/repo"
        );
        assert!(url_has_password("https://user:pw@example.com"));
        assert!(!url_has_password("https://user@example.com"));
    }

    #[test]
    fn test_env_prefix() {
        assert_eq!(env_prefix("internal-proxy"), "RIG_REPO_INTERNAL_PROXY_");
        assert_eq!(env_prefix("acme.cran"), "RIG_REPO_ACME_CRAN_");
    }

    #[test]
    fn test_under() {
        assert!(under("https://h/cran/src/contrib", "https://h/cran"));
        assert!(under("https://h/cran", "https://h/cran/"));
        assert!(!under("https://h/cranx", "https://h/cran"));
    }

    #[test]
    fn test_register_repo() {
        let url = register_repo("authtest-reg", "https://bob:pw@auth-reg.example.com/cran");
        assert_eq!(url, "https://bob@auth-reg.example.com/cran");
        let l = lookup_uncached("https://bob@auth-reg.example.com/cran/src/contrib/PACKAGES");
        assert_eq!(l.source, Some(Source::Url));
        assert_eq!(
            l.creds,
            Some(Credentials {
                username: "bob".to_string(),
                password: "pw".to_string()
            })
        );
        assert_eq!(l.repo.as_deref(), Some("authtest-reg"));
        // other hosts are not affected
        assert!(lookup_uncached("https://other-reg.example.com/cran")
            .creds
            .is_none());
    }
}
