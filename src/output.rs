//! User-facing messages.
//!
//! rig talks to the user on three channels, and every message belongs to
//! exactly one of them:
//!
//! - **stdout**: the result of the command (tables, paths, `--json` output),
//!   written with `println!` by the command itself.
//! - **stderr, through `OUTPUT`**: progress, success messages, warnings and
//!   errors meant for the human running rig. `OUTPUT` also copies each
//!   message to the log file, so call sites do not need an `info!`/`warn!`
//!   next to it.
//! - **the log** (`debug!`/`trace!`): details for debugging. Always written
//!   to the log file, and shown on stderr only with `-v` or `RIG_LOG`.
//!
//! How messages look depends on where stderr goes: glyphs and colors in a
//! terminal, `Error:`/`Warning:` prefixes otherwise, and GitHub Actions
//! workflow commands on GitHub Actions. `NO_COLOR`, `CLICOLOR_FORCE`,
//! `TERM=dumb` and `CI` are respected.

use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use indicatif::{ProgressBar, ProgressStyle};
use lazy_static::lazy_static;
use log::Level;
use owo_colors::{OwoColorize, Style};

lazy_static! {
    /// Global output handler instance
    pub static ref OUTPUT: Output = Output::new();
}

/// The log target of the copies of user messages that `OUTPUT` writes to the
/// log. The logger leaves these out of its stderr output, because `OUTPUT`
/// already showed them there.
pub const UI_LOG_TARGET: &str = "rig::ui";

/// What the terminal (or the lack of one) can show. Detected once, from the
/// environment, see `Term::detect()`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Term {
    /// stderr is a terminal that can show glyphs, e.g. `✓`.
    pub fancy: bool,
    /// Use ANSI colors on stderr.
    pub color: bool,
    /// Show animated progress bars on stderr.
    pub animate: bool,
    /// Running on GitHub Actions: use workflow commands for warnings and
    /// errors, so they show up as annotations.
    pub gha: bool,
    /// stdout is a terminal, for reports printed to stdout.
    pub stdout_tty: bool,
    /// Use ANSI colors on stdout.
    pub stdout_color: bool,
}

fn env_set(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| !v.is_empty())
}

impl Term {
    pub fn detect() -> Term {
        let dumb = std::env::var("TERM").is_ok_and(|t| t == "dumb");
        let no_color = env_set("NO_COLOR");
        let force_color = std::env::var("CLICOLOR_FORCE").is_ok_and(|v| !v.is_empty() && v != "0");
        let ci = env_set("CI");
        let stderr_tty = std::io::stderr().is_terminal();
        let stdout_tty = std::io::stdout().is_terminal();
        let fancy = stderr_tty && !dumb;
        Term {
            fancy,
            color: force_color || (fancy && !no_color),
            animate: fancy && !ci,
            gha: std::env::var("GITHUB_ACTIONS").is_ok_and(|v| v == "true"),
            stdout_tty,
            stdout_color: force_color || (stdout_tty && !dumb && !no_color),
        }
    }
}

/// Output handler for user-facing messages, see the module docs.
///
/// This struct is cheaply cloneable via Arc and thread-safe.
#[derive(Clone)]
pub struct Output {
    inner: Arc<OutputInner>,
}

struct OutputInner {
    term: Term,
    bar: RwLock<Option<ProgressBar>>,
    // `-q` was given.
    quiet: AtomicBool,
    // Temporary quiet mode, see `set_quiet()`.
    quiet_override: AtomicBool,
    // `-v` was given, see `set_verbose()`.
    verbose: AtomicBool,
    // If set, collect the lines here instead of writing them to stderr.
    capture: Mutex<Option<Vec<String>>>,
}

impl Output {
    /// Create a new Output handler for the current terminal
    pub fn new() -> Self {
        Self::with_term(Term::detect())
    }

    pub fn with_term(term: Term) -> Self {
        Self {
            inner: Arc::new(OutputInner {
                term,
                bar: RwLock::new(None),
                quiet: AtomicBool::new(false),
                quiet_override: AtomicBool::new(false),
                verbose: AtomicBool::new(false),
                capture: Mutex::new(None),
            }),
        }
    }

    pub fn term(&self) -> Term {
        self.inner.term
    }

    /// Route output around `bar` until the bar is unset again.
    ///
    /// A progress bar owns the last line of the terminal and redraws it from its
    /// own idea of what is on screen, so a plain `eprintln!` while the bar is
    /// live lands on the bar's line and leaves a half-overwritten frame behind.
    /// Prefer `progress()`, which does this for you.
    pub fn set_progress_bar(&self, bar: Option<ProgressBar>) {
        *self.inner.bar.write().unwrap() = bar;
    }

    /// Set from `-q`: drop info, status and success messages for the whole
    /// run. Warnings and errors are still shown.
    pub fn set_cli_quiet(&self, quiet: bool) {
        self.inner.quiet.store(quiet, Ordering::Relaxed);
    }

    /// Drop info and status messages until this is unset again, e.g. for a
    /// report that calls helpers that narrate their progress. Warnings and
    /// errors are still shown.
    pub fn set_quiet(&self, quiet: bool) {
        self.inner.quiet_override.store(quiet, Ordering::Relaxed);
    }

    pub fn is_quiet(&self) -> bool {
        self.inner.quiet.load(Ordering::Relaxed)
            || self.inner.quiet_override.load(Ordering::Relaxed)
    }

    /// Record that `-v` was given, so callers can show more detail to the
    /// user, e.g. the full build log of a package that failed to install.
    pub fn set_verbose(&self, verbose: bool) {
        self.inner.verbose.store(verbose, Ordering::Relaxed);
    }

    pub fn is_verbose(&self) -> bool {
        self.inner.verbose.load(Ordering::Relaxed)
    }

    /// Collect lines in memory instead of writing them to stderr, until
    /// `take_captured()` is called. For the C API, where stderr goes nowhere,
    /// and for tests.
    // Only the C API in lib.rs uses these, not the rig binary.
    #[allow(dead_code)]
    pub fn start_capture(&self) {
        *self.inner.capture.lock().unwrap() = Some(Vec::new());
    }

    /// Stop capturing, and return the captured lines.
    #[allow(dead_code)]
    pub fn take_captured(&self) -> Vec<String> {
        self.inner
            .capture
            .lock()
            .unwrap()
            .take()
            .unwrap_or_default()
    }

    /// Write a line to stderr, without the progress bar getting in the way.
    /// This is also what the logger uses for `-v` output.
    pub fn emit(&self, line: &str) {
        if let Some(lines) = self.inner.capture.lock().unwrap().as_mut() {
            lines.push(line.to_string());
            return;
        }
        match self.inner.bar.read().unwrap().as_ref() {
            Some(bar) => bar.suspend(|| eprintln!("{}", line)),
            None => eprintln!("{}", line),
        }
    }

    fn paint(&self, text: &str, style: Style) -> String {
        if self.inner.term.color {
            text.style(style).to_string()
        } else {
            text.to_string()
        }
    }

    // A line with a glyph in front in a terminal, and with `prefix` (which
    // may be empty) otherwise.
    fn line(
        &self,
        glyph: &str,
        glyph_style: Style,
        msg: &str,
        msg_style: Style,
        prefix: &str,
    ) -> String {
        if self.inner.term.fancy {
            format!(
                "{} {}",
                self.paint(glyph, glyph_style),
                self.paint(msg, msg_style)
            )
        } else {
            format!("{}{}", prefix, msg)
        }
    }

    fn log(&self, level: Level, msg: &str) {
        log::log!(target: UI_LOG_TARGET, level, "{}", msg);
    }

    /// Display a success message
    pub fn success(&self, msg: &str) {
        self.log(Level::Info, msg);
        if self.is_quiet() {
            return;
        }
        let green = Style::new().green();
        self.emit(&self.line("✓", green.bold(), msg, green, ""));
    }

    /// Display an error message
    pub fn error(&self, msg: &str) {
        self.log(Level::Error, msg);
        if self.inner.term.gha {
            self.emit(&gha_command("error", msg));
        } else {
            let red = Style::new().red().bold();
            self.emit(&self.line("✗", red, msg, red, "Error: "));
        }
    }

    /// Display a warning message
    pub fn warn(&self, msg: &str) {
        self.log(Level::Warn, msg);
        if self.inner.term.gha {
            self.emit(&gha_command("warning", msg));
        } else {
            let yellow = Style::new().yellow();
            self.emit(&self.line("⚠", yellow.bold(), msg, yellow, "Warning: "));
        }
    }

    /// Display an info message
    pub fn info(&self, msg: &str) {
        self.log(Level::Info, msg);
        if self.is_quiet() {
            return;
        }
        self.emit(&self.line("ℹ", Style::new().blue(), msg, Style::new(), ""));
    }

    /// Display a status message (for progress/operations)
    pub fn status(&self, msg: &str) {
        self.log(Level::Info, msg);
        if self.is_quiet() {
            return;
        }
        let blue = Style::new().blue();
        self.emit(&self.line("▶", blue, msg, blue, ""));
    }

    /// A green tick or a red cross, plus a space, to put in front of a line
    /// of a report on stdout. Empty if stdout is not a terminal.
    pub fn mark(&self, ok: bool) -> String {
        let term = self.inner.term;
        if !term.stdout_tty {
            return String::new();
        }
        let (glyph, style) = if ok {
            ("✓", Style::new().green().bold())
        } else {
            ("✗", Style::new().red().bold())
        };
        if term.stdout_color {
            format!("{} ", glyph.style(style))
        } else {
            format!("{} ", glyph)
        }
    }

    /// `label` in bold, for the section headers of a report on stdout. Plain
    /// if stdout is not a terminal.
    pub fn heading(&self, label: &str) -> String {
        if self.inner.term.stdout_color {
            label.bold().to_string()
        } else {
            label.to_string()
        }
    }

    /// Write directly to stderr with newline
    pub fn println(&self, msg: &str) {
        self.log(Level::Info, msg);
        self.emit(msg);
    }

    /// Start a progress bar of `len` packages, labelled `msg`. It is animated
    /// only in an interactive terminal; elsewhere (e.g. on CI) there is no
    /// bar, and `Progress::println()` lines are printed as plain lines.
    /// Call `Progress::finish()` when done.
    pub fn progress(&self, len: u64, msg: &str) -> Progress {
        let bar = if self.inner.term.animate {
            let bar = ProgressBar::new(len);
            bar.set_style(
                ProgressStyle::default_bar()
                    .template("{msg} [{bar:40.green/blue}] {pos}/{len} packages")
                    .unwrap()
                    .progress_chars("=>-"),
            );
            bar.set_message(msg.to_string());
            self.set_progress_bar(Some(bar.clone()));
            Some(bar)
        } else {
            None
        };
        Progress {
            output: self.clone(),
            bar,
        }
    }
}

impl Default for Output {
    fn default() -> Self {
        Self::new()
    }
}

/// A progress bar from `Output::progress()`.
#[derive(Clone)]
pub struct Progress {
    output: Output,
    bar: Option<ProgressBar>,
}

impl Progress {
    pub fn inc(&self, delta: u64) {
        if let Some(bar) = &self.bar {
            bar.inc(delta);
        }
    }

    /// A message about one finished step, e.g. one installed package.
    /// Dropped in quiet mode.
    pub fn println(&self, msg: &str) {
        self.output.log(Level::Info, msg);
        if self.output.is_quiet() {
            return;
        }
        let msg = if self.output.inner.term.fancy {
            format!("{} {}", self.output.paint("✓", Style::new().green()), msg)
        } else {
            msg.to_string()
        };
        // `emit()` suspends the bar while it writes.
        self.output.emit(&msg);
    }

    /// Remove the bar.
    pub fn finish(&self) {
        if let Some(bar) = &self.bar {
            bar.finish_and_clear();
            self.output.set_progress_bar(None);
        }
    }
}

/// A GitHub Actions workflow command, e.g. `::warning::msg`. See
/// <https://docs.github.com/en/actions/reference/workflow-commands-for-github-actions>.
fn gha_command(command: &str, msg: &str) -> String {
    let msg = msg
        .replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A");
    format!("::{}::{}", command, msg)
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

impl std::error::Error for ExitStatusError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.error.as_ref())
    }
}

/// An error with a message that says what rig was doing, and the error that
/// caused it. Created with `Context::context()`. `main` prints the whole
/// chain:
///
/// ```text
/// Error: Failed to install R 4.6.2
///   caused by: Cannot download https://...
///   caused by: HTTP 404
/// ```
#[derive(Debug)]
pub struct ContextError {
    msg: String,
    source: Box<dyn std::error::Error>,
}

impl std::fmt::Display for ContextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.msg)
    }
}

impl std::error::Error for ContextError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

/// Add a message to an error, to say what rig was trying to do, e.g.
/// `download(url).context("Failed to install R 4.6.2")?`.
pub trait Context<T> {
    fn context(self, msg: impl Into<String>) -> Result<T, Box<dyn std::error::Error>>;
    fn with_context<F: FnOnce() -> String>(self, f: F) -> Result<T, Box<dyn std::error::Error>>;
}

impl<T, E: Into<Box<dyn std::error::Error>>> Context<T> for Result<T, E> {
    fn context(self, msg: impl Into<String>) -> Result<T, Box<dyn std::error::Error>> {
        self.map_err(|e| {
            Box::new(ContextError {
                msg: msg.into(),
                source: e.into(),
            }) as Box<dyn std::error::Error>
        })
    }

    fn with_context<F: FnOnce() -> String>(self, f: F) -> Result<T, Box<dyn std::error::Error>> {
        self.map_err(|e| {
            Box::new(ContextError {
                msg: f(),
                source: e.into(),
            }) as Box<dyn std::error::Error>
        })
    }
}

/// The messages of `err` and its causes, to show to the user. Stops at a
/// `ReportedError`, which was already shown, and leaves out a cause whose
/// message is already part of the message before it (e.g.
/// `format!("Cannot read {}: {}", path, err)`). Empty if the whole error was
/// already shown.
pub fn error_messages(err: &(dyn std::error::Error + 'static)) -> Vec<String> {
    let mut msgs: Vec<String> = Vec::new();
    let mut next = Some(err);
    while let Some(e) = next {
        next = e.source();
        if e.is::<ReportedError>() {
            break;
        }
        if e.is::<ExitStatusError>() {
            continue;
        }
        let msg = e.to_string();
        if msg.is_empty() || msgs.last().is_some_and(|last| last.contains(&msg)) {
            continue;
        }
        msgs.push(msg);
    }
    msgs
}

/// Whether `err`, or one of its causes, is an `E`.
pub fn find_error<'a, E: std::error::Error + 'static>(
    err: &'a (dyn std::error::Error + 'static),
) -> Option<&'a E> {
    let mut next = Some(err);
    while let Some(e) = next {
        if let Some(found) = e.downcast_ref::<E>() {
            return Some(found);
        }
        next = e.source();
    }
    None
}

impl Output {
    /// Show an error that stops rig, with its causes. Only `main` (and the
    /// C API) should call this; everything else returns the error.
    pub fn fatal(&self, err: &(dyn std::error::Error + 'static)) {
        let msgs = error_messages(err);
        let Some((first, causes)) = msgs.split_first() else {
            return;
        };
        let mut text = first.clone();
        for cause in causes {
            text.push_str("\n  caused by: ");
            text.push_str(cause);
        }
        self.error(&text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(term: Term, quiet: bool, f: impl Fn(&Output)) -> Vec<String> {
        let out = Output::with_term(term);
        out.set_cli_quiet(quiet);
        out.start_capture();
        f(&out);
        out.take_captured()
    }

    fn all(out: &Output) {
        out.status("Downloading R");
        out.info("Using cache");
        out.success("Installed R");
        out.warn("Something odd");
        out.error("It broke\nbadly 100%");
    }

    #[test]
    fn plain_output() {
        let lines = run(Term::default(), false, all);
        assert_eq!(
            lines,
            vec![
                "Downloading R",
                "Using cache",
                "Installed R",
                "Warning: Something odd",
                "Error: It broke\nbadly 100%",
            ]
        );
    }

    #[test]
    fn fancy_output_without_color() {
        let term = Term {
            fancy: true,
            ..Term::default()
        };
        let lines = run(term, false, all);
        assert_eq!(
            lines,
            vec![
                "▶ Downloading R",
                "ℹ Using cache",
                "✓ Installed R",
                "⚠ Something odd",
                "✗ It broke\nbadly 100%",
            ]
        );
    }

    #[test]
    fn color_output() {
        let term = Term {
            fancy: true,
            color: true,
            ..Term::default()
        };
        let lines = run(term, false, all);
        assert_eq!(lines.len(), 5);
        assert!(lines.iter().all(|l| l.contains("\x1b[")));
    }

    #[test]
    fn gha_output() {
        let term = Term {
            gha: true,
            ..Term::default()
        };
        let lines = run(term, false, all);
        assert_eq!(
            lines,
            vec![
                "Downloading R",
                "Using cache",
                "Installed R",
                "::warning::Something odd",
                "::error::It broke%0Abadly 100%25",
            ]
        );
    }

    #[test]
    fn quiet_output() {
        let lines = run(Term::default(), true, all);
        assert_eq!(
            lines,
            vec!["Warning: Something odd", "Error: It broke\nbadly 100%"]
        );
    }

    #[test]
    fn quiet_override_does_not_unset_cli_quiet() {
        let lines = run(Term::default(), true, |out| {
            out.set_quiet(true);
            out.set_quiet(false);
            out.info("hidden");
        });
        assert!(lines.is_empty());
    }

    #[test]
    fn progress_without_terminal_prints_lines() {
        let lines = run(Term::default(), false, |out| {
            let pb = out.progress(2, "Installing");
            pb.println("Installed a 1.0");
            pb.inc(1);
            pb.println("Installed b 2.0");
            pb.inc(1);
            pb.finish();
        });
        assert_eq!(lines, vec!["Installed a 1.0", "Installed b 2.0"]);
    }

    fn boxed(msg: &str) -> Box<dyn std::error::Error> {
        msg.into()
    }

    #[test]
    fn error_chain() {
        let err = Err::<(), _>(boxed("HTTP 404"))
            .context("Cannot download https://x")
            .context("Failed to install R 4.6.2")
            .unwrap_err();
        assert_eq!(
            error_messages(err.as_ref()),
            vec![
                "Failed to install R 4.6.2",
                "Cannot download https://x",
                "HTTP 404"
            ]
        );
        let lines = run(Term::default(), false, |out| out.fatal(err.as_ref()));
        assert_eq!(
            lines,
            vec![
                "Error: Failed to install R 4.6.2\n  caused by: Cannot download https://x\n  caused by: HTTP 404"
            ]
        );
    }

    #[test]
    fn error_chain_skips_repeated_causes() {
        let err = Err::<(), _>(boxed("HTTP 404"))
            .context("Cannot download https://x: HTTP 404")
            .unwrap_err();
        assert_eq!(
            error_messages(err.as_ref()),
            vec!["Cannot download https://x: HTTP 404"]
        );
    }

    #[test]
    fn error_chain_stops_at_reported_error() {
        let reported: Box<dyn std::error::Error> = Box::new(ReportedError("shown".into()));
        assert!(error_messages(reported.as_ref()).is_empty());
        let err = Err::<(), _>(reported)
            .context("Failed to sync project")
            .unwrap_err();
        assert_eq!(error_messages(err.as_ref()), vec!["Failed to sync project"]);
        assert!(find_error::<ReportedError>(err.as_ref()).is_some());
    }

    #[test]
    fn exit_status_error_is_transparent() {
        let err: Box<dyn std::error::Error> = Box::new(ExitStatusError {
            status: 2,
            error: boxed("out of date"),
        });
        assert_eq!(error_messages(err.as_ref()), vec!["out of date"]);
        assert_eq!(
            find_error::<ExitStatusError>(err.as_ref()).unwrap().status,
            2
        );
    }

    #[test]
    fn marks_and_headings_follow_stdout() {
        let out = Output::with_term(Term::default());
        assert_eq!(out.mark(true), "");
        assert_eq!(out.heading("Project:"), "Project:");
        let out = Output::with_term(Term {
            stdout_tty: true,
            ..Term::default()
        });
        assert_eq!(out.mark(true), "✓ ");
        assert_eq!(out.mark(false), "✗ ");
    }
}
