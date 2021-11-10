
use futures::future;
use regex::Regex;
use lazy_static::lazy_static;
use semver::Version;

use crate::rversion::Rversion;
use crate::download::*;

const API_URI: &str = "https://api.r-hub.io/rversions/";

const MACOS_DEVEL_URI: &str =
    "https://mac.r-project.org/high-sierra/R-devel/R-devel.pkg";
const MACOS_DEVEL_ARM_URI: &str =
    "https://mac.r-project.org/big-sur/R-devel/R-devel.pkg";

const MACOS_325_URI: &str =
    "https://cloud.r-project.org/bin/macosx/old/R-3.2.4-revised.pkg";
const MACOS_OLD2_URI: &str =
    "https://cloud.r-project.org/bin/macosx/old/R-{}.pkg";
const MACOS_OLD_URI: &str =
    "https://cloud.r-project.org/bin/macosx/R-{}.pkg";
const MACOS_URI: &str =
    "https://cloud.r-project.org/bin/macosx/base/R-{}.pkg";
const MACOS_ARM_URI: &str =
    "https://cran.r-project.org/bin/macosx/big-sur-arm64/base/R-{}-arm64.pkg";

const WIN_DEVEL_URI: &str =
    "https://cran.r-project.org/bin/windows/base/R-devel-win.exe";

const DEVEL_VERSION_URI: &str =
    "https://svn.r-project.org/R/trunk/VERSION";

lazy_static! {
    static ref RE_OLDREL: Regex = Regex::new(r"^oldrel/[0-9]+$").unwrap();
    static ref RE_MINOR: Regex = Regex::new(r"^[0-9]+[.][0-9]+$").unwrap();
    static ref RE_VERSION: Regex = Regex::new(r"^[0-9]+[.][0-9]+[.][0-9]+$").unwrap();
}

#[tokio::main]
pub async fn resolve_versions(vers: Vec<String>, os: String,
                              arch: String) -> Vec<Rversion> {
    let client = reqwest::Client::new();
    let client = &client;
    let os = &os;
    let arch = &arch;
    let out: Vec<Rversion> =
        future::join_all(vers.into_iter().map(|mut ver| async move {
            if ver == "oldrel" { ver = "oldrel/1".to_string(); }
            if ver == "release" {
                resolve_release(client, os, arch).await
            } else if ver == "devel" {
                resolve_devel(client, os, arch).await
            } else if RE_OLDREL.is_match(&ver) {
                resolve_oldrel(client, &ver, os, arch).await
            } else if RE_MINOR.is_match(&ver) {
                resolve_minor(client, &ver, os, arch).await
            } else if RE_VERSION.is_match(&ver) {
                resolve_version(client, &ver, os, arch).await
            } else {
                panic!("Unknown version specification: {}", ver);
            }
        }))
        .await;

    out
}

async fn resolve_release(client: &reqwest::Client, os: &String,
                         arch: &String) -> Rversion {
    let url;
    if os == "macos" {
        url = API_URI.to_string() + "r-release-macos";
    } else if os == "win" {
        url = API_URI.to_string() + "r-release-win";
    } else {
        panic!("Unknown OS: {}", os);
    }

    let v = download_json(client, vec![url]).await;
    let v = &v[0]["version"];
    let v = match v {
        serde_json::Value::String(s) => s,
        _ => panic!("Failed to parse response from rversions API")
    };
    let dlurl = get_download_url(&v, os, arch);
    Rversion { version: v.to_string(), url: dlurl, arch: arch.to_string() }
}

async fn resolve_devel(client: &reqwest::Client, os: &String,
                       arch: &String) -> Rversion {
    let url = DEVEL_VERSION_URI.to_string();
    let txt = download_text(client, url).await;
    let ver = txt.split(" ").next().unwrap().to_string();
    if os == "macos" {
        if arch == "x86_64" {
            Rversion {
                version: ver,
                url: Some(MACOS_DEVEL_URI.to_string()),
                arch: arch.to_string()
            }
        } else {
            Rversion {
                version: ver,
                url: Some(MACOS_DEVEL_ARM_URI.to_string()),
                arch: arch.to_string()
            }
        }
    } else if os == "win" {
        Rversion {
            version: ver,
            url: Some(WIN_DEVEL_URI.to_string()),
            arch: arch.to_string()
        }
    } else {
        panic!("Unknown OS: {}", os);
    }
}

async fn resolve_oldrel(client: &reqwest::Client, ver: &String, os: &String,
                        arch: &String) -> Rversion {
    let url = API_URI.to_string() + "r-" + ver;
    let resp = download_json(client, vec![url]).await;
    let version = &resp[0]["version"];
    let version = match version {
        serde_json::Value::String(s) => s,
        _ => panic!("Invalid JSON response from rversion API")
    };

    let dlurl = get_download_url(version, os, arch);
    Rversion { version: version.to_string(), url: dlurl, arch: arch.to_string() }
}

async fn resolve_minor(client: &reqwest::Client, ver: &String, os: &String,
                       arch: &String) -> Rversion {
    let rvers = download_r_versions(client).await;
    let start = ver.to_owned() + ".0";
    let branch: Version = Version::parse(&start).unwrap();
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
                out = v.to_string();
                ok = true;
            }
    }

    if ! ok { panic!("Cannot resolve minor R version {}", ver); }

    let dlurl = get_download_url(&out, os, arch);
    Rversion { version: out, url: dlurl, arch: arch.to_string() }
}

async fn resolve_version(client: &reqwest::Client, ver: &String, os: &String,
                         arch: &String) -> Rversion {
    let rvers = download_r_versions(client).await;
    if ! rvers.contains(&ver) {
        panic!("Cannot find R version {}", ver);
    }
    let dlurl = get_download_url(ver, os, arch);
    Rversion { version: ver.to_string(), url: dlurl, arch: arch.to_string() }
}

fn get_download_url(ver: &String, os: &String, arch: &String)
                    -> Option<String> {
    fn rep(tmpl: &str, sub: &str) -> String {
        let re = Regex::new("[{][}]").unwrap();
        re.replace(tmpl, sub).to_string()
    }

    if os == "macos" {
        let vv = Version::parse(ver).unwrap();
        if arch == "x86_64" {
            let v2100 = Version::parse("2.10.0").unwrap();
            let v340 = Version::parse("3.4.0").unwrap();
            let v400 = Version::parse("4.0.0").unwrap();
            if ver == "3.2.5" {
                Some(MACOS_325_URI.to_string())
            } else if vv < v2100 {
                None
            } else if vv < v340 {
                Some(rep(MACOS_OLD2_URI, ver))
            } else if vv < v400 {
                Some(rep(MACOS_OLD_URI, ver))
            } else {
                Some(rep(MACOS_URI, ver))
            }
        } else if arch == "arm64" {
            let v410 = Version::parse("4.1.0").unwrap();
            if vv < v410 {
                None
            } else {
                Some(rep(MACOS_ARM_URI, ver))
            }
        } else {
            panic!("Unknown macOS arch: {}", arch);
        }
    } else if os == "win" {
        Some(String::from("TODO"))
    } else {
        panic!("Unknown OS: {}", os);
    }
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
