//! `rig ppm platforms`: the build targets P3M reports.

use std::error::Error;
use std::io::IsTerminal;

use clap::ArgMatches;
use owo_colors::OwoColorize;

use crate::ppm::{distro_table, print_table, use_color, want_json};
use crate::repos::binaries::{ppm_url, PpmDistro, PpmStatus};

pub fn sc_ppm_platforms(
    args: &ArgMatches,
    ppmargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    // The default 24-hour TTL: the build-target list changes a few times a
    // year, and this command should work offline.
    let status = PpmStatus::load(None)?;

    // Retired targets are more than half of the list on the public instance,
    // and none of them is something to build against today, so they are out of
    // the way unless asked for. `--all` also applies to `--json`: a filtered
    // report and an unfiltered export would not agree.
    let all = args.get_flag("all");
    let shown: Vec<&PpmDistro> = status.distros.iter().filter(|d| all || !d.hidden).collect();
    let hidden = status.distros.iter().filter(|d| d.hidden).count();

    if want_json(args, ppmargs, mainargs) {
        // Each entry as P3M sent it, including the fields the table omits.
        println!("{}", serde_json::to_string_pretty(&shown)?);
        return Ok(());
    }

    let tty = std::io::stdout().is_terminal();
    let color = use_color();

    let count = shown.len();
    let target_word = if count == 1 { "target" } else { "targets" };
    let head = if color {
        format!("{} build {}", count.cyan().bold(), target_word)
    } else {
        format!("{} build {}", count, target_word)
    };
    let tag = if all || hidden == 0 {
        format!("({})", ppm_url())
    } else {
        format!("({}, {} retired hidden)", ppm_url(), hidden)
    };
    println!(
        "{} {}",
        head,
        if color { tag.dimmed().to_string() } else { tag }
    );
    if count == 0 {
        return Ok(());
    }
    println!();

    // The `hidden` column only says anything when the retired targets are in
    // the table; without `--all` every cell in it would read `-`.
    print_table(&distro_table(shown.into_iter(), all));

    // Footer hints only when writing to a terminal, so redirected output is
    // the table alone.
    if tty {
        println!();
        let mut hints = vec![
            "`platform` is the name the binary index uses for the target; several",
            "distros can share one. `binaries` is off for the ones P3M serves but",
            "does not build for.",
        ];
        if all {
            hints.push("`hidden` marks targets P3M no longer advertises.");
        } else if hidden > 0 {
            hints.push("Pass --all to also list the targets P3M has retired.");
        }
        for hint in hints {
            if color {
                println!("{}", hint.dimmed());
            } else {
                println!("{}", hint);
            }
        }
    }

    Ok(())
}
