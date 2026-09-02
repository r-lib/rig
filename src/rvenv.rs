//! The project virtual environment (`.rvenv`) layout.
//!
//! A rig project keeps its package library in `.rvenv/lib`, next to the
//! `rproj.toml` manifest and the `rproj.lock` lock file. `rig proj init`
//! creates the part of that layout which is committed to version control:
//!
//! ```text
//! project-root/
//!   rproj.toml              # tracked -- manifest
//!   .Renviron               # tracked -- in-session activation
//!   .gitignore              # tracked -- /.rvenv/* + !/.rvenv/lib
//!   .rvenv/
//!     lib/.gitignore        # tracked -- keeps lib/ and lib/rig, ignores the rest
//!     lib/rig/              # tracked -- pre-built shim package
//!     lib/...               # untracked -- the real dependencies, from `rig proj sync`
//!     lib/.synced           # untracked -- sync stamp, the lock file's md5 sum
//! ```
//!
//! Two things here are less obvious than they look.
//!
//! `.rvenv/lib` itself is committed (as a directory containing only a
//! `.gitignore`) because vanilla R does not create a missing `R_LIBS_USER`
//! directory -- `install.packages()` against a missing one just fails. Only
//! rig's own R installations create it, from a block rig injects into
//! `Rprofile.site` (see `library_update_rprofile`), and a project has to work
//! on any R install. Note that git will not look inside an ignored directory
//! for a nested exception, so `/.rvenv/*` has to be followed by
//! `!/.rvenv/lib` -- and the same double negation is needed for `rig/` inside
//! `lib/.gitignore`.
//!
//! The shim package in `.rvenv/lib/rig` is committed pre-built, rather than
//! installed by `rig proj sync`, because its whole job is to be there
//! *before* the first sync: `.Renviron` names it in `R_DEFAULT_PACKAGES`, so
//! without it R prints its own unhelpful "package 'rig' in
//! options(\"defaultPackages\") was not found". See `src/data/rvenv-pkg` for
//! the source and `xtask/src/rvenv_shim.rs` for the build.
//!
//! Never write a file named `___default` into `.rvenv/lib`: that is the
//! sentinel of rig's own per-version library switching, in the same
//! `Rprofile.site` block.

use std::error::Error;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use simple_error::bail;

use crate::hardcoded::{HC_RVENV_SHIM_35, HC_RVENV_SHIM_40, HC_RVENV_SHIM_LT_35};
use crate::rproj::RPROJ_MANIFEST_FILE;
use crate::utils::write_atomically;

pub const RVENV_DIR: &str = ".rvenv";
pub const RVENV_LIB_SUBDIR: &str = "lib";
pub const RVENV_SHIM_PKG: &str = "rig";
pub const RVENV_RENVIRON_FILE: &str = ".Renviron";
pub const RVENV_GITIGNORE_FILE: &str = ".gitignore";
pub const RPROJ_LOCK_FILE: &str = "rproj.lock";

/// The stamp file `rig proj sync` writes into the project library: a copy of
/// the lock file it installed from. The shim package compares it to
/// `rproj.lock` to decide whether to warn about an unsynced project. A copy
/// rather than a hash, so that both sides only need to read files: base R has
/// no sha256, and md5 would mean one more dependency on the rig side.
pub const RVENV_SYNC_STAMP: &str = ".synced";

/// Record that the project library now matches `lock_file`.
pub fn write_sync_stamp(lib: &Path, lock_file: &Path) -> Result<(), Box<dyn Error>> {
    let lock = fs::read(lock_file)?;
    write_atomically(&lib.join(RVENV_SYNC_STAMP), &lock)
}

const GITIGNORE_START: &str = "# rig rvenv start";
const GITIGNORE_END: &str = "# rig rvenv end";

// -------------------------------------------------------------- file bodies --

/// The tracked project `.Renviron`.
///
/// This is the "in-session activation" leg: IDEs (RStudio, Positron, VS Code)
/// start R themselves, so there is no wrapper script and no `PATH` entry to
/// hook into. A project `.Rprofile` would be the obvious alternative, but it
/// *shadows* the user's `~/.Rprofile` entirely, which is renv's most
/// complained-about behavior. A project `.Renviron` shadows `~/.Renviron` the
/// same way, but the shim package restores it with `readRenviron()`.
///
/// `R_DEFAULT_PACKAGES` *replaces* the default package list rather than
/// prepending to it, so the whole list has to be spelled out here; leaving
/// out e.g. `stats` would silently drop it from `search()`.
fn renviron_body() -> &'static str {
    "\
# Managed by rig (rig proj init).
#
# R_LIBS_USER is deliberately relative: the `rig` package in .rvenv/lib
# re-exports it as an absolute path at load time, so child R processes
# started from a subdirectory still see the project library.
#
# R_DEFAULT_PACKAGES replaces R's default package list rather than adding to
# it, so the whole list has to be spelled out.
#
# Note that `R --vanilla` ignores this file entirely.
R_LIBS_USER=.rvenv/lib
R_DEFAULT_PACKAGES=rig,datasets,utils,grDevices,graphics,stats,methods
"
}

/// The block rig manages in the project's root `.gitignore`. Everything in
/// `.rvenv` is machine-specific except the library directory itself.
fn root_gitignore_block() -> String {
    format!(
        "\
{}
# Everything in .rvenv is machine-specific, except the library directory
# itself and the rig shim package in it. git does not look inside an ignored
# directory for a nested exception, hence the second line.
/.rvenv/*
!/.rvenv/lib
{}
",
        GITIGNORE_START, GITIGNORE_END
    )
}

/// `.rvenv/lib/.gitignore`: keep the directory and the shim package, ignore
/// the installed dependencies.
fn lib_gitignore_body() -> &'static str {
    "\
# Managed by rig (rig proj init). The library directory itself is committed,
# because vanilla R does not create a missing R_LIBS_USER. The rig shim
# package is committed so that a fresh clone works before the first
# `rig proj sync`. Everything else here is installed and not tracked.
*
!.gitignore
!rig
!rig/**
"
}

// -------------------------------------------------------------------- paths --

/// `<root>/.rvenv/lib`, the project package library.
pub fn project_library(root: &Path) -> PathBuf {
    root.join(RVENV_DIR).join(RVENV_LIB_SUBDIR)
}

/// The project root at or above `start`: the nearest directory holding an
/// `rproj.toml`, an `rproj.lock` or an `.rvenv` directory.
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut dir = start;
    loop {
        if dir.join(RPROJ_MANIFEST_FILE).exists()
            || dir.join(RPROJ_LOCK_FILE).exists()
            || dir.join(RVENV_DIR).is_dir()
        {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?;
    }
}

/// Everything `rig proj init` writes, in write order.
pub fn init_targets(root: &Path) -> Vec<PathBuf> {
    vec![
        root.join(RPROJ_MANIFEST_FILE),
        root.join(RVENV_RENVIRON_FILE),
        root.join(RVENV_GITIGNORE_FILE),
        project_library(root).join(RVENV_GITIGNORE_FILE),
        project_library(root).join(RVENV_SHIM_PKG),
    ]
}

/// The paths from [`init_targets`] that are already there, so that the
/// caller can name all of them at once instead of failing on the first.
///
/// The root `.gitignore` is special: rig only manages a marked block in it,
/// so an existing one that already has that block is not a conflict, it is
/// just a re-init.
pub fn existing_targets(root: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let gitignore = root.join(RVENV_GITIGNORE_FILE);
    let gitignore_is_ours = gitignore.exists() && gitignore_block(&gitignore)?.is_some();
    Ok(init_targets(root)
        .into_iter()
        .filter(|p| p.exists())
        .filter(|p| !(*p == gitignore && gitignore_is_ours))
        .collect())
}

// --------------------------------------------------------------- .gitignore --

/// The line range of rig's block in a `.gitignore` file, if it has one.
fn gitignore_block(path: &Path) -> Result<Option<(usize, usize)>, Box<dyn Error>> {
    let text = fs::read_to_string(path)?;
    let starts: Vec<usize> = text
        .lines()
        .enumerate()
        .filter(|(_, l)| l.trim() == GITIGNORE_START)
        .map(|(i, _)| i)
        .collect();
    let ends: Vec<usize> = text
        .lines()
        .enumerate()
        .filter(|(_, l)| l.trim() == GITIGNORE_END)
        .map(|(i, _)| i)
        .collect();
    match (starts.len(), ends.len()) {
        (0, 0) => Ok(None),
        (1, 1) if starts[0] < ends[0] => Ok(Some((starts[0], ends[0]))),
        _ => bail!(
            "{} has a malformed `{}` / `{}` block, fix it by hand",
            path.display(),
            GITIGNORE_START,
            GITIGNORE_END
        ),
    }
}

/// Create the project's root `.gitignore`, or add rig's block to it.
///
/// Never rewrites the whole file: an existing `.gitignore` is a file the user
/// (or `usethis`, or a git template) wrote, and clobbering it would drop
/// their ignore rules. Same fenced-block approach as
/// `library_update_rprofile` uses for `Rprofile.site`. Running this twice is
/// a no-op.
pub fn update_root_gitignore(root: &Path) -> Result<(), Box<dyn Error>> {
    let path = root.join(RVENV_GITIGNORE_FILE);
    let block = root_gitignore_block();
    if !path.exists() {
        write_atomically(&path, block.as_bytes())?;
        return Ok(());
    }

    let text = fs::read_to_string(&path)?;
    let new = match gitignore_block(&path)? {
        Some((start, end)) => {
            let lines: Vec<&str> = text.lines().collect();
            let mut out = lines[..start].join("\n");
            if start > 0 {
                out.push('\n');
            }
            out.push_str(&block);
            if end + 1 < lines.len() {
                out.push_str(&lines[end + 1..].join("\n"));
                out.push('\n');
            }
            out
        }
        None => {
            let mut out = text;
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&block);
            out
        }
    };
    write_atomically(&path, new.as_bytes())
}

// -------------------------------------------------------------- shim package --

/// Which pre-built copy of the shim package an R version can load.
///
/// R's installed-package format has two boundaries: serialization format 3
/// (the default from R 3.6.0) cannot be read by R < 3.5.0, and a package
/// installed by R < 4.0.0 is rejected by R >= 4.0.0. See
/// `xtask/src/rvenv_shim.rs` for the measured matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShimBracket {
    /// R < 3.5.0
    Lt35,
    /// R >= 3.5.0, < 4.0.0
    R35,
    /// R >= 4.0.0
    R40,
}

pub fn shim_bracket(r_version: &str) -> Result<ShimBracket, Box<dyn Error>> {
    let mut parts = r_version.split('.');
    let major: u32 = match parts.next().map(|p| p.parse()) {
        Some(Ok(v)) => v,
        _ => bail!("Cannot parse R version: {}", r_version),
    };
    // A version like "4.6" is fine, the minor part defaults to 0.
    let minor: u32 = match parts.next().map(|p| p.parse()) {
        Some(Ok(v)) => v,
        Some(Err(_)) => bail!("Cannot parse R version: {}", r_version),
        None => 0,
    };
    Ok(if major >= 4 {
        ShimBracket::R40
    } else if major == 3 && minor >= 5 {
        ShimBracket::R35
    } else {
        ShimBracket::Lt35
    })
}

fn shim_bytes(bracket: ShimBracket) -> &'static [u8] {
    match bracket {
        ShimBracket::Lt35 => HC_RVENV_SHIM_LT_35,
        ShimBracket::R35 => HC_RVENV_SHIM_35,
        ShimBracket::R40 => HC_RVENV_SHIM_40,
    }
}

/// Unpack the pre-built shim package into `<lib>/rig`.
pub fn unpack_shim_package(lib: &Path, bracket: ShimBracket) -> Result<(), Box<dyn Error>> {
    let pkg = lib.join(RVENV_SHIM_PKG);
    if pkg.exists() {
        fs::remove_dir_all(&pkg)?;
    }
    fs::create_dir_all(lib)?;
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(Cursor::new(shim_bytes(
        bracket,
    ))));
    // The modes in these archives are synthesized by `cargo xtask
    // gen-rvenv-shim`, not authored, so let the user's umask decide.
    ar.set_preserve_permissions(false);
    ar.set_preserve_mtime(false);
    ar.set_overwrite(true);
    ar.unpack(lib)?;
    Ok(())
}

// --------------------------------------------------------------------- init --

/// Write the tracked part of the `.rvenv` layout, and return what was
/// written. Does not write `rproj.toml`, that is the manifest half of
/// `rig proj init`.
///
/// The caller is expected to have run the [`existing_targets`] check first.
pub fn rvenv_init(root: &Path, r_version: &str) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let bracket = shim_bracket(r_version)?;
    let lib = project_library(root);

    let renviron = root.join(RVENV_RENVIRON_FILE);
    write_atomically(&renviron, renviron_body().as_bytes())?;

    update_root_gitignore(root)?;

    fs::create_dir_all(&lib)?;
    let lib_gitignore = lib.join(RVENV_GITIGNORE_FILE);
    write_atomically(&lib_gitignore, lib_gitignore_body().as_bytes())?;

    unpack_shim_package(&lib, bracket)?;

    Ok(vec![
        renviron,
        root.join(RVENV_GITIGNORE_FILE),
        lib_gitignore,
        lib.join(RVENV_SHIM_PKG),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shim_brackets() {
        assert_eq!(shim_bracket("3.0.0").unwrap(), ShimBracket::Lt35);
        assert_eq!(shim_bracket("3.4.4").unwrap(), ShimBracket::Lt35);
        assert_eq!(shim_bracket("3.5.0").unwrap(), ShimBracket::R35);
        assert_eq!(shim_bracket("3.6.3").unwrap(), ShimBracket::R35);
        assert_eq!(shim_bracket("3.9.9").unwrap(), ShimBracket::R35);
        assert_eq!(shim_bracket("4.0.0").unwrap(), ShimBracket::R40);
        assert_eq!(shim_bracket("4.6.1").unwrap(), ShimBracket::R40);
        assert_eq!(shim_bracket("4.6").unwrap(), ShimBracket::R40);
        assert_eq!(shim_bracket("5.0.0").unwrap(), ShimBracket::R40);
        assert!(shim_bracket("devel").is_err());
        assert!(shim_bracket("4.x.1").is_err());
    }

    #[test]
    fn renviron_body_is_what_the_shim_expects() {
        let body = renviron_body();
        assert!(body.contains("\nR_LIBS_USER=.rvenv/lib\n"));
        // The shim package has to come first, and the rest of R's default
        // package list has to be spelled out.
        assert!(body.contains(
            "\nR_DEFAULT_PACKAGES=rig,datasets,utils,grDevices,graphics,stats,methods\n"
        ));
    }

    #[test]
    fn gitignore_bodies_un_ignore_the_library() {
        let root = root_gitignore_block();
        assert!(root.contains("\n/.rvenv/*\n"));
        assert!(root.contains("\n!/.rvenv/lib\n"));
        let lib = lib_gitignore_body();
        assert!(lib.contains("\n*\n"));
        assert!(lib.contains("\n!.gitignore\n"));
        assert!(lib.contains("\n!rig\n"));
        assert!(lib.contains("\n!rig/**\n"));
    }

    #[test]
    fn gitignore_is_created_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        update_root_gitignore(tmp.path()).unwrap();
        let text = fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
        assert_eq!(text, root_gitignore_block());
    }

    #[test]
    fn gitignore_keeps_user_rules() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".gitignore");
        fs::write(&path, "*.Rproj\n.Rhistory\n").unwrap();
        update_root_gitignore(tmp.path()).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("*.Rproj\n.Rhistory\n"));
        assert!(text.contains("!/.rvenv/lib"));
    }

    #[test]
    fn gitignore_update_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".gitignore");
        fs::write(&path, "*.log\n").unwrap();
        update_root_gitignore(tmp.path()).unwrap();
        let once = fs::read_to_string(&path).unwrap();
        update_root_gitignore(tmp.path()).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), once);
    }

    #[test]
    fn gitignore_block_is_replaced_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".gitignore");
        fs::write(
            &path,
            format!(
                "before\n{}\nstale content\n{}\nafter\n",
                GITIGNORE_START, GITIGNORE_END
            ),
        )
        .unwrap();
        update_root_gitignore(tmp.path()).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("before\n"));
        assert!(text.ends_with("after\n"));
        assert!(!text.contains("stale content"));
        assert_eq!(text.matches(GITIGNORE_START).count(), 1);
    }

    #[test]
    fn gitignore_with_a_broken_block_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".gitignore");
        fs::write(&path, format!("{}\nno end marker\n", GITIGNORE_START)).unwrap();
        assert!(update_root_gitignore(tmp.path()).is_err());
    }

    #[test]
    fn a_managed_gitignore_is_not_a_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        update_root_gitignore(tmp.path()).unwrap();
        assert!(existing_targets(tmp.path()).unwrap().is_empty());

        let path = tmp.path().join(".gitignore");
        fs::write(&path, "*.log\n").unwrap();
        assert_eq!(existing_targets(tmp.path()).unwrap(), vec![path]);
    }

    #[test]
    fn find_project_root_walks_up() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("proj");
        let deep = root.join("R/sub/sub");
        fs::create_dir_all(&deep).unwrap();
        fs::write(root.join(RPROJ_MANIFEST_FILE), "").unwrap();
        assert_eq!(find_project_root(&deep).as_deref(), Some(root.as_path()));
        assert_eq!(find_project_root(&root).as_deref(), Some(root.as_path()));
    }

    #[test]
    fn find_project_root_finds_the_lock_file_and_the_venv() {
        let tmp = tempfile::tempdir().unwrap();
        let lock = tmp.path().join("lock/sub");
        fs::create_dir_all(&lock).unwrap();
        fs::write(lock.parent().unwrap().join(RPROJ_LOCK_FILE), "").unwrap();
        assert_eq!(find_project_root(&lock).unwrap(), lock.parent().unwrap());

        let venv = tmp.path().join("venv/sub");
        fs::create_dir_all(venv.parent().unwrap().join(RVENV_DIR)).unwrap();
        fs::create_dir_all(&venv).unwrap();
        assert_eq!(find_project_root(&venv).unwrap(), venv.parent().unwrap());
    }

    #[test]
    fn find_project_root_gives_up_at_the_filesystem_root() {
        let tmp = tempfile::tempdir().unwrap();
        let deep = tmp.path().join("a/b/c");
        fs::create_dir_all(&deep).unwrap();
        assert_eq!(find_project_root(&deep), None);
    }

    #[test]
    fn every_shim_bracket_unpacks() {
        for bracket in [ShimBracket::Lt35, ShimBracket::R35, ShimBracket::R40] {
            let tmp = tempfile::tempdir().unwrap();
            let lib = tmp.path().join("lib");
            unpack_shim_package(&lib, bracket).unwrap();
            let desc = fs::read_to_string(lib.join("rig/DESCRIPTION")).unwrap();
            assert!(desc.contains("Package: rig"), "{:?}", bracket);
            assert!(lib.join("rig/R/rig.rdb").exists(), "{:?}", bracket);
            assert!(lib.join("rig/R/rig.rdx").exists(), "{:?}", bracket);
            assert!(lib.join("rig/Meta/package.rds").exists(), "{:?}", bracket);
        }
    }

    #[test]
    fn unpacking_the_shim_twice_replaces_it() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = tmp.path().join("lib");
        unpack_shim_package(&lib, ShimBracket::R40).unwrap();
        let stray = lib.join("rig/stray-file");
        fs::write(&stray, "x").unwrap();
        unpack_shim_package(&lib, ShimBracket::R40).unwrap();
        assert!(!stray.exists());
        assert!(lib.join("rig/DESCRIPTION").exists());
    }

    #[test]
    fn rvenv_init_writes_the_tracked_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let written = rvenv_init(tmp.path(), "4.6.1").unwrap();
        for path in &written {
            assert!(path.exists(), "{} was not written", path.display());
        }
        // Everything init_targets() promises, except the manifest, which
        // `rig proj init` writes itself.
        let mut expected = init_targets(tmp.path());
        expected.retain(|p| !p.ends_with(RPROJ_MANIFEST_FILE));
        expected.sort();
        let mut written = written;
        written.sort();
        assert_eq!(written, expected);
        assert!(tmp.path().join(".rvenv/lib/rig/DESCRIPTION").exists());
    }
}
