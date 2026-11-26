//! A filesystem sandbox for building source packages.
//!
//! Building a source package runs code that comes with the package: its
//! `configure` script, its `Makevars`, and its R code, which `R CMD INSTALL`
//! evaluates to lazy-load the package and loads again to test it. In the
//! sandbox that code can still read whatever the user can, but it can only
//! write into the directories rig set up for this one build. It cannot change
//! rig's caches, other installed packages, the user's files or rig itself, so
//! whatever a build does, it only affects the package being built.
//!
//! This aligns with CRAN's policy, so it should work in most cases.
//!
//! On Linux the sandbox is Landlock, which needs no privileges and works in
//! containers. There is no sandbox support for other platforms.

use std::error::Error;
use std::path::Path;
use std::sync::OnceLock;

use log::{debug, warn};

use crate::output::OUTPUT;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxMode {
    /// Sandbox builds where the system supports it, and warn where it does not.
    Auto,
    /// Fail if builds cannot be sandboxed.
    Required,
    /// Do not sandbox builds.
    Off,
}

impl SandboxMode {
    pub fn parse(value: &str) -> Option<SandboxMode> {
        match value.trim().to_lowercase().as_str() {
            "auto" => Some(SandboxMode::Auto),
            "required" => Some(SandboxMode::Required),
            "off" => Some(SandboxMode::Off),
            _ => None,
        }
    }
}

static FLAG: OnceLock<SandboxMode> = OnceLock::new();
static MODE: OnceLock<SandboxMode> = OnceLock::new();

/// Record the mode given in the `--sandbox` flag for this run.
pub fn set_sandbox_mode(mode: SandboxMode) {
    let _ = FLAG.set(mode);
}

/// Whether and how builds are sandboxed in this run.
///
/// The `--sandbox` flag takes precedence over `RIG_SANDBOX`, which takes
/// precedence over the `sandbox` config entry.
pub fn sandbox_mode() -> Result<SandboxMode, Box<dyn Error>> {
    if let Some(mode) = MODE.get() {
        return Ok(*mode);
    }
    let flag = FLAG.get().copied();
    let env = std::env::var("RIG_SANDBOX").ok().filter(|v| !v.is_empty());
    let config = if flag.is_none() && env.is_none() {
        crate::config::get_global_config_value("sandbox")?
    } else {
        None
    };
    let mode = resolve_mode(flag, env.as_deref(), config.as_deref())?;
    let _ = MODE.set(mode);
    Ok(mode)
}

fn resolve_mode(
    flag: Option<SandboxMode>,
    env: Option<&str>,
    config: Option<&str>,
) -> Result<SandboxMode, Box<dyn Error>> {
    if let Some(mode) = flag {
        return Ok(mode);
    }
    let (source, value) = match (env, config) {
        (Some(env), _) => ("RIG_SANDBOX", env),
        (None, Some(config)) => ("'sandbox' in the rig config", config),
        (None, None) => return Ok(SandboxMode::Off),
    };
    match SandboxMode::parse(value) {
        Some(mode) => Ok(mode),
        None => bail!(
            "Invalid {} value: '{}', expected 'auto', 'required' or 'off'",
            source,
            value
        ),
    }
}

#[cfg(target_os = "linux")]
pub use linux::Sandbox;
#[cfg(not(target_os = "linux"))]
pub use unsupported::Sandbox;

impl Sandbox {
    /// A filesystem sandbox in which a build can only write below the
    /// `writable` directories, or `None` if this run does not sandbox builds.
    ///
    /// Errors if the sandbox is required but the system cannot provide it.
    pub fn for_build(writable: &[&Path]) -> Result<Option<Sandbox>, Box<dyn Error>> {
        let mode = sandbox_mode()?;
        if mode == SandboxMode::Off {
            return Ok(None);
        }
        match Sandbox::create(writable) {
            Ok(sandbox) => Ok(Some(sandbox)),
            Err(err) if mode == SandboxMode::Auto => {
                static WARNED: OnceLock<()> = OnceLock::new();
                if WARNED.set(()).is_ok() {
                    let msg = format!("Building packages without a sandbox: {}", err);
                    OUTPUT.warn(&msg);
                    warn!("{}", msg);
                } else {
                    debug!("Building without a sandbox: {}", err);
                }
                Ok(None)
            }
            Err(err) => bail!(
                "Cannot sandbox the build, which --sandbox=required asks for: {}",
                err
            ),
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::error::Error;
    use std::path::Path;

    use log::debug;

    /// The sandbox of one build.
    pub struct Sandbox {
        ruleset: std::os::fd::OwnedFd,
    }

    impl Sandbox {
        pub(super) fn create(writable: &[&Path]) -> Result<Sandbox, Box<dyn Error>> {
            use landlock::{
                AccessFs, CompatLevel, Compatible, PathBeneath, PathFd, Ruleset, RulesetAttr,
                RulesetCreatedAttr, Scope, ABI,
            };

            check_abi(landlock_abi())?;

            // Filesystem rights; always enforced.
            let handled = AccessFs::from_write(ABI::V9) & !AccessFs::IoctlDev;

            // These optional scopes keep the build from signalling or talking
            // to processes outside the sandbox, and we enable them when the
            // kernel is new enough to have them.
            //
            // Note that some kernels claim a Landlock ABI without all of its
            // scopes (e.g. RHEL 9.6 reports ABI 6 but refuses the signal
            // scope), so check them individually and enable as many as
            // possible.
            let scopes = [
                Scope::AbstractUnixSocket | Scope::Signal,
                Scope::AbstractUnixSocket.into(),
                Scope::Signal.into(),
                landlock::BitFlags::EMPTY,
            ];
            let mut created = None;
            let mut first_err = None;
            for scope in scopes {
                let mut ruleset = Ruleset::default()
                    .set_compatibility(CompatLevel::BestEffort)
                    .handle_access(handled)?;
                if !scope.is_empty() {
                    ruleset = ruleset.scope(scope)?;
                }
                match ruleset.create() {
                    Ok(ruleset) => {
                        created = Some(ruleset);
                        break;
                    }
                    Err(err) => {
                        debug!(
                            "Cannot create a Landlock ruleset with scopes {:?}: {}",
                            scope, err
                        );
                        first_err.get_or_insert(err);
                    }
                }
            }

            let mut ruleset = match (created, first_err) {
                (Some(ruleset), _) => ruleset,
                (None, Some(err)) => return Err(err.into()),
                (None, None) => bail!("cannot create a Landlock ruleset"),
            };

            for dir in writable {
                let fd = PathFd::new(dir).map_err(|err| format!("{}: {}", dir.display(), err))?;
                ruleset = ruleset.add_rule(PathBeneath::new(fd, handled))?;
            }

            // Writing to these is harmless, allowed by CRAN policy, and
            // required for many build tools.
            for device in ["/dev/null", "/dev/zero", "/dev/full", "/dev/tty"] {
                if let Ok(fd) = PathFd::new(device) {
                    let access = handled & AccessFs::from_file(ABI::V9);
                    ruleset = ruleset.add_rule(PathBeneath::new(fd, access))?;
                }
            }

            let ruleset: Option<std::os::fd::OwnedFd> = ruleset.into();
            match ruleset {
                Some(ruleset) => Ok(Sandbox { ruleset }),
                None => bail!("the kernel does not support Landlock"),
            }
        }

        /// Run `cmd` -- and all its subprocesses -- in the sandbox.
        ///
        /// The sandbox must live until `cmd` has been started.
        pub fn apply(&self, cmd: &mut tokio::process::Command) {
            use std::os::fd::AsRawFd;
            let fd = self.ruleset.as_raw_fd();
            // This runs in the child between `fork` and `exec` so must use
            // async-signal-safe calls and avoid allocation.
            unsafe {
                cmd.pre_exec(move || {
                    if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::syscall(libc::SYS_landlock_restrict_self, fd, 0) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
    }

    /// Whether a kernel with Landlock ABI `abi` can sandbox a build.
    fn check_abi(abi: i64) -> Result<(), Box<dyn Error>> {
        // ABI 1 refuses every rename or link into another directory in the
        // sandbox, which builds do all the time, and ABI 2 cannot stop
        // truncating a file by its path, which can destroy any file the user
        // can write. So ABI 3 is the minimum. Newer ABIs also restrict
        // connecting to Unix sockets (which could ask a process outside the
        // sandbox to write for the build), signalling processes outside the
        // sandbox, and abstract Unix sockets. We take advantage of these when
        // the kernel has them, but they're not strictly required.
        if abi < 3 {
            bail!(
                "this kernel has {}, rig needs Landlock ABI 3 or later (Linux 6.2)",
                match abi {
                    0 => "no Landlock, or it is disabled".to_string(),
                    abi => format!("Landlock ABI {}", abi),
                }
            );
        }
        Ok(())
    }

    /// The Landlock ABI version of the running kernel, 0 if it has none.
    fn landlock_abi() -> i64 {
        const LANDLOCK_CREATE_RULESET_VERSION: libc::c_uint = 1;
        let abi = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                std::ptr::null::<libc::c_void>(),
                0usize,
                LANDLOCK_CREATE_RULESET_VERSION,
            )
        };
        abi.max(0)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Whether this kernel can sandbox at all; the tests that need a real
        /// sandbox are skipped where it cannot.
        fn sandbox(writable: &[&Path]) -> Option<Sandbox> {
            match Sandbox::create(writable) {
                Ok(sandbox) => Some(sandbox),
                Err(err) => {
                    eprintln!("skipping, no sandbox: {}", err);
                    None
                }
            }
        }

        async fn sh(sandbox: &Sandbox, script: &str) -> bool {
            let mut cmd = tokio::process::Command::new("/bin/sh");
            cmd.arg("-c").arg(script);
            cmd.stderr(std::process::Stdio::null());
            sandbox.apply(&mut cmd);
            cmd.status().await.unwrap().success()
        }

        #[tokio::test]
        async fn a_sandboxed_build_writes_only_where_it_may() {
            let tmp = tempfile::tempdir().unwrap();
            let inside = tmp.path().join("inside");
            let outside = tmp.path().join("outside");
            std::fs::create_dir_all(&inside).unwrap();
            std::fs::create_dir_all(&outside).unwrap();
            std::fs::write(outside.join("file"), "x").unwrap();
            let Some(sandbox) = sandbox(&[&inside]) else {
                return;
            };

            let i = inside.display();
            let o = outside.display();
            assert!(
                sh(
                    &sandbox,
                    &format!("mkdir {i}/d && echo x > {i}/d/f && mv {i}/d/f {i}/g")
                )
                .await
            );
            assert!(sh(&sandbox, "echo x > /dev/null").await);
            assert!(!sh(&sandbox, &format!("echo x > {o}/new")).await);
            assert!(!sh(&sandbox, &format!("echo x >> {o}/file")).await);
            assert!(!sh(&sandbox, &format!("rm {o}/file")).await);
            assert!(!sh(&sandbox, &format!("mv {o}/file {i}/stolen")).await);
            assert!(!sh(&sandbox, &format!("ln -s /etc {o}/link")).await);
            // `truncate(2)` on the path, which needs no open file.
            if Path::new("/usr/bin/perl").exists() {
                assert!(
                    !sh(
                        &sandbox,
                        &format!("/usr/bin/perl -e 'truncate(\"{o}/file\", 0) or exit 1'")
                    )
                    .await
                );
            }
            assert!(!outside.join("new").exists());
            assert!(!outside.join("link").exists());
            assert_eq!(std::fs::read_to_string(outside.join("file")).unwrap(), "x");
            assert!(inside.join("g").exists());
        }

        #[test]
        fn landlock_abi_3_is_the_minimum() {
            assert!(check_abi(0).is_err());
            assert!(check_abi(1).is_err());
            assert!(check_abi(2).is_err());
            assert!(check_abi(3).is_ok());
            assert!(check_abi(9).is_ok());
        }

        /// Landlock rules belong to directories, not paths: once rig has moved
        /// a build's output out of the writable directory, whatever the build
        /// left running can no longer change it.
        #[tokio::test]
        async fn a_moved_build_output_is_out_of_reach() {
            let tmp = tempfile::tempdir().unwrap();
            let inside = tmp.path().join("inside");
            std::fs::create_dir_all(inside.join("pkg")).unwrap();
            let Some(sandbox) = sandbox(&[&inside]) else {
                return;
            };
            let moved = tmp.path().join("pkg");
            std::fs::rename(inside.join("pkg"), &moved).unwrap();
            assert!(!sh(&sandbox, &format!("echo x > {}/f", moved.display())).await);
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod unsupported {
    use std::error::Error;
    use std::path::Path;

    /// The sandbox of one build.
    pub struct Sandbox {}

    impl Sandbox {
        pub(super) fn create(_writable: &[&Path]) -> Result<Sandbox, Box<dyn Error>> {
            bail!("rig has no sandbox on this platform")
        }

        /// Run `cmd`, and everything it starts, in the sandbox.
        pub fn apply(&self, _cmd: &mut tokio::process::Command) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mode_is_off_unless_asked_for() {
        assert_eq!(resolve_mode(None, None, None).unwrap(), SandboxMode::Off);
    }

    #[test]
    fn the_flag_beats_the_environment_beats_the_config() {
        use SandboxMode::*;
        assert_eq!(
            resolve_mode(Some(Off), Some("required"), Some("auto")).unwrap(),
            Off
        );
        assert_eq!(
            resolve_mode(None, Some("required"), Some("auto")).unwrap(),
            Required
        );
        assert_eq!(resolve_mode(None, None, Some("auto")).unwrap(), Auto);
        assert_eq!(
            resolve_mode(None, Some(" Required "), None).unwrap(),
            Required
        );
    }

    #[test]
    fn an_invalid_mode_is_an_error() {
        assert!(resolve_mode(None, Some("requried"), None).is_err());
        assert!(resolve_mode(None, None, Some("yes")).is_err());
    }
}
