use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use indicatif::ProgressBar;
use lazy_static::lazy_static;
use owo_colors::OwoColorize;

lazy_static! {
    /// Global output handler instance
    pub static ref OUTPUT: Output = Output::new();
}

/// Output handler for user-facing messages
///
/// Separates user output from diagnostic logging:
/// - User output: Clean, formatted messages for end users
/// - Logging (via `log` crate): Detailed diagnostics in log files
///
/// This struct is cheaply cloneable via Arc and thread-safe.
#[derive(Clone)]
pub struct Output {
    inner: Arc<OutputInner>,
}

struct OutputInner {
    interactive: bool,
    bar: RwLock<Option<ProgressBar>>,
    // Error messages already shown, so `main` does not print them again
    // when the same error propagates up to it.
    errors: RwLock<Vec<String>>,
    // Drop info and status messages, see `set_quiet()`.
    quiet: AtomicBool,
}

impl Output {
    /// Create a new Output handler, detecting if stdout is a terminal
    pub fn new() -> Self {
        Self {
            inner: Arc::new(OutputInner {
                interactive: std::io::stdout().is_terminal(),
                bar: RwLock::new(None),
                errors: RwLock::new(Vec::new()),
                quiet: AtomicBool::new(false),
            }),
        }
    }

    /// Route output around `bar` until the bar is unset again.
    ///
    /// A progress bar owns the last line of the terminal and redraws it from its
    /// own idea of what is on screen, so a plain `eprintln!` while the bar is
    /// live lands on the bar's line and leaves a half-overwritten frame behind.
    pub fn set_progress_bar(&self, bar: Option<ProgressBar>) {
        *self.inner.bar.write().unwrap() = bar;
    }

    /// Drop info and status messages until this is unset again, e.g. for a
    /// report that calls helpers that narrate their progress. Warnings and
    /// errors are still shown.
    pub fn set_quiet(&self, quiet: bool) {
        self.inner.quiet.store(quiet, Ordering::Relaxed);
    }

    fn is_quiet(&self) -> bool {
        self.inner.quiet.load(Ordering::Relaxed)
    }

    fn emit(&self, line: &str) {
        match self.inner.bar.read().unwrap().as_ref() {
            Some(bar) => bar.suspend(|| eprintln!("{}", line)),
            None => eprintln!("{}", line),
        }
    }

    /// Display a success message
    pub fn success(&self, msg: &str) {
        if self.inner.interactive {
            self.emit(&format!("{} {}", "✓".green().bold(), msg.green()));
        } else {
            self.emit(msg);
        }
    }

    /// Display an error message
    pub fn error(&self, msg: &str) {
        self.inner.errors.write().unwrap().push(msg.to_string());
        if self.inner.interactive {
            self.emit(&format!("{} {}", "✗".red().bold(), msg.red().bold()));
        } else {
            self.emit(&format!("Error: {}", msg));
        }
    }

    /// Whether `msg` was already shown via `error()`
    pub fn error_shown(&self, msg: &str) -> bool {
        self.inner.errors.read().unwrap().iter().any(|e| e == msg)
    }

    /// Display a warning message
    pub fn warn(&self, msg: &str) {
        if self.inner.interactive {
            self.emit(&format!("{} {}", "⚠".yellow().bold(), msg.yellow()));
        } else {
            self.emit(&format!("Warning: {}", msg));
        }
    }

    /// Display an info message
    pub fn info(&self, msg: &str) {
        if self.is_quiet() {
            return;
        }
        if self.inner.interactive {
            self.emit(&format!("{} {}", "ℹ".blue(), msg));
        } else {
            self.emit(msg);
        }
    }

    /// Display a status message (for progress/operations)
    pub fn status(&self, msg: &str) {
        if self.is_quiet() {
            return;
        }
        if self.inner.interactive {
            self.emit(&format!("{} {}", "▶".blue(), msg.blue()));
        } else {
            self.emit(msg);
        }
    }

    /// A green tick or a red cross, plus a space, to put in front of a line
    /// of a report on stdout. Empty if stdout is not a terminal.
    pub fn mark(&self, ok: bool) -> String {
        if !self.inner.interactive {
            String::new()
        } else if ok {
            format!("{} ", "✓".green().bold())
        } else {
            format!("{} ", "✗".red().bold())
        }
    }

    /// `label` in bold, for the section headers of a report on stdout. Plain
    /// if stdout is not a terminal.
    pub fn heading(&self, label: &str) -> String {
        if self.inner.interactive {
            label.bold().to_string()
        } else {
            label.to_string()
        }
    }

    /// Write directly to stderr with newline
    pub fn println(&self, msg: &str) {
        self.emit(msg);
    }
}

impl Default for Output {
    fn default() -> Self {
        Self::new()
    }
}

/// An error whose message has already been shown to the user, so the
/// top-level handler in `main` should only log it, not print it again.
/// `Display` is the message itself, so a caller that logs or wraps the error
/// still sees the text.
#[derive(Debug)]
pub struct ReportedError(pub String);

impl std::fmt::Display for ReportedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ReportedError {}

/// An error that makes rig exit with `status` instead of 1, e.g. `rig proj
/// sync --check` exits with 1 if the environment is out of date, and with 2
/// on any other error. `Display` is the wrapped error's message.
#[derive(Debug)]
pub struct ExitStatusError {
    pub status: i32,
    pub error: Box<dyn std::error::Error>,
}

impl std::fmt::Display for ExitStatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.error)
    }
}

impl std::error::Error for ExitStatusError {}
