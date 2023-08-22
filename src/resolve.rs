use futures::future;
use std::error::Error;

use clap::ArgMatches;
use simple_error::bail;

use crate::common::*;
use crate::download::*;
use crate::rversion::*;
use crate::utils::*;

const API_URI: &str = "https://api.r-hub.io/rversions/resolve/";

pub fn get_resolve(args: &ArgMatches) -> Result<Rversion, Box<dyn Error>> {
    let platform = get_platform(args)?;
    let arch = get_arch(&platform, args);
    let str: &String = args.get_one("str").unwrap();
    let eps = vec![str.to_string()];

    if str.len() > 8 && (&str[..7] == "http://" || &str[..8] == "https://") {
        Ok(Rversion {
            version: None,
            url: Some(str.to_string()),
            arch: None,
        })
    } else {
        Ok(resolve_versions(eps, &platform, &arch)?[0].to_owned())
    }
}

#[tokio::main]
pub async fn resolve_versions(
    vers: Vec<String>,
    platform: &str,
    arch: &str
) -> Result<Vec<Rversion>, Box<dyn Error>> {
    let client = reqwest::Client::new();
    let client = &client;
    let out: Vec<Result<Rversion, Box<dyn Error>>> =
        future::join_all(vers.into_iter().map(move |ver| {
            async move {
                resolve_version(client, &ver, platform, arch).await
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
    ver: &str,
    platform: &str,
    arch: &str
) -> Result<Rversion, Box<dyn Error>> {
    let mut url = API_URI.to_string() + ver + "/" + platform;

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
