//! `rig ppm build-log`: P3M's build log for one package binary.
//!
//! P3M serves this as a fixed-layout HTML page rather than JSON (see
//! `get_binary_build_logs.go` in the P3M source, which builds the page with a
//! `toHTML()` method), so [`parse_build_log_html`] picks the fields back out
//! of that page instead of asking P3M for structured data it does not offer.

use std::error::Error;

use clap::ArgMatches;
use owo_colors::OwoColorize;
use simple_error::bail;

use crate::common::get_default_r_version;
use crate::platform::detect_platform;
use crate::ppm::{use_color, want_json};
use crate::repos::binaries::{ppm_url, validate_package_name, PpmStatus};
use crate::rversion::OsVersion;
use crate::textfmt::print_field;

/// Width of the label column in the scalar block, matching `rig ppm status`.
const LABEL_WIDTH: usize = 12;

#[derive(Debug, serde::Serialize)]
struct BuildLog {
    name: String,
    version: String,
    platform: String,
    exit_code: i64,
    started_at: String,
    ended_at: String,
    duration_seconds: i64,
    /// One entry per line of the page's "System dependencies" section, empty
    /// when it read "None".
    system_deps: Vec<String>,
    stdout: String,
    /// `Some` only for logs built before P3M's builder merged stdout and
    /// stderr into one interleaved stream: see the doc comment on P3M's
    /// `buildLog.Stderr` field.
    stderr: Option<String>,
}

pub fn sc_ppm_build_log(
    args: &ArgMatches,
    ppmargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let package = args.get_one::<String>("package").unwrap();
    // Before the name is echoed into a URL.
    validate_package_name(package)?;

    let explicit_platform = args.get_one::<String>("platform").map(|s| s.as_str());
    let explicit_arch = args.get_one::<String>("arch").map(|s| s.as_str());
    let (platform, arch) = match (explicit_platform, explicit_arch) {
        (Some(platform), Some(arch)) => (platform.to_string(), arch.to_string()),
        (platform, arch) => {
            let (detected_platform, detected_arch) = detect_current_ppm_target()?;
            (
                platform.map(str::to_string).unwrap_or(detected_platform),
                arch.map(str::to_string).unwrap_or(detected_arch),
            )
        }
    };
    let r_version = match args.get_one::<String>("r-version") {
        Some(r_version) => r_version.clone(),
        None => get_default_r_version()?.ok_or(
            "No default R version is set; pass --r-version or run `rig default <version>`",
        )?,
    };
    // P3M only accepts a minor R version (e.g. `4.6`), not a full patch
    // version like `rig default` reports (`4.6.1`).
    let r_version = minor_r_version(&r_version);
    let version = args.get_one::<String>("version").map(|v| v.as_str());

    let url = build_log_url(&ppm_url(), package, &platform, &r_version, &arch, version)?;
    let html = match fetch_build_log_(&url)? {
        Some(html) => html,
        None => bail!("No P3M build log for '{}' at {}", package, url),
    };
    let log = parse_build_log_html(&html)?;

    if want_json(args, ppmargs, mainargs) {
        println!("{}", serde_json::to_string_pretty(&log)?);
    } else {
        print_build_log(&log);
    }

    Ok(())
}

/// The current machine's platform and architecture, in the vocabulary P3M
/// uses for `--platform`/`--arch` (e.g. `("jammy", "x86_64")`,
/// `("macos", "arm64")`), for when either is not given explicitly.
fn detect_current_ppm_target() -> Result<(String, String), Box<dyn Error>> {
    let os = detect_platform()?;
    let status = PpmStatus::load(None)?;
    status
        .ppm_platform(&os)
        .ok_or_else(|| format!("P3M has no build target for the current platform ({}); pass --platform and --arch explicitly", platform_label(&os)).into())
}

/// `4.6.1` -> `4.6`: P3M's `r_version` query param only accepts a minor
/// version, and rejects a full patch version with a 400.
fn minor_r_version(version: &str) -> String {
    match version.split_once('.') {
        Some((major, rest)) => match rest.split_once('.') {
            Some((minor, _patch)) => format!("{}.{}", major, minor),
            None => version.to_string(),
        },
        None => version.to_string(),
    }
}

/// A human-readable name for an [`OsVersion`], for the error above.
fn platform_label(os: &OsVersion) -> String {
    match (&os.distro, &os.version) {
        (Some(distro), Some(version)) => format!("{} {}", distro, version),
        _ => os.os.clone(),
    }
}

/// Build the log URL, letting [`reqwest::Url`] handle percent-encoding of the
/// query values instead of interpolating them into the string by hand.
fn build_log_url(
    base: &str,
    package: &str,
    distribution: &str,
    r_version: &str,
    arch: &str,
    version: Option<&str>,
) -> Result<reqwest::Url, Box<dyn Error>> {
    let mut url = reqwest::Url::parse(&format!(
        "{}/__api__/repos/cran/packages/{}/binaries/logs",
        base.trim_end_matches('/'),
        package
    ))?;
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("distribution", distribution)
            .append_pair("r_version", r_version)
            .append_pair("arch", arch);
        if let Some(version) = version {
            query.append_pair("version", version);
        }
    }
    Ok(url)
}

async fn fetch_build_log(url: &reqwest::Url) -> Result<Option<String>, Box<dyn Error>> {
    let resp = reqwest::Client::new().get(url.clone()).send().await?;
    // P3M's CDN reports a missing log as either status, per its own comment.
    if resp.status() == reqwest::StatusCode::NOT_FOUND
        || resp.status() == reqwest::StatusCode::FORBIDDEN
    {
        return Ok(None);
    }
    if !resp.status().is_success() {
        bail!("Failed to fetch build log: {}", resp.status());
    }
    Ok(Some(resp.text().await?))
}

#[tokio::main]
async fn fetch_build_log_(url: &reqwest::Url) -> Result<Option<String>, Box<dyn Error>> {
    fetch_build_log(url).await
}

/// Pick a field out of P3M's generated build-log page.
///
/// Every field is a `<h2>heading</h2>` immediately followed by either a
/// `<p>...</p>` or a `<pre>...</pre>`, so this looks for the heading and reads
/// whichever of the two follows it.
fn html_section(html: &str, heading: &str) -> Option<String> {
    let heading_tag = format!("<h2>{}</h2>", heading);
    let start = html.find(&heading_tag)? + heading_tag.len();
    let rest = html[start..].trim_start();
    for (open, close) in [("<p>", "</p>"), ("<pre>", "</pre>")] {
        if let Some(inner) = rest.strip_prefix(open) {
            let end = inner.find(close)?;
            return Some(html_unescape(&inner[..end]));
        }
    }
    None
}

fn html_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&#34;", "\"")
        .replace("&#39;", "'")
        // Has to run last: the other replacements would themselves get
        // unescaped again if this ran first.
        .replace("&amp;", "&")
}

fn parse_build_log_html(html: &str) -> Result<BuildLog, Box<dyn Error>> {
    let field = |heading: &str| -> Result<String, Box<dyn Error>> {
        html_section(html, heading)
            .ok_or_else(|| format!("Build log page has no '{}' section", heading).into())
    };

    let name = field("Name")?;
    let version = field("Version")?;
    let platform = field("Platform")?;
    let exit_code: i64 = field("Exit code")?
        .parse()
        .map_err(|_| "Build log page has a non-numeric exit code")?;
    let started_at = field("Started")?;
    let ended_at = field("Ended")?;
    let duration_text = field("Duration")?;
    let duration_seconds: i64 = duration_text
        .split_whitespace()
        .next()
        .and_then(|n| n.parse().ok())
        .ok_or("Build log page has an unparseable duration")?;

    let system_deps_text = field("System dependencies")?;
    let system_deps = if system_deps_text == "None" {
        vec![]
    } else {
        system_deps_text.lines().map(str::to_string).collect()
    };

    // Older logs kept stdout and stderr apart; current ones interleave both
    // into one "Output" section. See [`BuildLog::stderr`].
    let (stdout, stderr) = if html.contains("<h2>Output (stdout)</h2>") {
        (
            output_text(field("Output (stdout)")?),
            Some(output_text(field("Output (stderr)")?)),
        )
    } else {
        (output_text(field("Output")?), None)
    };

    Ok(BuildLog {
        name,
        version,
        platform,
        exit_code,
        started_at,
        ended_at,
        duration_seconds,
        system_deps,
        stdout,
        stderr,
    })
}

/// P3M prints `(empty)` in place of an empty stream; undo that for the parsed
/// value so `--json` reports what the build actually produced.
fn output_text(text: String) -> String {
    if text == "(empty)" {
        String::new()
    } else {
        text
    }
}

fn print_build_log(log: &BuildLog) {
    let color = use_color();

    let head = format!("{} {}", log.name, log.version);
    println!("{}", if color { head.bold().to_string() } else { head });
    println!();

    for (label, value) in [
        ("platform", log.platform.clone()),
        ("exit_code", log.exit_code.to_string()),
        ("started", log.started_at.clone()),
        ("ended", log.ended_at.clone()),
        ("duration", format!("{} seconds", log.duration_seconds)),
    ] {
        print_field(label, &value, LABEL_WIDTH, color);
    }

    println!();
    section_title("System dependencies", color);
    if log.system_deps.is_empty() {
        println!("None");
    } else {
        for dep in &log.system_deps {
            println!("{}", dep);
        }
    }

    println!();
    if let Some(stderr) = &log.stderr {
        section_title("Output (stdout)", color);
        print_output(&log.stdout);
        println!();
        section_title("Output (stderr)", color);
        print_output(stderr);
    } else {
        section_title("Output", color);
        print_output(&log.stdout);
    }
}

fn section_title(title: &str, color: bool) {
    if color {
        println!("{}", title.bold());
    } else {
        println!("{}", title);
    }
}

fn print_output(text: &str) {
    if text.is_empty() {
        println!("(empty)");
    } else {
        println!("{}", text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minor_r_version_strips_the_patch_component() {
        assert_eq!(minor_r_version("4.6.1"), "4.6");
        assert_eq!(minor_r_version("4.6"), "4.6");
        assert_eq!(minor_r_version("devel"), "devel");
    }

    fn page(body: &str) -> String {
        format!(
            "<!DOCTYPE html>\n<html>\n<head>\n<title>Package Build Log</title>\n</head>\n<body>\n<h1>Package Build Log</h1>\n{}\n</body>\n</html>",
            body
        )
    }

    #[test]
    fn parses_a_merged_stdout_stderr_log() {
        let html = page(
            "<h2>Name</h2>\n<p>dplyr</p>\n\
             <h2>Version</h2>\n<p>1.1.4</p>\n\
             <h2>Platform</h2>\n<p>4.5-jammy-x86_64</p>\n\
             <h2>Exit code</h2>\n<p>0</p>\n\
             <h2>Started</h2>\n<p>2026-01-02T03:04:05Z</p>\n\
             <h2>Ended</h2>\n<p>2026-01-02T03:05:07Z</p>\n\
             <h2>Duration</h2>\n<p>62 seconds</p>\n\
             <h2>System dependencies</h2>\n<pre>libicu-dev\nzlib1g-dev</pre>\n\
             <h2>Output</h2>\n<pre>* installing *source* package</pre>\n",
        );
        let log = parse_build_log_html(&html).unwrap();
        assert_eq!(log.name, "dplyr");
        assert_eq!(log.version, "1.1.4");
        assert_eq!(log.platform, "4.5-jammy-x86_64");
        assert_eq!(log.exit_code, 0);
        assert_eq!(log.duration_seconds, 62);
        assert_eq!(log.system_deps, vec!["libicu-dev", "zlib1g-dev"]);
        assert_eq!(log.stdout, "* installing *source* package");
        assert_eq!(log.stderr, None);
    }

    #[test]
    fn parses_a_legacy_split_stdout_stderr_log() {
        let html = page(
            "<h2>Name</h2>\n<p>foo</p>\n\
             <h2>Version</h2>\n<p>1.0.0</p>\n\
             <h2>Platform</h2>\n<p>4.4-focal-x86_64</p>\n\
             <h2>Exit code</h2>\n<p>1</p>\n\
             <h2>Started</h2>\n<p>2020-01-01T00:00:00Z</p>\n\
             <h2>Ended</h2>\n<p>2020-01-01T00:01:00Z</p>\n\
             <h2>Duration</h2>\n<p>60 seconds</p>\n\
             <h2>System dependencies</h2>\n<p>None</p>\n\
             <h2>Output (stdout)</h2>\n<pre>building...</pre>\n\
             <h2>Output (stderr)</h2>\n<p>(empty)</p>\n",
        );
        let log = parse_build_log_html(&html).unwrap();
        assert_eq!(log.exit_code, 1);
        assert!(log.system_deps.is_empty());
        assert_eq!(log.stdout, "building...");
        assert_eq!(log.stderr, Some(String::new()));
    }

    #[test]
    fn empty_output_becomes_an_empty_string() {
        let html = page(
            "<h2>Name</h2>\n<p>bar</p>\n\
             <h2>Version</h2>\n<p>2.0.0</p>\n\
             <h2>Platform</h2>\n<p>4.6-macos-arm64</p>\n\
             <h2>Exit code</h2>\n<p>0</p>\n\
             <h2>Started</h2>\n<p>2026-02-02T00:00:00Z</p>\n\
             <h2>Ended</h2>\n<p>2026-02-02T00:00:05Z</p>\n\
             <h2>Duration</h2>\n<p>5 seconds</p>\n\
             <h2>System dependencies</h2>\n<p>None</p>\n\
             <h2>Output</h2>\n<p>(empty)</p>\n",
        );
        let log = parse_build_log_html(&html).unwrap();
        assert_eq!(log.stdout, "");
    }

    #[test]
    fn html_entities_are_unescaped() {
        let html = page(
            "<h2>Name</h2>\n<p>foo</p>\n\
             <h2>Version</h2>\n<p>1.0.0</p>\n\
             <h2>Platform</h2>\n<p>4.5-jammy-x86_64</p>\n\
             <h2>Exit code</h2>\n<p>1</p>\n\
             <h2>Started</h2>\n<p>2026-01-01T00:00:00Z</p>\n\
             <h2>Ended</h2>\n<p>2026-01-01T00:00:01Z</p>\n\
             <h2>Duration</h2>\n<p>1 seconds</p>\n\
             <h2>System dependencies</h2>\n<p>None</p>\n\
             <h2>Output</h2>\n<pre>a &lt;b&gt; &amp; c &#34;d&#34; &#39;e&#39;</pre>\n",
        );
        let log = parse_build_log_html(&html).unwrap();
        assert_eq!(log.stdout, "a <b> & c \"d\" 'e'");
    }

    #[test]
    fn build_log_url_encodes_query_values() {
        let url = build_log_url(
            "https://p3m.dev",
            "dplyr",
            "jammy",
            "4.5",
            "x86_64",
            Some("1.1 4"),
        )
        .unwrap();
        assert_eq!(
            url.as_str(),
            "https://p3m.dev/__api__/repos/cran/packages/dplyr/binaries/logs?distribution=jammy&r_version=4.5&arch=x86_64&version=1.1+4"
        );
    }
}
