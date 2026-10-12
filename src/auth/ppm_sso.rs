//! Posit Package Manager single sign-on (SSO), compatible with pak.
//!
//! The PPM token is obtained with an OAuth device flow, and stored in the
//! same file pak (pkgcache) and Posit's own tools use, `~/.ppm/tokens.toml`:
//!
//! ```toml
//! [[connections]]
//! address = "https://ppm.example.com"
//! token = "eyJ..."
//! auth_type = "sso"
//! ```

use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use log::*;
use owo_colors::OwoColorize;
use serde::Deserialize;

use crate::output::OUTPUT;
use crate::utils::http_client;

/// The PPM instance named by `PACKAGEMANAGER_ADDRESS`, without a trailing `/`.
pub fn ppm_address() -> Option<String> {
    let addr = std::env::var("PACKAGEMANAGER_ADDRESS").ok()?;
    let addr = addr.trim().trim_end_matches('/');
    if addr.is_empty() {
        None
    } else {
        Some(addr.to_string())
    }
}

/// `PACKAGEMANAGER_SSO_TOKEN_FILE`, or `~/.ppm/tokens.toml`.
pub fn token_file_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("PACKAGEMANAGER_SSO_TOKEN_FILE") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    Some(super::home_dir()?.join(".ppm").join("tokens.toml"))
}

#[derive(Debug, Clone, Deserialize)]
pub struct Connection {
    pub address: String,
    pub token: String,
}

#[derive(Debug, Deserialize)]
struct TokenFile {
    #[serde(default)]
    connections: Vec<Connection>,
}

/// All connections of the token file. A missing or unreadable file has none.
pub fn read_connections() -> Vec<Connection> {
    match token_file_path() {
        Some(path) => read_connections_at(&path),
        None => vec![],
    }
}

fn read_connections_at(path: &Path) -> Vec<Connection> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return vec![];
    };
    match toml::from_str::<TokenFile>(&text) {
        Ok(f) => f.connections,
        Err(e) => {
            warn!("Cannot parse PPM token file {}: {}", path.display(), e);
            vec![]
        }
    }
}

/// The stored token for the PPM instance at `address`.
pub fn stored_token(address: &str) -> Option<String> {
    let address = address.trim_end_matches('/');
    read_connections()
        .into_iter()
        .find(|c| c.address.trim_end_matches('/') == address)
        .map(|c| c.token)
}

/// Add or update the token of `address` in the token file, keeping
/// everything else in it. The file is only readable by the user.
pub fn write_token(address: &str, token: &str) -> Result<PathBuf, Box<dyn Error>> {
    let path = token_file_path().ok_or("Cannot determine the PPM token file path")?;
    write_token_at(&path, address, token)?;
    Ok(path)
}

fn write_token_at(path: &Path, address: &str, token: &str) -> Result<(), Box<dyn Error>> {
    use toml_edit::{value, ArrayOfTables, DocumentMut, Item, Table};
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|e| format!("Cannot parse PPM token file {}: {}", path.display(), e))?;

    if !doc.contains_key("connections") {
        doc.insert("connections", Item::ArrayOfTables(ArrayOfTables::new()));
    }
    let conns = doc["connections"]
        .as_array_of_tables_mut()
        .ok_or_else(|| format!("Invalid PPM token file {}", path.display()))?;
    let address = address.trim_end_matches('/');
    let existing = conns.iter_mut().find(|t| {
        t.get("address")
            .and_then(|a| a.as_str())
            .is_some_and(|a| a.trim_end_matches('/') == address)
    });
    match existing {
        Some(t) => {
            t["token"] = value(token);
        }
        None => {
            let mut t = Table::new();
            t["address"] = value(address);
            t["token"] = value(token);
            t["auth_type"] = value("sso");
            conns.push(t);
        }
    }

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    write_private(path, doc.to_string().as_bytes())
}

/// Remove `address` from the token file. `Ok(false)` if it was not there.
pub fn remove_token(address: &str) -> Result<bool, Box<dyn Error>> {
    match token_file_path() {
        Some(path) => remove_token_at(&path, address),
        None => Ok(false),
    }
}

fn remove_token_at(path: &Path, address: &str) -> Result<bool, Box<dyn Error>> {
    use toml_edit::DocumentMut;
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(false);
    };
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|e| format!("Cannot parse PPM token file {}: {}", path.display(), e))?;
    let address = address.trim_end_matches('/');
    let Some(conns) = doc
        .get_mut("connections")
        .and_then(|c| c.as_array_of_tables_mut())
    else {
        return Ok(false);
    };
    let before = conns.len();
    conns.retain(|t| {
        !t.get("address")
            .and_then(|a| a.as_str())
            .is_some_and(|a| a.trim_end_matches('/') == address)
    });
    if conns.len() == before {
        return Ok(false);
    }
    write_private(path, doc.to_string().as_bytes())?;
    Ok(true)
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), Box<dyn Error>> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        // `mode()` only applies to new files
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        f.write_all(bytes)?;
    }
    #[cfg(not(unix))]
    std::fs::write(path, bytes)?;
    Ok(())
}

// ------------------------------------------------------------------------
// JWT

/// The interesting claims of a JWT. The signature is not checked, this is only
/// to tell the user about the token, and to skip expired ones.
#[derive(Debug, Default, Clone, Deserialize)]
pub struct JwtClaims {
    pub iss: Option<String>,
    pub sub: Option<String>,
    pub iat: Option<f64>,
    pub exp: Option<f64>,
}

pub fn jwt_claims(token: &str) -> Option<JwtClaims> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// A token without an `exp` claim, or one that is not a JWT, never expires as
/// far as rig can tell.
pub fn is_expired(token: &str) -> bool {
    match jwt_claims(token).and_then(|c| c.exp) {
        Some(exp) => exp <= now_secs(),
        None => false,
    }
}

// ------------------------------------------------------------------------
// HTTP

fn form_encode(pairs: &[(&str, &str)]) -> String {
    fn enc(s: &str) -> String {
        let mut out = String::new();
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    out.push(b as char)
                }
                _ => out.push_str(&format!("%{:02X}", b)),
            }
        }
        out
    }
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", enc(k), enc(v)))
        .collect::<Vec<_>>()
        .join("&")
}

#[tokio::main]
async fn post_form(
    url: &str,
    pairs: &[(&str, &str)],
) -> Result<(u16, serde_json::Value), Box<dyn Error>> {
    let resp = http_client()
        .post(url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form_encode(pairs))
        .send()
        .await?;
    let status = resp.status().as_u16();
    let bytes = resp.bytes().await?;
    let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    Ok((status, body))
}

/// Whether `token` is accepted by the PPM instance at `address`.
#[tokio::main]
pub async fn can_authenticate(address: &str, token: &str) -> Result<bool, Box<dyn Error>> {
    let resp = http_client()
        .get(address)
        .basic_auth("__token__", Some(token))
        .send()
        .await?;
    let status = resp.status().as_u16();
    Ok(status < 500 && status != 401 && status != 403)
}

/// Exchange an identity (OIDC id) token for a PPM token.
pub fn identity_to_ppm_token(
    address: &str,
    identity_token: &str,
) -> Result<String, Box<dyn Error>> {
    let url = format!("{}/__api__/token", address);
    let (status, body) = post_form(
        &url,
        &[
            (
                "grant_type",
                "urn:ietf:params:oauth:grant-type:token-exchange",
            ),
            ("subject_token", identity_token),
            (
                "subject_token_type",
                "urn:ietf:params:oauth:token-type:id_token",
            ),
        ],
    )?;
    if status >= 400 {
        bail!(
            "Failed to exchange identity token for a PPM token at {} (HTTP {})",
            address,
            status
        );
    }
    match body.get("access_token").and_then(|t| t.as_str()) {
        Some(t) => Ok(t.to_string()),
        None => bail!(
            "Failed to exchange identity token for a PPM token at {}: no access token in response",
            address
        ),
    }
}

/// The identity token in `PACKAGEMANAGER_IDENTITY_TOKEN_FILE`, if set. This is
/// how CI systems with OIDC identity tokens log in, without a browser.
pub fn identity_token_from_file() -> Option<String> {
    let path = std::env::var("PACKAGEMANAGER_IDENTITY_TOKEN_FILE").ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    let token = text.lines().next()?.trim().to_string();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

fn base64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn pkce_pair() -> (String, String) {
    use rand::RngCore;
    use sha2::{Digest, Sha256};
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let verifier = base64url(&bytes);
    let challenge = base64url(&Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let res = std::process::Command::new("open").arg(url).status();
    #[cfg(target_os = "linux")]
    let res = std::process::Command::new("xdg-open").arg(url).status();
    #[cfg(target_os = "windows")]
    let res = std::process::Command::new("cmd.exe")
        .args(["/c", "start", "", url])
        .status();
    if let Err(e) = res {
        debug!("Cannot open browser: {}", e);
    }
}

/// Log in to the PPM instance at `address` with the device flow, and return
/// an identity token.
fn device_flow(address: &str, browser: bool) -> Result<String, Box<dyn Error>> {
    let (verifier, challenge) = pkce_pair();
    let (status, init) = post_form(
        &format!("{}/__api__/device", address),
        &[
            ("code_challenge_method", "S256"),
            ("code_challenge", &challenge),
        ],
    )?;
    if status >= 400 {
        bail!(
            "Failed to start device authorization at {} (HTTP {}). Is SSO enabled on this Package Manager instance?",
            address,
            status
        );
    }
    let s = |k: &str| init.get(k).and_then(|v| v.as_str()).map(|v| v.to_string());
    let display_uri = s("verification_uri_complete")
        .or_else(|| s("verification_uri"))
        .ok_or("No verification URI in the device authorization response")?;
    let user_code = s("user_code").unwrap_or_default();
    let device_code =
        s("device_code").ok_or("No device code in the device authorization response")?;
    let interval = init.get("interval").and_then(|v| v.as_u64()).unwrap_or(5);
    let expires_in = init
        .get("expires_in")
        .and_then(|v| v.as_u64())
        .unwrap_or(300);

    OUTPUT.info(&format!("Log in at {}", display_uri.bold()));
    if !user_code.is_empty() {
        OUTPUT.info(&format!(
            "and enter the code {} when prompted.",
            user_code.bold().magenta()
        ));
    }
    if browser {
        open_browser(&display_uri);
    }
    OUTPUT.info("Waiting for the browser login to finish...");

    let start = Instant::now();
    let mut interval = Duration::from_secs(interval.max(1));
    while start.elapsed() < Duration::from_secs(expires_in) {
        let (status, body) = post_form(
            &format!("{}/__api__/device_access", address),
            &[("device_code", &device_code), ("code_verifier", &verifier)],
        )?;
        match status {
            200 => match body.get("id_token").and_then(|v| v.as_str()) {
                Some(t) => return Ok(t.to_string()),
                None => bail!("No identity token in the device authorization response"),
            },
            400 => match body.get("error").and_then(|v| v.as_str()) {
                Some("access_denied") => bail!("Login denied"),
                Some("expired_token") => bail!("Device authorization request expired"),
                Some("slow_down") => interval += Duration::from_secs(5),
                _ => {} // authorization_pending
            },
            s => bail!("Device authorization failed (HTTP {})", s),
        }
        std::thread::sleep(interval);
    }
    bail!("Device authorization timed out")
}

/// Log in to the PPM instance at `address`, store the PPM token in the token
/// file and return it. Uses `PACKAGEMANAGER_IDENTITY_TOKEN_FILE` if set,
/// otherwise the browser based device flow.
pub fn login(address: &str, browser: bool) -> Result<(String, PathBuf), Box<dyn Error>> {
    let identity = match identity_token_from_file() {
        Some(t) => t,
        None => device_flow(address, browser)?,
    };
    let token = identity_to_ppm_token(address, &identity)?;
    let path = write_token(address, &token)?;
    Ok((token, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jwt(payload: &str) -> String {
        format!(
            "{}.{}.sig",
            base64url(br#"{"alg":"HS256","typ":"JWT"}"#),
            base64url(payload.as_bytes())
        )
    }

    #[test]
    fn test_jwt_expiry() {
        let past = jwt(r#"{"iss":"https://idp","sub":"me","exp":1000}"#);
        assert!(is_expired(&past));
        let claims = jwt_claims(&past).unwrap();
        assert_eq!(claims.iss.as_deref(), Some("https://idp"));
        let future = jwt(&format!(r#"{{"exp":{}}}"#, now_secs() + 3600.0));
        assert!(!is_expired(&future));
        assert!(!is_expired("not-a-jwt"));
    }

    #[test]
    fn test_form_encode() {
        assert_eq!(
            form_encode(&[("a", "x y"), ("b", "c:d/e")]),
            "a=x%20y&b=c%3Ad%2Fe"
        );
    }

    #[test]
    fn test_token_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("tokens.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "# keep me\n[[connections]]\naddress = \"https://other.example.com\"\ntoken = \"t0\"\nauth_type = \"sso\"\n",
        )
        .unwrap();
        write_token_at(&path, "https://ppm.example.com/", "t1").unwrap();
        write_token_at(&path, "https://ppm.example.com", "t2").unwrap();
        let conns = read_connections_at(&path);
        assert_eq!(conns.len(), 2);
        assert_eq!(conns[0].token, "t0");
        assert_eq!(conns[1].address, "https://ppm.example.com");
        assert_eq!(conns[1].token, "t2");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# keep me"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert!(remove_token_at(&path, "https://ppm.example.com").unwrap());
        assert!(!remove_token_at(&path, "https://ppm.example.com").unwrap());
        assert_eq!(read_connections_at(&path).len(), 1);

        // a new file
        let path2 = dir.path().join("new.toml");
        write_token_at(&path2, "https://ppm.example.com", "t3").unwrap();
        assert_eq!(read_connections_at(&path2)[0].token, "t3");
    }

    #[test]
    fn test_device_flow() {
        use wiremock::matchers::{body_string_contains, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let rt = tokio::runtime::Runtime::new().unwrap();
        let server = rt.block_on(MockServer::start());
        rt.block_on(async {
            Mock::given(method("POST"))
                .and(path("/__api__/device"))
                .and(body_string_contains("code_challenge_method=S256"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "device_code": "dev123",
                    "user_code": "ABCD-EFGH",
                    "verification_uri": "https://idp.example.com/activate",
                    "interval": 1,
                    "expires_in": 30
                })))
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/__api__/device_access"))
                .and(body_string_contains("device_code=dev123"))
                .respond_with(
                    ResponseTemplate::new(400)
                        .set_body_json(serde_json::json!({"error": "authorization_pending"})),
                )
                .up_to_n_times(1)
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/__api__/device_access"))
                .and(body_string_contains("code_verifier="))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(serde_json::json!({"id_token": "idtok"})),
                )
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/__api__/token"))
                .and(body_string_contains("subject_token=idtok"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(serde_json::json!({"access_token": "ppmtok"})),
                )
                .mount(&server)
                .await;
        });
        let address = server.uri();
        // run outside of the runtime, as in rig
        let identity = std::thread::spawn({
            let address = address.clone();
            move || device_flow(&address, false).map_err(|e| e.to_string())
        })
        .join()
        .unwrap()
        .unwrap();
        assert_eq!(identity, "idtok");
        let token = std::thread::spawn(move || {
            identity_to_ppm_token(&address, &identity).map_err(|e| e.to_string())
        })
        .join()
        .unwrap()
        .unwrap();
        assert_eq!(token, "ppmtok");
    }

    #[test]
    fn test_pkce() {
        let (v, c) = pkce_pair();
        assert_eq!(v.len(), 43);
        assert_eq!(c.len(), 43);
        assert_ne!(v, c);
    }
}
