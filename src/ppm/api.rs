//! A small client for the Posit Package Manager API, `<ppm>/__api__/...`.
//!
//! Every request goes through [`crate::auth::api_request`], so a PPM token
//! (from `PACKAGEMANAGER_TOKEN`, `rig auth login`, or an identity token) is
//! sent as a bearer token. Anonymous requests work for the public endpoints.
//!
//! The async functions take the base URL explicitly, so that tests can point
//! them at a mock server. The sync wrappers use [`ppm_url`].

use std::error::Error;
use std::fmt;

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::repos::binaries::ppm_url;
use crate::utils::http_client;

/// A query string, as name-value pairs. A name can repeat.
pub type Query = Vec<(String, String)>;

/// Build a query from pairs of string-ish values.
pub fn query<K: ToString, V: ToString>(pairs: &[(K, V)]) -> Query {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// An error that PPM reported, e.g. `{"code":4,"error":"repository with
/// name 'nope' does not exist","payload":null}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub status: u16,
    /// PPM's own error code, if the body had one.
    pub code: Option<i64>,
    pub message: String,
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Package Manager error (HTTP {}): {}",
            self.status, self.message
        )?;
        if let Ok(st) = reqwest::StatusCode::from_u16(self.status) {
            write!(f, "{}", crate::auth::status_hint(st))?;
        }
        Ok(())
    }
}

impl Error for ApiError {}

/// The URL of an API endpoint. `segments` are the path segments after
/// `__api__`, they are percent-encoded here, so a package or repository name
/// can never change the path.
pub fn api_url(base: &str, segments: &[&str], q: &Query) -> Result<reqwest::Url, Box<dyn Error>> {
    let mut url = reqwest::Url::parse(&format!("{}/__api__", base.trim_end_matches('/')))?;
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| format!("Invalid Package Manager URL: {}", base))?;
        for s in segments {
            path.push(s);
        }
    }
    if !q.is_empty() {
        let mut qp = url.query_pairs_mut();
        for (k, v) in q {
            qp.append_pair(k, v);
        }
    }
    Ok(url)
}

/// Turn an error response into an [`ApiError`].
fn api_error(status: u16, body: &[u8]) -> ApiError {
    let parsed: Option<Value> = serde_json::from_slice(body).ok();
    let code = parsed
        .as_ref()
        .and_then(|v| v.get("code"))
        .and_then(|c| c.as_i64());
    let message = parsed
        .as_ref()
        .and_then(|v| v.get("error"))
        .and_then(|e| e.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            let text = String::from_utf8_lossy(body).trim().to_string();
            if text.is_empty() {
                reqwest::StatusCode::from_u16(status)
                    .ok()
                    .and_then(|s| s.canonical_reason())
                    .unwrap_or("unknown error")
                    .to_string()
            } else {
                text.chars().take(300).collect()
            }
        });
    ApiError {
        status,
        code,
        message,
    }
}

/// Send a request and return the body, or an [`ApiError`] for a non-2xx
/// status.
pub async fn send(
    req: reqwest::RequestBuilder,
) -> Result<(reqwest::header::HeaderMap, Vec<u8>), Box<dyn Error>> {
    let resp = req.send().await?;
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = resp.bytes().await?.to_vec();
    if !status.is_success() {
        return Err(Box::new(api_error(status.as_u16(), &body)));
    }
    Ok((headers, body))
}

/// `GET` an endpoint and parse the JSON response.
pub async fn get_at<T: DeserializeOwned>(
    base: &str,
    segments: &[&str],
    q: &Query,
) -> Result<T, Box<dyn Error>> {
    let url = api_url(base, segments, q)?;
    let req = crate::auth::api_request(&http_client(), reqwest::Method::GET, url.as_str());
    let (_, body) = send(req).await?;
    serde_json::from_slice(&body)
        .map_err(|e| format!("Cannot parse Package Manager response from {}: {}", url, e).into())
}

/// `GET` a paged endpoint (`_page`, `_limit`), until `limit` items, or
/// until there are no more. A page that is shorter than the page size is the
/// last one.
pub async fn get_paged_at<T: DeserializeOwned>(
    base: &str,
    segments: &[&str],
    q: &Query,
    limit: usize,
) -> Result<Vec<T>, Box<dyn Error>> {
    // PPM caps the page size, and the page size has to be the same for every
    // page, as `_page` counts pages of that size.
    let page_size = limit.clamp(1, PAGE_SIZE);
    let mut out: Vec<T> = Vec::new();
    let mut page = 1;
    while out.len() < limit {
        let mut q = q.clone();
        q.push(("_page".to_string(), page.to_string()));
        q.push(("_limit".to_string(), page_size.to_string()));
        let items: Vec<T> = get_at(base, segments, &q).await?;
        let n = items.len();
        out.extend(items);
        if n < page_size {
            break;
        }
        page += 1;
    }
    out.truncate(limit);
    Ok(out)
}

/// The largest page rig asks for.
const PAGE_SIZE: usize = 100;

#[tokio::main]
async fn get_<T: DeserializeOwned>(
    base: &str,
    segments: &[&str],
    q: &Query,
) -> Result<T, Box<dyn Error>> {
    get_at(base, segments, q).await
}

#[tokio::main]
async fn get_paged_<T: DeserializeOwned>(
    base: &str,
    segments: &[&str],
    q: &Query,
    limit: usize,
) -> Result<Vec<T>, Box<dyn Error>> {
    get_paged_at(base, segments, q, limit).await
}

/// `GET <ppm>/__api__/<segments>?<q>`, parsed.
pub fn get<T: DeserializeOwned>(segments: &[&str], q: &Query) -> Result<T, Box<dyn Error>> {
    get_(&ppm_url(), segments, q)
}

/// A paged `GET`, see [`get_paged_at`].
pub fn get_paged<T: DeserializeOwned>(
    segments: &[&str],
    q: &Query,
    limit: usize,
) -> Result<Vec<T>, Box<dyn Error>> {
    get_paged_(&ppm_url(), segments, q, limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn test_api_url() {
        let u = api_url(
            "https://ppm.example.com/",
            &["repos", "cran", "packages"],
            &vec![],
        )
        .unwrap();
        assert_eq!(
            u.as_str(),
            "https://ppm.example.com/__api__/repos/cran/packages"
        );
        let u = api_url(
            "https://ppm.example.com/sub",
            &["repos", "my repo/x", "packages"],
            &query(&[("name_like", "a&b"), ("names", "x"), ("names", "y")]),
        )
        .unwrap();
        assert_eq!(
            u.as_str(),
            "https://ppm.example.com/sub/__api__/repos/my%20repo%2Fx/packages?name_like=a%26b&names=x&names=y"
        );
    }

    #[test]
    fn test_api_error() {
        let e = api_error(
            400,
            br#"{"code":2,"error":"Usage statistics are disabled","payload":null}"#,
        );
        assert_eq!(e.code, Some(2));
        assert_eq!(e.message, "Usage statistics are disabled");
        let e = api_error(502, b"");
        assert_eq!(e.code, None);
        assert_eq!(e.message, "Bad Gateway");
        let e = api_error(403, b"<html>nope</html>");
        assert_eq!(e.message, "<html>nope</html>");
        assert!(e.to_string().contains("rig auth status"));
    }

    #[test]
    fn test_paging() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let server = rt.block_on(MockServer::start());
        rt.block_on(async {
            for (page, items) in [("1", vec![1, 2, 3]), ("2", vec![4, 5, 6]), ("3", vec![7])] {
                Mock::given(method("GET"))
                    .and(path("/__api__/repos/cran/packages"))
                    .and(query_param("_page", page))
                    .and(query_param("_limit", "3"))
                    .respond_with(ResponseTemplate::new(200).set_body_json(items))
                    .mount(&server)
                    .await;
            }
            Mock::given(method("GET"))
                .and(path("/__api__/repos/nope/packages"))
                .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
                    "code": 4, "error": "repository with name 'nope' does not exist", "payload": null
                })))
                .mount(&server)
                .await;
        });
        let base = server.uri();
        let q: Query = vec![];
        let all: Vec<i32> = rt
            .block_on(get_paged_at(
                &base,
                &["repos", "cran", "packages"],
                &q,
                3 * 10,
            ))
            .unwrap_or_default();
        // limit 30 asks for pages of 30, which the mock does not answer
        assert!(all.is_empty());
        let r: Vec<i32> = rt
            .block_on(get_paged_at(&base, &["repos", "cran", "packages"], &q, 3))
            .unwrap();
        assert_eq!(r, vec![1, 2, 3]);
        let err = rt
            .block_on(get_paged_at::<i32>(
                &base,
                &["repos", "nope", "packages"],
                &q,
                3,
            ))
            .unwrap_err();
        let ae = err.downcast_ref::<ApiError>().unwrap();
        assert_eq!((ae.status, ae.code), (404, Some(4)));
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn test_paging_more_pages() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let server = rt.block_on(MockServer::start());
        rt.block_on(async {
            for (page, items) in [
                ("1", (0..100).collect::<Vec<i32>>()),
                ("2", (100..150).collect()),
            ] {
                Mock::given(method("GET"))
                    .and(path("/__api__/x"))
                    .and(query_param("_page", page))
                    .and(query_param("_limit", "100"))
                    .respond_with(ResponseTemplate::new(200).set_body_json(items))
                    .mount(&server)
                    .await;
            }
        });
        let q: Query = vec![];
        let r: Vec<i32> = rt
            .block_on(get_paged_at(&server.uri(), &["x"], &q, 120))
            .unwrap();
        assert_eq!(r.len(), 120);
        assert_eq!(r[119], 119);
        let r: Vec<i32> = rt
            .block_on(get_paged_at(&server.uri(), &["x"], &q, 1000))
            .unwrap();
        assert_eq!(r.len(), 150);
    }
}
