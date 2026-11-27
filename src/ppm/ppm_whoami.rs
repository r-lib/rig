//! `rig ppm whoami`: who Package Manager thinks you are, and the token rig
//! sends to its API.

use std::error::Error;

use clap::ArgMatches;
use serde::Serialize;
use serde_json::Value;

use crate::auth::cmd::{fmt_time, mask};
use crate::auth::ppm_sso::{is_expired, jwt_claims};
use crate::auth::{lookup_uncached, Credentials};
use crate::ppm::api::{self, api_url, ApiError};
use crate::ppm::want_json;
use crate::repos::binaries::ppm_url;
use crate::textfmt::print_field;

#[derive(Debug, Serialize)]
struct WhoAmI {
    url: String,
    /// What `/__api__/user` says. `{}` for an anonymous user.
    user: Value,
    token: Option<TokenInfo>,
    /// Whether `/__api__/verify-auth` accepted the token. `None` if there is
    /// no token, or the server does not check API tokens.
    token_valid: Option<bool>,
    /// What the server said about the token, if it did not accept it.
    token_error: Option<String>,
}

#[derive(Debug, Serialize)]
struct TokenInfo {
    source: String,
    token: String,
    subject: Option<String>,
    issuer: Option<String>,
    scopes: Vec<String>,
    issued_at: Option<String>,
    expires_at: Option<String>,
    expired: bool,
}

/// Scopes of a PPM token, from its `scope` or `scopes` claim, which can be a
/// space-separated string or a list.
fn token_scopes(token: &str) -> Vec<String> {
    let Some(payload) = token.split('.').nth(1) else {
        return vec![];
    };
    use base64::Engine;
    let Ok(bytes) =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('='))
    else {
        return vec![];
    };
    let Ok(claims) = serde_json::from_slice::<Value>(&bytes) else {
        return vec![];
    };
    let mut out = vec![];
    for key in ["scope", "scopes", "scp"] {
        match claims.get(key) {
            Some(Value::String(s)) => out.extend(s.split_whitespace().map(|s| s.to_string())),
            Some(Value::Array(a)) => {
                out.extend(a.iter().filter_map(|v| v.as_str()).map(|s| s.to_string()))
            }
            _ => {}
        }
    }
    out
}

fn token_info(creds: &Credentials, source: String) -> TokenInfo {
    let claims = jwt_claims(&creds.password).unwrap_or_default();
    TokenInfo {
        source,
        token: mask(&creds.password),
        subject: claims.sub,
        issuer: claims.iss,
        scopes: token_scopes(&creds.password),
        issued_at: fmt_time(claims.iat),
        expires_at: fmt_time(claims.exp),
        expired: is_expired(&creds.password),
    }
}

pub fn sc_ppm_whoami(
    args: &ArgMatches,
    ppmargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let base = ppm_url();
    let user: Value = api::get(&["user"], &vec![])?;

    let lookup = lookup_uncached(api_url(&base, &["user"], &vec![])?.as_str());
    let token = match (&lookup.creds, &lookup.source) {
        (Some(c), Some(src)) if c.username == "__token__" => Some(token_info(c, src.to_string())),
        _ => None,
    };
    let (token_valid, token_error) = if token.is_some() {
        match api::get::<Value>(&["verify-auth"], &vec![]) {
            Ok(_) => (Some(true), None),
            Err(e) => match e.downcast_ref::<ApiError>() {
                // 11: API token authentication is off, nothing to verify
                Some(ae) if ae.code == Some(11) => (None, Some(ae.message.clone())),
                Some(ae) => (Some(false), Some(ae.message.clone())),
                None => return Err(e),
            },
        }
    } else {
        (None, None)
    };

    let res = WhoAmI {
        url: base,
        user,
        token,
        token_valid,
        token_error,
    };
    if want_json(args, ppmargs, mainargs) {
        println!("{}", serde_json::to_string_pretty(&res)?);
        return Ok(());
    }

    let color = crate::ppm::use_color();
    const W: usize = 14;
    print_field("server", &res.url, W, color);
    // The fields of the user, to skip the same fields of the token below.
    let mut printed: Vec<(String, String)> = vec![];
    match res.user.as_object() {
        Some(o) if !o.is_empty() => {
            for (k, v) in o {
                let v = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                print_field(k, &v, W, color);
                printed.push((k.clone(), v));
            }
        }
        _ => print_field("user", "anonymous", W, color),
    }
    let new_field = |k: &str, v: &str| !printed.iter().any(|(pk, pv)| pk == k && pv == v);
    match &res.token {
        None => print_field("token", "none (see `rig auth status`)", W, color),
        Some(t) => {
            print_field(
                "token",
                &format!("{} (from {})", t.token, t.source),
                W,
                color,
            );
            if let Some(s) = t.subject.as_deref().filter(|s| new_field("subject", s)) {
                print_field("subject", s, W, color);
            }
            if let Some(s) = t.issuer.as_deref().filter(|s| new_field("issuer", s)) {
                print_field("issuer", s, W, color);
            }
            if !t.scopes.is_empty() {
                print_field("scopes", &t.scopes.join(", "), W, color);
            }
            if let Some(s) = &t.expires_at {
                let s = if t.expired {
                    format!("{} (expired)", s)
                } else {
                    s.clone()
                };
                print_field("expires", &s, W, color);
            }
            let valid = match (res.token_valid, &res.token_error) {
                (Some(true), _) => "yes".to_string(),
                (Some(false), Some(e)) => format!("no: {}", e),
                (None, Some(e)) => format!("not checked: {}", e),
                _ => "-".to_string(),
            };
            print_field("valid", &valid, W, color);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn jwt(payload: &str) -> String {
        let enc = |b: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b);
        format!(
            "{}.{}.sig",
            enc(br#"{"alg":"HS256"}"#),
            enc(payload.as_bytes())
        )
    }

    #[test]
    fn test_token_scopes() {
        assert_eq!(
            token_scopes(&jwt(r#"{"scope":"repos:read sources:write"}"#)),
            vec!["repos:read", "sources:write"]
        );
        assert_eq!(
            token_scopes(&jwt(r#"{"scopes":["global:admin"]}"#)),
            vec!["global:admin"]
        );
        assert!(token_scopes("not-a-jwt").is_empty());
    }
}
