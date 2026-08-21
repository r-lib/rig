//! `rig ppm builds`: every published build of one package.
//!
//! The data is the per-package binary index described in
//! `crate::repos::binaries`. Note that it comes from rig's own index host, not
//! from the instance `rig ppm url` names: P3M serves no such file.

use std::error::Error;
use std::io::IsTerminal;

use clap::ArgMatches;
use owo_colors::OwoColorize;
use simple_error::bail;
use tabular::{row, Table};

use crate::ppm::{print_table, use_color, want_json};
use crate::repos::binaries::{
    binary_index_url, load_binary_index, validate_package_name, BinaryIndex,
};

/// One `pkg@version=sha256` entry of a build's `linkingto`.
#[derive(Debug, serde::Serialize)]
struct BuildLinkingTo<'a> {
    package: &'a str,
    version: &'a str,
    /// Upstream CRAN identity hash, like [`BuildRow::sha256`].
    sha256: &'a str,
}

/// One row of `rig ppm builds`, and the shape of its `--json` output.
#[derive(Debug, serde::Serialize)]
struct BuildRow<'a> {
    package: &'a str,
    version: &'a str,
    /// `source`, `macos`, `windows`, or a Linux codename such as `jammy`.
    platform: &'a str,
    /// `*` on source rows.
    arch: &'a str,
    /// `*` on source rows.
    r_version: &'a str,
    /// Hash of the upstream CRAN source tarball, *not* a checksum of `url`, and
    /// so no use for verifying a download. See the `crate::repos::binaries`
    /// module docs.
    sha256: &'a str,
    url: &'a str,
    /// What the build was compiled against. Empty on source rows and for
    /// packages without `LinkingTo:`.
    linkingto: Vec<BuildLinkingTo<'a>>,
}

pub fn sc_ppm_builds(
    args: &ArgMatches,
    ppmargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let package = args.get_one::<String>("package").unwrap();
    // Before the name is echoed into a URL or a cache path.
    validate_package_name(package)?;

    let cached = match load_binary_index(package, None)? {
        Some(cached) => cached,
        None => bail!(
            "No P3M build index for package '{}' ({})",
            package,
            binary_index_url(package)
        ),
    };
    let index = &cached.index;

    let version = args.get_one::<String>("version").map(|v| v.as_str());
    let rows = build_rows(index, package, version)?;

    if want_json(args, ppmargs, mainargs) {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        print_builds(&rows, package, version, index.versions().len());
    }

    Ok(())
}

/// The rows to report, oldest version first.
///
/// Split out of [`sc_ppm_builds`] so it can be tested against the index
/// fixtures without a network.
fn build_rows<'a>(
    index: &'a BinaryIndex,
    package: &'a str,
    version: Option<&str>,
) -> Result<Vec<BuildRow<'a>>, Box<dyn Error>> {
    let versions: Vec<&'a str> = match version {
        Some(wanted) => {
            if index.version_index(wanted).is_none() {
                // Naming the range turns a typo into an obvious diagnosis,
                // where an empty table would not.
                let known = index.versions();
                match (known.first(), known.last()) {
                    (Some(oldest), Some(newest)) => bail!(
                        "No version '{}' of {} in the P3M build index (it has {} versions, {} to {})",
                        wanted,
                        package,
                        known.len(),
                        oldest,
                        newest
                    ),
                    _ => bail!("The P3M build index for {} is empty", package),
                }
            }
            vec![index
                .versions()
                .iter()
                .find(|v| v.as_str() == wanted)
                .unwrap()
                .as_str()]
        }
        // `versions()` is numerically ascending, which is the order to print:
        // the newest version ends up last, next to the prompt, where a long
        // listing leaves it on screen.
        None => index.versions().iter().map(|v| v.as_str()).collect(),
    };

    let mut rows: Vec<BuildRow<'a>> = vec![];
    for version in versions {
        for row in index.rows_for_version(version) {
            rows.push(BuildRow {
                package,
                version: row.version().original,
                platform: row.platform(),
                arch: row.arch(),
                r_version: row.r_version(),
                sha256: row.sha256(),
                url: row.url(),
                linkingto: row
                    .linkingto()
                    .map(|l| BuildLinkingTo {
                        package: l.package,
                        version: l.version,
                        sha256: l.sha256,
                    })
                    .collect(),
            });
        }
    }
    Ok(rows)
}

/// `cli@3.6.6, BH@1.87.0-1`, or `-` when the build links to nothing.
fn linkingto_cell(row: &BuildRow) -> String {
    if row.linkingto.is_empty() {
        return "-".to_string();
    }
    row.linkingto
        .iter()
        .map(|l| format!("{}@{}", l.package, l.version))
        .collect::<Vec<_>>()
        .join(", ")
}

fn print_builds(rows: &[BuildRow], package: &str, version: Option<&str>, nversions: usize) {
    let tty = std::io::stdout().is_terminal();
    let color = use_color();

    // -- Header ------------------------------------------------------------
    let count = rows.len();
    let build_word = if count == 1 { "build" } else { "builds" };
    let what = match version {
        Some(version) => format!("{} {}", package, version),
        None => package.to_string(),
    };
    let head = if color {
        format!("{} {} of {}", count.cyan().bold(), build_word, what)
    } else {
        format!("{} {} of {}", count, build_word, what)
    };
    if version.is_some() {
        println!("{}", head);
    } else {
        let version_word = if nversions == 1 {
            "version"
        } else {
            "versions"
        };
        let tag = format!("({} {})", nversions, version_word);
        println!(
            "{} {}",
            head,
            if color { tag.dimmed().to_string() } else { tag }
        );
    }
    if count == 0 {
        return;
    }
    println!();

    // -- Table -------------------------------------------------------------
    // `linkingto` is second-to-last on purpose: it is the widest variable
    // column, and with only `url` after it its padding cannot shift anything
    // else. It has to be here at all because rows that share
    // `(version, platform, arch, r_version)` differ in nothing else, and would
    // otherwise look like duplicates.
    let mut tab: Table = Table::new("{:<}   {:<}   {:<}   {:<}   {:<}   {:<}");
    tab.add_row(row!(
        "version",
        "platform",
        "arch",
        "r_version",
        "linkingto",
        "url"
    ));
    for row in rows {
        tab.add_row(row!(
            row.version,
            row.platform,
            row.arch,
            row.r_version,
            linkingto_cell(row),
            row.url
        ));
    }
    print_table(&tab);

    if tty {
        println!();
        for hint in [
            "Rows that agree on version, platform, arch and r_version are different",
            "builds of the same version, told apart by `linkingto`. Platform `source`",
            "is the CRAN source tarball rather than a build.",
        ] {
            if color {
                println!("{}", hint.dimmed());
            } else {
                println!("{}", hint);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repos::binaries::{blob, parse_binaries_tsv};
    use std::fs;
    use std::path::PathBuf;

    fn index(name: &str, package: &str) -> BinaryIndex {
        let bytes = fs::read(PathBuf::from("tests/fixtures/binaries").join(name)).unwrap();
        let rows = parse_binaries_tsv(&bytes).unwrap();
        let blob = blob::build(package, &rows).unwrap();
        BinaryIndex::open_blob(&blob).unwrap()
    }

    #[test]
    fn rows_are_oldest_version_first() {
        let index = index("simple.tsv", "testpkg");
        let rows = build_rows(&index, "testpkg", None).unwrap();
        assert_eq!(rows.len(), index.num_rows());
        // The newest version goes last, so it is what a long listing leaves on
        // screen.
        let newest = index.latest_version().unwrap().original.to_string();
        assert_eq!(rows.last().unwrap().version, newest);
        assert_ne!(rows[0].version, newest);
        assert_eq!(rows[0].version, *index.versions().first().unwrap());
        assert!(rows.iter().all(|r| r.package == "testpkg"));
    }

    #[test]
    fn version_filter_keeps_only_that_version() {
        let index = index("simple.tsv", "testpkg");
        let all = build_rows(&index, "testpkg", None).unwrap();
        let wanted = index.versions().first().unwrap().clone();
        let rows = build_rows(&index, "testpkg", Some(&wanted)).unwrap();
        assert!(!rows.is_empty());
        assert!(rows.len() < all.len());
        assert!(rows.iter().all(|r| r.version == wanted));
    }

    #[test]
    fn unknown_version_is_an_error_naming_the_range() {
        let index = index("simple.tsv", "testpkg");
        let err = build_rows(&index, "testpkg", Some("9.9.9"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("9.9.9"), "{}", err);
        assert!(err.contains(index.versions().last().unwrap()), "{}", err);
    }

    #[test]
    fn linkingto_cell_formats_or_dashes() {
        let index = index("dplyr.tsv.zst", "dplyr");
        // 0.7.4 rather than a recent version: dplyr dropped `LinkingTo:` along
        // the way, so its newest builds link to nothing.
        let rows = build_rows(&index, "dplyr", Some("0.7.4")).unwrap();

        // Source rows never link to anything.
        let source = rows.iter().find(|r| r.platform == "source").unwrap();
        assert!(source.linkingto.is_empty());
        assert_eq!(linkingto_cell(source), "-");

        let linked = rows.iter().find(|r| !r.linkingto.is_empty()).unwrap();
        let cell = linkingto_cell(linked);
        assert!(cell.contains("plogr@"), "{}", cell);
        // The hashes are for `--json` only; the cell is names and versions.
        assert!(!cell.contains('='), "{}", cell);
    }

    /// The reason the `linkingto` column exists: without it these rows are
    /// indistinguishable.
    #[test]
    fn same_target_can_have_several_builds() {
        let index = index("dplyr.tsv.zst", "dplyr");
        let rows = build_rows(&index, "dplyr", Some("0.7.4")).unwrap();
        let same: Vec<&BuildRow> = rows
            .iter()
            .filter(|r| r.platform == "xenial" && r.arch == "x86_64" && r.r_version == "3.4")
            .collect();
        assert!(
            same.len() > 1,
            "expected several builds, got {}",
            same.len()
        );
        let cells: Vec<String> = same.iter().map(|r| linkingto_cell(r)).collect();
        let mut unique = cells.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            cells.len(),
            "linkingto did not tell them apart"
        );
    }
}
