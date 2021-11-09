
use futures::future;
use regex::Regex;
use lazy_static::lazy_static;
use semver::Version;

const API_URI: &str = "https://api.r-hub.io/rversions/";
const MACOS_DEVEL_URI: &str =
    "https://mac.r-project.org/high-sierra/R-devel/R-devel.pkg";
const WIN_DEVEL_URI: &str =
    "https://cran.r-project.org/bin/windows/base/R-devel-win.exe";

lazy_static! {
    static ref RE_OLDREL: Regex = Regex::new(r"^oldrel/[0-9]+$").unwrap();
    static ref RE_MINOR: Regex = Regex::new(r"^[0-9]+[.][0-9]+$").unwrap();
    static ref RE_VERSION: Regex = Regex::new(r"^[0-9]+[.][0-9]+[.][0-9]+$").unwrap();
}


#[tokio::main]
pub async fn resolve_versions(vers: Vec<String>, os: String) -> Vec<String> {
    let client = reqwest::Client::new();
    let client = &client;
    let os = &os;
    let out: Vec<String> =
        future::join_all(vers.into_iter().map(|mut ver| async move {
            if ver == "oldrel" { ver = "oldrel/1".to_string(); }
            if ver == "release" {
                resolve_release(client, os).await
            } else if ver == "devel" {
                resolve_devel(client, os).await
            } else if RE_OLDREL.is_match(&ver) {
                resolve_oldrel(client, &ver, os).await
            } else if RE_MINOR.is_match(&ver) {
                resolve_minor(client, &ver, os).await
            } else if RE_VERSION.is_match(&ver) {
                resolve_version(client, &ver, os).await
            } else {
                panic!("Unknown version specification: {}", ver);
            }
        }))
        .await;

    out
}

async fn resolve_release(client: &reqwest::Client, os: &String) -> String {
    if os == "macos" {
        let url = API_URI.to_string() + "r-release-macos";
        let v = download_json(client, vec![url]).await;
        String::from(&v[0].to_string())
    } else if os == "win" {
        let url = API_URI.to_string() + "r-release-win";
        let v = download_json(client, vec![url]).await;
        String::from(&v[0].to_string())
    } else {
        panic!("Unknown OS: {}", os);
    }
}

async fn resolve_devel(_client: &reqwest::Client, os: &String) -> String {
    if os == "macos" {
        MACOS_DEVEL_URI.to_string()
    } else if os == "win" {
        WIN_DEVEL_URI.to_string()
    } else {
        panic!("Unknown OS: {}", os);
    }
}

async fn resolve_oldrel(client: &reqwest::Client, ver: &String, os: &String) -> String {
    let url = API_URI.to_string() + "r-" + ver;
    let resp = download_json(client, vec![url]).await;
    let version = &resp[0]["version"];
    let version = match version {
        serde_json::Value::String(s) => s,
        _ => panic!("Invalid JSON response from rversion API")
    };

    version.to_string()
}

async fn resolve_minor(client: &reqwest::Client, ver: &String, os: &String) -> String {
    let rvers = download_r_versions(client).await;
    let start = ver.to_owned() + ".0";
    let branch: Version = Version::parse(&start).unwrap();
    let mut fullver: Version = Version::parse("0.0.0").unwrap();
    let mut out = String::from("");
    let mut ok = false;

    for v in rvers.iter() {
        let mut vv = v.to_owned();
        if RE_MINOR.is_match(&v) {
            vv = vv + ".0"
        }
        let vvv = Version::parse(&vv).unwrap();
        if vvv.major == branch.major && vvv.minor == branch.minor &&
            vvv.patch >= branch.patch {
                fullver = vvv;
                out = v.to_string();
                ok = true;
            }
    }

    if ! ok { panic!("Cannot resolve minor R version {}", ver); }
    out
}

async fn resolve_version(client: &reqwest::Client, ver: &String, os: &String) -> String {
    let rvers = download_r_versions(client).await;
    if ! rvers.contains(&ver) {
        panic!("Cannot find R version {}", ver);
    }
    ver.to_string()
}

async fn download_r_versions(client: &reqwest::Client) -> Vec<String> {
    let url = API_URI.to_string() + "r-versions";
    let resp = download_json(client, vec![url]).await;
    let resp = &resp[0];
    let resp = match resp {
        serde_json::Value::Array(v) => v,
        _ => panic!("Invalid JSON response from rversion API")
    };
    let vers: Vec<String> = resp.into_iter().map(|rec| {
        let ver = &rec["version"];
        match ver {
            serde_json::Value::String(s) => s.to_string(),
            _ => panic!("Invalid JSON response from rversion API")
        }
    }).collect();

    vers
}

async fn download_json(client: &reqwest::Client, urls: Vec<String>) -> Vec<serde_json::Value> {
    let vers: Vec<serde_json::Value> =
        future::join_all(urls.into_iter().map(|url| {
            async move {
                let resp = client.get(url).send()
                    .await
                    .expect("Cannot query R versions API");
                let json: serde_json::Value = resp.json()
                    .await
                    .expect("Cannot parse JSON response");
                json
            }
        }))
        .await;

    vers
}
