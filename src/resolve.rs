
use futures::future;

const API_URI: &str = "https://api.r-hub.io/rversions/";

#[tokio::main]
pub async fn resolve_urls(eps: Vec<String>) -> Vec<String> {
    let client = reqwest::Client::new();
    let urls: Vec<String> = eps.iter()
        .map(|ep| API_URI.to_string() + &ep)
        .collect();

    let vers: Vec<String> =
        future::join_all(urls.into_iter().map(|url| {
            let client = &client;
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
