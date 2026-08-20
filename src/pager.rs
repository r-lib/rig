//! Paging long output through `$RIG_PAGER` / `$PAGER` / `less`, the way git
//! does it. Used for `--help` and for `rig repos package-info`, whose output
//! includes the package README.

use std::io::{IsTerminal, Write};
use std::process::{Command, Stdio};

/// Show `text`, paging it through `$RIG_PAGER` / `$PAGER` / `less` when stdout
/// is a terminal.
///
/// When stdout is not a terminal, or the pager is disabled (an empty value or
/// `cat`), missing or fails to start, `text` is printed directly. `text` may
/// contain ANSI escapes; `less` is started with `-R` so it keeps them, but the
/// caller is responsible for not producing any when stdout is not a terminal.
pub fn page_text(text: &str) {
    if !std::io::stdout().is_terminal() {
        print!("{}", text);
        return;
    }

    // Resolve the pager: RIG_PAGER -> PAGER -> `less`. An empty value or
    // `cat` disables paging.
    let pager = std::env::var("RIG_PAGER")
        .or_else(|_| std::env::var("PAGER"))
        .unwrap_or_else(|_| "less".to_string());
    let pager = pager.trim();
    if pager.is_empty() || pager == "cat" {
        print!("{}", text);
        return;
    }

    if pager.split_whitespace().next() == Some("less") && !command_on_path("less") {
        print!("{}", text);
        return;
    }

    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(pager);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = Command::new("sh");
        c.arg("-c").arg(pager);
        c
    };

    // Sensible `less` defaults (like git): -F quit if one screen, -R keep
    // colors, -X don't clear the screen. Only set when the user has not.
    if pager.split_whitespace().next() == Some("less") && std::env::var_os("LESS").is_none() {
        cmd.env("LESS", "FRX");
    }

    let child = cmd.stdin(Stdio::piped()).spawn();
    let mut child = match child {
        Ok(child) => child,
        // Pager could not be started (e.g. no `less` on Windows): print directly.
        Err(_) => {
            print!("{}", text);
            return;
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(text.as_bytes()).is_err() {
            // Writing failed (pager exited early, etc.): fall back to printing.
            drop(stdin);
            let _ = child.wait();
            print!("{}", text);
            return;
        }
    }

    let _ = child.wait();
}

/// Whether `cmd` can be found in `$PATH`.
pub fn command_on_path(cmd: &str) -> bool {
    let path = match std::env::var_os("PATH") {
        Some(p) => p,
        None => return false,
    };
    // On Windows, executables carry an extension; check PATHEXT (defaulting to
    // the common set) in addition to the bare name.
    #[cfg(windows)]
    let exts: Vec<String> = {
        let pathext =
            std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".to_string());
        std::iter::once(String::new())
            .chain(pathext.split(';').map(|e| e.to_string()))
            .collect()
    };
    #[cfg(not(windows))]
    let exts: Vec<String> = vec![String::new()];

    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let candidate = dir.join(format!("{}{}", cmd, ext));
            if candidate.is_file() {
                return true;
            }
        }
    }
    false
}
