//! `rig ppm status`: P3M's status document, as a report.

use std::collections::BTreeMap;
use std::error::Error;
use std::time::Duration;

use clap::ArgMatches;
use owo_colors::OwoColorize;
use tabular::{row, Table};

use crate::ppm::{distro_table, print_table, use_color, want_json};
use crate::repos::binaries::{ppm_status_url, PpmMacosUrls, PpmStatus, PpmSupportWindow};
use crate::textfmt::print_field;

/// Width of the label column in the scalar block.
const LABEL_WIDTH: usize = 22;

pub fn sc_ppm_status(
    args: &ArgMatches,
    ppmargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    // Zero TTL, i.e. always revalidate. Unlike `rig ppm platforms`, this
    // command's whole job is reporting live server state — `build_date` and the
    // support window — so a copy up to a day old would be the wrong answer. The
    // document is about 12 KB.
    let status = PpmStatus::load(Some(Duration::ZERO))?;

    if want_json(args, ppmargs, mainargs) {
        // Includes `extra`, so fields rig does not model are still reported.
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        print_status(&status, &ppm_status_url());
    }
    Ok(())
}

fn print_status(status: &PpmStatus, status_url: &str) {
    let color = use_color();

    // -- Header ------------------------------------------------------------
    let name = if status.version.is_empty() {
        "Posit Package Manager".to_string()
    } else {
        format!("Posit Package Manager {}", status.version)
    };
    if color {
        println!("{}   {}", name.bold(), status_url.dimmed());
    } else {
        println!("{}   {}", name, status_url);
    }
    println!();

    // -- Scalars -----------------------------------------------------------
    // `yes`/`no` here rather than the `yesno` the tables use: in a
    // `label   value` block a `-` reads as "the instance did not say", which is
    // what an empty string means, not as "false".
    for (label, value) in [
        ("version", status.version.clone()),
        ("build_date", status.build_date.clone()),
        ("cran_repo", status.cran_repo.clone()),
        ("r_configured", yes_no(status.r_configured)),
        ("python_configured", yes_no(status.python_configured)),
        ("binaries_enabled", yes_no(status.binaries_enabled)),
        ("auth_enabled", yes_no(status.auth_enabled)),
        (
            "support_window",
            support_window_cell(&status.support_window),
        ),
    ] {
        let value = if value.is_empty() {
            "-".to_string()
        } else {
            value
        };
        print_field(label, &value, LABEL_WIDTH, color);
    }

    // -- R versions --------------------------------------------------------
    section("R versions", status.r_versions.len(), color);
    if !status.r_versions.is_empty() {
        let mut tab = Table::new("{:<}");
        tab.add_row(row!("r_version"));
        for version in &status.r_versions {
            tab.add_row(row!(version));
        }
        print_table(&tab);
    }

    // -- Build targets -----------------------------------------------------
    // Every entry, retired ones included: this command reports the status
    // document, where `rig ppm platforms` answers "what can I build against".
    section("Build targets", status.distros.len(), color);
    if !status.distros.is_empty() {
        print_table(&distro_table(status.distros.iter(), true));
    }

    // -- Bioconductor ------------------------------------------------------
    section("Bioconductor versions", status.bioc_versions.len(), color);
    if !status.bioc_versions.is_empty() {
        let mut tab = Table::new("{:<}   {:<}   {:<}");
        tab.add_row(row!("bioc_version", "r_version", "cran_snapshot"));
        for bioc in &status.bioc_versions {
            tab.add_row(row!(
                dash(&bioc.bioc_version),
                dash(&bioc.r_version),
                dash(&bioc.cran_snapshot)
            ));
        }
        print_table(&tab);
    }

    // -- macOS builds ------------------------------------------------------
    section("macOS binaries", status.macos_urls.len(), color);
    if !status.macos_urls.is_empty() {
        let mut tab = Table::new("{:<}   {:<}   {:<}");
        tab.add_row(row!("r_version", "arm64", "x86_64"));
        for key in macos_url_keys(&status.macos_urls) {
            let urls = &status.macos_urls[key];
            tab.add_row(row!(key, dash(&urls.arm64), dash(&urls.x86_64)));
        }
        print_table(&tab);
    }
}

/// A blank line, then a section title with its row count.
fn section(title: &str, count: usize, color: bool) {
    println!();
    let title = format!("{} ({})", title, count);
    if color {
        println!("{}", title.bold());
    } else {
        println!("{}", title);
    }
}

fn dash(value: &str) -> &str {
    if value.is_empty() {
        "-"
    } else {
        value
    }
}

fn yes_no(value: bool) -> String {
    if value { "yes" } else { "no" }.to_string()
}

/// `513 days left (reminder at 90 days)`, or `-` when the instance sent no
/// support window.
fn support_window_cell(window: &PpmSupportWindow) -> String {
    if window.days_left == 0 && window.days_reminder == 0 {
        return "-".to_string();
    }
    format!(
        "{} days left (reminder at {} days)",
        window.days_left, window.days_reminder
    )
}

/// The `macos_urls` keys in the order to print them: `default` first, then R
/// versions newest first.
///
/// Needed because the map is keyed by strings, and sorting those puts `4.10`
/// before `4.2`.
fn macos_url_keys(urls: &BTreeMap<String, PpmMacosUrls>) -> Vec<&String> {
    let mut keys: Vec<&String> = urls.keys().collect();
    keys.sort_by(|a, b| match (r_minor_key(a), r_minor_key(b)) {
        (Some(a), Some(b)) => b.cmp(&a),
        // Anything that is not an R version — `default` — goes first.
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (None, None) => a.cmp(b),
    });
    keys
}

/// `"4.6"` -> `Some((4, 6))`. `None` for `default` and anything else that is
/// not a `<major>.<minor>` pair.
fn r_minor_key(key: &str) -> Option<(u32, u32)> {
    let (major, minor) = key.split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(names: &[&str]) -> BTreeMap<String, PpmMacosUrls> {
        names
            .iter()
            .map(|n| (n.to_string(), PpmMacosUrls::default()))
            .collect()
    }

    #[test]
    fn macos_keys_are_default_then_newest_r_first() {
        let urls = keys(&["4.2", "4.10", "default", "3.6"]);
        let sorted: Vec<&str> = macos_url_keys(&urls).iter().map(|k| k.as_str()).collect();
        // A plain string sort would put "4.10" before "4.2", and "3.6" first.
        assert_eq!(sorted, vec!["default", "4.10", "4.2", "3.6"]);
    }

    #[test]
    fn macos_keys_handle_only_default() {
        let urls = keys(&["default"]);
        assert_eq!(macos_url_keys(&urls).len(), 1);
        assert!(macos_url_keys(&keys(&[])).is_empty());
    }

    #[test]
    fn r_minor_key_parses_only_real_r_versions() {
        assert_eq!(r_minor_key("4.6"), Some((4, 6)));
        assert_eq!(r_minor_key("4.10"), Some((4, 10)));
        assert_eq!(r_minor_key("default"), None);
        assert_eq!(r_minor_key(""), None);
        assert_eq!(r_minor_key("4"), None);
        assert_eq!(r_minor_key("4.x"), None);
    }

    #[test]
    fn support_window_formats_or_dashes() {
        assert_eq!(
            support_window_cell(&PpmSupportWindow {
                disable_expiry_banner: false,
                days_left: 513,
                days_reminder: 90,
            }),
            "513 days left (reminder at 90 days)"
        );
        assert_eq!(
            support_window_cell(&PpmSupportWindow::default()),
            "-".to_string()
        );
    }
}
