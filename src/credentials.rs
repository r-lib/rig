//! Reading git/GitHub credentials the same way `git` itself would: from the
//! OS credential store (osxkeychain / wincred / manager-core / the plaintext
//! `store` helper, whichever `git config credential.helper` names), via
//! `gix`'s own implementation of the `git credential` helper protocol -- and,
//! taking priority over both the store and the generic `GITHUB_PAT`/
//! `GITHUB_TOKEN` env vars, a host-specific env var computed the same way the
//! R `gitcreds` package's `gitcreds_cache_envvar()` does (also used by
//! `usethis`/`gh`), so a token already set up for those tools is picked up
//! here too. No system `git` binary is invoked.

/// A token for `https://github.com`, for the GitHub REST API and
/// `codeload.github.com` tarball downloads.
///
/// Priority: a host-specific env var (see [`host_env_var_name`]), then
/// `GITHUB_PAT`, then `GITHUB_TOKEN`, then the system git credential store.
/// Returns `None`, not an error, when nothing is configured --
/// unauthenticated access is still valid for public repos.
pub fn github_token() -> Option<String> {
    const GITHUB: &str = "https://github.com";
    env_token(GITHUB).or_else(|| store_token(GITHUB))
}

/// Wire credentials into a `gix` clone, so a `git::` source on a private host
/// authenticates the same way `git clone` would: a host-specific env var
/// (checked first, per URL actually being connected to), then the system git
/// credential store.
#[allow(clippy::result_large_err)]
pub fn configure_gix_clone(prepare: gix::clone::PrepareFetch) -> gix::clone::PrepareFetch {
    prepare.configure_connection(|conn| {
        conn.set_credentials(|action| {
            if let gix::credentials::helper::Action::Get(ctx) = &action {
                if let Some(url) = ctx.url.as_ref() {
                    if let Some(token) = env_token(&url.to_string()) {
                        return Ok(Some(gix::credentials::protocol::Outcome {
                            identity: gix::sec::identity::Account {
                                username: token,
                                password: String::new(),
                            },
                            next: gix::credentials::helper::NextAction::from(ctx.clone()),
                        }));
                    }
                }
            }
            gix::credentials::builtin(action)
        });
        Ok(())
    })
}

/// Env-var-only lookup for `url`: a host-specific var, then (for a
/// github.com/api.github.com URL only) the generic `GITHUB_PAT`/
/// `GITHUB_TOKEN`. Never touches the credential store, so it is cheap enough
/// to call per connection attempt.
fn env_token(url: &str) -> Option<String> {
    if let Some(name) = host_env_var_name(url) {
        if let Ok(tok) = std::env::var(&name) {
            if !tok.is_empty() {
                return Some(tok);
            }
        }
    }
    if is_github_host(url) {
        if let Ok(tok) = std::env::var("GITHUB_PAT") {
            if !tok.is_empty() {
                return Some(tok);
            }
        }
        if let Ok(tok) = std::env::var("GITHUB_TOKEN") {
            if !tok.is_empty() {
                return Some(tok);
            }
        }
    }
    None
}

fn is_github_host(url: &str) -> bool {
    parse_url(url)
        .and_then(|u| u.host().map(|h| h.to_ascii_lowercase()))
        .is_some_and(|h| h == "github.com" || h == "api.github.com")
}

fn store_token(url: &str) -> Option<String> {
    let action = gix::credentials::helper::Action::get_for_url(url);
    let outcome = gix::credentials::builtin(action).ok()??;
    let identity = outcome.identity;
    if !identity.password.is_empty() {
        Some(identity.password)
    } else if !identity.username.is_empty() {
        Some(identity.username)
    } else {
        None
    }
}

fn parse_url(url: &str) -> Option<gix::Url> {
    gix::url::parse(gix::bstr::BStr::new(url.as_bytes())).ok()
}

/// The env var name for `url`, matching the R `gitcreds` package's
/// `gitcreds_cache_envvar()` exactly (down to the `GITHUB_PAT_` prefix,
/// which it keeps for any host, not only GitHub, for compatibility with
/// established `usethis`/`gh` convention). E.g. `https://github.com` ->
/// `GITHUB_PAT_GITHUB_COM`, `https://alice@example.com` ->
/// `GITHUB_PAT_ALICE_AT_EXAMPLE_COM`.
fn host_env_var_name(url: &str) -> Option<String> {
    let parsed = parse_url(url)?;
    let host = parsed.host()?;

    let proto_raw = format!("{}_", parsed.scheme.as_str());
    let proto = if proto_raw == "http_" || proto_raw == "https_" {
        String::new()
    } else {
        proto_raw
    };

    let user_part = match parsed.user() {
        Some(user) if !user.is_empty() => format!("{}_AT_", user),
        _ => String::new(),
    };

    let host0 = if host.eq_ignore_ascii_case("api.github.com") {
        "github.com"
    } else {
        host
    };
    let host1 = collapse_runs(host0, |c| c == '.' || c == ':', '_');
    let host_final: String = host1
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                'x'
            }
        })
        .collect();

    let slug1 = format!("{}{}{}", proto, user_part, host_final);
    let slug2 = if slug1.starts_with("AT_") {
        format!("AT_{}", slug1)
    } else {
        slug1
    };
    let slug3 = if slug2.starts_with(|c: char| c.is_ascii_digit()) {
        format!("AT_{}", slug2)
    } else {
        slug2
    };

    Some(format!("GITHUB_PAT_{}", slug3.to_uppercase()))
}

/// Replace every maximal run of a char matching `is_match` with a single
/// `replacement`, the same as R's `gsub("[...]+", replacement, x)`.
fn collapse_runs(s: &str, is_match: impl Fn(char) -> bool, replacement: char) -> String {
    let mut out = String::new();
    let mut in_run = false;
    for c in s.chars() {
        if is_match(c) {
            if !in_run {
                out.push(replacement);
                in_run = true;
            }
        } else {
            out.push(c);
            in_run = false;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_env_var_name_matches_gitcreds() {
        assert_eq!(
            host_env_var_name("https://github.com").as_deref(),
            Some("GITHUB_PAT_GITHUB_COM")
        );
        assert_eq!(
            host_env_var_name("https://api.github.com").as_deref(),
            Some("GITHUB_PAT_GITHUB_COM")
        );
        assert_eq!(
            host_env_var_name("https://gitlab.com/foo/bar.git").as_deref(),
            Some("GITHUB_PAT_GITLAB_COM")
        );
        assert_eq!(
            host_env_var_name("https://alice@gitlab.example.com/x").as_deref(),
            Some("GITHUB_PAT_ALICE_AT_GITLAB_EXAMPLE_COM")
        );
        assert_eq!(
            host_env_var_name("git://example.com/x.git").as_deref(),
            Some("GITHUB_PAT_GIT_EXAMPLE_COM")
        );
    }

    /// `GITHUB_PAT`/`GITHUB_TOKEN`/host-specific env vars are process-global,
    /// so every case lives in one test rather than racing a parallel test
    /// that also sets them (see the similar note in `pkgsource::git::tests`).
    #[test]
    fn env_vars_take_priority_in_order() {
        let host_var = "GITHUB_PAT_GITHUB_COM";
        std::env::remove_var(host_var);
        std::env::remove_var("GITHUB_PAT");
        std::env::remove_var("GITHUB_TOKEN");

        // Not asserting `env_token` is `None` here: unlike the env vars below,
        // it is not reset, and the machine running this test may have a real
        // credential store entry for github.com.

        std::env::set_var("GITHUB_TOKEN", "from-token-var");
        assert_eq!(github_token().as_deref(), Some("from-token-var"));

        std::env::set_var("GITHUB_PAT", "from-pat-var");
        assert_eq!(github_token().as_deref(), Some("from-pat-var"));

        std::env::set_var(host_var, "from-host-specific-var");
        assert_eq!(github_token().as_deref(), Some("from-host-specific-var"));

        std::env::remove_var(host_var);
        std::env::remove_var("GITHUB_PAT");
        std::env::remove_var("GITHUB_TOKEN");
    }
}
