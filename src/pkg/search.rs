//! `rig pkg search`: search CRAN packages, like `pkgsearch::pkg_search()`.
//!
//! The search itself runs on the pkgsearch web service
//! (<https://search.r-pkg.org>), an Elasticsearch instance that indexes the
//! DESCRIPTION files of all CRAN packages, plus their number of reverse
//! dependencies and downloads. We send the same query as pkgsearch, so the
//! ranking is the same, and print the results in pkgsearch's long or short
//! format.

use std::env;
use std::error::Error;
use std::io::IsTerminal;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::ArgMatches;
use lazy_static::lazy_static;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::utils::http_client;

const DEFAULT_SERVER: &str = "https://search.r-pkg.org";

/// Output width. rig does not detect the terminal width, and pkgsearch uses
/// the width of the R console, which is 80 by default.
const WIDTH: usize = 80;

/// Default number of results, if there is no `--size`. The long format
/// takes more lines per package, so it shows fewer.
fn default_size(short: bool) -> u32 {
    if short {
        20
    } else {
        8
    }
}

pub fn sc_pkg_search(
    args: &ArgMatches,
    pkgargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let json = args.get_flag("json") || pkgargs.get_flag("json") || mainargs.get_flag("json");
    let short = args.get_flag("short");
    let query: Vec<&str> = args
        .get_many::<String>("query")
        .unwrap_or_default()
        .map(|s| s.as_str())
        .collect();
    let query = query.join(" ");
    if query.trim().is_empty() {
        bail!("Search query is empty");
    }
    let from = *args.get_one::<u32>("from").unwrap_or(&1);
    let size = args
        .get_one::<u32>("size")
        .copied()
        .unwrap_or_else(|| default_size(short));
    let server = env::var("R_PKG_SEARCH_SERVER").unwrap_or_else(|_| DEFAULT_SERVER.to_string());

    let result = search(&server, &query, from, size)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }

    let color = std::io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none();
    let now = unix_now();
    let mut lines = if short {
        format_short(&result, now, color)
    } else {
        format_long(&result, now, color)
    };
    if let Some(next) = format_next(&result, short, color) {
        lines.push(String::new());
        lines.push(next);
    }
    let mut text = lines.join("\n");
    text.push('\n');
    crate::pager::page_text(&text);
    Ok(())
}

// ------------------------------------------------------------------------
// Query and response

/// The search results, as `rig pkg search --json` prints them.
#[derive(Debug, Serialize)]
struct SearchResult {
    query: String,
    from: u32,
    size: u32,
    total: u64,
    max_score: f64,
    /// Search time on the server, in milliseconds.
    took: u64,
    hits: Vec<SearchHit>,
}

#[derive(Debug, Serialize)]
struct SearchHit {
    score: f64,
    package: String,
    version: String,
    title: String,
    description: String,
    date: Option<String>,
    maintainer_name: String,
    maintainer_email: Option<String>,
    revdeps: u64,
    downloads_last_month: u64,
    license: Option<String>,
    url: Option<String>,
    bugreports: Option<String>,
}

#[derive(Deserialize)]
struct Response {
    #[serde(default)]
    took: u64,
    hits: Hits,
}

#[derive(Deserialize)]
struct Hits {
    total: Total,
    max_score: Option<f64>,
    #[serde(default)]
    hits: Vec<Hit>,
}

/// The search server reports the total as a number, while Elasticsearch
/// itself uses `{"value": N, ...}`. Accept both.
#[derive(Deserialize)]
#[serde(untagged)]
enum Total {
    Count(u64),
    Object { value: u64 },
}

#[derive(Deserialize)]
struct Hit {
    #[serde(rename = "_score")]
    score: Option<f64>,
    #[serde(rename = "_id")]
    id: String,
    #[serde(rename = "_source", default)]
    source: Source,
}

#[derive(Deserialize, Default)]
struct Source {
    #[serde(rename = "Version")]
    version: Option<String>,
    #[serde(rename = "Title")]
    title: Option<String>,
    #[serde(rename = "Description")]
    description: Option<String>,
    date: Option<String>,
    #[serde(rename = "Maintainer")]
    maintainer: Option<String>,
    revdeps: Option<u64>,
    downloads: Option<u64>,
    #[serde(rename = "License")]
    license: Option<String>,
    #[serde(rename = "URL")]
    url: Option<String>,
    #[serde(rename = "BugReports")]
    bugreports: Option<String>,
}

/// The Elasticsearch query, the same as pkgsearch's.
///
/// Every word may match any field (`must`), matching all words adds to the
/// score, matching the whole phrase adds more, and the score is multiplied
/// by the square root of the number of reverse dependencies.
fn build_query(query: &str) -> serde_json::Value {
    serde_json::json!({
        "query": {
            "function_score": {
                "functions": [
                    {
                        "field_value_factor": {
                            "field": "revdeps",
                            "modifier": "sqrt",
                            "factor": 1
                        }
                    }
                ],
                "query": {
                    "bool": {
                        "must": [
                            {
                                "multi_match": {
                                    "query": query,
                                    "type": "most_fields"
                                }
                            }
                        ],
                        "should": [
                            {
                                "multi_match": {
                                    "query": query,
                                    "fields": ["Title^10", "Description^2", "_all"],
                                    "type": "phrase",
                                    "analyzer": "english_and_synonyms",
                                    "boost": 10
                                }
                            },
                            {
                                "multi_match": {
                                    "query": query,
                                    "fields": [
                                        "Package^20",
                                        "Title^10",
                                        "Description^2",
                                        "Author^5",
                                        "Maintainer^6",
                                        "_all"
                                    ],
                                    "operator": "and",
                                    "analyzer": "english_and_synonyms",
                                    "boost": 5
                                }
                            }
                        ]
                    }
                }
            }
        }
    })
}

#[tokio::main]
async fn search(
    server: &str,
    query: &str,
    from: u32,
    size: u32,
) -> Result<SearchResult, Box<dyn Error>> {
    let url = format!(
        "{}/package/_search?from={}&size={}",
        server.trim_end_matches('/'),
        from.saturating_sub(1),
        size
    );
    let resp = http_client()
        .post(&url)
        .timeout(Duration::from_secs(60))
        .json(&build_query(query))
        .send()
        .await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        let reason = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| {
                v.pointer("/error/root_cause/0/reason")
                    .and_then(|r| r.as_str())
                    .map(|r| r.to_string())
            });
        match reason {
            Some(reason) => bail!("Search server failure ({}): {}", status, reason),
            None => bail!("Search server failure ({})", status),
        }
    }
    parse_response(&body, query, from, size)
}

fn parse_response(
    body: &str,
    query: &str,
    from: u32,
    size: u32,
) -> Result<SearchResult, Box<dyn Error>> {
    let resp: Response = serde_json::from_str(body)
        .map_err(|e| format!("Invalid response from search server: {}", e))?;
    let total = match resp.hits.total {
        Total::Count(n) => n,
        Total::Object { value } => value,
    };
    let hits = resp
        .hits
        .hits
        .into_iter()
        .map(|hit| {
            let src = hit.source;
            let maintainer = src.maintainer.unwrap_or_default();
            SearchHit {
                score: hit.score.unwrap_or(0.0),
                package: hit.id,
                version: src.version.unwrap_or_default(),
                title: src.title.unwrap_or_default(),
                description: src.description.unwrap_or_default(),
                date: src.date,
                maintainer_name: maintainer_name(&maintainer),
                maintainer_email: maintainer_email(&maintainer),
                revdeps: src.revdeps.unwrap_or(0),
                downloads_last_month: src.downloads.unwrap_or(1),
                license: src.license,
                url: src.url,
                bugreports: src.bugreports,
            }
        })
        .collect();
    Ok(SearchResult {
        query: query.to_string(),
        from,
        size,
        total,
        max_score: resp.hits.max_score.unwrap_or(0.0),
        took: resp.took,
        hits,
    })
}

lazy_static! {
    static ref RE_MAINT_EMAIL_TAIL: Regex = Regex::new(r"(?s)\s+<.*$").unwrap();
    static ref RE_MAINT_EMAIL: Regex = Regex::new(r"(?s)^.*<([^>]+)>.*$").unwrap();
}

fn maintainer_name(maintainer: &str) -> String {
    RE_MAINT_EMAIL_TAIL.replace(maintainer, "").to_string()
}

fn maintainer_email(maintainer: &str) -> Option<String> {
    RE_MAINT_EMAIL
        .captures(maintainer)
        .map(|c| c[1].to_string())
}

// ------------------------------------------------------------------------
// Dates

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Parse an ISO 8601 timestamp, e.g. `2024-06-21T20:16:33+00:00`, into
/// seconds since the Unix epoch. A missing time means midnight, a missing
/// time zone means UTC.
fn parse_iso_8601(s: &str) -> Option<i64> {
    let s = s.trim();
    let num = |a: usize, b: usize| -> Option<i64> {
        let part = s.get(a..b)?;
        if part.bytes().all(|c| c.is_ascii_digit()) {
            part.parse().ok()
        } else {
            None
        }
    };
    let year = num(0, 4)?;
    if s.get(4..5)? != "-" || s.get(7..8)? != "-" {
        return None;
    }
    let month = num(5, 7)?;
    let day = num(8, 10)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut secs = days_from_civil(year, month, day) * 86400;
    let rest = &s[10..];
    if rest.is_empty() {
        return Some(secs);
    }
    if !rest.starts_with('T') && !rest.starts_with(' ') {
        return None;
    }
    let hour = num(11, 13)?;
    if s.get(13..14)? != ":" {
        return None;
    }
    let min = num(14, 16)?;
    let mut pos = 16;
    let mut sec = 0;
    if s.get(16..17) == Some(":") {
        sec = num(17, 19)?;
        pos = 19;
    }
    secs += hour * 3600 + min * 60 + sec;
    // Fractional seconds are ignored.
    let mut rest = &s[pos..];
    if let Some(frac) = rest.strip_prefix('.') {
        let digits = frac.bytes().take_while(|c| c.is_ascii_digit()).count();
        rest = &frac[digits..];
    }
    match rest {
        "" | "Z" | "z" => Some(secs),
        _ => {
            let sign = match &rest[..1] {
                "+" => 1,
                "-" => -1,
                _ => return None,
            };
            let tz = rest[1..].replace(':', "");
            if tz.len() != 4 || !tz.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let offset = tz[..2].parse::<i64>().ok()? * 3600 + tz[2..].parse::<i64>().ok()? * 60;
            Some(secs - sign * offset)
        }
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// The time between `then` and `now`, in words, e.g. "3 months ago". Same
/// as pkgsearch's default format.
fn time_ago(then: i64, now: i64) -> String {
    let seconds = (now - then) as f64;
    let minutes = seconds / 60.0;
    let hours = minutes / 60.0;
    let days = hours / 24.0;
    let years = days / 365.25;
    let r = |x: f64| x.round() as i64;
    if seconds < 10.0 {
        "moments ago".to_string()
    } else if seconds < 45.0 {
        "less than a minute ago".to_string()
    } else if seconds < 90.0 {
        "about a minute ago".to_string()
    } else if minutes < 45.0 {
        format!("{} minutes ago", r(minutes))
    } else if minutes < 90.0 {
        "about an hour ago".to_string()
    } else if hours < 24.0 {
        format!("{} hours ago", r(hours))
    } else if hours < 42.0 {
        "a day ago".to_string()
    } else if days < 30.0 {
        format!("{} days ago", r(days))
    } else if days < 45.0 {
        "about a month ago".to_string()
    } else if days < 335.0 {
        format!("{} months ago", r(days / 30.0))
    } else if years < 1.5 {
        "about a year ago".to_string()
    } else {
        format!("{} years ago", r(years))
    }
}

/// Terse version of [`time_ago`], always three characters wide for less
/// than 100 years, e.g. ` 3M`. Same as pkgsearch's terse format.
fn time_ago_terse(then: i64, now: i64) -> String {
    let seconds = (now - then) as f64;
    let minutes = seconds / 60.0;
    let hours = minutes / 60.0;
    let days = hours / 24.0;
    let years = days / 365.25;
    let r = |x: f64| x.round() as i64;
    if seconds < 50.0 {
        format!("{:>2}s", r(seconds))
    } else if minutes < 50.0 {
        format!("{:>2}m", r(minutes))
    } else if hours < 18.0 {
        format!("{:>2}h", r(hours))
    } else if days < 30.0 {
        format!("{:>2}d", r(days))
    } else if days < 335.0 {
        format!("{:>2}M", r(days / 30.0))
    } else {
        format!("{:>2}y", r(years))
    }
}

// ------------------------------------------------------------------------
// Output

fn width(s: &str) -> usize {
    s.chars().count()
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Word-wrap `text` so no line is longer than `width` characters, with
/// `indent` spaces before the first line and `exdent` before the rest.
fn wrap_indent(text: &str, width: usize, indent: usize, exdent: usize) -> Vec<String> {
    let mut lines: Vec<String> = vec![];
    let mut line = " ".repeat(indent);
    let mut empty = true;
    for word in text.split_whitespace() {
        if empty {
            line.push_str(word);
            empty = false;
        } else if self::width(&line) + 1 + self::width(word) <= width {
            line.push(' ');
            line.push_str(word);
        } else {
            lines.push(std::mem::replace(&mut line, " ".repeat(exdent)));
            line.push_str(word);
        }
    }
    if !empty {
        lines.push(line);
    }
    lines
}

/// Quote a search query for the shell, if needed.
fn shell_quote(s: &str) -> String {
    let plain = |c: char| c.is_alphanumeric() || "-_.,:/@+=".contains(c);
    if !s.is_empty() && s.chars().all(plain) {
        s.to_string()
    } else if !s.contains(['"', '$', '`', '\\', '!']) {
        format!("\"{}\"", s)
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// The command that shows the next page of results, if there are more.
fn format_next(result: &SearchResult, short: bool, color: bool) -> Option<String> {
    use owo_colors::OwoColorize;
    let next = result.from as u64 + result.hits.len() as u64;
    if result.hits.is_empty() || next > result.total {
        return None;
    }
    let count = (result.size as u64).min(result.total - next + 1);
    let mut cmd = format!(
        "rig pkg search {} --from {}",
        shell_quote(&result.query),
        next
    );
    if result.size != default_size(short) {
        cmd.push_str(&format!(" -n {}", result.size));
    }
    if short {
        cmd.push_str(" --short");
    }
    let label = format!(
        "Next {} {}:",
        count,
        if count == 1 { "result" } else { "results" }
    );
    Some(if color {
        format!("{} {}", label.dimmed(), cmd.green())
    } else {
        format!("{} {}", label, cmd)
    })
}

fn plural(n: u64) -> &'static str {
    if n == 1 {
        "package"
    } else {
        "packages"
    }
}

/// `- "query" -------------- N packages in X seconds -`
fn format_header(result: &SearchResult, color: bool) -> String {
    use owo_colors::OwoColorize;
    let left = format!("- \"{}\"", result.query);
    let right = format!(
        " {} {} in {} seconds -",
        result.total,
        plural(result.total),
        result.took as f64 / 1000.0
    );
    let fill = (WIDTH - 1).saturating_sub(width(&left) + 1 + width(&right));
    if color {
        format!(
            "{} {}{}",
            format!("- \"{}\"", result.query.bold()).dimmed(),
            "-".repeat(fill).dimmed(),
            right.dimmed()
        )
    } else {
        format!("{} {}{}", left, "-".repeat(fill), right)
    }
}

fn score_pct(hit: &SearchHit, max_score: f64) -> i64 {
    if max_score > 0.0 {
        (hit.score / max_score * 100.0).round() as i64
    } else {
        0
    }
}

fn hit_date(hit: &SearchHit) -> Option<i64> {
    hit.date.as_deref().and_then(parse_iso_8601)
}

/// One line per package: rank, score (percent of the best score), name,
/// version, maintainer, time since release and title.
fn format_short(result: &SearchResult, now: i64, color: bool) -> Vec<String> {
    use owo_colors::OwoColorize;
    let mut out = vec![format_header(result, color)];
    if result.hits.is_empty() {
        return out;
    }

    let header = ["#", "", "package", "version", "by", "@"];
    let rows: Vec<[String; 6]> = result
        .hits
        .iter()
        .enumerate()
        .map(|(i, hit)| {
            [
                (result.from as usize + i).to_string(),
                score_pct(hit, result.max_score).to_string(),
                hit.package.clone(),
                hit.version.clone(),
                hit.maintainer_name.clone(),
                hit_date(hit)
                    .map(|d| time_ago_terse(d, now))
                    .unwrap_or_else(|| "-".to_string()),
            ]
        })
        .collect();
    let mut widths = header.map(width);
    for row in &rows {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(width(cell));
        }
    }
    // Numbers and dates are right aligned, text is left aligned.
    let right_aligned = [true, true, false, false, false, true];
    let pad = |i: usize, s: &str| -> String {
        let n = widths[i].saturating_sub(width(s));
        if right_aligned[i] {
            format!("{}{}", " ".repeat(n), s)
        } else {
            format!("{}{}", s, " ".repeat(n))
        }
    };

    let table_width = widths.iter().sum::<usize>() + widths.len() - 1;
    let title_width = WIDTH.saturating_sub(table_width + 2);
    let show_title = title_width >= 5;

    let mut head = (0..header.len())
        .map(|i| pad(i, header[i]))
        .collect::<Vec<_>>()
        .join(" ");
    if show_title {
        head.push_str(" title");
    }
    let head = head.trim_end().to_string();
    out.push(if color {
        head.dimmed().to_string()
    } else {
        head
    });

    for (row, hit) in rows.iter().zip(&result.hits) {
        let cells: Vec<String> = (0..row.len()).map(|i| pad(i, &row[i])).collect();
        let mut line = if color {
            [
                cells[0].dimmed().to_string(),
                cells[1].dimmed().to_string(),
                cells[2].bold().to_string(),
                cells[3].clone(),
                cells[4].clone(),
                cells[5].dimmed().to_string(),
            ]
            .join(" ")
        } else {
            cells.join(" ")
        };
        if show_title {
            let title = collapse_ws(&hit.title);
            let title = if width(&title) <= title_width {
                title
            } else {
                let cut: String = title.chars().take(title_width - 3).collect();
                format!("{}...", cut.trim_end())
            };
            line.push(' ');
            line.push_str(&title);
        }
        out.push(line.trim_end().to_string());
    }
    out
}

/// Several lines per package: rank, name, version, maintainer and release
/// date, then the title, the description and the URLs.
fn format_long(result: &SearchResult, now: i64, color: bool) -> Vec<String> {
    use owo_colors::OwoColorize;
    let text_width = WIDTH * 9 / 10;
    let mut out = vec![format_header(result, color)];
    for (i, hit) in result.hits.iter().enumerate() {
        out.push(String::new());
        let rank = (result.from as usize + i).to_string();
        let pkg_ver = format!("{} {} @ {}", rank, hit.package, hit.version);
        let score = format!("(score {})", score_pct(hit, result.max_score));
        let left = format!("{} {}", pkg_ver, score);
        let ago = hit_date(hit)
            .map(|d| time_ago(d, now))
            .unwrap_or_else(|| "-".to_string());
        let right = format!("{}, {}", hit.maintainer_name, ago);
        let left_col = if color {
            format!(
                "{} {} {} {} {}",
                rank.dimmed(),
                hit.package.bold(),
                "@".dimmed(),
                hit.version,
                score.dimmed()
            )
        } else {
            left.clone()
        };
        let right_col = if color {
            right.dimmed().to_string()
        } else {
            right.clone()
        };
        let avail = (WIDTH - 1).saturating_sub(width(&left) + 1);
        if width(&right) <= avail {
            let pad = avail - width(&right);
            out.push(format!("{} {}{}", left_col, " ".repeat(pad), right_col));
        } else {
            out.push(left_col);
            out.push(format!(
                "{}{}",
                " ".repeat((WIDTH - 1).saturating_sub(width(&right))),
                right_col
            ));
        }
        let rule = "-".repeat(width(&pkg_ver));
        out.push(if color {
            rule.dimmed().to_string()
        } else {
            rule
        });

        let title = format!("# {}", collapse_ws(&hit.title));
        for line in wrap_indent(&title, text_width, 2, 4) {
            out.push(if color {
                line.italic().to_string()
            } else {
                line
            });
        }
        out.extend(wrap_indent(&hit.description, text_width, 2, 2));
        if let Some(url) = &hit.url {
            for u in url.split(|c: char| c == ',' || c.is_whitespace()) {
                if !u.is_empty() {
                    let u = format!("  {}", u);
                    out.push(if color { u.cyan().to_string() } else { u });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(package: &str, score: f64, title: &str, date: &str) -> SearchHit {
        SearchHit {
            score,
            package: package.to_string(),
            version: "1.0.0".to_string(),
            title: title.to_string(),
            description: "Some\n  description   text.".to_string(),
            date: Some(date.to_string()),
            maintainer_name: "Jane Doe".to_string(),
            maintainer_email: Some("jane@example.com".to_string()),
            revdeps: 1,
            downloads_last_month: 100,
            license: Some("MIT".to_string()),
            url: Some("https://a.org, https://b.org".to_string()),
            bugreports: None,
        }
    }

    fn result(hits: Vec<SearchHit>) -> SearchResult {
        SearchResult {
            query: "plot".to_string(),
            from: 1,
            size: 10,
            total: 123,
            max_score: 200.0,
            took: 22,
            hits,
        }
    }

    #[test]
    fn query_matches_pkgsearch() {
        let q = build_query("network visualization");
        let fs = &q["query"]["function_score"];
        assert_eq!(fs["functions"][0]["field_value_factor"]["field"], "revdeps");
        assert_eq!(fs["functions"][0]["field_value_factor"]["modifier"], "sqrt");
        let b = &fs["query"]["bool"];
        assert_eq!(
            b["must"][0]["multi_match"]["query"],
            "network visualization"
        );
        assert_eq!(b["must"][0]["multi_match"]["type"], "most_fields");
        assert_eq!(b["should"][0]["multi_match"]["type"], "phrase");
        assert_eq!(b["should"][0]["multi_match"]["boost"], 10);
        assert_eq!(b["should"][1]["multi_match"]["operator"], "and");
        assert_eq!(b["should"][1]["multi_match"]["fields"][0], "Package^20");
        assert_eq!(b["should"][1]["multi_match"]["boost"], 5);
    }

    #[test]
    fn iso_8601() {
        assert_eq!(parse_iso_8601("1970-01-01"), Some(0));
        assert_eq!(parse_iso_8601("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_iso_8601("2026-04-09T08:50:18+00:00"),
            Some(1775724618)
        );
        assert_eq!(
            parse_iso_8601("2026-04-09T10:50:18+02:00"),
            Some(1775724618)
        );
        assert_eq!(parse_iso_8601("2026-04-09T08:50:18.123Z"), Some(1775724618));
        assert_eq!(parse_iso_8601("2000-02-29 12:00:00"), Some(951825600));
        assert_eq!(parse_iso_8601("not a date"), None);
        assert_eq!(parse_iso_8601("2026-13-01"), None);
    }

    #[test]
    fn time_ago_formats() {
        let day = 86400;
        assert_eq!(time_ago(0, 5), "moments ago");
        assert_eq!(time_ago(0, 30), "less than a minute ago");
        assert_eq!(time_ago(0, 60), "about a minute ago");
        assert_eq!(time_ago(0, 10 * 60), "10 minutes ago");
        assert_eq!(time_ago(0, 60 * 60), "about an hour ago");
        assert_eq!(time_ago(0, 5 * 3600), "5 hours ago");
        assert_eq!(time_ago(0, 30 * 3600), "a day ago");
        assert_eq!(time_ago(0, 3 * day), "3 days ago");
        assert_eq!(time_ago(0, 40 * day), "about a month ago");
        assert_eq!(time_ago(0, 90 * day), "3 months ago");
        assert_eq!(time_ago(0, 400 * day), "about a year ago");
        assert_eq!(time_ago(0, 1096 * day), "3 years ago");

        assert_eq!(time_ago_terse(0, 5), " 5s");
        assert_eq!(time_ago_terse(0, 10 * 60), "10m");
        assert_eq!(time_ago_terse(0, 5 * 3600), " 5h");
        assert_eq!(time_ago_terse(0, 3 * day), " 3d");
        assert_eq!(time_ago_terse(0, 90 * day), " 3M");
        assert_eq!(time_ago_terse(0, 1096 * day), " 3y");
    }

    #[test]
    fn maintainer() {
        let m = "Gábor Csárdi <gabor@posit.co>";
        assert_eq!(maintainer_name(m), "Gábor Csárdi");
        assert_eq!(maintainer_email(m).as_deref(), Some("gabor@posit.co"));
        assert_eq!(maintainer_name("Jane Doe"), "Jane Doe");
        assert_eq!(maintainer_email("Jane Doe"), None);
    }

    #[test]
    fn response_parsing() {
        let body = r#"{
          "took": 22,
          "hits": {
            "total": 2,
            "max_score": 10.5,
            "hits": [
              {"_id": "cli", "_score": 10.5, "_source": {
                "Version": "3.6.6", "Title": "Helpers", "Description": "Desc",
                "date": "2026-04-09T08:50:18+00:00",
                "Maintainer": "Jane Doe <jane@example.com>",
                "revdeps": 1858, "downloads": 2119452, "License": "MIT",
                "URL": "https://cli.r-lib.org", "BugReports": "https://x"}},
              {"_id": "other", "_score": 1.0, "_source": {"Version": "0.1"}}
            ]
          }
        }"#;
        let r = parse_response(body, "cli", 1, 10).unwrap();
        assert_eq!(r.total, 2);
        assert_eq!(r.took, 22);
        assert_eq!(r.max_score, 10.5);
        assert_eq!(r.hits.len(), 2);
        assert_eq!(r.hits[0].package, "cli");
        assert_eq!(r.hits[0].version, "3.6.6");
        assert_eq!(r.hits[0].maintainer_name, "Jane Doe");
        assert_eq!(
            r.hits[0].maintainer_email.as_deref(),
            Some("jane@example.com")
        );
        assert_eq!(r.hits[0].revdeps, 1858);
        assert_eq!(r.hits[0].downloads_last_month, 2119452);
        assert_eq!(r.hits[1].downloads_last_month, 1);
        assert_eq!(r.hits[1].title, "");

        // Elasticsearch style total
        let body = r#"{"took": 1, "hits": {"total": {"value": 0, "relation": "eq"},
                       "max_score": null, "hits": []}}"#;
        let r = parse_response(body, "x", 1, 10).unwrap();
        assert_eq!(r.total, 0);
        assert!(r.hits.is_empty());
    }

    #[test]
    fn short_output() {
        let now = parse_iso_8601("2026-09-30").unwrap();
        let r = result(vec![
            hit(
                "ggplot2",
                200.0,
                "Create Elegant Data Visualisations Using the Grammar of Graphics",
                "2026-06-30",
            ),
            hit("lattice", 50.0, "Trellis\n   Graphics", "2025-09-30"),
        ]);
        let lines = format_short(&r, now, false);
        assert_eq!(
            lines,
            vec![
                "- \"plot\" -------------------------------------- 123 packages in 0.022 seconds -",
                "#     package version by         @ title",
                "1 100 ggplot2 1.0.0   Jane Doe  3M Create Elegant Data Visualisations Using...",
                "2  25 lattice 1.0.0   Jane Doe  1y Trellis Graphics",
            ]
        );
        for line in &lines {
            assert!(width(line) < WIDTH);
        }
    }

    #[test]
    fn long_output() {
        let now = parse_iso_8601("2026-09-30").unwrap();
        let r = result(vec![hit("ggplot2", 200.0, "Create Plots", "2026-06-30")]);
        let lines = format_long(&r, now, false);
        assert_eq!(
            lines,
            vec![
                "- \"plot\" -------------------------------------- 123 packages in 0.022 seconds -",
                "",
                "1 ggplot2 @ 1.0.0 (score 100)                            Jane Doe, 3 months ago",
                "-----------------",
                "  # Create Plots",
                "  Some description text.",
                "  https://a.org",
                "  https://b.org",
            ]
        );
    }

    #[test]
    fn long_title_wraps() {
        let lines = wrap_indent(&format!("# {}", "word ".repeat(30)), 72, 2, 4);
        assert!(lines.len() > 1);
        assert!(lines[0].starts_with("  # word"));
        assert!(lines[1].starts_with("    word"));
        assert!(lines.iter().all(|l| width(l) <= 72));
    }

    #[test]
    fn next_page() {
        let hits = || (0..8).map(|_| hit("p", 1.0, "t", "2026-01-01")).collect();
        let mut r = result(hits());
        r.size = 8;
        r.query = "permutation test".to_string();
        assert_eq!(
            format_next(&r, false, false).as_deref(),
            Some("Next 8 results: rig pkg search \"permutation test\" --from 9")
        );
        r.query = "cli".to_string();
        r.from = 11;
        r.size = 5;
        r.hits.truncate(5);
        assert_eq!(
            format_next(&r, true, false).as_deref(),
            Some("Next 5 results: rig pkg search cli --from 16 -n 5 --short")
        );
        // Fewer left than a page
        r.total = 17;
        assert_eq!(
            format_next(&r, false, false).as_deref(),
            Some("Next 2 results: rig pkg search cli --from 16 -n 5")
        );
        // Last page
        r.total = 15;
        assert_eq!(format_next(&r, false, false), None);
        // No results
        assert_eq!(format_next(&result(vec![]), false, false), None);
    }

    #[test]
    fn quoting() {
        assert_eq!(shell_quote("ggplot2"), "ggplot2");
        assert_eq!(shell_quote("a b"), "\"a b\"");
        assert_eq!(shell_quote("it's"), "\"it's\"");
        assert_eq!(shell_quote("$x's"), "'$x'\\''s'");
    }

    #[test]
    fn empty_result() {
        let mut r = result(vec![]);
        r.total = 0;
        let lines = format_short(&r, 0, false);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("0 packages in"));
    }

    #[test]
    fn search_against_mock_server() {
        use wiremock::matchers::{body_partial_json, method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let rt = tokio::runtime::Runtime::new().unwrap();
        let server = rt.block_on(MockServer::start());
        rt.block_on(
            Mock::given(method("POST"))
                .and(path("/package/_search"))
                .and(query_param("from", "10"))
                .and(query_param("size", "5"))
                .and(body_partial_json(serde_json::json!({
                    "query": {"function_score": {"query": {"bool": {"must": [
                        {"multi_match": {"query": "ggplot2", "type": "most_fields"}}
                    ]}}}}
                })))
                .respond_with(ResponseTemplate::new(200).set_body_string(
                    r#"{"took": 3, "hits": {"total": 1, "max_score": 2.0,
                        "hits": [{"_id": "ggplot2", "_score": 2.0,
                                  "_source": {"Version": "3.5.1"}}]}}"#,
                ))
                .mount(&server),
        );
        rt.block_on(
            Mock::given(method("POST"))
                .and(path("/package/_search"))
                .and(query_param("from", "0"))
                .respond_with(
                    ResponseTemplate::new(400)
                        .set_body_string(r#"{"error": {"root_cause": [{"reason": "bad query"}]}}"#),
                )
                .mount(&server),
        );

        let r = search(&server.uri(), "ggplot2", 11, 5).unwrap();
        assert_eq!(r.total, 1);
        assert_eq!(r.hits[0].package, "ggplot2");
        assert_eq!(r.hits[0].version, "3.5.1");

        let err = search(&server.uri(), "ggplot2", 1, 5).unwrap_err();
        assert!(err.to_string().contains("bad query"), "{}", err);
    }
}
