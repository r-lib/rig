
use futures::future;

const API_URI: &str = "https://api.r-hub.io/rversions/";
const MACOS_DEVEL_URI: &str =
    "https://mac.r-project.org/high-sierra/R-devel/R-devel.pkg";
const WIN_DEVEL_URI: &str =
    "https://cran.r-project.org/bin/windows/base/R-devel-win.exe";

#[tokio::main]
pub async fn resolve_versions(vers: Vec<String>, os: String) -> Vec<String> {
    let client = reqwest::Client::new();
    let client = &client;
    let os = &os;
    let out: Vec<String> =
        future::join_all(vers.into_iter().map(|ver| async move {
            if ver == "release" {
                resolve_release(client, os).await
            } else if ver == "devel" {
                resolve_devel(client, os).await
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
        String::from(&v[0])
    } else if os == "win" {
        let url = API_URI.to_string() + "r-release-win";
        let v = download_json(client, vec![url]).await;
        String::from(&v[0])
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

async fn download_json(client: &reqwest::Client, urls: Vec<String>) -> Vec<String> {
    let vers: Vec<String> =
        future::join_all(urls.into_iter().map(|url| {
            async move {
                let resp = client.get(url).send()
                    .await
                    .expect("Cannot query R versions API");
                let json: serde_json::Value = resp.json()
                    .await
                    .expect("Cannot parse JSON response");
                json.to_string()
            }
        }))
        .await;

    vers
}
