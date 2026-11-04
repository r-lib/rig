//! `rig auth`: credentials for repositories and Package Manager instances.

use std::error::Error;
use std::io::{BufRead, IsTerminal};

use clap::ArgMatches;
use serde::Serialize;
use tabular::{Row, Table};

use super::*;

pub fn sc_auth(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        Some(("login", s)) => sc_auth_login(s),
        Some(("logout", s)) => sc_auth_logout(s),
        Some(("status", s)) => sc_auth_status(s, args, mainargs),
        Some(("token", s)) => sc_auth_token(s),
        _ => Ok(()),
    }
}

/// The URL argument, the URL of the repository the argument names, or
/// `PACKAGEMANAGER_ADDRESS`.
fn target_url(args: &ArgMatches) -> Result<String, Box<dyn Error>> {
    let url = match args.get_one::<String>("url") {
        Some(u) if !u.contains("://") => repo_url_by_name(u.trim())?,
        Some(u) => u.trim().trim_end_matches('/').to_string(),
        None => match ppm_sso::ppm_address() {
            Some(a) => a,
            None => bail!(
                "No URL given, and PACKAGEMANAGER_ADDRESS is not set. Give the URL or name of a repository, or the URL of a Package Manager instance."
            ),
        },
    };
    if parse_url(&url).is_none() {
        bail!(
            "Invalid URL: '{}', it must start with 'https://' or 'http://'",
            url
        );
    }
    if url_has_password(&url) {
        bail!("The URL must not include a password: {}", redact_url(&url));
    }
    Ok(url)
}

/// The URL of the configured repository `name`, see [`status_repos`].
fn repo_url_by_name(name: &str) -> Result<String, Box<dyn Error>> {
    let repos = status_repos();
    match repos.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
        Some((_, url)) => Ok(url.trim_end_matches('/').to_string()),
        None => {
            let mut names: Vec<&str> = repos.iter().map(|(n, _)| n.as_str()).collect();
            names.sort_by_key(|n| n.to_lowercase());
            names.dedup();
            bail!(
                "No repository called '{}'. Give a URL, or one of: {}",
                name,
                names.join(", ")
            )
        }
    }
}

/// Whether `url` will use a password saved for `user` at `service`: it must
/// include `user`, and `service` must be one of its credential store keys.
fn url_uses_keyring_entry(url: &str, service: &str, user: &str) -> bool {
    parse_url(url).is_some_and(|p| {
        p.username.as_deref() == Some(user)
            && p.auth_domains()
                .iter()
                .any(|d| d == service || strip_userinfo(d) == service)
    })
}

/// The configured repositories (see [`status_repos`]) that will use a
/// password saved for `user` at `service`.
fn repos_using_keyring_entry(service: &str, user: &str) -> Vec<String> {
    status_repos()
        .into_iter()
        .filter(|(_, url)| url_uses_keyring_entry(url, service, user))
        .map(|(name, _)| name)
        .collect()
}

fn read_password(args: &ArgMatches, prompt: &str) -> Result<String, Box<dyn Error>> {
    let pw = match args.get_one::<String>("password").map(|s| s.as_str()) {
        Some("-") => {
            let mut line = String::new();
            std::io::stdin().lock().read_line(&mut line)?;
            line.trim_end_matches(['\n', '\r']).to_string()
        }
        Some(pw) => pw.to_string(),
        None => {
            if !std::io::stdin().is_terminal() {
                bail!("No password given. Use `--password -` to read it from the standard input.");
            }
            rpassword::prompt_password(prompt)?
        }
    };
    if pw.is_empty() {
        bail!("The password must not be empty");
    }
    Ok(pw)
}

fn sc_auth_login(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let url = target_url(args)?;
    let p = parse_url(&url).unwrap();
    let username = args
        .get_one::<String>("username")
        .cloned()
        .or(p.username.clone());

    let sso = match username.as_deref() {
        None => true,
        Some("__token__") => args.get_one::<String>("password").is_none(),
        Some(_) => false,
    };
    if sso {
        // PPM SSO
        let address = p.clean.trim_end_matches('/').to_string();
        let (_token, path) = ppm_sso::login(&address, !args.get_flag("no-browser"))?;
        OUTPUT.success(&format!(
            "Logged in to {}, token saved in {}",
            address,
            path.display()
        ));
        let used = ppm_sso::ppm_address().as_deref() == Some(address.as_str())
            || status_repos()
                .iter()
                .any(|(_, url)| under(&strip_userinfo(url), &address));
        if !used {
            OUTPUT.info(&format!(
                "No configured repository is at {} yet. rig uses the token for \
                     every URL there, e.g. for repositories you add later.",
                address
            ));
        }
    } else {
        // username and password, into the system credential store
        let user = username.unwrap_or_default();
        let service = p.clean.trim_end_matches('/').to_string();
        let password = read_password(args, &format!("Password for {} at {}: ", user, service))?;
        store::set(&service, &user, &password)
            .map_err(|e| format!("Cannot save the password in the credential store: {}", e))?;
        OUTPUT.success(&format!(
            "Saved the password of {} for {} in the credential store ({})",
            user,
            service,
            store::BACKEND
        ));
        if repos_using_keyring_entry(&service, &user).is_empty() {
            let with_user = service.replacen("://", &format!("://{}@", user), 1);
            OUTPUT.info(&format!(
                "No configured repository uses this password yet. rig uses it for \
                     repository URLs at {} that include the username, e.g. {}",
                service, with_user
            ));
        }
    }
    Ok(())
}

fn sc_auth_logout(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let url = target_url(args)?;
    let p = parse_url(&url).unwrap();
    let username = args
        .get_one::<String>("username")
        .cloned()
        .or(p.username.clone());
    let service = p.clean.trim_end_matches('/').to_string();
    match username {
        Some(user) if user != "__token__" => match store::delete(&service, &user) {
            Ok(true) => OUTPUT.success(&format!(
                "Removed the password of {} for {} from the credential store",
                user, service
            )),
            Ok(false) => OUTPUT.info(&format!(
                "No password of {} for {} in the credential store",
                user, service
            )),
            Err(e) => bail!("Cannot access the credential store: {}", e),
        },
        _ => {
            if ppm_sso::remove_token(&service)? {
                OUTPUT.success(&format!("Removed the PPM SSO token of {}", service));
            } else {
                OUTPUT.info(&format!("No PPM SSO token for {}", service));
            }
        }
    }
    Ok(())
}

fn sc_auth_token(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let url = target_url(args)?;
    let url = match args.get_one::<String>("username") {
        Some(u) => {
            let clean = strip_userinfo(&url);
            clean.replacen("://", &format!("://{}@", u), 1)
        }
        None => url,
    };
    let l = lookup_uncached(&url);
    match l.creds {
        Some(c) => {
            println!("{}", c.password);
            Ok(())
        }
        None => {
            let mut msg = format!("No credentials for {}", redact_url(&url));
            for note in l.notes {
                msg.push_str(&format!("\n  {}", note));
            }
            bail!("{}", msg)
        }
    }
}

// ------------------------------------------------------------------------
// status

#[derive(Serialize)]
struct RepoAuthStatus {
    name: String,
    url: String,
    username: Option<String>,
    found: bool,
    source: Option<String>,
    notes: Vec<String>,
}

#[derive(Serialize)]
struct SsoStatus {
    address: String,
    token_file: String,
    token: String,
    issuer: Option<String>,
    subject: Option<String>,
    issued_at: Option<String>,
    expires_at: Option<String>,
    expired: bool,
    valid: Option<bool>,
}

#[derive(Serialize)]
struct AuthStatus {
    netrc: Option<String>,
    repositories: Vec<RepoAuthStatus>,
    ppm_sso: Vec<SsoStatus>,
}

fn mask(token: &str) -> String {
    let n = token.chars().count();
    if n < 12 {
        return "***".to_string();
    }
    let head: String = token.chars().take(3).collect();
    let tail: String = token.chars().skip(n - 4).collect();
    format!("{}...{}", head, tail)
}

fn fmt_time(secs: Option<f64>) -> Option<String> {
    let secs = secs? as i64;
    let ts = jiff::Timestamp::from_second(secs).ok()?;
    Some(ts.strftime("%Y-%m-%d %H:%M:%S UTC").to_string())
}

fn repo_status(name: &str, url: &str) -> RepoAuthStatus {
    let url = register_repo(name, url);
    let l = lookup_uncached(&url);
    RepoAuthStatus {
        name: name.to_string(),
        url,
        username: l.username,
        found: l.creds.is_some(),
        source: l.source.map(|s| s.to_string()),
        notes: l.notes,
    }
}

/// The repositories to report on: the default R installation's, and the ones
/// added with `rig repos add`.
fn status_repos() -> Vec<(String, String)> {
    let mut repos: Vec<(String, String)> = vec![];
    if let Ok(cfg) = crate::repos::configured::configured_repos(None, true, true) {
        for r in cfg.repos {
            repos.push((r.name, r.url));
        }
    }
    if let Ok(custom) = crate::repos::get_custom_repos() {
        for r in custom {
            if !repos.iter().any(|(_, u)| u.trim_end_matches('/') == r.url) {
                repos.push((r.name, r.url));
            }
        }
    }
    // Only HTTP(S) URLs can have credentials. This also drops R's `@CRAN@`
    // placeholder for the CRAN mirror.
    repos.retain(|(_, url)| parse_url(url).is_some());
    repos
}

fn sc_auth_status(
    args: &ArgMatches,
    authargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let connect = args.get_flag("connect");

    let repositories: Vec<RepoAuthStatus> = match args.get_one::<String>("url") {
        Some(name) if !name.contains("://") => {
            vec![repo_status(name, &repo_url_by_name(name)?)]
        }
        Some(url) => {
            let p = parse_url(url).ok_or_else(|| format!("Invalid URL: '{}'", url))?;
            vec![repo_status(&p.host, url)]
        }
        None => status_repos()
            .iter()
            .map(|(name, url)| repo_status(name, url))
            .collect(),
    };

    let token_file = ppm_sso::token_file_path()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let mut ppm_sso: Vec<SsoStatus> = ppm_sso::read_connections()
        .into_iter()
        .filter(|c| match args.get_one::<String>("url") {
            Some(u) => {
                under(&strip_userinfo(u), &c.address) || under(&c.address, &strip_userinfo(u))
            }
            None => true,
        })
        .map(|c| {
            let claims = ppm_sso::jwt_claims(&c.token).unwrap_or_default();
            let valid = if connect {
                ppm_sso::can_authenticate(&c.address, &c.token).ok()
            } else {
                None
            };
            SsoStatus {
                address: c.address.clone(),
                token_file: token_file.clone(),
                token: mask(&c.token),
                issuer: claims.iss,
                subject: claims.sub,
                issued_at: fmt_time(claims.iat),
                expires_at: fmt_time(claims.exp),
                expired: ppm_sso::is_expired(&c.token),
                valid,
            }
        })
        .collect();
    ppm_sso.sort_by(|a, b| a.address.cmp(&b.address));

    let status = AuthStatus {
        netrc: super::netrc::netrc_path()
            .filter(|p| p.exists())
            .map(|p| p.display().to_string()),
        repositories,
        ppm_sso,
    };

    if args.get_flag("json") || authargs.get_flag("json") || mainargs.get_flag("json") {
        println!("{}", serde_json::to_string_pretty(&status)?);
        return Ok(());
    }

    let yesno = |b: bool| if b { "yes" } else { "no" };
    let mut tab = Table::new("{:<}  {:<}  {:<}  {:<}  {:<}");
    tab.add_row(
        Row::new()
            .with_cell("name")
            .with_cell("url")
            .with_cell("user")
            .with_cell("auth")
            .with_cell("source"),
    );
    for r in &status.repositories {
        let source = match (&r.source, r.notes.first()) {
            (Some(s), _) => s.clone(),
            (None, Some(n)) => n.clone(),
            (None, None) => "-".to_string(),
        };
        tab.add_row(
            Row::new()
                .with_cell(&r.name)
                .with_cell(&r.url)
                .with_cell(r.username.as_deref().unwrap_or("-"))
                .with_cell(yesno(r.found))
                .with_cell(source),
        );
    }
    print!("{}", tab);

    if !status.ppm_sso.is_empty() {
        println!();
        println!("PPM SSO tokens ({})", token_file);
        let mut tab = Table::new("{:<}  {:<}  {:<}  {:<}  {:<}");
        tab.add_row(
            Row::new()
                .with_cell("address")
                .with_cell("token")
                .with_cell("subject")
                .with_cell("expires")
                .with_cell(if connect { "valid" } else { "" }),
        );
        for s in &status.ppm_sso {
            let expires = match (&s.expires_at, s.expired) {
                (Some(t), true) => format!("{} (expired)", t),
                (Some(t), false) => t.clone(),
                (None, _) => "-".to_string(),
            };
            let valid = match s.valid {
                Some(v) => yesno(v).to_string(),
                None => "".to_string(),
            };
            tab.add_row(
                Row::new()
                    .with_cell(&s.address)
                    .with_cell(&s.token)
                    .with_cell(s.subject.as_deref().unwrap_or("-"))
                    .with_cell(expires)
                    .with_cell(valid),
            );
        }
        print!("{}", tab);
    }

    if let Some(n) = &status.netrc {
        println!();
        println!("netrc file: {}", n);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_uses_keyring_entry() {
        let url = "https://alice@cran.acme.com/cran";
        assert!(url_uses_keyring_entry(
            url,
            "https://cran.acme.com/cran",
            "alice"
        ));
        assert!(url_uses_keyring_entry(
            url,
            "https://cran.acme.com",
            "alice"
        ));
        assert!(url_uses_keyring_entry(url, "cran.acme.com", "alice"));
        // a deeper URL is not a key
        assert!(!url_uses_keyring_entry(
            url,
            "https://cran.acme.com/cran/src/contrib",
            "alice"
        ));
        // another user, another host, no user in the URL
        assert!(!url_uses_keyring_entry(
            url,
            "https://cran.acme.com/cran",
            "bob"
        ));
        assert!(!url_uses_keyring_entry(
            url,
            "https://other.acme.com",
            "alice"
        ));
        assert!(!url_uses_keyring_entry(
            "https://cran.acme.com/cran",
            "https://cran.acme.com/cran",
            "alice"
        ));
    }
}
