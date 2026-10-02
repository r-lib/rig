//! Run the install commands, the way pak does: directly as root, with `sudo`
//! if that needs no password, and otherwise not at all.

use std::error::Error;
use std::ffi::OsString;

use log::{debug, info};

use crate::run::run;

/// How the commands can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Privilege {
    Root,
    /// With `sudo`. `interactive` allows it to ask for a password.
    Sudo {
        interactive: bool,
    },
    /// rig cannot run them, the user has to.
    None,
}

/// The `sysreqs-sudo` setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SudoSetting {
    /// Use `sudo` if it works without a password.
    Auto,
    /// Use `sudo`, even if it asks for a password.
    Always,
    Never,
}

#[cfg(unix)]
fn is_root() -> bool {
    nix::unistd::geteuid().is_root()
}

#[cfg(not(unix))]
fn is_root() -> bool {
    false
}

/// Whether `sudo` runs a command without asking for a password.
fn passwordless_sudo() -> bool {
    duct::cmd!("sudo", "-n", "true")
        .stdin_null()
        .stdout_null()
        .stderr_null()
        .unchecked()
        .run()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn privilege(setting: SudoSetting) -> Privilege {
    decide_privilege(is_root(), setting, passwordless_sudo)
}

/// [`privilege`], with the checks passed in, for the tests. `sudo -n` only
/// runs when its answer matters.
pub fn decide_privilege(
    root: bool,
    setting: SudoSetting,
    passwordless_sudo: impl Fn() -> bool,
) -> Privilege {
    if root {
        return Privilege::Root;
    }
    match setting {
        SudoSetting::Never => Privilege::None,
        SudoSetting::Always => Privilege::Sudo { interactive: true },
        SudoSetting::Auto if passwordless_sudo() => Privilege::Sudo { interactive: false },
        SudoSetting::Auto => Privilege::None,
    }
}

/// One command to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// A shell command line from a rule, run with `sh -c`.
    Shell(String),
    Argv(Vec<String>),
}

impl Step {
    /// How to show the step to the user, e.g. for a manual install.
    pub fn display(&self, sudo: bool) -> String {
        match (self, sudo) {
            (Step::Shell(cmd), false) => cmd.clone(),
            // `sudo` has to cover the whole command line, not just its first
            // command.
            (Step::Shell(cmd), true) => format!("sudo sh -c '{}'", cmd.replace('\'', "'\\''")),
            (Step::Argv(argv), false) => argv.join(" "),
            (Step::Argv(argv), true) => format!("sudo {}", argv.join(" ")),
        }
    }

    /// The program and its arguments, with `privilege` applied.
    pub fn command(&self, privilege: Privilege) -> (OsString, Vec<OsString>) {
        let mut argv: Vec<String> = match self {
            Step::Shell(cmd) => vec!["sh".to_string(), "-c".to_string(), cmd.clone()],
            Step::Argv(argv) => argv.clone(),
        };
        match privilege {
            Privilege::Sudo { interactive: true } => argv.insert(0, "sudo".to_string()),
            Privilege::Sudo { interactive: false } => {
                argv.insert(0, "-n".to_string());
                argv.insert(0, "sudo".to_string());
            }
            Privilege::Root | Privilege::None => {}
        }
        let mut argv = argv.into_iter().map(OsString::from);
        let program = argv.next().unwrap_or_default();
        (program, argv.collect())
    }
}

/// Run `steps` in order, stopping at the first failure.
pub fn run_steps(steps: &[Step], privilege: Privilege) -> Result<(), Box<dyn Error>> {
    for step in steps {
        let (program, args) = step.command(privilege);
        info!("Running {}", step.display(false));
        debug!("{:?} {:?}", program, args);
        run(program, args, "system requirements")
            .map_err(|e| format!("`{}` failed: {}", step.display(false), e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privilege_follows_pak() {
        let never = || -> bool { panic!("sudo -n must not run") };
        assert_eq!(
            decide_privilege(true, SudoSetting::Auto, never),
            Privilege::Root
        );
        assert_eq!(
            decide_privilege(true, SudoSetting::Never, never),
            Privilege::Root
        );
        assert_eq!(
            decide_privilege(false, SudoSetting::Auto, || true),
            Privilege::Sudo { interactive: false }
        );
        assert_eq!(
            decide_privilege(false, SudoSetting::Auto, || false),
            Privilege::None
        );
        assert_eq!(
            decide_privilege(false, SudoSetting::Never, never),
            Privilege::None
        );
        assert_eq!(
            decide_privilege(false, SudoSetting::Always, never),
            Privilege::Sudo { interactive: true }
        );
    }

    #[test]
    fn steps_get_sudo() {
        let step = Step::Argv(vec![
            "apt-get".into(),
            "install".into(),
            "-y".into(),
            "zlib1g-dev".into(),
        ]);
        let (p, a) = step.command(Privilege::Sudo { interactive: false });
        assert_eq!(p, "sudo");
        assert_eq!(a, vec!["-n", "apt-get", "install", "-y", "zlib1g-dev"]);
        let (p, a) = step.command(Privilege::Root);
        assert_eq!(p, "apt-get");
        assert_eq!(a.len(), 3);

        let step = Step::Shell("rpm -q epel-release || dnf install -y epel-release".into());
        let (p, a) = step.command(Privilege::Sudo { interactive: true });
        assert_eq!(p, "sudo");
        assert_eq!(a[0], "sh");
        assert_eq!(a[1], "-c");
        assert_eq!(
            step.display(true),
            "sudo sh -c 'rpm -q epel-release || dnf install -y epel-release'"
        );
        assert_eq!(
            Step::Shell("echo 'hi'".into()).display(true),
            "sudo sh -c 'echo '\\''hi'\\'''"
        );
    }
}
