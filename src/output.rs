use std::io::IsTerminal;
use std::sync::Arc;

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
}

impl Output {
    /// Create a new Output handler, detecting if stdout is a terminal
    pub fn new() -> Self {
        Self {
            inner: Arc::new(OutputInner {
                interactive: std::io::stdout().is_terminal(),
            }),
        }
    }

    /// Display a success message
    pub fn success(&self, msg: &str) {
        if self.inner.interactive {
            eprintln!("{} {}", "✓".green().bold(), msg.green());
        } else {
            eprintln!("{}", msg);
        }
    }

    /// Display an error message
    pub fn error(&self, msg: &str) {
        if self.inner.interactive {
            eprintln!("{} {}", "✗".red().bold(), msg.red().bold());
        } else {
            eprintln!("Error: {}", msg);
        }
    }

    /// Display a warning message
    pub fn warn(&self, msg: &str) {
        if self.inner.interactive {
            eprintln!("{} {}", "⚠".yellow().bold(), msg.yellow());
        } else {
            eprintln!("Warning: {}", msg);
        }
    }

    /// Display an info message
    pub fn info(&self, msg: &str) {
        if self.inner.interactive {
            eprintln!("{} {}", "ℹ".blue(), msg);
        } else {
            eprintln!("{}", msg);
        }
    }

    /// Display a status message (for progress/operations)
    pub fn status(&self, msg: &str) {
        if self.inner.interactive {
            eprintln!("{} {}", "▶".blue(), msg.blue());
        } else {
            eprintln!("{}", msg);
        }
    }

    /// Write directly to stderr with newline
    pub fn println(&self, msg: &str) {
        eprintln!("{}", msg);
    }
}

impl Default for Output {
    fn default() -> Self {
        Self::new()
    }
}
