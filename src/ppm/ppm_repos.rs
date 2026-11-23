//! `rig ppm repos`, `rig ppm snapshots`, `rig ppm sources` and
//! `rig ppm transactions`: the repositories of a Package Manager instance,
//! their snapshots, and where their packages come from.

use std::error::Error;

use clap::ArgMatches;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tabular::{Row, Table};

use crate::output::OUTPUT;
use crate::ppm::api::{self, ApiError, Query};
use crate::ppm::{print_table, want_json, yesno};
use crate::repos::binaries::ppm_url;

/// Repository types whose packages `rig ppm pkg info` and `history` search.
const PACKAGE_REPO_TYPES: &[&str] = &["R", "Bioconductor"];

/// Run `fetch` on the repository a package command should use, and return
/// the repository's name along with the result.
///
/// With `--repo`, that is the only repository tried. Otherwise the R and
/// Bioconductor repositories that are not hidden are tried in the order
/// `/repos` lists them, like Package Manager does, and the first one that
/// has the package wins. A 404 means the package is not in that repository.
/// Other errors, e.g. a repository needing a token, also move on to the next
/// one, but if no repository has the package, the first of these errors is
/// returned, since it might be the reason.
pub(crate) fn first_repo_with<T>(
    args: &ArgMatches,
    package: &str,
    mut fetch: impl FnMut(&str) -> Result<T, Box<dyn Error>>,
) -> Result<(String, T), Box<dyn Error>> {
    if let Some(repo) = args.get_one::<String>("repo") {
        return Ok((repo.clone(), fetch(repo)?));
    }
    let names = load_package_repos()?;
    match first_with(&names, fetch)? {
        Some(x) => Ok(x),
        None => bail!(
            "Package '{}' is not in any repository of {}. Searched: {}",
            package,
            ppm_url(),
            if names.is_empty() {
                "none".to_string()
            } else {
                names.join(", ")
            }
        ),
    }
}

/// The names of the R and Bioconductor repositories that are not hidden, in
/// the server's order. Package Manager searches repositories in this order.
pub(crate) fn load_package_repos() -> Result<Vec<String>, Box<dyn Error>> {
    let repos: Vec<Repo> = api::get(&["repos"], &vec![])?;
    Ok(package_repos(&repos))
}

/// The repositories to search for a package, in the server's order.
fn package_repos(repos: &[Repo]) -> Vec<String> {
    repos
        .iter()
        .filter(|r| !r.hidden && PACKAGE_REPO_TYPES.contains(&r.repo_type.as_str()))
        .map(|r| r.name.clone())
        .collect()
}

/// The search loop of [`first_repo_with`]. `None` if no repository has the
/// package.
fn first_with<T>(
    names: &[String],
    mut fetch: impl FnMut(&str) -> Result<T, Box<dyn Error>>,
) -> Result<Option<(String, T)>, Box<dyn Error>> {
    let mut other_error: Option<Box<dyn Error>> = None;
    for name in names {
        match fetch(name) {
            Ok(x) => return Ok(Some((name.clone(), x))),
            Err(e) => match e.downcast_ref::<api::ApiError>() {
                Some(ae) if ae.status == 404 => {}
                Some(_) => {
                    other_error.get_or_insert(e);
                }
                None => return Err(e),
            },
        }
    }
    match other_error {
        Some(e) => Err(e),
        None => Ok(None),
    }
}

/// `--bioc-version`, if the command has it and it was given.
fn bioc_arg(args: &ArgMatches) -> Option<&String> {
    args.try_get_one::<String>("bioc-version").ok().flatten()
}

/// Add `--bioc-version` to a query, if it was given.
pub(crate) fn bioc_query(args: &ArgMatches, q: &mut Query) {
    if let Some(v) = bioc_arg(args) {
        q.push(("bioc_version".to_string(), v.to_string()));
    }
}

/// Whether the server asked for a Bioconductor version.
pub(crate) fn needs_bioc_version(e: &(dyn Error + 'static)) -> bool {
    matches!(
        e.downcast_ref::<ApiError>(),
        Some(ae) if ae.message.contains("Bioconductor version not provided")
    )
}

/// The newest Bioconductor version of a Bioconductor repository, from its
/// sources, which are listed once per Bioconductor version.
pub(crate) fn latest_bioc_version(repo: &str) -> Result<String, Box<dyn Error>> {
    let sources = load_sources(Some(repo))?;
    newest_version(sources.iter().filter_map(|s| s.bioc_version()))
        .map(|v| v.to_string())
        .ok_or_else(|| {
            format!(
                "Cannot find the Bioconductor versions of repository '{}', use --bioc-version",
                repo
            )
            .into()
        })
}

/// The newest of some `x.y` versions.
pub(crate) fn newest_version<'a>(versions: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let key = |v: &str| -> Vec<u64> { v.split('.').map(|p| p.parse().unwrap_or(0)).collect() };
    versions.max_by_key(|v| key(v))
}

/// Call `fetch` with `q` plus `--bioc-version`. If there is no
/// `--bioc-version`, and `repo` turns out to be a Bioconductor repository,
/// try again with its newest Bioconductor version. Returns the result and
/// the Bioconductor version used, if any.
pub(crate) fn with_bioc_version<T>(
    args: &ArgMatches,
    repo: &str,
    q: &Query,
    fetch: impl Fn(&Query) -> Result<T, Box<dyn Error>>,
) -> Result<(T, Option<String>), Box<dyn Error>> {
    let explicit = bioc_arg(args).cloned();
    let mut q = q.clone();
    bioc_query(args, &mut q);
    match fetch(&q) {
        Ok(x) => Ok((x, explicit)),
        Err(e) if explicit.is_none() && needs_bioc_version(e.as_ref()) => {
            let v = latest_bioc_version(repo)?;
            q.push(("bioc_version".to_string(), v.clone()));
            Ok((fetch(&q)?, Some(v)))
        }
        Err(e) => Err(e),
    }
}

/// Run `fetch` on the repositories a command should query: the one `--repo`
/// names, or every R and Bioconductor repository that is not hidden, in the
/// server's order. Returns the repository names with the results.
///
/// With `--repo` every error is an error. Otherwise repositories without the
/// data (400 or 404) are skipped, other Package Manager errors, e.g. a
/// repository needing a token, are skipped with a warning.
pub(crate) fn query_repos<T>(
    args: &ArgMatches,
    mut fetch: impl FnMut(&str) -> Result<T, Box<dyn Error>>,
) -> Result<Vec<(String, T)>, Box<dyn Error>> {
    if let Some(repo) = args.get_one::<String>("repo") {
        return Ok(vec![(repo.clone(), fetch(repo)?)]);
    }
    let mut out = vec![];
    for repo in load_package_repos()? {
        match fetch(&repo) {
            Ok(x) => out.push((repo, x)),
            Err(e) => match e.downcast_ref::<ApiError>() {
                Some(ae) if ae.status == 400 || ae.status == 404 => {}
                Some(_) => OUTPUT.warn(&format!("Skipping repository {}: {}", repo, e)),
                None => return Err(e),
            },
        }
    }
    Ok(out)
}

/// The first R repository of the instance that is not hidden, in the
/// server's order. Used where a single repository is needed.
pub(crate) fn first_r_repo() -> Result<String, Box<dyn Error>> {
    let repos: Vec<Repo> = api::get(&["repos"], &vec![])?;
    repos
        .into_iter()
        .find(|r| !r.hidden && r.repo_type == "R")
        .map(|r| r.name)
        .ok_or_else(|| "This Package Manager has no R repositories".into())
}

/// `--limit`, with a default.
pub(crate) fn limit_arg(args: &ArgMatches, default: usize) -> usize {
    args.get_one::<usize>("limit").copied().unwrap_or(default)
}

// ------------------------------------------------------------------------
// repos

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Repo {
    pub id: i64,
    pub name: String,
    #[serde(rename = "type", default)]
    pub repo_type: String,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether the repository needs a token. Not every PPM version reports
    /// this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<bool>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

pub fn sc_ppm_repos(
    args: &ArgMatches,
    ppmargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let mut q: Query = vec![];
    if let Some(t) = args.get_one::<String>("type") {
        q.push(("type".to_string(), t.to_string()));
    }
    let mut repos: Vec<Repo> = api::get(&["repos"], &q)?;
    if !args.get_flag("all") {
        repos.retain(|r| !r.hidden);
    }
    // Keep the server's order: it is the order Package Manager searches
    // repositories in.

    if want_json(args, ppmargs, mainargs) {
        println!("{}", serde_json::to_string_pretty(&repos)?);
        return Ok(());
    }
    let with_hidden = args.get_flag("all");
    let mut spec = "{:<}   {:<}   {:<}   {:<}   {:<}".to_string();
    if with_hidden {
        spec.push_str("   {:<}");
    }
    let mut tab = Table::new(&spec);
    let mut header = Row::new()
        .with_cell("name")
        .with_cell("id")
        .with_cell("type")
        .with_cell("auth")
        .with_cell("url");
    if with_hidden {
        header = header.with_cell("hidden");
    }
    tab.add_row(header);
    let base = ppm_url();
    for r in &repos {
        let mut row = Row::new()
            .with_cell(&r.name)
            .with_cell(r.id)
            .with_cell(&r.repo_type)
            .with_cell(yesno(r.auth.unwrap_or(false)))
            .with_cell(format!("{}/{}/latest", base, r.name));
        if with_hidden {
            row = row.with_cell(yesno(r.hidden));
        }
        tab.add_row(row);
    }
    print_table(&tab);
    Ok(())
}

// ------------------------------------------------------------------------
// snapshots

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Snapshot {
    /// The snapshot's name in repository URLs, e.g. `2024-01-02`. Some
    /// repositories, e.g. Git ones, have no names for their snapshots.
    #[serde(default)]
    pub alias: String,
    pub date: String,
    #[serde(default)]
    pub repo_id: i64,
    #[serde(default)]
    pub source_id: i64,
    #[serde(default)]
    pub transaction_id: i64,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// All snapshots of a repository, newest first.
pub(crate) fn load_snapshots(repo: &str) -> Result<Vec<Snapshot>, Box<dyn Error>> {
    // The endpoint is not paged, it ignores `_limit`
    let mut snaps: Vec<Snapshot> = api::get(&["repos", repo, "transaction-dates"], &vec![])?;
    snaps.sort_by(|a, b| b.date.cmp(&a.date));
    Ok(snaps)
}

impl Snapshot {
    /// The snapshot's name, or its date if it has no name. A date at
    /// midnight is shown without the time.
    fn name(&self) -> &str {
        if self.alias.is_empty() {
            self.date.strip_suffix("T00:00:00Z").unwrap_or(&self.date)
        } else {
            &self.alias
        }
    }
}

pub fn sc_ppm_snapshots(
    args: &ArgMatches,
    ppmargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let found = query_repos(args, |repo| {
        let (mut snaps, _) = with_bioc_version(args, repo, &vec![], |q| {
            api::get::<Vec<Snapshot>>(&["repos", repo, "transaction-dates"], q)
        })?;
        for s in snaps.iter_mut() {
            s.extra
                .insert("repo".to_string(), Value::String(repo.to_string()));
        }
        Ok(snaps)
    })?;
    let mut snaps: Vec<Snapshot> = found.into_iter().flat_map(|(_, s)| s).collect();
    // Stable, so the repositories stay in order for the same date
    snaps.sort_by(|a, b| b.date.cmp(&a.date));
    if let Some(since) = args.get_one::<String>("since") {
        snaps.retain(|s| s.date.as_str() >= since.as_str());
    }
    if !args.get_flag("all") {
        snaps.truncate(limit_arg(args, 20));
    }

    if want_json(args, ppmargs, mainargs) {
        println!("{}", serde_json::to_string_pretty(&snaps)?);
        return Ok(());
    }
    let base = ppm_url();
    let mut tab = Table::new("{:<}   {:<}   {:<}");
    tab.add_row(
        Row::new()
            .with_cell("snapshot")
            .with_cell("repo")
            .with_cell("url"),
    );
    for s in &snaps {
        let repo = s.extra.get("repo").and_then(|r| r.as_str()).unwrap_or("");
        let url = if s.alias.is_empty() {
            String::new()
        } else {
            format!("{}/{}/{}", base, repo, s.alias)
        };
        tab.add_row(
            Row::new()
                .with_cell(s.name())
                .with_cell(repo)
                .with_cell(url),
        );
    }
    print_table(&tab);
    Ok(())
}

// ------------------------------------------------------------------------
// sources

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Source {
    pub id: i64,
    pub name: String,
    /// A number in the API, see [`source_type_name`].
    #[serde(rename = "type", default)]
    pub source_type: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl Source {
    /// The Bioconductor version of a Bioconductor source. The sources of a
    /// Bioconductor repository are listed once per version.
    pub fn bioc_version(&self) -> Option<&str> {
        self.extra.get("bioc_version").and_then(|v| v.as_str())
    }

    pub fn type_name(&self) -> String {
        match &self.source_type {
            Value::Number(n) => n
                .as_i64()
                .map(source_type_name)
                .unwrap_or("unknown")
                .to_string(),
            Value::String(s) => s.clone(),
            _ => String::new(),
        }
    }
}

/// The names of PPM's source types, from the ppmtools R package.
pub(crate) fn source_type_name(t: i64) -> &'static str {
    match t {
        0 => "local",
        1 => "cran",
        2 => "curated-cran",
        3 => "git",
        4 => "pypi-legacy",
        5 => "bioc-internal",
        6 => "cran-snapshot",
        7 => "bioc",
        8 => "local-python",
        9 => "pypi",
        10 => "curated-pypi",
        _ => "unknown",
    }
}

/// `/__api__/sources` answers with `{"sources": [...]}` in some versions and
/// with a plain array in others.
#[derive(Deserialize)]
#[serde(untagged)]
enum SourceList {
    Wrapped {
        #[serde(alias = "Sources")]
        sources: Vec<Source>,
    },
    Plain(Vec<Source>),
}

/// The sources of a repository, or with `None` all sources, which needs a
/// token.
pub(crate) fn load_sources(repo: Option<&str>) -> Result<Vec<Source>, Box<dyn Error>> {
    let list = match repo {
        Some(repo) => SourceList::Plain(api::get(&["repos", repo, "sources"], &vec![])?),
        None => api::get(&["sources"], &vec![])?,
    };
    Ok(match list {
        SourceList::Wrapped { sources } => sources,
        SourceList::Plain(sources) => sources,
    })
}

pub fn sc_ppm_sources(
    args: &ArgMatches,
    ppmargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let repo = args.get_one::<String>("repo").map(|s| s.as_str());
    let mut sources = load_sources(repo)?;
    if repo.is_none() {
        sources.sort_by(|a, b| a.name.cmp(&b.name));
    }

    if want_json(args, ppmargs, mainargs) {
        println!("{}", serde_json::to_string_pretty(&sources)?);
        return Ok(());
    }
    // Not every Package Manager version reports these, only show them if
    // some source has them.
    let bioc = |s: &Source| -> String { s.bioc_version().unwrap_or("").to_string() };
    let with_bioc = sources.iter().any(|s| !bioc(s).is_empty());
    let with_sync = sources.iter().any(|s| s.last_sync.is_some());
    let ncol = 3 + with_bioc as usize + with_sync as usize;
    let mut tab = Table::new(&vec!["{:<}"; ncol].join("   "));
    let mut header = Row::new()
        .with_cell("name")
        .with_cell("id")
        .with_cell("type");
    if with_bioc {
        header = header.with_cell("bioc_version");
    }
    if with_sync {
        header = header.with_cell("last_sync");
    }
    tab.add_row(header);
    for s in &sources {
        let mut row = Row::new()
            .with_cell(&s.name)
            .with_cell(s.id)
            .with_cell(s.type_name());
        if with_bioc {
            row = row.with_cell(bioc(s));
        }
        if with_sync {
            row = row.with_cell(s.last_sync.as_deref().unwrap_or(""));
        }
        tab.add_row(row);
    }
    print_table(&tab);
    Ok(())
}

// ------------------------------------------------------------------------
// transactions

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Transaction {
    #[serde(default)]
    created: Option<String>,
    #[serde(default)]
    completed: Option<String>,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    packages_added: Option<i64>,
    #[serde(default)]
    packages_updated: Option<i64>,
    #[serde(default)]
    packages_removed: Option<i64>,
    #[serde(default)]
    packages_archived: Option<i64>,
    #[serde(default)]
    published_to: Option<String>,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

impl Transaction {
    /// When the transaction finished, or started, if it has not finished.
    fn time(&self) -> &str {
        self.completed
            .as_deref()
            .or(self.created.as_deref())
            .unwrap_or("")
    }
}

/// The sources to show transactions for, with a repository they are in: the
/// one `--source` names (name or id), or all of them. A Bioconductor repository lists its
/// source once per Bioconductor version, these are merged.
fn transaction_sources(
    found: Vec<(String, Vec<Source>)>,
    select: Option<&str>,
) -> Vec<(String, Source)> {
    let mut seen = std::collections::HashSet::new();
    found
        .into_iter()
        .flat_map(|(repo, sources)| sources.into_iter().map(move |s| (repo.clone(), s)))
        .filter(|(_, s)| select.is_none_or(|x| s.name == x || s.id.to_string() == x))
        .filter(|(_, s)| seen.insert(s.id))
        .collect()
}

/// Merge the transactions of several sources, newest first, at most `limit`.
fn merge_transactions(mut txs: Vec<Transaction>, limit: usize) -> Vec<Transaction> {
    // Stable, so transactions with the same time keep their order
    txs.sort_by(|a, b| b.time().cmp(a.time()));
    txs.truncate(limit);
    txs
}

pub fn sc_ppm_transactions(
    args: &ArgMatches,
    ppmargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let limit = limit_arg(args, 20);
    let select = args.get_one::<String>("source").map(|s| s.as_str());
    // A source can be in several repositories, it has the same transactions
    // in all of them, so it is only queried once.
    let found = query_repos(args, |repo| load_sources(Some(repo)))?;
    let sources = transaction_sources(found, select);
    if sources.is_empty() {
        match (select, args.get_one::<String>("repo")) {
            (Some(x), Some(repo)) => bail!(
                "Repository '{}' has no source '{}', see `rig ppm sources {}`",
                repo,
                x,
                repo
            ),
            (Some(x), None) => bail!("No repository has a source '{}'", x),
            (None, _) => bail!("No sources found"),
        }
    }
    let bioc = bioc_arg(args).map(|s| s.as_str());
    let mut txs: Vec<Transaction> = vec![];
    for (repo, src) in &sources {
        // Bioconductor sources need a Bioconductor version. Without
        // --bioc-version, use the first one the source lists, the newest.
        let bioc_version = src.bioc_version().map(|v| bioc.unwrap_or(v));
        let mut q: Query = vec![];
        if let Some(v) = bioc_version {
            q.push(("bioc_version".to_string(), v.to_string()));
        }
        let id = src.id.to_string();
        let mut these: Vec<Transaction> =
            api::get_paged(&["repos", repo, "sources", &id, "transactions"], &q, limit)?;
        for t in these.iter_mut() {
            t.extra
                .insert("source".to_string(), Value::String(src.name.clone()));
            if let Some(v) = bioc_version {
                t.extra
                    .insert("bioc_version".to_string(), Value::String(v.to_string()));
            }
        }
        txs.extend(these);
    }
    let txs = merge_transactions(txs, limit);

    if want_json(args, ppmargs, mainargs) {
        println!("{}", serde_json::to_string_pretty(&txs)?);
        return Ok(());
    }
    let num = |n: Option<i64>| n.map(|n| n.to_string()).unwrap_or_else(|| "-".to_string());
    let field = |t: &Transaction, name: &str| -> String {
        t.extra
            .get(name)
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string()
    };
    let mut tab = Table::new("{:<}   {:<}   {:<}   {:<}   {:>}   {:>}   {:>}   {:>}");
    tab.add_row(
        Row::new()
            .with_cell("completed")
            .with_cell("source")
            .with_cell("bioc_version")
            .with_cell("action")
            .with_cell("added")
            .with_cell("updated")
            .with_cell("archived")
            .with_cell("removed"),
    );
    for t in &txs {
        tab.add_row(
            Row::new()
                .with_cell(t.time())
                .with_cell(field(t, "source"))
                .with_cell(field(t, "bioc_version"))
                .with_cell(t.action.as_deref().unwrap_or(""))
                .with_cell(num(t.packages_added))
                .with_cell(num(t.packages_updated))
                .with_cell(num(t.packages_archived))
                .with_cell(num(t.packages_removed)),
        );
    }
    print_table(&tab);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_newest_version() {
        let v = ["3.9", "3.24", "3.10", "3.2"];
        assert_eq!(newest_version(v.iter().copied()), Some("3.24"));
        assert_eq!(newest_version(std::iter::empty()), None);
    }

    #[test]
    fn test_needs_bioc_version() {
        let e: Box<dyn Error> = Box::new(api::ApiError {
            status: 404,
            code: Some(2),
            message: "Bioconductor version not provided".to_string(),
        });
        assert!(needs_bioc_version(e.as_ref()));
        let e: Box<dyn Error> = Box::new(api::ApiError {
            status: 404,
            code: Some(4),
            message: "Package 'x' not found".to_string(),
        });
        assert!(!needs_bioc_version(e.as_ref()));
    }

    fn api_err(status: u16) -> Box<dyn Error> {
        Box::new(api::ApiError {
            status,
            code: None,
            message: format!("HTTP {}", status),
        })
    }

    #[test]
    fn test_transaction_sources() {
        let json = r#"[
          {"id":5,"name":"github","type":3},
          {"id":1,"name":"cran","type":1},
          {"id":3,"name":"bioconductor","type":7,"bioc_version":"3.22"},
          {"id":3,"name":"bioconductor","type":7,"bioc_version":"3.21"}
        ]"#;
        let all = || -> Vec<(String, Vec<Source>)> {
            vec![
                ("all".to_string(), serde_json::from_str(json).unwrap()),
                (
                    "cran".to_string(),
                    serde_json::from_str(r#"[{"id":1,"name":"cran","type":1}]"#).unwrap(),
                ),
            ]
        };
        let ids = |v: Vec<(String, Source)>| v.iter().map(|(_, s)| s.id).collect::<Vec<_>>();
        assert_eq!(ids(transaction_sources(all(), None)), vec![5, 1, 3]);
        assert_eq!(ids(transaction_sources(all(), Some("cran"))), vec![1]);
        assert_eq!(ids(transaction_sources(all(), Some("5"))), vec![5]);
        assert!(transaction_sources(all(), Some("nope")).is_empty());
    }

    #[test]
    fn test_merge_transactions() {
        let t = |time: &str, src: &str| -> Transaction {
            serde_json::from_value(serde_json::json!({"completed": time, "source": src})).unwrap()
        };
        let txs = vec![
            t("2026-11-20T00:00:00Z", "cran"),
            t("2026-11-22T00:00:00Z", "cran"),
            t("2026-11-21T00:00:00Z", "github"),
        ];
        let got: Vec<String> = merge_transactions(txs, 2)
            .iter()
            .map(|t| format!("{} {}", t.time(), t.extra["source"].as_str().unwrap()))
            .collect();
        assert_eq!(
            got,
            vec!["2026-11-22T00:00:00Z cran", "2026-11-21T00:00:00Z github"]
        );
    }

    #[test]
    fn test_package_repos() {
        let json = r#"[
          {"id":8,"name":"agent-plugins","type":"Plugins","hidden":false},
          {"id":1,"name":"all","type":"R","hidden":true},
          {"id":4,"name":"bioconductor","type":"Bioconductor","hidden":false},
          {"id":2,"name":"cran","type":"R","hidden":false},
          {"id":5,"name":"pypi","type":"Python","hidden":false},
          {"id":9,"name":"git","type":"R","hidden":false}
        ]"#;
        let repos: Vec<Repo> = serde_json::from_str(json).unwrap();
        assert_eq!(package_repos(&repos), vec!["bioconductor", "cran", "git"]);
    }

    #[test]
    fn test_first_with() {
        let names: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        let mut tried = vec![];
        let res = first_with(&names, |r| {
            tried.push(r.to_string());
            if r == "b" {
                Ok(42)
            } else {
                Err(api_err(404))
            }
        })
        .unwrap();
        assert_eq!(res, Some(("b".to_string(), 42)));
        assert_eq!(tried, vec!["a", "b"]);

        // Not found anywhere
        let res = first_with(&names, |_| Err::<i32, _>(api_err(404))).unwrap();
        assert!(res.is_none());

        // An auth error is skipped, but reported if nothing has the package
        let res = first_with(&names, |r| match r {
            "a" => Err(api_err(401)),
            "b" => Ok(1),
            _ => Err(api_err(404)),
        })
        .unwrap();
        assert_eq!(res, Some(("b".to_string(), 1)));
        let err = first_with(&names, |r| {
            if r == "b" {
                Err::<i32, _>(api_err(403))
            } else {
                Err(api_err(404))
            }
        })
        .unwrap_err();
        assert_eq!(err.downcast_ref::<api::ApiError>().unwrap().status, 403);

        // Other errors stop the search
        let mut tried = 0;
        let err = first_with(&names, |_| {
            tried += 1;
            Err::<i32, Box<dyn Error>>("network down".into())
        })
        .unwrap_err();
        assert_eq!(err.to_string(), "network down");
        assert_eq!(tried, 1);
    }

    #[test]
    fn test_parse_repos() {
        let json = r#"[{"id":8,"name":"agent-plugins","created":"2026-09-09T18:56:54Z","type":"Plugins","hidden":false},
            {"id":1,"name":"all","created":"2020-04-23T12:05:18Z","description":"CRAN","type":"R","hidden":true,"auth":true,"new_field":1}]"#;
        let repos: Vec<Repo> = serde_json::from_str(json).unwrap();
        assert_eq!(repos.len(), 2);
        assert_eq!(repos[0].repo_type, "Plugins");
        assert!(repos[1].hidden);
        assert_eq!(repos[1].auth, Some(true));
        let out = serde_json::to_value(&repos[1]).unwrap();
        assert_eq!(out["new_field"], 1);
        assert_eq!(out["type"], "R");
    }

    #[test]
    fn test_parse_snapshots() {
        let json = r#"[{"repo_id":2,"source_id":1,"transaction_id":0,"alias":"2017-10-10","date":"2017-10-10T00:00:00Z"}]"#;
        let s: Vec<Snapshot> = serde_json::from_str(json).unwrap();
        assert_eq!(s[0].alias, "2017-10-10");
    }

    #[test]
    fn test_parse_sources() {
        let json = r#"[{"id":1,"name":"cran","created":"2020-04-23T12:04:31Z","last_sync":"2024-05-01 10:33:38.740251 +0000 UTC","type":1,"version":"v3/1/1","strict":true,"order":1}]"#;
        let s: SourceList = serde_json::from_str(json).unwrap();
        let SourceList::Plain(s) = s else { panic!() };
        assert_eq!(s[0].type_name(), "cran");
        let json = r#"{"sources":[{"id":3,"name":"internal","type":0}]}"#;
        let s: SourceList = serde_json::from_str(json).unwrap();
        let SourceList::Wrapped { sources } = s else {
            panic!()
        };
        assert_eq!(sources[0].type_name(), "local");
        assert_eq!(source_type_name(99), "unknown");
    }

    #[test]
    fn test_parse_transactions() {
        let json = r#"[{"created":"2026-11-22T00:00:00Z","completed":"2026-11-22T00:00:00Z","type":3,"tag":"","source_id":1,"guid":"","packages_added":5,"packages_updated":53,"packages_archived":null,"packages_removed":0,"action":"Sync","published_from":null,"published_to":"2026-11-22T00:00:00Z","result":0}]"#;
        let t: Vec<Transaction> = serde_json::from_str(json).unwrap();
        assert_eq!(t[0].packages_added, Some(5));
        assert_eq!(t[0].packages_archived, None);
    }
}
