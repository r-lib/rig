//! `--exclude-newer`: hide CRAN package versions published after a date from
//! the dependency solver.
//!
//! A version's publish date is the P3M snapshot it first appeared in, i.e. the
//! date in its ALLPACKAGES `DownloadURL`, see
//! [`crate::repos::cranlike_metadata::snapshot_date`]. That is a day, so the
//! cutoff is a day too: a version is kept if its snapshot date is on or before
//! the cutoff.

use std::error::Error;
use std::str::FromStr;

use clap::ArgMatches;
use jiff::civil::Date;
use jiff::{Span, Timestamp, Zoned};
use simple_error::*;

/// The first P3M snapshot. ALLPACKAGES stamps every version published before
/// it with this date, so an earlier cutoff cannot be applied correctly.
pub const FIRST_SNAPSHOT: &str = "2017-10-10";

/// A parsed `--exclude-newer` value (or `exclude-newer` project setting).
#[derive(Debug, Clone)]
pub enum ExcludeNewerSpec {
    /// An absolute date, from `YYYY-MM-DD` or an RFC 3339 timestamp.
    Date(Date),
    /// A span relative to today, e.g. `7 days` or `P2W`, with the value as the
    /// user wrote it, which is what the lock file records.
    Span(String, Span),
}

impl FromStr for ExcludeNewerSpec {
    type Err = Box<dyn Error>;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        // Timestamp first: `Date` would also accept one, dropping its offset.
        if let Ok(ts) = s.parse::<Timestamp>() {
            return Ok(ExcludeNewerSpec::Date(
                ts.to_zoned(jiff::tz::TimeZone::UTC).date(),
            ));
        }
        if let Ok(date) = s.parse::<Date>() {
            return Ok(ExcludeNewerSpec::Date(date));
        }
        if let Ok(span) = s.parse::<Span>() {
            return Ok(ExcludeNewerSpec::Span(s.to_string(), span.abs()));
        }
        bail!(
            "Invalid exclude-newer value `{}`, expected a date (YYYY-MM-DD), \
             an RFC 3339 timestamp, or a span like `7 days`",
            s
        );
    }
}

impl ExcludeNewerSpec {
    /// The cutoff day as `YYYY-MM-DD`: versions published after it are hidden.
    pub fn cutoff(&self) -> Result<String, Box<dyn Error>> {
        self.cutoff_from(Zoned::now().date())
    }

    /// [`Self::cutoff`], with a relative span counted back from `today`.
    fn cutoff_from(&self, today: Date) -> Result<String, Box<dyn Error>> {
        let date = match self {
            ExcludeNewerSpec::Date(date) => *date,
            ExcludeNewerSpec::Span(orig, span) => today
                .checked_sub(*span)
                .map_err(|e| format!("Invalid exclude-newer span `{}`: {}", orig, e))?,
        };
        let cutoff = date.to_string();
        if cutoff.as_str() < FIRST_SNAPSHOT {
            bail!(
                "Cannot exclude packages newer than {}: rig only knows package \
                 publication dates from {} on",
                cutoff,
                FIRST_SNAPSHOT
            );
        }
        Ok(cutoff)
    }

    /// The span as written, for a relative spec, `None` for a date.
    pub fn span(&self) -> Option<&str> {
        match self {
            ExcludeNewerSpec::Date(_) => None,
            ExcludeNewerSpec::Span(orig, _) => Some(orig),
        }
    }
}

/// The parsed `--exclude-newer` argument of a subcommand, if it was given.
///
/// The argument is a plain string in `src/args.rs`, because that file is also
/// compiled into the build script, where this module does not exist.
pub fn exclude_newer_arg(args: &ArgMatches) -> Result<Option<ExcludeNewerSpec>, Box<dyn Error>> {
    args.get_one::<String>("exclude-newer")
        .map(|value| value.parse())
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cutoff(s: &str, today: &str) -> String {
        let spec: ExcludeNewerSpec = s.parse().unwrap();
        spec.cutoff_from(today.parse().unwrap()).unwrap()
    }

    #[test]
    fn a_date_is_the_cutoff() {
        assert_eq!(cutoff("2020-01-01", "2026-01-01"), "2020-01-01");
    }

    #[test]
    fn a_timestamp_uses_its_utc_date() {
        assert_eq!(
            cutoff("2020-01-01T23:30:00-02:00", "2026-01-01"),
            "2020-01-02"
        );
        assert_eq!(cutoff("2020-01-01T10:00:00Z", "2026-01-01"), "2020-01-01");
    }

    #[test]
    fn a_span_counts_back_from_today() {
        assert_eq!(cutoff("7 days", "2026-01-05"), "2025-12-29");
        assert_eq!(cutoff("2 weeks", "2026-01-15"), "2026-01-01");
        assert_eq!(cutoff("P1M", "2026-03-31"), "2026-02-28");
        assert_eq!(cutoff("1 year", "2026-01-01"), "2025-01-01");
    }

    #[test]
    fn a_span_keeps_what_the_user_wrote() {
        let spec: ExcludeNewerSpec = " 7 days ".parse().unwrap();
        assert_eq!(spec.span(), Some("7 days"));
        let spec: ExcludeNewerSpec = "2020-01-01".parse().unwrap();
        assert_eq!(spec.span(), None);
    }

    #[test]
    fn invalid_values_are_errors() {
        assert!("yesterday".parse::<ExcludeNewerSpec>().is_err());
        assert!("2020-13-01".parse::<ExcludeNewerSpec>().is_err());
        assert!("".parse::<ExcludeNewerSpec>().is_err());
    }

    #[test]
    fn dates_before_the_first_snapshot_are_errors() {
        let spec: ExcludeNewerSpec = "2017-10-09".parse().unwrap();
        assert!(spec.cutoff_from("2026-01-01".parse().unwrap()).is_err());
        assert_eq!(cutoff(FIRST_SNAPSHOT, "2026-01-01"), FIRST_SNAPSHOT);
        let spec: ExcludeNewerSpec = "20 years".parse().unwrap();
        assert!(spec.cutoff_from("2026-01-01".parse().unwrap()).is_err());
    }
}
