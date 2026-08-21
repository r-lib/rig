use std::error::Error;
use std::io::IsTerminal;

use clap::ArgMatches;
use tabular::*;

use crate::download::probe_urls_;
use crate::platform::{detect_platform, platform_to_pkg_type};
use crate::repos::configured::configured_repos;
use crate::repos::cranlike_metadata::{cranlike_urls, minor_r_version, package_type_to_path};
use crate::repositories::RepoFileEntry;

/// One row of `rig repos status`, and the shape of its `--json` output.
#[derive(serde::Serialize)]
struct RepoStatus {
    name: String,
    description: String,
    url: String,
    default: bool,
    /// Package types the R installation's `repositories` file declares.
    types: Vec<&'static str>,
    /// The repository serves prebuilt Linux binaries as source packages.
    linux_binaries: bool,
    /// The `PACKAGES` index that was probed.
    probed_url: String,
    /// Package type of the probed index, `source` or an R binary type.
    pkg_type: String,
    /// `true` if the index for this platform was missing (404) and the source
    /// index was probed instead.
    source_only: bool,
    /// HTTP status of the probe, `null` if there was no response.
    status: Option<u16>,
    /// Why there was no response.
    error: Option<String>,
    /// Time to the response headers, `null` if there was no response.
    ping_ms: Option<u128>,
    /// `Last-Modified` of the index, verbatim.
    last_modified: Option<String>,
}

pub fn sc_repos_status(
    args: &ArgMatches,
    libargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let rver = args.get_one::<String>("r-version").map(|x| x.as_str());
    let all = args.get_flag("all");

    // Always resolved: a URL with `%v` / `%bm` in it cannot be probed.
    let cfg = configured_repos(rver, all, true)?;
    let numver = cfg.numeric_version()?;
    let mut platform = detect_platform()?;
    if let Some(arch) = install_arch(&cfg.rver) {
        // An x86_64 R on an arm64 Mac uses the x86_64 binaries, not the
        // machine's.
        platform.arch = arch.to_string();
    }

    // The index R itself would use here, so that the ping is the latency the
    // user actually pays when installing a package. `platform_to_pkg_type`
    // has no answer on Linux, where P3M serves binaries as source packages
    // from a distro-specific URL.
    let pkg_type = match platform_to_pkg_type(&platform, &numver) {
        Some(pt) => match minor_r_version(&numver) {
            Ok(minor) => (pt, minor),
            Err(_) => ("source".to_string(), String::new()),
        },
        None => ("source".to_string(), String::new()),
    };
    let (pkg_type, minor) = pkg_type;
    let path = package_type_to_path(&pkg_type, &minor)?;
    let source_path = package_type_to_path("source", &minor)?;

    let urls: Vec<String> = cfg
        .repos
        .iter()
        .map(|r| cranlike_urls(&r.url, &path)[0].clone())
        .collect();
    let mut probes = probe_urls_(&urls);

    // A repository that has no index for this platform is not broken, it just
    // has source packages only; report those from the source index.
    let mut source_only = vec![false; probes.len()];
    if pkg_type != "source" {
        let retry: Vec<usize> = probes
            .iter()
            .enumerate()
            .filter(|(_, p)| p.status == Some(404))
            .map(|(i, _)| i)
            .collect();
        if !retry.is_empty() {
            let retry_urls: Vec<String> = retry
                .iter()
                .map(|&i| cranlike_urls(&cfg.repos[i].url, &source_path)[0].clone())
                .collect();
            for (&i, probe) in retry.iter().zip(probe_urls_(&retry_urls)) {
                if probe.status == Some(404) {
                    // Neither index is there, keep the original 404.
                    continue;
                }
                probes[i] = probe;
                source_only[i] = true;
            }
        }
    }

    // `--raw` is about what is printed; the probes needed the resolved URLs.
    let display_urls: Vec<String> = if args.get_flag("raw") {
        configured_repos(rver, all, false)?
            .repos
            .into_iter()
            .map(|r| r.url)
            .collect()
    } else {
        cfg.repos.iter().map(|r| r.url.clone()).collect()
    };

    let mut rows: Vec<RepoStatus> = vec![];
    for (i, repo) in cfg.repos.iter().enumerate() {
        let probe = &probes[i];
        rows.push(RepoStatus {
            name: repo.name.clone(),
            description: repo.description.clone(),
            url: display_urls
                .get(i)
                .cloned()
                .unwrap_or_else(|| repo.url.clone()),
            default: repo.default,
            types: declared_types(repo),
            linux_binaries: serves_linux_binaries(&repo.url),
            probed_url: probe.url.clone(),
            pkg_type: if source_only[i] {
                "source".to_string()
            } else {
                pkg_type.clone()
            },
            source_only: source_only[i],
            status: probe.status,
            error: probe.error.clone(),
            ping_ms: probe.status.map(|_| probe.elapsed_ms),
            last_modified: probe.last_modified.clone(),
        });
    }

    if args.get_flag("json") || libargs.get_flag("json") || mainargs.get_flag("json") {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        print_status(&rows, &cfg.rver, &numver, &path);
    }

    Ok(())
}

/// The package types the `repositories` file declares for a repository.
fn declared_types(entry: &RepoFileEntry) -> Vec<&'static str> {
    let mut types = vec![];
    if entry.source {
        types.push("source");
    }
    if entry.win_binary {
        types.push("win.binary");
    }
    if entry.mac_binary {
        types.push("mac.binary");
    }
    types
}

/// The architecture an R installation was built for, if its name says so.
///
/// On macOS rig names an installation `<version>-x86_64` or `<version>-arm64`
/// when it is not the machine's own architecture, and those installations use
/// the binaries of the architecture they were built for. `None` means "whatever
/// the machine is".
fn install_arch(rver: &str) -> Option<&'static str> {
    if rver.ends_with("-x86_64") {
        Some("x86_64")
    } else if rver.ends_with("-arm64") || rver.ends_with("-aarch64") {
        Some("aarch64")
    } else {
        None
    }
}

/// Whether the source packages of a repository are really prebuilt Linux
/// binaries.
///
/// P3M has no Linux entry in R's package-type vocabulary: it serves binaries as
/// source packages from a distro-specific URL, either `__linux__/<distro>` or
/// the portable `manylinux` one. There is no way to tell from the `PACKAGES`
/// index alone without downloading it, but the URL says so.
fn serves_linux_binaries(url: &str) -> bool {
    url.contains("/__linux__/") || url.contains("manylinux")
}

/// The `types` cell: the declared types, shortened, with a marker for a
/// repository whose source packages are Linux binaries.
fn types_cell(row: &RepoStatus) -> String {
    let mut cell = row
        .types
        .iter()
        .map(|t| match *t {
            "win.binary" => "win",
            "mac.binary" => "mac",
            other => other,
        })
        .collect::<Vec<&str>>()
        .join(", ");
    if cell.is_empty() {
        cell.push('-');
    }
    if row.linux_binaries {
        cell.push('*');
    }
    cell
}

/// The `status` cell: what the server said, or why it said nothing.
fn status_cell(row: &RepoStatus) -> String {
    match (row.status, &row.error) {
        (Some(status), _) if (200..300).contains(&status) => {
            if row.source_only {
                "source only".to_string()
            } else {
                "ok".to_string()
            }
        }
        (Some(status), _) => status.to_string(),
        (None, Some(err)) => err.clone(),
        (None, None) => "failed".to_string(),
    }
}

fn format_ping(ping_ms: Option<u128>) -> String {
    match ping_ms {
        Some(ms) => format!("{} ms", ms),
        None => "-".to_string(),
    }
}

/// `Last-Modified` as `YYYY-MM-DD HH:MM`, or verbatim if it is not the usual
/// `Wed, 20 Aug 2026 14:03:11 GMT` shape.
fn format_modified(last_modified: Option<&str>) -> String {
    let hdr = match last_modified {
        Some(h) => h.trim(),
        None => return "-".to_string(),
    };

    let parts: Vec<&str> = hdr.split_whitespace().collect();
    if parts.len() < 5 {
        return hdr.to_string();
    }
    let month = match month_number(parts[2]) {
        Some(m) => m,
        None => return hdr.to_string(),
    };
    let day: u32 = match parts[1].parse() {
        Ok(d) => d,
        Err(_) => return hdr.to_string(),
    };
    let year = parts[3];
    if year.len() != 4 || !year.chars().all(|c| c.is_ascii_digit()) {
        return hdr.to_string();
    }
    let time: Vec<&str> = parts[4].split(':').collect();
    if time.len() != 3 {
        return hdr.to_string();
    }

    format!("{}-{:02}-{:02} {}:{}", year, month, day, time[0], time[1])
}

fn month_number(month: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    MONTHS
        .iter()
        .position(|m| *m == month)
        .map(|i| i as u32 + 1)
}

fn print_status(rows: &[RepoStatus], rver: &str, numver: &str, path: &str) {
    use owo_colors::OwoColorize;

    let tty = std::io::stdout().is_terminal();
    let color = tty && std::env::var_os("NO_COLOR").is_none();

    // -- Header ------------------------------------------------------------
    let count = rows.len();
    let repo_word = if count == 1 {
        "repository"
    } else {
        "repositories"
    };
    let rname = if rver == numver {
        format!("R {}", numver)
    } else {
        format!("R {} ({})", numver, rver)
    };
    if color {
        println!(
            "{} {} of {}, index {}",
            count.cyan().bold(),
            repo_word,
            rname,
            path
        );
    } else {
        println!("{} {} of {}, index {}", count, repo_word, rname, path);
    }
    if count == 0 {
        return;
    }
    println!();

    // -- Table -------------------------------------------------------------
    let mut tab: Table = Table::new("{:<}   {:>}   {:<}   {:<}   {:<}   {:<}");
    tab.add_row(row!("name", "ping", "status", "types", "updated", "url"));
    tab.add_heading(
        "-----------------------------------------------------------------------------",
    );
    for row in rows {
        tab.add_row(row!(
            &row.name,
            format_ping(row.ping_ms),
            status_cell(row),
            types_cell(row),
            format_modified(row.last_modified.as_deref()),
            &row.url
        ));
    }
    print!("{}", tab);

    // -- Footer ------------------------------------------------------------
    // Explanations only, so they are skipped when the output is redirected and
    // scripts and saved listings get the table alone.
    if tty {
        let mut hints = vec![
            "`ping` is the time the repository took to answer for the index above.".to_string(),
        ];
        hints
            .push("`types` is what the R installation's `repositories` file declares.".to_string());
        if rows.iter().any(|r| r.source_only) {
            hints.push(
                "`source only`: this repository has no index for this platform and R version."
                    .to_string(),
            );
        }
        if rows.iter().any(|r| r.linux_binaries) {
            hints.push(
                "`*`: the source packages of this repository are prebuilt Linux binaries."
                    .to_string(),
            );
        }
        println!();
        for hint in hints {
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

    fn entry(source: bool, win_binary: bool, mac_binary: bool) -> RepoFileEntry {
        RepoFileEntry {
            name: "CRAN".to_string(),
            description: "CRAN".to_string(),
            url: "https://cloud.r-project.org".to_string(),
            default: true,
            source,
            win_binary,
            mac_binary,
        }
    }

    fn row(status: Option<u16>, error: Option<&str>, source_only: bool) -> RepoStatus {
        RepoStatus {
            name: "CRAN".to_string(),
            description: "CRAN".to_string(),
            url: "https://cloud.r-project.org".to_string(),
            default: true,
            types: vec!["source", "win.binary", "mac.binary"],
            linux_binaries: false,
            probed_url: "https://cloud.r-project.org/src/contrib/PACKAGES.gz".to_string(),
            pkg_type: "source".to_string(),
            source_only,
            status,
            error: error.map(|e| e.to_string()),
            ping_ms: status.map(|_| 42),
            last_modified: None,
        }
    }

    #[test]
    fn declared_types_covers_every_combination() {
        assert_eq!(
            declared_types(&entry(true, true, true)),
            vec!["source", "win.binary", "mac.binary"]
        );
        assert_eq!(
            declared_types(&entry(true, false, true)),
            vec!["source", "mac.binary"]
        );
        assert_eq!(
            declared_types(&entry(true, true, false)),
            vec!["source", "win.binary"]
        );
        assert_eq!(declared_types(&entry(true, false, false)), vec!["source"]);
        assert_eq!(
            declared_types(&entry(false, true, true)),
            vec!["win.binary", "mac.binary"]
        );
        assert_eq!(
            declared_types(&entry(false, false, true)),
            vec!["mac.binary"]
        );
        assert_eq!(
            declared_types(&entry(false, true, false)),
            vec!["win.binary"]
        );
        assert!(declared_types(&entry(false, false, false)).is_empty());
    }

    #[test]
    fn linux_binary_repos_are_recognized_from_the_url() {
        assert!(serves_linux_binaries(
            "https://packagemanager.posit.co/cran/__linux__/jammy/latest"
        ));
        assert!(serves_linux_binaries(
            "https://packagemanager.posit.co/cran/__linux__/manylinux_2_28/latest"
        ));
        assert!(!serves_linux_binaries("https://cloud.r-project.org"));
        assert!(!serves_linux_binaries("https://example.com/linux/cran"));
    }

    #[test]
    fn install_arch_comes_from_the_installation_name() {
        assert_eq!(install_arch("4.5.1-x86_64"), Some("x86_64"));
        assert_eq!(install_arch("4.6-arm64"), Some("aarch64"));
        assert_eq!(install_arch("4.6"), None);
        assert_eq!(install_arch("devel"), None);
    }

    #[test]
    fn types_cell_shortens_and_marks() {
        let mut r = row(Some(200), None, false);
        assert_eq!(types_cell(&r), "source, win, mac");
        r.linux_binaries = true;
        assert_eq!(types_cell(&r), "source, win, mac*");
        r.linux_binaries = false;
        r.types = vec![];
        assert_eq!(types_cell(&r), "-");
    }

    #[test]
    fn status_cell_reports_status_or_error() {
        assert_eq!(status_cell(&row(Some(200), None, false)), "ok");
        assert_eq!(status_cell(&row(Some(206), None, false)), "ok");
        assert_eq!(status_cell(&row(Some(200), None, true)), "source only");
        assert_eq!(status_cell(&row(Some(404), None, false)), "404");
        assert_eq!(status_cell(&row(Some(403), None, false)), "403");
        assert_eq!(
            status_cell(&row(None, Some("timeout"), false)),
            "timeout".to_string()
        );
        assert_eq!(status_cell(&row(None, None, false)), "failed");
    }

    #[test]
    fn ping_is_missing_without_a_response() {
        assert_eq!(format_ping(Some(41)), "41 ms");
        assert_eq!(format_ping(None), "-");
    }

    #[test]
    fn last_modified_is_shortened_or_kept_verbatim() {
        assert_eq!(
            format_modified(Some("Wed, 20 Aug 2026 14:03:11 GMT")),
            "2026-08-20 14:03"
        );
        assert_eq!(
            format_modified(Some("Sun, 03 May 2026 04:05:06 GMT")),
            "2026-05-03 04:05"
        );
        assert_eq!(format_modified(None), "-");
        // Not the usual shape, so it is passed through.
        assert_eq!(format_modified(Some("yesterday")), "yesterday");
        assert_eq!(
            format_modified(Some("Wed, 20 Foo 2026 14:03:11 GMT")),
            "Wed, 20 Foo 2026 14:03:11 GMT"
        );
        assert_eq!(
            format_modified(Some("Wed, 20 Aug 26 14:03:11 GMT")),
            "Wed, 20 Aug 26 14:03:11 GMT"
        );
        assert_eq!(
            format_modified(Some("Wed, 20 Aug 2026 1403 GMT")),
            "Wed, 20 Aug 2026 1403 GMT"
        );
    }

    #[test]
    fn probed_index_paths() {
        assert_eq!(
            cranlike_urls(
                "https://cloud.r-project.org",
                &package_type_to_path("mac.binary.big-sur-arm64", "4.6").unwrap()
            )[0],
            "https://cloud.r-project.org/bin/macosx/big-sur-arm64/contrib/4.6/PACKAGES.gz"
        );
        assert_eq!(
            cranlike_urls(
                "https://cloud.r-project.org",
                &package_type_to_path("source", "").unwrap()
            )[0],
            "https://cloud.r-project.org/src/contrib/PACKAGES.gz"
        );
    }
}
