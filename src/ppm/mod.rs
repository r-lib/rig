//! `rig ppm`: what Posit Package Manager offers.
//!
//! This group reports on a Package Manager instance — which platforms and R
//! versions it builds for, which repositories, snapshots, packages and builds
//! it has — and publishes packages to it. It is deliberately separate from
//! `rig repos`, which manages the repositories *configured for an R
//! installation*, and from `rig pkg`, which reads package metadata.
//!
//! Two different hosts are involved, which is the one thing to keep straight:
//!
//! * Everything else reads from `<ppm>/__api__/...`, see [`api`], where
//!   `<ppm>` honors `PACKAGEMANAGER_ADDRESS`.
//! * [`ppm_builds`] reads the per-package binary index, which is rig's own
//!   derived data on its own host and does *not* follow
//!   `PACKAGEMANAGER_ADDRESS`. No P3M instance serves those files.

use std::env;
use std::error::Error;
use std::io::IsTerminal;

use clap::ArgMatches;
use tabular::{Row, Table};

use crate::repos::binaries::{ppm_url, PpmDistro, PpmStatus};

pub mod api;
mod ppm_build_log;
mod ppm_builds;
mod ppm_downloads;
mod ppm_packages;
mod ppm_platforms;
mod ppm_publish;
mod ppm_repos;
mod ppm_status;
mod ppm_vulns;
mod ppm_whoami;

use ppm_build_log::{resolve_target, sc_ppm_build_log};
use ppm_builds::sc_ppm_builds;
use ppm_downloads::sc_ppm_downloads;
use ppm_packages::{sc_ppm_history, sc_ppm_package, sc_ppm_packages};
use ppm_platforms::sc_ppm_platforms;
use ppm_publish::sc_ppm_publish;
use ppm_repos::{
    load_snapshots, sc_ppm_repos, sc_ppm_snapshots, sc_ppm_sources, sc_ppm_transactions,
};
use ppm_status::sc_ppm_status;
use ppm_vulns::sc_ppm_vulns;
use ppm_whoami::sc_ppm_whoami;

pub fn sc_ppm(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        Some(("pkg", s)) => sc_ppm_pkg(s, args, mainargs),
        // Hidden: these were top-level commands before `rig ppm pkg`
        Some(("builds", s)) => sc_ppm_builds(s, args, mainargs),
        Some(("build-log", s)) => sc_ppm_build_log(s, args, mainargs),
        Some(("platforms", s)) => sc_ppm_platforms(s, args, mainargs),
        Some(("r-versions", s)) => sc_ppm_r_versions(s, args, mainargs),
        Some(("repos", s)) => sc_ppm_repos(s, args, mainargs),
        Some(("snapshots", s)) => sc_ppm_snapshots(s, args, mainargs),
        Some(("sources", s)) => sc_ppm_sources(s, args, mainargs),
        Some(("status", s)) => sc_ppm_status(s, args, mainargs),
        Some(("transactions", s)) => sc_ppm_transactions(s, args, mainargs),
        Some(("url", s)) => sc_ppm_url(s, args, mainargs),
        Some(("whoami", s)) => sc_ppm_whoami(s, args, mainargs),
        // Anything else means clap and this match disagree, which is a bug
        // here rather than a user error, so say so instead of exiting 0.
        Some((name, _)) => bail!("Internal error: unknown `rig ppm` subcommand: {}", name),
        None => Ok(()),
    }
}

/// `rig ppm pkg`: the package commands. `ppmargs` are the arguments of
/// `rig ppm`, for `--json`.
fn sc_ppm_pkg(
    args: &ArgMatches,
    ppmargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        Some(("builds", s)) => sc_ppm_builds(s, ppmargs, mainargs),
        Some(("build-log", s)) => sc_ppm_build_log(s, ppmargs, mainargs),
        Some(("downloads", s)) => sc_ppm_downloads(s, ppmargs, mainargs),
        Some(("history", s)) => sc_ppm_history(s, ppmargs, mainargs),
        Some(("info", s)) => sc_ppm_package(s, ppmargs, mainargs),
        Some(("list", s)) => sc_ppm_packages(s, ppmargs, mainargs),
        Some(("publish", s)) => sc_ppm_publish(s, ppmargs, mainargs),
        Some(("vulns", s)) => sc_ppm_vulns(s, ppmargs, mainargs),
        Some((name, _)) => bail!("Internal error: unknown `rig ppm pkg` subcommand: {}", name),
        None => Ok(()),
    }
}

/// `--json` is accepted at every level: `rig --json ppm status`,
/// `rig ppm --json status` and `rig ppm status --json`.
pub(crate) fn want_json(args: &ArgMatches, ppmargs: &ArgMatches, mainargs: &ArgMatches) -> bool {
    args.get_flag("json") || ppmargs.get_flag("json") || mainargs.get_flag("json")
}

/// Whether to color output: only for a terminal, and never when `NO_COLOR` is
/// set.
pub(crate) fn use_color() -> bool {
    std::io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none()
}

/// Render a boolean as a table cell. `-` rather than `no`, so that the
/// interesting value is the one that stands out.
pub(crate) fn yesno(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "-"
    }
}

/// Print a table whose first row is its header, with a rule under that header
/// spanning the table.
///
/// `tabular` pads every column to fit the widest cell in it, and does not pad
/// the last one at all, so how wide the table ends up is only known once it is
/// rendered — these tables carry URLs and dependency lists. Adding the rule as
/// a heading row would therefore mean guessing; rendering first and measuring
/// is what keeps it from being ragged.
pub(crate) fn print_table(tab: &Table) {
    let rendered = tab.to_string();
    let width = rendered
        .lines()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    let mut lines = rendered.lines();
    if let Some(header) = lines.next() {
        println!("{}", header);
        println!("{}", "-".repeat(width));
        for line in lines {
            println!("{}", line);
        }
    }
}

/// The build-target table, shared by `rig ppm platforms` and the `distros`
/// section of `rig ppm status`.
///
/// Each entry is shown as P3M sent it: no dropping of binary-less targets, and
/// no deduplication of the several entries that map onto the same `platform`
/// (`centos7` and `rhel7` both build `centos7`). Which entries to pass is the
/// caller's decision.
///
/// `with_hidden` adds the `hidden` column, and is for callers that include the
/// retired targets; without them every cell in it would read `-`.
pub(crate) fn distro_table<'a>(
    distros: impl Iterator<Item = &'a PpmDistro>,
    with_hidden: bool,
) -> Table {
    let mut spec = "{:<}   {:<}   {:<}   {:<}   {:<}   {:<}   {:<}".to_string();
    if with_hidden {
        spec.push_str("   {:<}");
    }
    let mut tab = Table::new(&spec);

    let mut header = Row::new()
        .with_cell("name")
        .with_cell("os")
        .with_cell("platform")
        .with_cell("distribution")
        .with_cell("release")
        .with_cell("arch")
        .with_cell("binaries");
    if with_hidden {
        header = header.with_cell("hidden");
    }
    tab.add_row(header);

    for distro in distros {
        let mut row = Row::new()
            .with_cell(&distro.name)
            .with_cell(&distro.os)
            .with_cell(distro.platform())
            .with_cell(&distro.distribution)
            .with_cell(&distro.release)
            .with_cell(distro.arch.join(", "))
            .with_cell(yesno(distro.binaries));
        if with_hidden {
            row = row.with_cell(yesno(distro.hidden));
        }
        tab.add_row(row);
    }
    tab
}

/// `rig ppm url`: the base URL of the P3M instance rig reports on, or with
/// `--repo`, `--snapshot` or `--platform`, the URL of a repository.
fn sc_ppm_url(
    args: &ArgMatches,
    ppmargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let base = ppm_url();
    let repo = args.get_one::<String>("repo");
    let snapshot = args.get_one::<String>("snapshot");
    let platform = args.get_one::<String>("platform");
    let url = if repo.is_none() && snapshot.is_none() && platform.is_none() {
        base
    } else {
        let repo = match repo {
            Some(r) => r.clone(),
            None => ppm_repos::first_r_repo()?,
        };
        let repo = repo.as_str();
        let snapshot = match snapshot.map(|s| s.as_str()) {
            None | Some("latest") => "latest".to_string(),
            Some(s) => resolve_snapshot(repo, s)?,
        };
        let linux = match platform.map(|s| s.as_str()) {
            None => None,
            Some(p) => {
                // `current` is the current machine
                let p = if p == "current" { None } else { Some(p) };
                let (target, _arch) = resolve_target(p, None)?;
                let status = PpmStatus::load(None)?;
                let is_linux = status
                    .distros
                    .iter()
                    .any(|d| d.platform() == target && d.os == "linux");
                is_linux.then_some(target)
            }
        };
        repo_url(&base, repo, &snapshot, linux.as_deref())
    };
    if want_json(args, ppmargs, mainargs) {
        #[derive(serde::Serialize)]
        struct PpmUrl<'a> {
            url: &'a str,
        }
        println!("{}", serde_json::to_string_pretty(&PpmUrl { url: &url })?);
    } else {
        // Bare, so `$(rig ppm url)` is the URL and nothing else.
        println!("{}", url);
    }
    Ok(())
}

/// The URL of a PPM repository. Linux binaries have their own URLs, macOS
/// and Windows binaries are served from the plain repository URL.
fn repo_url(base: &str, repo: &str, snapshot: &str, linux: Option<&str>) -> String {
    match linux {
        Some(distro) => format!("{}/{}/__linux__/{}/{}", base, repo, distro, snapshot),
        None => format!("{}/{}/{}", base, repo, snapshot),
    }
}

/// A snapshot of `repo`, checked against its list of snapshots. A date
/// without a snapshot means the latest snapshot before it, as in PPM's own
/// UI.
fn resolve_snapshot(repo: &str, snapshot: &str) -> Result<String, Box<dyn Error>> {
    let mut snaps = load_snapshots(repo)?;
    // Snapshots without a name cannot be used in a URL: repositories with
    // several sources, or a curated or Git source, are pinned with a URL
    // from Package Manager's UI instead.
    snaps.retain(|s| !s.alias.is_empty());
    if snaps.is_empty() {
        bail!(
            "Repository '{}' has no dated snapshots. Repositories with several sources, \
             or with a curated or Git source, are pinned with a snapshot URL from \
             Package Manager's UI.",
            repo
        );
    }
    if snaps.iter().any(|s| s.alias == snapshot) {
        return Ok(snapshot.to_string());
    }
    // newest first, so this is the latest one on or before the date
    let before = snaps
        .iter()
        .find(|s| s.date.get(..10).is_some_and(|d| d <= snapshot));
    match before {
        Some(s) if looks_like_date(snapshot) => Ok(s.alias.clone()),
        _ => bail!(
            "No snapshot '{}' in repository '{}', see `rig ppm snapshots {}`",
            snapshot,
            repo,
            repo
        ),
    }
}

fn looks_like_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// `rig ppm r-versions`: the minor R versions P3M builds binaries for.
fn sc_ppm_r_versions(
    args: &ArgMatches,
    ppmargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let status = PpmStatus::load(None)?;
    if want_json(args, ppmargs, mainargs) {
        println!("{}", serde_json::to_string_pretty(&status.r_versions)?);
    } else {
        // One per line and no header, in P3M's own newest-first order, so the
        // output pipes into other tools unchanged.
        for version in &status.r_versions {
            println!("{}", version);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repo_url() {
        assert_eq!(
            repo_url("https://p3m.dev", "cran", "latest", None),
            "https://p3m.dev/cran/latest"
        );
        assert_eq!(
            repo_url("https://p3m.dev", "cran", "2024-01-02", Some("noble")),
            "https://p3m.dev/cran/__linux__/noble/2024-01-02"
        );
    }

    #[test]
    fn test_looks_like_date() {
        assert!(looks_like_date("2024-01-02"));
        assert!(!looks_like_date("2024-1-02"));
        assert!(!looks_like_date("123456"));
    }
}
