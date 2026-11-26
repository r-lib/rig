//! `rig ppm pkg downloads`: download counts of packages, from the usage
//! statistics of a Package Manager instance. These are off on the public
//! instance, and on servers where the admin has not turned them on.

use std::error::Error;

use clap::ArgMatches;
use serde::Serialize;
use serde_json::Value;
use tabular::{Row, Table};

use crate::output::OUTPUT;
use crate::ppm::api::{self, query, ApiError};
use crate::ppm::{print_table, want_json};
use crate::repos::binaries::validate_package_name;

#[derive(Debug, Clone, Serialize)]
struct Downloads {
    package: String,
    days: u32,
    count: u64,
}

#[derive(Debug, Clone, Serialize)]
struct DailyDownloads {
    package: String,
    /// `YYYY-MM-DD`
    date: String,
    count: u64,
}

/// Warn for `--daily` with more days than this: it is one request per day
/// and package.
const MANY_DAYS: u32 = 90;

/// Downloads of `package` in the last `days` days.
fn count(package: &str, days: u32) -> Result<u64, Box<dyn Error>> {
    let res: Vec<Value> = api::get(
        &["metrics", "packages"],
        &query(&[
            ("_sourceType", "r".to_string()),
            ("_days", days.to_string()),
            ("_sort", "count".to_string()),
            ("_order", "desc".to_string()),
            ("_limit", "10".to_string()),
            ("name_like", package.to_string()),
        ]),
    )
    .map_err(explain)?;
    Ok(res
        .iter()
        .find(|r| r.get("name").and_then(|n| n.as_str()) == Some(package))
        .and_then(|r| r.get("count"))
        .and_then(|c| c.as_u64())
        .unwrap_or(0))
}

/// PPM answers with a 400 if usage statistics are off.
fn explain(err: Box<dyn Error>) -> Box<dyn Error> {
    if let Some(e) = err.downcast_ref::<ApiError>() {
        if e.message.contains("statistics are disabled") {
            return "This Package Manager does not collect usage statistics, so it has no download counts. (They are off on the public Package Manager.)".into();
        }
    }
    err
}

pub fn sc_ppm_downloads(
    args: &ArgMatches,
    ppmargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let packages: Vec<String> = args
        .get_many::<String>("package")
        .unwrap()
        .cloned()
        .collect();
    for p in &packages {
        validate_package_name(p)?;
    }
    let days = args.get_one::<u32>("days").copied().unwrap_or(30);
    let json = want_json(args, ppmargs, mainargs);

    if !args.get_flag("daily") {
        let mut res = vec![];
        for p in &packages {
            res.push(Downloads {
                package: p.clone(),
                days,
                count: count(p, days)?,
            });
        }
        if json {
            println!("{}", serde_json::to_string_pretty(&res)?);
            return Ok(());
        }
        let mut tab = Table::new("{:<}   {:>}");
        tab.add_row(
            Row::new()
                .with_cell("package")
                .with_cell(format!("downloads (last {} days)", days)),
        );
        for r in &res {
            tab.add_row(Row::new().with_cell(&r.package).with_cell(r.count));
        }
        print_table(&tab);
        return Ok(());
    }

    // Daily counts: PPM only reports totals over the last N days, so the
    // count of day N is total(N) - total(N - 1).
    if days > MANY_DAYS {
        OUTPUT.warn(&format!(
            "--daily with --days {} needs {} requests per package",
            days, days
        ));
    }
    let today = jiff::Zoned::now().date();
    let mut res = vec![];
    for p in &packages {
        let mut prev = 0;
        for d in 1..=days {
            let total = count(p, d)?;
            let date = today
                .checked_sub(jiff::Span::new().days(i64::from(d - 1)))
                .map(|d| d.to_string())
                .unwrap_or_default();
            res.push(DailyDownloads {
                package: p.clone(),
                date,
                count: total.saturating_sub(prev),
            });
            prev = total;
        }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&res)?);
        return Ok(());
    }
    let mut tab = Table::new("{:<}   {:<}   {:>}");
    tab.add_row(
        Row::new()
            .with_cell("package")
            .with_cell("date")
            .with_cell("downloads"),
    );
    for r in &res {
        tab.add_row(
            Row::new()
                .with_cell(&r.package)
                .with_cell(&r.date)
                .with_cell(r.count),
        );
    }
    print_table(&tab);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_explain() {
        let err: Box<dyn Error> = Box::new(ApiError {
            status: 400,
            code: Some(2),
            message: "Usage statistics are disabled".to_string(),
        });
        assert!(explain(err)
            .to_string()
            .contains("does not collect usage statistics"));
        let err: Box<dyn Error> = "other".into();
        assert_eq!(explain(err).to_string(), "other");
    }
}
