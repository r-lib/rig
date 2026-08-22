//! `rig ppm`: what Posit Package Manager offers.
//!
//! This group reports on P3M itself — which platforms and R versions it builds
//! for, and which builds exist for a package. It is deliberately separate from
//! `rig repos`, which manages the repositories *configured for an R
//! installation*, and from `rig pkg`, which reads package metadata.
//!
//! Two different hosts are involved, which is the one thing to keep straight:
//!
//! * [`ppm_platforms`], [`ppm_status`] and `r-versions` read P3M's status
//!   document at `<ppm>/__api__/status`, where `<ppm>` honors
//!   `PACKAGEMANAGER_ADDRESS`.
//! * [`ppm_builds`] reads the per-package binary index, which is rig's own
//!   derived data on its own host and does *not* follow
//!   `PACKAGEMANAGER_ADDRESS`. No P3M instance serves those files.

use std::env;
use std::error::Error;
use std::io::IsTerminal;

use clap::ArgMatches;
use simple_error::bail;
use tabular::{Row, Table};

use crate::repos::binaries::{ppm_url, PpmDistro, PpmStatus};

mod ppm_builds;
mod ppm_platforms;
mod ppm_status;

use ppm_builds::sc_ppm_builds;
use ppm_platforms::sc_ppm_platforms;
use ppm_status::sc_ppm_status;

pub fn sc_ppm(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        Some(("builds", s)) => sc_ppm_builds(s, args, mainargs),
        Some(("platforms", s)) => sc_ppm_platforms(s, args, mainargs),
        Some(("r-versions", s)) => sc_ppm_r_versions(s, args, mainargs),
        Some(("status", s)) => sc_ppm_status(s, args, mainargs),
        Some(("url", s)) => sc_ppm_url(s, args, mainargs),
        // Anything else means clap and this match disagree, which is a bug
        // here rather than a user error, so say so instead of exiting 0.
        Some((name, _)) => bail!("Internal error: unknown `rig ppm` subcommand: {}", name),
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

/// `rig ppm url`: the base URL of the P3M instance rig reports on.
fn sc_ppm_url(
    args: &ArgMatches,
    ppmargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let url = ppm_url();
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
