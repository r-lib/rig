//! `rig run --install <script>` and `rig run --uninstall <name>`: put a
//! command for an R script into rig's quick-link directory
//! (`get_binary_dir()`), so it can be run by name, like any other command.
//!
//! On Windows the command is a copy of the `rig-shim.exe` template with a V3
//! footer (see src/shim_format.rs) that runs `rig.exe run [<flags>] -f
//! <script>` with the user's arguments after it. This is the only way to run
//! a script by name on Windows, which ignores `#!` lines. On macOS and Linux
//! it is a small `sh` wrapper that does the same.
//!
//! `<flags>` are the rig flags of the script's `#!` line, e.g. `--rscript`
//! for `#!/usr/bin/env -S rig run --rscript`, so the same script behaves the
//! same way on every platform. They are read once, when the command is
//! installed.

use std::error::Error;
use std::path::{Path, PathBuf};

use clap::ArgMatches;
use log::info;

use crate::output::OUTPUT;
use crate::utils::*;

#[cfg(target_os = "windows")]
use crate::windows::{read_shim_link, write_shim_link_args};

/// The marker of a Windows script command starts with this, followed by the
/// absolute path of the script.
#[cfg(target_os = "windows")]
const SCRIPT_MARKER_PREFIX: &str = "script:";

/// The second line of a Unix script command starts with this, followed by
/// the absolute path of the script.
#[cfg(not(target_os = "windows"))]
const SCRIPT_WRAPPER_TAG: &str = "# rig script command for ";

/// The rig flags in the `#!` line of a script, i.e. everything after
/// `rig run`. Empty if the line is not a `#!` line or does not run
/// `rig run`.
pub fn shebang_run_flags(first_line: &str) -> Vec<String> {
    let Some(rest) = first_line.strip_prefix("#!") else {
        return vec![];
    };
    let words: Vec<&str> = rest.split_whitespace().collect();
    for i in 0..words.len() {
        let base = words[i].rsplit(['/', '\\']).next().unwrap_or(words[i]);
        let base = base.strip_suffix(".exe").unwrap_or(base);
        if base == "rig" && words.get(i + 1) == Some(&"run") {
            return words[i + 2..].iter().map(|w| w.to_string()).collect();
        }
    }
    vec![]
}

/// The arguments that the command passes to rig, before the user's own
/// arguments.
fn launcher_args(flags: &[String], script: &Path) -> Vec<String> {
    let mut args = vec!["run".to_string()];
    args.extend(flags.iter().cloned());
    args.push("-f".to_string());
    args.push(script.display().to_string());
    args
}

/// Check the name of a script command. It must be a plain file name, and it
/// must not be one of rig's own quick links (`R`, `Rscript`, `RS`, `R-*`),
/// which rig may rewrite or delete at any time.
fn check_command_name(name: &str) -> Result<(), Box<dyn Error>> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', ':'])
        || name.starts_with('-')
    {
        bail!("Invalid command name: '{}'", name);
    }
    let lower = name.to_lowercase();
    if lower == "r"
        || lower == "rs"
        || lower == "rscript"
        || lower == "rig"
        || lower == "rig-shim"
        || lower.starts_with("r-")
    {
        bail!(
            "Cannot use '{}' as a command name, rig uses it for its own quick links. \
             Use --name to choose another name.",
            name
        );
    }
    Ok(())
}

/// The default command name of a script: its file name without the `.R`
/// extension.
fn default_command_name(script: &Path) -> Result<String, Box<dyn Error>> {
    let has_r_ext = script
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("r"));
    let name = if has_r_ext {
        script.file_stem()
    } else {
        script.file_name()
    };
    match name.and_then(|n| n.to_str()) {
        Some(n) => Ok(n.to_string()),
        None => bail!(
            "Cannot work out a command name for {}, use --name",
            script.display()
        ),
    }
}

#[cfg(target_os = "windows")]
fn command_path(bindir: &Path, name: &str) -> PathBuf {
    bindir.join(format!("{}.exe", name))
}

#[cfg(not(target_os = "windows"))]
fn command_path(bindir: &Path, name: &str) -> PathBuf {
    bindir.join(name)
}

/// The script that the command at `path` runs, or `None` if `path` is not a
/// script command created by rig.
#[cfg(target_os = "windows")]
fn installed_script(path: &Path) -> Option<String> {
    let (_target, marker) = read_shim_link(path)?;
    marker
        .strip_prefix(SCRIPT_MARKER_PREFIX)
        .map(|s| s.to_string())
}

#[cfg(not(target_os = "windows"))]
fn installed_script(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .nth(1)?
        .strip_prefix(SCRIPT_WRAPPER_TAG)
        .map(|s| s.to_string())
}

/// Quote a string for `sh`.
#[cfg(not(target_os = "windows"))]
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(not(target_os = "windows"))]
fn wrapper_script(rig: &str, script: &Path, args: &[String]) -> String {
    let mut cmd = vec!["exec".to_string(), sh_quote(rig)];
    cmd.extend(args.iter().map(|a| sh_quote(a)));
    cmd.push("\"$@\"".to_string());
    format!(
        "#!/bin/sh\n{}{}\n# Created by `rig run --install`, remove with `rig run --uninstall`.\n{}\n",
        SCRIPT_WRAPPER_TAG,
        script.display(),
        cmd.join(" ")
    )
}

#[cfg(target_os = "windows")]
fn write_command(
    path: &Path,
    rig: &str,
    script: &Path,
    args: &[String],
) -> Result<(), Box<dyn Error>> {
    let marker = format!("{}{}", SCRIPT_MARKER_PREFIX, script.display());
    write_shim_link_args(path, rig, &marker, args)
}

#[cfg(not(target_os = "windows"))]
fn write_command(
    path: &Path,
    rig: &str,
    script: &Path,
    args: &[String],
) -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, wrapper_script(rig, script, args))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn ensure_binary_dir_on_path() -> Result<(), Box<dyn Error>> {
    crate::windows::ensure_binary_dir_on_path()
}

#[cfg(not(target_os = "windows"))]
fn ensure_binary_dir_on_path() -> Result<(), Box<dyn Error>> {
    check_local_bin_path()
}

/// In admin mode the quick-link directory is usually not writable for the
/// user, so rig re-runs itself with `sudo` / as administrator. Like the R
/// quick links, an admin-mode directory that is already writable (e.g. one
/// set with `RIG_BINARY_DIR`) needs no escalation.
fn escalate_if_needed(bindir: &Path, task: &str) -> Result<(), Box<dyn Error>> {
    if get_mode()? != Mode::Admin || tempfile::tempfile_in(bindir).is_ok() {
        return Ok(());
    }
    crate::escalate::escalate(task)
}

fn first_line(path: &Path) -> String {
    use std::io::BufRead;
    std::fs::File::open(path)
        .ok()
        .and_then(|f| std::io::BufReader::new(f).lines().next())
        .and_then(|l| l.ok())
        .unwrap_or_default()
}

/// `rig run --install <script> [--name <name>]`.
pub fn sc_run_install(args: &ArgMatches) -> Result<i32, Box<dyn Error>> {
    let script = args
        .get_one::<String>("install")
        .expect("--install has a value");
    let script = Path::new(script);
    if !script.is_file() {
        bail!("Script file does not exist: {}", script.display());
    }
    let script = std::path::absolute(script)?;

    let name = match args.get_one::<String>("name") {
        Some(n) => n.to_string(),
        None => default_command_name(&script)?,
    };
    check_command_name(&name)?;

    let bindir = PathBuf::from(get_binary_dir()?);
    escalate_if_needed(&bindir, "adding a command for an R script")?;
    std::fs::create_dir_all(&bindir)?;
    let path = command_path(&bindir, &name);
    let op = if path.exists() {
        if installed_script(&path).is_none() {
            bail!(
                "{} already exists, and it is not a command for an R script. \
                 Use --name to choose another name.",
                path.display()
            );
        }
        "Updating"
    } else {
        "Adding"
    };

    let flags = shebang_run_flags(&first_line(&script));
    let rig = std::env::current_exe()?.display().to_string();
    let launcher = launcher_args(&flags, &script);
    write_command(&path, &rig, &script, &launcher)?;

    OUTPUT.status(&format!(
        "{} command {} -> {}",
        op,
        path.display(),
        script.display()
    ));
    info!("{} command {} -> {}", op, path.display(), script.display());

    ensure_binary_dir_on_path()?;
    Ok(0)
}

/// `rig run --uninstall <name>`.
pub fn sc_run_uninstall(args: &ArgMatches) -> Result<i32, Box<dyn Error>> {
    let name = args
        .get_one::<String>("uninstall")
        .expect("--uninstall has a value");
    check_command_name(name)?;

    let bindir = PathBuf::from(get_binary_dir()?);
    escalate_if_needed(&bindir, "removing the command of an R script")?;
    let path = command_path(&bindir, name);
    if !path.exists() {
        bail!(
            "There is no command called '{}' in {}",
            name,
            bindir.display()
        );
    }
    let Some(script) = installed_script(&path) else {
        bail!(
            "{} is not a command for an R script, rig will not remove it.",
            path.display()
        );
    };
    std::fs::remove_file(&path)?;
    OUTPUT.status(&format!("Removed command {} -> {}", path.display(), script));
    info!("Removed command {} -> {}", path.display(), script);
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn shebang_flags_from_env_s() {
        assert_eq!(shebang_run_flags("#!/usr/bin/env -S rig run"), s(&[]));
        assert_eq!(
            shebang_run_flags("#!/usr/bin/env -S rig run --rscript -r 4.4"),
            s(&["--rscript", "-r", "4.4"])
        );
    }

    #[test]
    fn shebang_flags_from_full_path() {
        assert_eq!(
            shebang_run_flags("#!/usr/local/bin/rig run --no-project"),
            s(&["--no-project"])
        );
        assert_eq!(
            shebang_run_flags("#! C:\\rig\\rig.exe run --rscript"),
            s(&["--rscript"])
        );
    }

    #[test]
    fn shebang_flags_not_rig() {
        assert_eq!(shebang_run_flags("#!/usr/bin/env Rscript"), s(&[]));
        assert_eq!(shebang_run_flags("#!/usr/bin/env -S rig"), s(&[]));
        assert_eq!(shebang_run_flags("# rig run --rscript"), s(&[]));
        assert_eq!(shebang_run_flags(""), s(&[]));
    }

    #[test]
    fn launcher_args_put_the_script_last() {
        let script = Path::new("/home/me/hello.R");
        assert_eq!(
            launcher_args(&s(&["--rscript"]), script),
            s(&["run", "--rscript", "-f", "/home/me/hello.R"])
        );
    }

    #[test]
    fn command_names() {
        assert!(check_command_name("hello").is_ok());
        assert!(check_command_name("my-tool").is_ok());
        for bad in [
            "",
            ".",
            "..",
            "a/b",
            "a\\b",
            "-x",
            "R",
            "r",
            "Rscript",
            "RS",
            "R-4.5",
            "r-release",
            "rig",
            "rig-shim",
        ] {
            assert!(check_command_name(bad).is_err(), "{}", bad);
        }
    }

    #[test]
    fn default_names() {
        assert_eq!(
            default_command_name(Path::new("/x/hello.R")).unwrap(),
            "hello"
        );
        assert_eq!(
            default_command_name(Path::new("/x/hello.r")).unwrap(),
            "hello"
        );
        assert_eq!(
            default_command_name(Path::new("/x/hello")).unwrap(),
            "hello"
        );
        assert_eq!(
            default_command_name(Path::new("/x/hello.sh")).unwrap(),
            "hello.sh"
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn wrapper_is_recognized_and_quoted() {
        let dir = tempfile::tempdir().unwrap();
        let script = Path::new("/home/me/it's here.R");
        let args = launcher_args(&s(&["--rscript"]), script);
        let path = dir.path().join("hello");
        write_command(&path, "/usr/local/bin/rig", script, &args).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("#!/bin/sh\n"));
        assert!(text.contains(
            "exec '/usr/local/bin/rig' 'run' '--rscript' '-f' '/home/me/it'\\''s here.R' \"$@\"\n"
        ));
        assert_eq!(
            installed_script(&path).as_deref(),
            Some("/home/me/it's here.R")
        );

        let other = dir.path().join("other");
        std::fs::write(&other, "#!/bin/sh\necho hi\n").unwrap();
        assert!(installed_script(&other).is_none());
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn wrapper_runs_with_the_right_arguments() {
        let dir = tempfile::tempdir().unwrap();
        // A fake `rig` that prints its arguments, one per line.
        let fake = dir.path().join("rig");
        std::fs::write(
            &fake,
            "#!/bin/sh\nfor a in \"$@\"; do echo \"[$a]\"; done\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let script = Path::new("/x/hello.R");
        let path = dir.path().join("hello");
        write_command(
            &path,
            fake.to_str().unwrap(),
            script,
            &launcher_args(&[], script),
        )
        .unwrap();
        let out = std::process::Command::new(&path)
            .args(["a b", "--", "-f"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "[run]\n[-f]\n[/x/hello.R]\n[a b]\n[--]\n[-f]\n"
        );
    }
}
