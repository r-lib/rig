
use std::error::Error;
use std::path::PathBuf;

use serde_derive::Deserialize;
use serde_derive::Serialize;
use simple_error::*;

use crate::common::*;
use crate::rversion::*;
use crate::utils::*;

#[derive(Serialize, Deserialize, Debug)]
#[allow(non_snake_case)]
struct REnvLockfileR {
    Version: String,
}

#[derive(Serialize, Deserialize, Debug)]
#[allow(non_snake_case)]
struct REnvLockfile {
    R: REnvLockfileR,
}

pub fn parse_r_version(lockfile: PathBuf)
                       -> Result<String, Box<dyn Error>> {
    let contents = read_file_string(&lockfile)?;
    let lockf: REnvLockfile = serde_json::from_str(&contents)?;
    Ok(lockf.R.Version.to_string())
}

fn filter_ok_versions(all: Vec<InstalledVersion>)
                      -> Vec<OKInstalledVersion> {
    let mut ok: Vec<OKInstalledVersion> = vec![];
    for ver in all.iter() {
        match ver {
            InstalledVersion {
                name: n, version: Some(v), path: Some(p), binary: Some(b)
            } => {
                if let Ok(sv) = semver::Version::parse(v) {
                    ok.push(OKInstalledVersion {
                        name: n.to_string(),
                        version: sv,
                        path: p.to_string(),
                        binary: b.to_string()
                    });
                }
            },
            _ => {}
        }
    }
    ok
}

pub fn match_r_version(ver: &str)
                       -> Result<OKInstalledVersion, Box<dyn Error>> {
    let allvers = sc_get_list_details()?;
    let mut okvers = filter_ok_versions(allvers);
    okvers.sort();

    let ver = try_with!(
        semver::Version::parse(ver),
        "Invalid R version in renv.lock file: {:?}",
        ver
    );

    // Matching major.minor
    let goodvers: Vec<OKInstalledVersion> =
        okvers.into_iter()
        .filter(|v| {
            v.version.major == ver.major && v.version.minor == ver.minor
        })
        .collect();

    // If we have a perfect match, then reduce further
    let goodvers2: Vec<OKInstalledVersion>;
    match goodvers.iter().find(|v| v.version.patch == ver.patch) {
        Some(_) => {
            goodvers2 =
                goodvers.into_iter()
                .filter(|v| v.version.patch == ver.patch)
                .collect();
        },
        None => goodvers2 = goodvers
    };

    // If we have an arm64 version, reduce to those
    let goodvers3: Vec<OKInstalledVersion>;
    match goodvers2.iter().find(|v| v.name.ends_with("-arm64")) {
        Some(_) => {
            goodvers3 =
                goodvers2.into_iter()
                .filter(|v| v.name.ends_with("-arm64"))
                .collect();
        },
        None => goodvers3 = goodvers2
    };

    // Choose the latest one of these (there is surely at least on left)
    match goodvers3.last() {
        Some(v) => Ok(v.to_owned()),
        None => bail!("Cannot find any R version close to R {}, \
                       required by renv lock file.", ver)
    }
}
