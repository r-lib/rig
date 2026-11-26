//! The logger behind the `log` macros.
//!
//! It has two outputs, each with its own filter:
//!
//! - the log file (`rig.log` in the logs directory), which gets everything at
//!   `debug` level, including copies of the messages `OUTPUT` shows, on every
//!   run;
//! - stderr, which is off by default, and shows `debug!`/`trace!` lines with
//!   `-v`/`-vv`, or what the `RIG_LOG` (or `RUST_LOG`) filter asks for. Lines
//!   go through `OUTPUT`, so they do not break progress bars. The copies of
//!   `OUTPUT` messages are not shown again.
//!
//! See `output.rs` for which messages go where.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use env_filter::Filter;
use log::{Level, LevelFilter, Log, Metadata, Record};
use owo_colors::OwoColorize;

use crate::output::{OUTPUT, UI_LOG_TARGET};

/// Start a new log file once the old one is larger than this.
const MAX_LOG_SIZE: u64 = 10 * 1024 * 1024;

struct Logger {
    file: Option<Mutex<File>>,
    file_filter: Filter,
    stderr_filter: Option<Filter>,
    // Show timestamps and source locations on stderr.
    detailed: bool,
}

fn filter(spec: &str) -> Filter {
    env_filter::Builder::new().parse(spec).build()
}

/// The filter for `-v` (1) and `-vv` (2) or more.
fn verbose_filter(verbose: u8) -> &'static str {
    match verbose {
        0 => "off",
        1 => "rig=debug,reqwest=info,hyper=info,pubgrub=info",
        _ => "rig=trace,reqwest=trace,hyper=trace,pubgrub=trace",
    }
}

/// Set up logging. `verbose` is the number of `-v` flags. Never fails: if
/// the log file cannot be opened, rig runs without it.
pub fn init(verbose: u8, log_file: Option<&Path>) {
    let file_filter = filter(if verbose >= 2 {
        verbose_filter(2)
    } else {
        "rig=debug,reqwest=warn,hyper=warn,pubgrub=warn"
    });

    let env_spec = ["RIG_LOG", "RUST_LOG"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|v| !v.is_empty());
    let stderr_filter = match (env_spec, verbose) {
        (Some(spec), _) => Some(filter(&spec)),
        (None, 0) => None,
        (None, v) => Some(filter(verbose_filter(v))),
    };

    let file = log_file.and_then(open_log_file).map(Mutex::new);

    let mut max_level = stderr_filter
        .as_ref()
        .map(|f| f.filter())
        .unwrap_or(LevelFilter::Off);
    if file.is_some() {
        max_level = max_level.max(file_filter.filter());
    }

    let logger = Logger {
        file,
        file_filter,
        stderr_filter,
        detailed: verbose >= 1,
    };
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(max_level);
    }
}

fn open_log_file(path: &Path) -> Option<File> {
    if let Some(dir) = path.parent() {
        crate::cache::create_logs_dir(dir).ok()?;
    }
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_LOG_SIZE) {
        let _ = std::fs::rename(path, path.with_extension("log.old"));
    }
    OpenOptions::new().create(true).append(true).open(path).ok()
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        (self.file.is_some() && self.file_filter.enabled(metadata))
            || self
                .stderr_filter
                .as_ref()
                .is_some_and(|f| f.enabled(metadata))
    }

    fn log(&self, record: &Record) {
        if let Some(file) = &self.file {
            if self.file_filter.matches(record) {
                let line = format!(
                    "[{} {} {}:{}] {}\n",
                    jiff::Timestamp::now()
                        .round(jiff::Unit::Millisecond)
                        .unwrap_or_default(),
                    record.level(),
                    record.file().unwrap_or("unknown"),
                    record.line().unwrap_or(0),
                    record.args()
                );
                if let Ok(mut file) = file.lock() {
                    let _ = file.write_all(line.as_bytes());
                }
            }
        }

        let to_stderr = record.target() != UI_LOG_TARGET
            && self
                .stderr_filter
                .as_ref()
                .is_some_and(|f| f.matches(record));
        if to_stderr {
            OUTPUT.emit(&self.stderr_line(record));
        }
    }

    fn flush(&self) {
        if let Some(file) = &self.file {
            if let Ok(mut file) = file.lock() {
                let _ = file.flush();
            }
        }
    }
}

impl Logger {
    fn stderr_line(&self, record: &Record) -> String {
        let level = record.level().to_string();
        let level = if OUTPUT.term().color {
            match record.level() {
                Level::Error => level.red().bold().to_string(),
                Level::Warn => level.yellow().bold().to_string(),
                Level::Info => level.blue().to_string(),
                Level::Debug => level.cyan().to_string(),
                Level::Trace => level.white().to_string(),
            }
        } else {
            level
        };
        if self.detailed {
            format!(
                "[{} {} {}:{}] {}",
                jiff::Timestamp::now()
                    .round(jiff::Unit::Millisecond)
                    .unwrap_or_default(),
                level,
                record.file().unwrap_or("unknown"),
                record.line().unwrap_or(0),
                record.args()
            )
        } else {
            format!("[{}] {}", level, record.args())
        }
    }
}
