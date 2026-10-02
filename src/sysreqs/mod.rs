//! System requirements: the OS packages R packages need on Linux, to compile
//! a source package or to load a binary built for a distribution.
//!
//! The lockfile records each package's `SystemRequirements` field (see
//! `RprojLockPackage::system_requirements`). Before installing, [`ensure`]
//! matches these against the r-system-requirements rules for the local
//! distribution, finds the OS packages that are not installed, and installs
//! them, the way pak does: as root, or with `sudo` if it needs no password.
//! Otherwise it shows the commands to run.
//!
//! Matching works on every OS, see [`resolve::resolve`], so that rig can list
//! the system requirements of a Linux platform anywhere. Only checking and
//! installing OS packages is Linux-only.

pub mod install;
pub mod installed;
pub mod platform;
pub mod resolve;
pub mod rules;

use std::collections::HashMap;
use std::error::Error;
use std::sync::Mutex;

use log::{info, warn};

use crate::output::OUTPUT;
use crate::platform::detect_platform;
use crate::rproj::RprojLockPackage;

use install::{privilege, run_steps, Privilege, Step, SudoSetting};
use platform::SysreqsSystem;
use resolve::{collect, resolve, MatchedRule};
use rules::RuleDb;

/// The `sysreqs` setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Install system requirements on the Linux distributions the rules
    /// know, quietly skip them on the others.
    Auto,
    /// Like `Auto`, but warn on an unknown distribution.
    On,
    /// Show the missing OS packages and the commands that install them, but
    /// do not run the commands.
    Print,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub mode: Mode,
    pub sudo: SudoSetting,
    /// Refresh the package index before installing.
    pub update: bool,
}

/// A setting from its environment variable, else from the rig config file.
fn setting(env: &str, key: &str) -> Result<Option<String>, Box<dyn Error>> {
    if let Ok(val) = std::env::var(env) {
        if !val.trim().is_empty() {
            return Ok(Some(val.trim().to_string()));
        }
    }
    if let Some(val) = crate::config::get_global_config_value(key)? {
        return Ok(Some(val));
    }
    Ok(crate::config::get_global_config_bool(key)?.map(|b| b.to_string()))
}

fn parse_bool(name: &str, value: &str) -> Result<bool, Box<dyn Error>> {
    match value.to_lowercase().as_str() {
        "true" | "yes" | "1" => Ok(true),
        "false" | "no" | "0" => Ok(false),
        _ => bail!("Invalid {}: '{}', expected 'true' or 'false'", name, value),
    }
}

fn parse_mode(name: &str, value: &str) -> Result<Mode, Box<dyn Error>> {
    if value.eq_ignore_ascii_case("auto") {
        Ok(Mode::Auto)
    } else if parse_bool(name, value)? {
        Ok(Mode::On)
    } else {
        Ok(Mode::Off)
    }
}

/// The `sysreqs` setting: `auto`, `print`, or a boolean.
fn parse_sysreqs_mode(name: &str, value: &str) -> Result<Mode, Box<dyn Error>> {
    if value.eq_ignore_ascii_case("print") {
        return Ok(Mode::Print);
    }
    parse_mode(name, value).map_err(|_| {
        format!(
            "Invalid {}: '{}', expected 'auto', 'true', 'false' or 'print'",
            name, value
        )
        .into()
    })
}

/// `--sysreqs` / `--no-sysreqs`, `None` if neither is given, or if the
/// command does not have them, e.g. `rig run`, which syncs too.
pub fn cli_flag(args: &clap::ArgMatches) -> Option<bool> {
    if matches!(args.try_get_one::<bool>("sysreqs"), Ok(Some(true))) {
        Some(true)
    } else if matches!(args.try_get_one::<bool>("no-sysreqs"), Ok(Some(true))) {
        Some(false)
    } else {
        None
    }
}

/// The settings, with `cli` (`--sysreqs` / `--no-sysreqs`) taking precedence
/// over `RIG_SYSREQS` and the `sysreqs` config key. pak's `PKG_SYSREQS=false`
/// environment variable turns them off as well, unless rig's own setting says
/// otherwise.
pub fn settings(cli: Option<bool>) -> Result<Settings, Box<dyn Error>> {
    let mode = match cli {
        Some(true) => Mode::On,
        Some(false) => Mode::Off,
        None => match setting("RIG_SYSREQS", "sysreqs")? {
            Some(val) => parse_sysreqs_mode("sysreqs setting", &val)?,
            None => match std::env::var("PKG_SYSREQS") {
                Ok(val) if parse_bool("PKG_SYSREQS", &val).ok() == Some(false) => Mode::Off,
                _ => Mode::Auto,
            },
        },
    };
    let sudo = match setting("RIG_SYSREQS_SUDO", "sysreqs-sudo")? {
        Some(val) => match parse_mode("sysreqs-sudo setting", &val)? {
            Mode::Auto => SudoSetting::Auto,
            Mode::On => SudoSetting::Always,
            Mode::Off | Mode::Print => SudoSetting::Never,
        },
        None => SudoSetting::Auto,
    };
    let update = match setting("RIG_SYSREQS_UPDATE", "sysreqs-update")? {
        Some(val) => parse_bool("sysreqs-update setting", &val)?,
        None => true,
    };
    Ok(Settings { mode, sudo, update })
}

lazy_static::lazy_static! {
    /// The OS packages each R package still lacks after [`ensure`], for the
    /// message of a failed install.
    static ref STILL_MISSING: Mutex<HashMap<String, Vec<String>>> = Mutex::new(HashMap::new());
}

/// The OS packages `package` needs that [`ensure`] could not install.
pub fn still_missing(package: &str) -> Vec<String> {
    STILL_MISSING
        .lock()
        .ok()
        .and_then(|m| m.get(package).cloned())
        .unwrap_or_default()
}

fn remember_missing(rules: &[&MatchedRule], missing: &[String]) {
    let Ok(mut map) = STILL_MISSING.lock() else {
        return;
    };
    map.clear();
    for rule in rules {
        for pkg in &rule.r_packages {
            let entry = map.entry(pkg.clone()).or_default();
            for os in rule.packages.iter().filter(|p| missing.contains(p)) {
                if !entry.contains(os) {
                    entry.push(os.clone());
                }
            }
        }
    }
}

/// The `(package, SystemRequirements)` of the packages that record one.
fn requirements(packages: &[&RprojLockPackage]) -> Vec<(String, String)> {
    packages
        .iter()
        .filter_map(|p| {
            p.system_requirements
                .as_ref()
                .map(|s| (p.package.clone(), s.clone()))
        })
        .collect()
}

/// The commands that install `missing`.
fn install_steps(
    system: &SysreqsSystem,
    rules: &[&MatchedRule],
    missing: &[String],
    update: bool,
) -> Vec<Step> {
    let mut steps: Vec<Step> = collect(rules, |r| &r.pre_install)
        .into_iter()
        .map(Step::Shell)
        .collect();
    if update {
        if let Some(cmd) = system.update_command() {
            steps.push(Step::Argv(cmd));
        }
    }
    steps.push(Step::Argv(system.install_command(missing)));
    steps.extend(
        collect(rules, |r| &r.post_install)
            .into_iter()
            .map(Step::Shell),
    );
    steps
}

/// What [`check`] found.
#[derive(Debug)]
pub enum Checked {
    /// The rules do not know the local Linux distribution, e.g.
    /// `amzn 2023`.
    Unsupported(String),
    /// Every OS package that the R packages need is installed. `needed` is
    /// empty if they need none.
    Installed {
        needed: Vec<String>,
    },
    Missing(Missing),
}

/// The OS packages that are missing, and the rules that need them.
#[derive(Debug)]
pub struct Missing {
    pub system: SysreqsSystem,
    /// The rules that need at least one of `missing`.
    pub rules: Vec<MatchedRule>,
    pub missing: Vec<String>,
}

impl Missing {
    /// The R packages that need the OS package `os`, sorted.
    pub fn needed_by(&self, os: &str) -> Vec<&str> {
        let mut out: Vec<&str> = self
            .rules
            .iter()
            .filter(|r| r.packages.iter().any(|p| p == os))
            .flat_map(|r| r.r_packages.iter().map(|p| p.as_str()))
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// The missing OS packages that the R package `pkg` needs.
    pub fn for_package(&self, pkg: &str) -> Vec<String> {
        let rules: Vec<&MatchedRule> = self
            .rules
            .iter()
            .filter(|r| r.r_packages.iter().any(|p| p == pkg))
            .collect();
        collect(&rules, |r| &r.packages)
            .into_iter()
            .filter(|p| self.missing.contains(p))
            .collect()
    }

    /// The commands that install the missing OS packages.
    pub fn steps(&self, update: bool) -> Vec<Step> {
        let rules: Vec<&MatchedRule> = self.rules.iter().collect();
        install_steps(&self.system, &rules, &self.missing, update)
    }
}

/// Which of the OS packages `packages` need, given as `(R package,
/// SystemRequirements)`, are missing on this machine. Only works on Linux,
/// elsewhere it reports an unsupported system.
pub fn check(packages: &[(String, String)]) -> Result<Checked, Box<dyn Error>> {
    let platform = detect_platform()?;
    let Some(system) = SysreqsSystem::from_os_version(&platform) else {
        return Ok(Checked::Unsupported(format!(
            "{} {}",
            platform.distro.as_deref().unwrap_or("unknown"),
            platform.version.as_deref().unwrap_or("")
        )));
    };
    info!("Checking system requirements on {}", system.display());
    let db = RuleDb::load()?;
    let matched = resolve(&db, &system, packages);
    let all: Vec<&MatchedRule> = matched.iter().collect();
    let needed = collect(&all, |r| &r.packages);
    if needed.is_empty() {
        return Ok(Checked::Installed { needed });
    }
    let missing = installed::missing(&system, &needed)?;
    if missing.is_empty() {
        return Ok(Checked::Installed { needed });
    }
    let rules = matched
        .into_iter()
        .filter(|r| r.packages.iter().any(|p| missing.contains(p)))
        .collect();
    Ok(Checked::Missing(Missing {
        system,
        rules,
        missing,
    }))
}

/// Install the OS packages that `packages` need and that are missing, before
/// rig installs `packages` themselves. Does nothing except on Linux.
///
/// `cli` is `--sysreqs` / `--no-sysreqs`. With `dry_run`, only shows what it
/// would do. A failure to install is not an error: the R packages may
/// install anyway, and if not, their failure names the missing OS packages.
pub fn ensure(
    packages: &[&RprojLockPackage],
    cli: Option<bool>,
    dry_run: bool,
) -> Result<(), Box<dyn Error>> {
    if !cfg!(target_os = "linux") {
        return Ok(());
    }
    let settings = settings(cli)?;
    if settings.mode == Mode::Off {
        info!("System requirements are turned off");
        return Ok(());
    }
    let wanted = requirements(packages);
    if wanted.is_empty() {
        return Ok(());
    }

    // Before saying anything about checking: on a distribution the rules do
    // not know there is nothing to check.
    let platform = detect_platform()?;
    if SysreqsSystem::from_os_version(&platform).is_none() {
        let msg = format!(
            "Not installing system requirements: rig does not know the system \
             packages of this Linux distribution ({} {})",
            platform.distro.as_deref().unwrap_or("unknown"),
            platform.version.as_deref().unwrap_or("")
        );
        if matches!(settings.mode, Mode::On | Mode::Print) {
            OUTPUT.warn(&msg);
        }
        info!("{}", msg);
        return Ok(());
    }

    OUTPUT.status("Checking system requirements");
    let found = match check(&wanted) {
        // Checked above already.
        Ok(Checked::Unsupported(_)) => return Ok(()),
        Ok(Checked::Installed { needed }) => {
            if !needed.is_empty() {
                let word = if needed.len() == 1 {
                    "package is"
                } else {
                    "packages are"
                };
                OUTPUT.success(&format!(
                    "All {} required system {} installed",
                    needed.len(),
                    word
                ));
            }
            info!(
                "All system requirements are installed: {}",
                needed.join(", ")
            );
            return Ok(());
        }
        Ok(Checked::Missing(found)) => found,
        Err(e) => {
            let msg = format!("Cannot check which system packages are installed: {}", e);
            OUTPUT.warn(&msg);
            warn!("{}", msg);
            return Ok(());
        }
    };

    let rules: Vec<&MatchedRule> = found.rules.iter().collect();
    let missing = &found.missing;
    remember_missing(&rules, missing);
    let word = if missing.len() == 1 {
        "package"
    } else {
        "packages"
    };
    OUTPUT.status(&format!("Missing {} system {}:", missing.len(), word));
    for os in missing {
        OUTPUT.println(&format!(
            "  {} (for {})",
            os,
            found.needed_by(os).join(", ")
        ));
    }
    info!("Missing system packages: {}", missing.join(", "));

    let steps = found.steps(settings.update);
    let print_only = dry_run || settings.mode == Mode::Print;
    let privilege = if print_only {
        Privilege::None
    } else {
        privilege(settings.sudo)
    };

    if print_only {
        OUTPUT.info("Would install them with:");
    } else if privilege == Privilege::None {
        OUTPUT.warn(
            "Cannot install system packages: rig is not running as root and \
             `sudo` needs a password. Install them with:",
        );
    }
    if privilege == Privilege::None {
        for step in &steps {
            OUTPUT.println(&format!("  {}", step.display(true)));
        }
        return Ok(());
    }

    OUTPUT.status(&format!("Installing {} system {}", missing.len(), word));
    match run_steps(&steps, privilege) {
        Ok(()) => {
            remember_missing(&[], &[]);
            OUTPUT.success(&format!("Installed {} system {}", missing.len(), word));
            info!("Installed system packages: {}", missing.join(", "));
        }
        Err(e) => {
            let msg = format!("Failed to install system packages: {}", e);
            OUTPUT.warn(&msg);
            warn!("{}", msg);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_steps_in_order() {
        let system = SysreqsSystem::new("rhel", "9").unwrap();
        let rule = MatchedRule {
            rule: "gdal".to_string(),
            packages: vec!["gdal-devel".to_string()],
            pre_install: vec!["dnf install -y epel-release".to_string()],
            post_install: vec![],
            r_packages: vec!["sf".to_string()],
        };
        let steps = install_steps(&system, &[&rule], &["gdal-devel".to_string()], true);
        assert_eq!(
            steps,
            vec![
                Step::Shell("dnf install -y epel-release".to_string()),
                Step::Argv(vec![
                    "dnf".to_string(),
                    "install".to_string(),
                    "-y".to_string(),
                    "gdal-devel".to_string()
                ]),
            ]
        );

        let system = SysreqsSystem::new("ubuntu", "24.04").unwrap();
        let steps = install_steps(&system, &[], &["zlib1g-dev".to_string()], true);
        assert_eq!(
            steps[0],
            Step::Argv(vec!["apt-get".to_string(), "update".to_string()])
        );
        let steps = install_steps(&system, &[], &["zlib1g-dev".to_string()], false);
        assert_eq!(steps.len(), 1);
    }

    #[test]
    fn remembers_what_is_still_missing() {
        let rule = MatchedRule {
            rule: "libcurl".to_string(),
            packages: vec!["libcurl4-openssl-dev".to_string()],
            pre_install: vec![],
            post_install: vec![],
            r_packages: vec!["curl".to_string()],
        };
        remember_missing(&[&rule], &["libcurl4-openssl-dev".to_string()]);
        assert_eq!(still_missing("curl"), vec!["libcurl4-openssl-dev"]);
        assert!(still_missing("cli").is_empty());
        remember_missing(&[], &[]);
        assert!(still_missing("curl").is_empty());
    }

    #[test]
    fn modes() {
        assert_eq!(parse_mode("x", "auto").unwrap(), Mode::Auto);
        assert_eq!(parse_mode("x", "TRUE").unwrap(), Mode::On);
        assert_eq!(parse_mode("x", "false").unwrap(), Mode::Off);
        assert!(parse_mode("x", "maybe").is_err());
        assert_eq!(parse_sysreqs_mode("x", "print").unwrap(), Mode::Print);
        assert_eq!(parse_sysreqs_mode("x", "auto").unwrap(), Mode::Auto);
        assert!(parse_sysreqs_mode("x", "maybe").is_err());
        // `print` is only a `sysreqs` value.
        assert!(parse_mode("x", "print").is_err());
    }
}
