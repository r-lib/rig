// `rig self update` -- update the rig binary itself in place.
//
// This only works if rig was installed by the `install.sh` / `install.ps1`
// scripts; see `crate::install_receipt` for how that's determined.
//
// Version discovery deliberately avoids the GitHub API (which would need a
// token to get a decent rate limit for anonymous use): the stable path reads
// the `Location` header of the redirect that
// `https://github.com/<repo>/releases/latest` returns, and the
// `--pre-release` path scrapes release tags out of the plain releases page.

use std::error::Error;

use clap::ArgMatches;
use log::info;
use regex::Regex;
use simple_error::bail;

use crate::install::unpack_package;
use crate::install_receipt::{
    gate, read_receipt, receipt_path, refusal_message, GateResult, Receipt,
};
use crate::output::OUTPUT;
use crate::utils::http_client;
use crate::utils::http_client_builder;
use crate::utils::write_atomically;

const REPO: &str = "r-lib/rig";

// Mirrors the platform/arch tokens `install.sh` / `install.ps1` compute, so
// the asset name built here matches what those scripts (and
// `tools/release-assets.sh`) actually publish.
fn detect_plat_arch() -> Result<(String, String), Box<dyn Error>> {
    let plat = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        bail!("unsupported operating system");
    };

    let arch = match std::env::consts::ARCH {
        "aarch64" | "arm64" => {
            if plat == "macos" {
                "arm64"
            } else {
                "aarch64"
            }
        }
        "x86_64" => "x86_64",
        other => bail!("unsupported architecture: {}", other),
    };

    Ok((plat.to_string(), arch.to_string()))
}

fn asset_name(plat: &str, arch: &str, vtoken: &str) -> String {
    let ext = if plat == "windows" { "zip" } else { "tar.gz" };
    format!("rig-{}-{}-{}.{}", plat, arch, vtoken, ext)
}

fn parse_version(tag: &str) -> Result<semver::Version, Box<dyn Error>> {
    let stripped = tag.strip_prefix('v').unwrap_or(tag);
    Ok(semver::Version::parse(stripped)?)
}

fn pick_highest_tag(tags: &[String]) -> Option<(String, semver::Version)> {
    tags.iter()
        .filter_map(|tag| parse_version(tag).ok().map(|v| (tag.clone(), v)))
        .max_by(|a, b| a.1.cmp(&b.1))
}

// Extracts the tag from a `Location` header value such as
// `https://github.com/r-lib/rig/releases/tag/v0.10.0`.
fn tag_from_location_header(location: &str) -> Option<String> {
    location.rsplit('/').next().map(|s| s.to_string())
}

// Extracts every `releases/tag/<tag>` reference out of the releases page HTML.
fn tags_from_releases_page(html: &str) -> Vec<String> {
    let re = Regex::new(r#"releases/tag/([^"'/?#]+)"#).unwrap();
    let mut tags: Vec<String> = re.captures_iter(html).map(|c| c[1].to_string()).collect();
    tags.sort();
    tags.dedup();
    tags
}

#[tokio::main]
async fn latest_stable_tag() -> Result<String, Box<dyn Error>> {
    let client = http_client_builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let url = format!("https://github.com/{}/releases/latest", REPO);
    let resp = client.get(&url).send().await?;
    let location = resp
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .ok_or("GitHub did not return a redirect for the latest release")?;
    tag_from_location_header(location).ok_or_else(|| "cannot parse latest release tag".into())
}

#[tokio::main]
async fn all_tags() -> Result<Vec<String>, Box<dyn Error>> {
    let client = http_client();
    let url = format!("https://github.com/{}/releases", REPO);
    let resp = client.get(&url).send().await?.error_for_status()?;
    let html = resp.text().await?;
    Ok(tags_from_releases_page(&html))
}

fn resolve_target_tag(pre_release: bool) -> Result<(String, semver::Version), Box<dyn Error>> {
    if pre_release {
        let tags = all_tags()?;
        pick_highest_tag(&tags).ok_or_else(|| "no releases found".into())
    } else {
        let tag = latest_stable_tag()?;
        let version = parse_version(&tag)?;
        Ok((tag, version))
    }
}

pub fn sc_self_update(args: &ArgMatches, _mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let receipt = read_receipt()?;
    let receipt = match gate(receipt) {
        GateResult::Ok(r) => r,
        other => bail!("{}", refusal_message("rig self update", &other)),
    };

    let current = semver::Version::parse(env!("CARGO_PKG_VERSION"))?;
    let pre_release = args.get_flag("pre-release");
    let (tag, latest) = resolve_target_tag(pre_release)?;

    if latest <= current {
        OUTPUT.success(&format!("rig {} is already the latest version", current));
        return Ok(());
    }

    if args.get_flag("dry-run") {
        OUTPUT.status(&format!(
            "rig {} is available (currently {})",
            latest, current
        ));
        return Ok(());
    }

    let (plat, arch) = detect_plat_arch()?;
    let vtoken = tag.strip_prefix('v').unwrap_or(&tag);
    let asset = asset_name(&plat, &arch, vtoken);
    let url = format!(
        "https://github.com/{}/releases/download/{}/{}",
        REPO, tag, asset
    );

    let tmp_dir = tempfile::tempdir()?;
    let archive_path = tmp_dir.path().join(&asset);
    OUTPUT.status(&format!("Downloading {} ...", asset));
    let client = http_client();
    crate::download::download_file(&client, &url, archive_path.as_os_str())?;

    let extract_dir = tmp_dir.path().join("extracted");
    std::fs::create_dir_all(&extract_dir)?;
    unpack_package(&archive_path, &extract_dir)?;

    let bin_name = if plat == "windows" { "rig.exe" } else { "rig" };
    let new_bin = extract_dir.join("bin").join(bin_name);
    if !new_bin.exists() {
        bail!("downloaded archive did not contain {}", bin_name);
    }

    OUTPUT.status("Replacing the running rig binary ...");
    self_replace::self_replace(&new_bin)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let current_exe = std::env::current_exe()?;
        let mut perms = std::fs::metadata(&current_exe)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&current_exe, perms)?;
    }

    let updated_receipt = Receipt {
        rig_version: vtoken.to_string(),
        installed_at: rfc3339_now(),
        ..receipt
    };
    write_atomically(
        &receipt_path()?,
        serde_json::to_string_pretty(&updated_receipt)?.as_bytes(),
    )?;

    OUTPUT.success(&format!("rig updated: {} -> {}", current, latest));
    info!("rig self update: {} -> {}", current, latest);

    Ok(())
}

// Small local helper so this module doesn't need a `chrono`/`time` crate
// dependency just for an RFC 3339 timestamp in the receipt.
fn rfc3339_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Days since epoch -> proleptic Gregorian date, then HH:MM:SS from the
    // remainder. This is a tiny, dependency-free RFC 3339 UTC formatter.
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);

    let mut z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    z -= era * 146_097;
    let doe = z as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m_ = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m_ <= 2 { y + 1 } else { y };

    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, m_, d, h, m, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_highest_tag_prefers_release_over_prerelease() {
        let tags = vec![
            "v0.10.0-beta3".to_string(),
            "v0.9.0".to_string(),
            "v0.10.0".to_string(),
        ];
        let (tag, version) = pick_highest_tag(&tags).unwrap();
        assert_eq!(tag, "v0.10.0");
        assert_eq!(version, semver::Version::new(0, 10, 0));
    }

    #[test]
    fn pick_highest_tag_ignores_unparseable() {
        let tags = vec!["latest".to_string(), "v0.9.0".to_string()];
        let (tag, _) = pick_highest_tag(&tags).unwrap();
        assert_eq!(tag, "v0.9.0");
    }

    #[test]
    fn asset_name_matches_install_scripts() {
        assert_eq!(
            asset_name("macos", "arm64", "0.10.0"),
            "rig-macos-arm64-0.10.0.tar.gz"
        );
        assert_eq!(
            asset_name("linux", "x86_64", "latest"),
            "rig-linux-x86_64-latest.tar.gz"
        );
        assert_eq!(
            asset_name("windows", "x86_64", "0.10.0"),
            "rig-windows-x86_64-0.10.0.zip"
        );
    }

    #[test]
    fn tag_from_location_header_parses() {
        assert_eq!(
            tag_from_location_header("https://github.com/r-lib/rig/releases/tag/v0.10.0"),
            Some("v0.10.0".to_string())
        );
    }

    #[test]
    fn tags_from_releases_page_scrapes_hrefs() {
        let html = r#"
            <a href="/r-lib/rig/releases/tag/v0.10.0">v0.10.0</a>
            <a href="/r-lib/rig/releases/tag/v0.10.0-beta3">v0.10.0-beta3</a>
            <a href="/r-lib/rig/releases/tag/v0.9.0">v0.9.0</a>
        "#;
        let mut tags = tags_from_releases_page(html);
        tags.sort();
        assert_eq!(
            tags,
            vec![
                "v0.10.0".to_string(),
                "v0.10.0-beta3".to_string(),
                "v0.9.0".to_string(),
            ]
        );
    }

    // Hits the real network; run explicitly with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn latest_stable_tag_resolves_a_real_tag() {
        let tag = latest_stable_tag().unwrap();
        assert!(tag.starts_with('v'));
        parse_version(&tag).unwrap();
    }
}
