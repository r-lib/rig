use futures::future;
use std::error::Error;

use simple_error::bail;

use crate::download::*;
use crate::rversion::*;
use crate::utils::*;

const API_URI: &str = "https://api.r-hub.io/rversions/resolve/";

#[tokio::main]
pub async fn resolve_versions(
    vers: Vec<String>,
    os: String,
    arch: String,
    linux: Option<LinuxVersion>,
) -> Result<Vec<Rversion>, Box<dyn Error>> {
    let client = reqwest::Client::new();
    let client = &client;
    let os = &os;
    let arch = &arch;
    let out: Vec<Result<Rversion, Box<dyn Error>>> =
        future::join_all(vers.into_iter().map(move |ver| {
            let linux = linux.clone();
            async move {
                resolve_version(client, &ver, os, arch, linux).await
            }
        }))
        .await;

    // We quit with the first error we found
    let mut out2: Vec<Rversion> = vec![];
    for o in out {
        match o {
            Ok(x) => out2.push(x),
            Err(x) => bail!("Failed to resolve R version: {}", x.to_string()),
        };
    }

    Ok(out2)
}

async fn resolve_version(
    client: &reqwest::Client,
    ver: &String,
    os: &String,
    arch: &String,
    linux: Option<LinuxVersion>,
) -> Result<Rversion, Box<dyn Error>> {
    let mut url = API_URI.to_string() + ver + "/" + os;

    if os == "linux" {
        let linux = linux.unwrap();
        url = url + "-" + &linux.distro + "-" + &linux.version;
    }

    if arch != "default" {
        url = url + "/" + arch;
    }

    let resp = download_json(client, vec![url]).await?;
    let resp = &resp[0];

    let version: String = unquote(&resp["version"].to_string());
    let dlurl = Some(unquote(&resp["url"].to_string()));
    Ok(Rversion {
        version: Some(version),
        url: dlurl,
        arch: Some(arch.to_string()),
    })
}
