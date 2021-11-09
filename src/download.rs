
use futures::future;

pub async fn download_text(client: &reqwest::Client, url: String) -> String {
    let resp = client.get(url).send().await;
    let body = resp.unwrap().text().await;
    body.unwrap()
}

pub async fn download_json(client: &reqwest::Client, urls: Vec<String>) -> Vec<serde_json::Value> {
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
