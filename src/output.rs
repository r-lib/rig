use std::io::IsTerminal;
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
}

impl Output {
    /// Create a new Output handler, detecting if stdout is a terminal
    pub fn new() -> Self {
        Self {
            inner: Arc::new(OutputInner {
                interactive: std::io::stdout().is_terminal(),
                bar: RwLock::new(None),
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
        if self.inner.interactive {
            self.emit(&format!("{} {}", "✗".red().bold(), msg.red().bold()));
        } else {
            self.emit(&format!("Error: {}", msg));
        }
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
        if self.inner.interactive {
            self.emit(&format!("{} {}", "ℹ".blue(), msg));
        } else {
            self.emit(msg);
        }
    }

    /// Display a status message (for progress/operations)
    pub fn status(&self, msg: &str) {
        if self.inner.interactive {
            self.emit(&format!("{} {}", "▶".blue(), msg.blue()));
        } else {
            self.emit(msg);
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
