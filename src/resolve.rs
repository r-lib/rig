use futures::future;
use std::error::Error;

use lazy_static::lazy_static;
use regex::Regex;
use semver::Version;
use simple_error::{bail, SimpleError};

use crate::download::*;
use crate::rversion::*;
use crate::utils::*;

const API_URI: &str = "https://api.r-hub.io/rversions/";

const MACOS_DEVEL_URI: &str =
    "https://mac.R-project.org/high-sierra/last-success/R-devel-x86_64.pkg";
const MACOS_DEVEL_ARM_URI: &str =
    "https://mac.r-project.org/big-sur/last-success/R-devel-arm64.pkg";

const MACOS_325_URI: &str = "https://cloud.r-project.org/bin/macosx/old/R-3.2.4-revised.pkg";
const MACOS_OLD2_URI: &str = "https://cloud.r-project.org/bin/macosx/old/R-{}.pkg";
const MACOS_OLD_URI: &str = "https://cloud.r-project.org/bin/macosx/R-{}.pkg";
const MACOS_URI: &str = "https://cloud.r-project.org/bin/macosx/base/R-{}.pkg";
const MACOS_ARM_URI: &str =
    "https://cloud.r-project.org/bin/macosx/big-sur-arm64/base/R-{}-arm64.pkg";

const WIN_DEVEL_URI: &str = "https://cloud.r-project.org/bin/windows/base/R-devel-win.exe";
const WIN_URI: &str = "https://cloud.r-project.org/bin/windows/base/old/{}/R-{}-win.exe";
const WIN_OLD: &str = "https://cran-archive.r-project.org/bin/windows/base/old/{}/R-{}-win.exe";

const DEVEL_VERSION_URI: &str = "https://svn.r-project.org/R/trunk/VERSION";

lazy_static! {
    static ref RE_OLDREL: Regex = Regex::new(r"^oldrel/[0-9]+$").unwrap();
    static ref RE_MINOR: Regex = Regex::new(r"^[0-9]+[.][0-9]+$").unwrap();
    static ref RE_VERSION: Regex = Regex::new(r"^[0-9]+[.][0-9]+[.][0-9]+$").unwrap();
}

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
        future::join_all(vers.into_iter().map(move |mut ver| {
            let linux = linux.clone();
            async move {
                if ver == "oldrel" {
                    ver = "oldrel/1".to_string();
                }
                if ver == "release" {
                    resolve_release(client, os, arch, linux).await
                } else if ver == "devel" {
                    resolve_devel(client, os, arch, linux).await
                } else if ver == "next" {
                    resolve_next(client, os, arch, linux).await
                } else if RE_OLDREL.is_match(&ver) {
                    resolve_oldrel(client, &ver, os, arch, linux).await
                } else if RE_MINOR.is_match(&ver) {
                    resolve_minor(client, &ver, os, arch, linux).await
                } else if RE_VERSION.is_match(&ver) {
                    resolve_version(client, &ver, os, arch, linux).await
                } else {
                    bail!("Unknown version specification: {}", ver);
                }
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

async fn resolve_release(
    client: &reqwest::Client,
    os: &String,
    arch: &String,
    linux: Option<LinuxVersion>,
) -> Result<Rversion, Box<dyn Error>> {
    let url;
    if os == "macos" {
        url = API_URI.to_string() + "r-release-macos";
    } else if os == "win" {
        url = API_URI.to_string() + "r-release-win";
    } else if os == "linux" {
        url = API_URI.to_string() + "r-release";
    } else {
        bail!("Unknown OS: {}", os);
    }

    let v = download_json(client, vec![url]).await?;
    let v = &v[0]["version"];
    let v = match v {
        serde_json::Value::String(s) => s,
        _ => bail!("Failed to parse response from rversions API"),
    };
    let dlurl = get_download_url(&v, os, arch, linux)?;
    Ok(Rversion {
        version: Some(v.to_string()),
        url: dlurl,
        arch: Some(arch.to_string()),
    })
}

async fn resolve_devel(
    client: &reqwest::Client,
    os: &String,
    arch: &String,
    linux: Option<LinuxVersion>,
) -> Result<Rversion, Box<dyn Error>> {
    let url = DEVEL_VERSION_URI.to_string();
    let txt = download_text(client, url).await?;
    let ver = txt
        .split(" ")
        .next()
        .ok_or(SimpleError::new("Cannot determine devel version"))?
        .to_string();
    if os == "macos" {
        if arch == "x86_64" {
            Ok(Rversion {
                version: Some(ver),
                url: Some(MACOS_DEVEL_URI.to_string()),
                arch: Some(arch.to_string()),
            })
        } else {
            Ok(Rversion {
                version: Some(ver),
                url: Some(MACOS_DEVEL_ARM_URI.to_string()),
                arch: Some(arch.to_string()),
            })
        }
    } else if os == "win" {
        Ok(Rversion {
            version: Some(ver),
            url: Some(WIN_DEVEL_URI.to_string()),
            arch: Some(arch.to_string()),
        })
    } else if os == "linux" {
        fn rep(tmpl: &str, sub: &str) -> Result<String, Box<dyn Error>> {
            let re = Regex::new("[{][}]")?;
            Ok(re.replace_all(tmpl, sub).to_string())
        }
        let linux = linux.ok_or(SimpleError::new("Internal error, no Linux distro"))?;
        let url = rep(&linux.url, "devel")?;
        Ok(Rversion {
            version: Some(ver),
            url: Some(url),
            arch: None,
        })
    } else {
        bail!("Unknown OS: {}", os);
    }
}

async fn resolve_next(
    client: &reqwest::Client,
    os: &String,
    arch: &String,
    linux: Option<LinuxVersion>,
) -> Result<Rversion, Box<dyn Error>> {
    let ep: String;
    if os == "win" {
        ep = "/r-next-win".to_string();
    } else if os == "macos" {
        ep = "/r-next-macos-".to_string() + arch;
    } else if os == "linux" {
        ep = "/r-next".to_string();
    } else {
        bail!("Unknown OS:{}", os);
    }

    let url = API_URI.to_string() + &ep;
    let resp = download_json(client, vec![url]).await?;
    let resp = &resp[0];

    let version: String = unquote(&resp["version"].to_string());
    let url: Option<String>;

    if os == "linux" {
        fn rep(tmpl: &str, sub: &str) -> Result<String, Box<dyn Error>> {
            let re = Regex::new("[{][}]")?;
            Ok(re.replace_all(tmpl, sub).to_string())
        }
        let linux = linux.ok_or(SimpleError::new("Internal error, no Linux distro"))?;
        url = Some(rep(&linux.url, "next")?);
    } else {
        url = Some(unquote(&resp["URL"].to_string()));
    }

    Ok(Rversion {
        version: Some(version),
        url: url,
        arch: Some(arch.to_string()),
    })
}

async fn resolve_oldrel(
    client: &reqwest::Client,
    ver: &String,
    os: &String,
    arch: &String,
    linux: Option<LinuxVersion>,
) -> Result<Rversion, Box<dyn Error>> {
    let url = API_URI.to_string() + "r-" + ver;
    let resp = download_json(client, vec![url]).await?;
    let version = &resp[0]["version"];
    let version = match version {
        serde_json::Value::String(s) => s,
        _ => bail!("Invalid JSON response from rversion API"),
    };

    let dlurl = get_download_url(version, os, arch, linux)?;
    Ok(Rversion {
        version: Some(version.to_string()),
        url: dlurl,
        arch: Some(arch.to_string()),
    })
}

async fn resolve_minor(
    client: &reqwest::Client,
    ver: &String,
    os: &String,
    arch: &String,
    linux: Option<LinuxVersion>,
) -> Result<Rversion, Box<dyn Error>> {
    let rvers = download_r_versions(client).await;
    let start = ver.to_owned() + ".0";
    let branch: Version = Version::parse(&start)?;
    let mut out = String::from("");
    let mut ok = false;

    for v in rvers?.iter() {
        let mut vv = v.to_owned();
        if RE_MINOR.is_match(&v) {
            vv = vv + ".0"
        }
        let vvv = Version::parse(&vv)?;
        if vvv.major == branch.major && vvv.minor == branch.minor && vvv.patch >= branch.patch {
            out = v.to_string();
            ok = true;
        }
    }

    if !ok {
        bail!("Cannot resolve minor R version {}", ver);
    }

    let dlurl = get_download_url(&out, os, arch, linux)?;
    Ok(Rversion {
        version: Some(out),
        url: dlurl,
        arch: Some(arch.to_string()),
    })
}

async fn resolve_version(
    client: &reqwest::Client,
    ver: &String,
    os: &String,
    arch: &String,
    linux: Option<LinuxVersion>,
) -> Result<Rversion, Box<dyn Error>> {
    let rvers = download_r_versions(client).await;
    if !rvers?.contains(&ver) {
        bail!("Cannot find R version {}", ver);
    }
    let dlurl = get_download_url(ver, os, arch, linux)?;
    Ok(Rversion {
        version: Some(ver.to_string()),
        url: dlurl,
        arch: Some(arch.to_string()),
    })
}

fn get_download_url(
    ver: &String,
    os: &String,
    arch: &String,
    linux: Option<LinuxVersion>,
) -> Result<Option<String>, Box<dyn Error>> {
    fn rep(tmpl: &str, sub: &str) -> Result<String, Box<dyn Error>> {
        let re = Regex::new("[{][}]")?;
        Ok(re.replace_all(tmpl, sub).to_string())
    }

    let vv = Version::parse(ver)?;
    if os == "macos" {
        if arch == "x86_64" {
            let v2100 = Version::parse("2.10.0")?;
            let v340 = Version::parse("3.4.0")?;
            let v400 = Version::parse("4.0.0")?;
            if ver == "3.2.5" {
                Ok(Some(MACOS_325_URI.to_string()))
            } else if vv < v2100 {
                Ok(None)
            } else if vv < v340 {
                Ok(Some(rep(MACOS_OLD2_URI, ver)?))
            } else if vv < v400 {
                Ok(Some(rep(MACOS_OLD_URI, ver)?))
            } else {
                Ok(Some(rep(MACOS_URI, ver)?))
            }
        } else if arch == "arm64" {
            let v410 = Version::parse("4.1.0")?;
            if vv < v410 {
                Ok(None)
            } else {
                Ok(Some(rep(MACOS_ARM_URI, ver)?))
            }
        } else {
            bail!("Unknown macOS arch: {}", arch);
        }
    } else if os == "win" {
        let v340 = Version::parse("3.4.0")?;
        if vv < v340 {
            Ok(Some(rep(WIN_OLD, ver)?))
        } else {
            Ok(Some(rep(WIN_URI, ver)?))
        }
    } else if os == "linux" {
        let linux = linux.ok_or(SimpleError::new("Internal error, no Linux distro"))?;
        Ok(Some(rep(&linux.url, ver)?))
    } else {
        bail!("Unknown OS: {}", os);
    }
}

async fn download_r_versions(client: &reqwest::Client) -> Result<Vec<String>, Box<dyn Error>> {
    let url = API_URI.to_string() + "r-versions";
    let resp = download_json(client, vec![url]).await?;
    let resp = &resp[0];
    let resp = match resp {
        serde_json::Value::Array(v) => v,
        _ => bail!("Invalid JSON response from rversion API"),
    };

    let mut vers: Vec<String> = vec![];
    for rec in resp {
        let ver = &rec["version"];
        match ver {
            serde_json::Value::String(s) => vers.push(s.to_string()),
            _ => bail!("Invalid JSON response from rversion API"),
        }
    }

    Ok(vers)
}
