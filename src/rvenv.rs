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
//!   .gitignore              # tracked -- ignores /.rvenv/
//!   .rvenvlib/
//!     rvenv/                # tracked -- the shim package
//! ```
//!
//! `rig proj sync` adds the machine-specific rest, none of which is
//! committed:
//!
//! ```text
//!   .rvenv/
//!     lib/...               # the real dependencies
//!     lib/.synced           # sync stamp, a copy of the lock file
//!     rvenv.cfg             # what this environment was built against
//!     onload.R              # rest of the shim package's `.onLoad()`
//!     etc/repositories      # what R_REPOSITORIES points at
//!     bin/R, bin/Rscript    # wrapper scripts (.exe shims on Windows)
//!     bin/activate*         # shell activation scripts
//! ```
//!
//! Two things here are less obvious than they look.
//!
//! `.rvenvlib` is rig's own half of the environment, kept out of `.rvenv`
//! entirely -- not just out of `.rvenv/lib` -- so that `.rvenv` holds nothing
//! but machine-specific output of `rig proj sync` and can be deleted and
//! rebuilt at any time without touching version control.
//!
//! The shim package in `.rvenvlib/rvenv` is committed, and written by
//! `rig proj init` rather than installed by `rig proj sync`, because its whole
//! job is to be there *before* the first sync: `.Renviron` names it in
//! `R_DEFAULT_PACKAGES`, so without it R prints its own unhelpful "package
//! 'rvenv' in options(\"defaultPackages\") was not found". That is also why
//! `.Renviron` points `R_LIBS_USER` at `.rvenvlib` rather than at the
//! project library: it is the one library R has to be able to load a package
//! from at startup. The shim then re-points `R_LIBS_USER` at the absolute
//! `.rvenv/lib` and makes that `.libPaths()[1]`. See `src/data/rvenv-pkg` for
//! the source, [`write_shim_package`] for what is written, and
//! `xtask/src/rvenv_shim.rs` for the two generated files it needs.
//!
//! Before the first `rig proj sync` there is no `.rvenv/lib` at all, and then
//! the shim does the opposite: it puts `R_LIBS_USER` and `.libPaths()` back to
//! what the session would have had outside the project, so that the user is
//! not left with rig's own directory as their library. `.Renviron` records the
//! original value in `RVENV_R_LIBS_USER` for it.
//!
//! Never write a file named `___default` into `.rvenv/lib`: that is the
//! sentinel of rig's own per-version library switching, in the same
//! `Rprofile.site` block.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use simple_error::bail;

use crate::hardcoded::{
    HC_RVENV_PKG_CODE, HC_RVENV_PKG_DESCRIPTION, HC_RVENV_PKG_LICENSE, HC_RVENV_PKG_META,
    HC_RVENV_PKG_NAMESPACE,
};
use crate::repos::binaries::ppm_url;
use crate::repositories::{write_repositories_file, RepoFileEntry, RepositoriesContents};
use crate::rproj::{Repository as ManifestRepository, Rproj, Workspace, RPROJ_MANIFEST_FILE};
use crate::utils::write_atomically;
#[cfg(not(windows))]
use crate::utils::write_executable;

pub const RVENV_DIR: &str = ".rvenv";
pub const RVENV_LIB_SUBDIR: &str = "lib";
pub const RVENV_BIN_SUBDIR: &str = "bin";
pub const RVENV_ETC_SUBDIR: &str = "etc";
pub const RVENV_SHIM_DIR: &str = ".rvenvlib";
pub const RVENV_SHIM_PKG: &str = "rvenv";
pub const RVENV_RENVIRON_FILE: &str = ".Renviron";
pub const RVENV_GITIGNORE_FILE: &str = ".gitignore";
pub const RVENV_CFG_FILE: &str = "rvenv.cfg";
pub const RVENV_ONLOAD_FILE: &str = "onload.R";
pub const RVENV_REPOS_FILE: &str = "repositories";
pub const RPROJ_LOCK_FILE: &str = "rproj.lock";

/// What `R_LIBS_SITE` is set to in an active environment.
///
/// Setting it empty does not reliably disable the site library on every R
/// version, so point it at a path that cannot exist instead.
pub const RVENV_NO_SITE: &str = "/nonexistent/rvenv-no-site";

/// The repository R uses when the project's manifest names none.
const RVENV_DEFAULT_REPO_URL: &str = "https://cloud.r-project.org";

/// The name the environment's main repository goes into the repositories
/// file under. R starts with an unresolved `CRAN = "@CRAN@"` entry in
/// `getOption("repos")`, and only a repository of that name replaces it.
const RVENV_CRAN_NAME: &str = "CRAN";

/// The menu name of the P3M entry in the repositories file.
const PPM_MENU_NAME: &str = "Posit Public Package Manager";

/// The P3M repository a lock file target installs from, or `None` for a
/// source-only lock file, which has no P3M target.
///
/// The lock file's platform is a P3M target name, `<platform>-<arch>`, e.g.
/// `macos-arm64` or `jammy-x86_64`, and the platform part is exactly what
/// goes into a Linux binary URL. macOS and Windows have no such path
/// component: their binaries are served from the top-level repository. A
/// source-only solve records the machine's architecture instead, e.g.
/// `x86_64`, with no P3M target in it.
///
/// The URL is the `latest` snapshot rather than the dated snapshot the lock
/// file's package URLs point at: an `install.packages()` in the environment
/// is by definition installing something the lock file does not have, so it
/// should see current versions.
fn ppm_repo_url(platform: &str) -> Option<String> {
    let target = platform
        .strip_suffix("-x86_64")
        .or_else(|| platform.strip_suffix("-arm64"))?;
    match target {
        "" => None,
        "macos" | "windows" => Some(format!("{}/cran/latest", ppm_url())),
        linux => Some(format!("{}/cran/__linux__/{}/latest", ppm_url(), linux)),
    }
}

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
# R_LIBS_USER names rig's own library, not the project library: it only has to
# get R far enough to load the `rvenv` package below. That package points
# R_LIBS_USER at the project library, as an absolute path, so that child R
# processes started from a subdirectory still see it. The path here is
# deliberately relative, because this file is committed to version control.
#
# RVENV_R_LIBS_USER keeps the R_LIBS_USER this R installation had set by the
# time this file is read, so that the `rvenv` package can put it back if the
# project has no library yet. It is still unexpanded at this point, e.g. `%U`;
# R expands the %-specs later, and so does the package.
#
# R_DEFAULT_PACKAGES replaces R's default package list rather than adding to
# it, so the whole list has to be spelled out.
#
# Note that `R --vanilla` ignores this file entirely.
RVENV_R_LIBS_USER=${R_LIBS_USER}
R_LIBS_USER=.rvenvlib
R_DEFAULT_PACKAGES=rvenv,datasets,utils,grDevices,graphics,stats,methods
"
}

/// The block rig manages in the project's root `.gitignore`. Everything in
/// `.rvenv` is machine-specific; the pre-built rvenv package a fresh clone
/// needs before the first `rig proj sync` lives in `.rvenvlib` instead, which
/// is tracked and needs no exception here.
fn root_gitignore_block() -> String {
    format!(
        "\
{}
# Everything in .rvenv is machine-specific output of `rig proj sync`; it can
# be deleted and rebuilt at any time. The committed rvenv shim package lives
# in .rvenvlib instead.
/.rvenv/
{}
",
        GITIGNORE_START, GITIGNORE_END
    )
}

// -------------------------------------------------------------------- paths --

/// `<root>/.rvenv`, the project environment.
pub fn project_venv(root: &Path) -> PathBuf {
    root.join(RVENV_DIR)
}

/// `<root>/.rvenv/lib`, the project package library.
pub fn project_library(root: &Path) -> PathBuf {
    root.join(RVENV_DIR).join(RVENV_LIB_SUBDIR)
}

/// `<root>/.rvenvlib`, rig's own library inside the project.
///
/// It holds nothing but the shim package, and is the only committed part of
/// the environment. Keeping it out of `.rvenv` entirely means `.rvenv` holds
/// only machine-specific output, and [`project_library`] contains only the
/// project's own packages.
pub fn project_shim_library(root: &Path) -> PathBuf {
    root.join(RVENV_SHIM_DIR)
}

/// `<root>/.rvenvlib/rvenv`, the shim package.
///
/// Also the marker of an initialized project: `rig proj init` writes it, and
/// nothing else creates it.
pub fn project_shim_package(root: &Path) -> PathBuf {
    project_shim_library(root).join(RVENV_SHIM_PKG)
}

/// `<root>/.rvenv/bin`, the wrapper scripts and the activation scripts.
pub fn project_bin(root: &Path) -> PathBuf {
    root.join(RVENV_DIR).join(RVENV_BIN_SUBDIR)
}

/// `<root>/.rvenv/etc`, the environment's own R configuration files.
pub fn project_etc(root: &Path) -> PathBuf {
    root.join(RVENV_DIR).join(RVENV_ETC_SUBDIR)
}

/// `<root>/.rvenv/bin/R`, the wrapper that starts the environment's R.
///
/// This is what `rig run` executes instead of the default R: the wrapper
/// carries the environment of [`rvenv_env_vars`], which a plain R binary or a
/// symlink would not.
pub fn project_r_wrapper(root: &Path) -> PathBuf {
    project_bin(root).join(if cfg!(windows) { "R.exe" } else { "R" })
}

/// Why the project environment in `root` is not usable as it is, or `None` if
/// it is up to date. Callers use this both to decide whether to run
/// `rig proj sync` and to tell the user why they are waiting for one.
pub fn rvenv_sync_needed(root: &Path) -> Result<Option<String>, Box<dyn Error>> {
    let cfg = match read_rvenv_cfg(root)? {
        None => return Ok(Some("the environment has not been created yet".to_string())),
        Some(cfg) => cfg,
    };

    if !cfg.r_binary.exists() {
        return Ok(Some(format!(
            "R {} is not installed at {} any more",
            cfg.r_version,
            cfg.r_binary.display()
        )));
    }

    let wrapper = project_r_wrapper(root);
    if !wrapper.exists() {
        return Ok(Some(format!("{} is missing", wrapper.display())));
    }

    let lock_path = root.join(RPROJ_LOCK_FILE);
    if !lock_path.exists() {
        return Ok(Some(format!("there is no {} yet", RPROJ_LOCK_FILE)));
    }

    let stamp_path = project_library(root).join(RVENV_SYNC_STAMP);
    if !stamp_path.exists() {
        return Ok(Some(
            "the project library has not been synced yet".to_string(),
        ));
    }

    // The stamp is a copy of the lock file `rig proj sync` installed from, so
    // comparing the bytes is the whole staleness check.
    if fs::read(&stamp_path)? != fs::read(&lock_path)? {
        return Ok(Some(format!(
            "{} has changed since the last sync",
            RPROJ_LOCK_FILE
        )));
    }

    Ok(None)
}

/// The project root at or above `start`: the nearest directory holding an
/// `rproj.toml`, an `rproj.lock`, an `.rvenv` directory or an `.rvenvlib`
/// directory.
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut dir = start;
    loop {
        if dir.join(RPROJ_MANIFEST_FILE).exists()
            || dir.join(RPROJ_LOCK_FILE).exists()
            || dir.join(RVENV_DIR).is_dir()
            || dir.join(RVENV_SHIM_DIR).is_dir()
        {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?;
    }
}

/// The workspace root at or above `start`, if `start` belongs to one.
///
/// Two ways to belong: the project at or above `start` declares
/// `[workspace]` itself, or one of its ancestors declares a `[workspace]`
/// whose `members` cover it. The nearest such ancestor wins, so a workspace
/// nested inside another workspace claims its own members.
///
/// A manifest that does not parse is not an error here: this runs before the
/// command has decided which manifest it cares about, and an ancestor
/// directory that happens to hold a broken `rproj.toml` should not stop a
/// perfectly good project below it from being used. The manifest of the root
/// that is actually picked is read again, and reported, by the caller.
pub fn find_workspace_root(start: &Path) -> Result<Option<PathBuf>, Box<dyn Error>> {
    let Some(project) = find_project_root(start) else {
        return Ok(None);
    };

    if let Some(ws) = read_workspace(&project) {
        if !ws.members.is_empty() {
            return Ok(Some(project));
        }
    }

    let mut dir = project.parent();
    while let Some(candidate) = dir {
        if let Some(ws) = read_workspace(candidate) {
            if expand_workspace_members(candidate, &ws)?.contains(&project) {
                return Ok(Some(candidate.to_path_buf()));
            }
        }
        dir = candidate.parent();
    }

    Ok(None)
}

/// The `[workspace]` table of the manifest in `dir`, if there is a manifest
/// with one and it parses. See [`find_workspace_root`] on why an unparsable
/// manifest is `None` rather than an error.
fn read_workspace(dir: &Path) -> Option<Workspace> {
    let text = fs::read_to_string(dir.join(RPROJ_MANIFEST_FILE)).ok()?;
    toml::from_str::<Rproj>(&text).ok()?.workspace
}

/// The directories of a workspace's members: the `members` patterns expanded,
/// minus the `exclude` ones, plus `root` itself, which is always a member of
/// its own workspace.
///
/// Only path work, no manifest reading, so that [`find_workspace_root`] can
/// use it to probe an ancestor without failing on a member that is missing a
/// manifest. Use [`workspace_members`] to get members that are usable.
pub fn expand_workspace_members(
    root: &Path,
    ws: &Workspace,
) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut members: Vec<PathBuf> = vec![root.to_path_buf()];
    for pattern in &ws.members {
        members.extend(expand_member_pattern(root, pattern)?);
    }

    let exclude = pattern_matcher(&ws.exclude)?;
    let mut kept: Vec<PathBuf> = vec![];
    for member in members {
        // A pattern is written relative to the root, so that is what it is
        // matched against. The root itself is "", which no pattern matches,
        // so `exclude` cannot remove it -- and should not: the root manifest
        // is the one that declares the workspace.
        let relative = member
            .strip_prefix(root)
            .map(slash_path)
            .unwrap_or_default();
        if !relative.is_empty() && exclude.is_match(&relative) {
            continue;
        }
        if !kept.contains(&member) {
            kept.push(member);
        }
    }

    // Deterministic order regardless of readdir order, with the root first:
    // this is the order members are solved and reported in.
    kept[1..].sort();
    Ok(kept)
}

/// The directories of a workspace's members, each verified to hold a
/// manifest. `root` is the workspace root, `ws` its `[workspace]` table.
pub fn workspace_members(root: &Path, ws: &Workspace) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let members = expand_workspace_members(root, ws)?;
    for member in &members {
        if !member.join(RPROJ_MANIFEST_FILE).exists() {
            bail!(
                "Workspace member {} has no {}",
                member.display(),
                RPROJ_MANIFEST_FILE
            );
        }
    }
    Ok(members)
}

/// One `members` pattern expanded to the directories it matches.
///
/// The pattern is matched one path component at a time, so a `*` never
/// reaches across a directory separator, and only the directories a pattern
/// can actually match are read -- as opposed to walking the whole tree and
/// matching paths against it, which in a monorepo means walking every
/// member's `.rvenv` too.
///
/// A pattern with no wildcards names one directory and has to match it: a
/// misspelled member is a mistake worth reporting. A wildcard pattern that
/// matches nothing is not an error, the same way `packages/*` in an empty
/// monorepo is not.
fn expand_member_pattern(root: &Path, pattern: &str) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut dirs: Vec<PathBuf> = vec![root.to_path_buf()];
    // Of the whole pattern, not of the component being matched: the
    // `packages` of `packages/*` is a literal, but a monorepo that has not
    // grown a `packages` directory yet is not misconfigured.
    let wildcard = pattern.contains(['*', '?', '[']);

    for component in pattern.split(['/', '\\']) {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." {
            bail!(
                "Invalid workspace member `{}`: a member cannot be outside the \
                 workspace root, they all share one project library",
                pattern
            );
        }

        let mut next: Vec<PathBuf> = vec![];
        if component.contains(['*', '?', '[']) {
            let glob = GlobBuilder::new(component)
                .literal_separator(true)
                .build()?
                .compile_matcher();
            for dir in &dirs {
                let Ok(entries) = fs::read_dir(dir) else {
                    continue;
                };
                for entry in entries {
                    let entry = entry?;
                    if !entry.file_type()?.is_dir() {
                        continue;
                    }
                    let name = entry.file_name().to_string_lossy().into_owned();
                    // `.rvenv`, `.rvenvlib`, `.git`: a wildcard member pattern
                    // is about the monorepo's own directories, never about the
                    // tooling's.
                    if name.starts_with('.') {
                        continue;
                    }
                    if glob.is_match(&name) {
                        next.push(entry.path());
                    }
                }
            }
        } else {
            for dir in &dirs {
                let path = dir.join(component);
                if path.is_dir() {
                    next.push(path);
                }
            }
            if next.is_empty() && !wildcard {
                bail!(
                    "Workspace member `{}` is not a directory in {}",
                    pattern,
                    root.display()
                );
            }
        }
        dirs = next;
    }

    Ok(dirs)
}

/// One matcher for a list of member patterns, matched against root-relative
/// slash-separated paths. `*` matches within one path component only, so
/// `packages/*` does not match `packages/a/b`.
fn pattern_matcher(patterns: &[String]) -> Result<GlobSet, Box<dyn Error>> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(
            GlobBuilder::new(&pattern.replace('\\', "/"))
                .literal_separator(true)
                .build()?,
        );
    }
    Ok(builder.build()?)
}

/// A relative path with `/` separators, which is how member patterns are
/// written on every platform.
fn slash_path(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Everything `rig proj init` writes, in write order.
pub fn init_targets(root: &Path) -> Vec<PathBuf> {
    vec![
        root.join(RPROJ_MANIFEST_FILE),
        root.join(RVENV_RENVIRON_FILE),
        root.join(RVENV_GITIGNORE_FILE),
        project_shim_package(root),
    ]
}

/// The paths from [`init_targets`] that are already there, so that the
/// caller can name all of them at once instead of failing on the first.
///
/// The root `.gitignore` is special: rig only ever merges a marked block into
/// it (see [`update_root_gitignore`]), never overwrites the rest of the
/// file, so an existing `.gitignore` -- with or without rig's block -- is
/// never a conflict.
pub fn existing_targets(root: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let gitignore = root.join(RVENV_GITIGNORE_FILE);
    Ok(init_targets(root)
        .into_iter()
        .filter(|p| p.exists())
        .filter(|p| *p != gitignore)
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

/// Write the shim package into `<lib>/rvenv`, replacing whatever is there.
///
/// This writes an *installed* R package without running R, which works
/// because the shim needs no lazy-load database: `loadNamespace()` sources
/// `<pkg>/R/<pkg>` if it finds it, so that file is the package's R source
/// verbatim. What is left of an install is `Meta/package.rds`, which
/// `library()` insists on, and the `Built:` field of `DESCRIPTION`. Both are
/// generated and committed, see `xtask/src/rvenv_shim.rs` for how, and why
/// they claim to have been built by R 4.0.0.
///
/// One copy has to work with every R the project may be opened with, because
/// the result is committed to version control -- so this deliberately does not
/// depend on the project's R version.
pub fn write_shim_package(lib: &Path) -> Result<(), Box<dyn Error>> {
    let pkg = lib.join(RVENV_SHIM_PKG);
    if pkg.exists() {
        fs::remove_dir_all(&pkg)?;
    }
    fs::create_dir_all(pkg.join("R"))?;
    fs::create_dir_all(pkg.join("Meta"))?;
    // The embedded text files may be checked out with CRLF, e.g. on Windows
    // without a .gitattributes rule. R reads either, but the project keeps a
    // copy of them under version control, so write the same bytes everywhere.
    for (path, contents) in [
        ("DESCRIPTION", HC_RVENV_PKG_DESCRIPTION),
        ("NAMESPACE", HC_RVENV_PKG_NAMESPACE),
        ("LICENSE", HC_RVENV_PKG_LICENSE),
        // No extension: this is the file `loadNamespace()` sources, and it is
        // named after the package.
        ("R/rvenv", HC_RVENV_PKG_CODE),
    ] {
        write_atomically(&pkg.join(path), contents.replace("\r\n", "\n").as_bytes())?;
    }
    write_atomically(&pkg.join("Meta").join("package.rds"), HC_RVENV_PKG_META)?;
    Ok(())
}

// --------------------------------------------------------------------- init --

/// Write the tracked part of the `.rvenv` layout, and return what was
/// written. Does not write `rproj.toml`, that is the manifest half of
/// `rig proj init`.
///
/// The caller is expected to have run the [`existing_targets`] check first.
pub fn rvenv_init(root: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let shim_lib = project_shim_library(root);

    let renviron = root.join(RVENV_RENVIRON_FILE);
    write_atomically(&renviron, renviron_body().as_bytes())?;

    update_root_gitignore(root)?;

    // The project library itself is not created here: `rig proj sync` makes
    // it, and until then there is nothing to put in it.
    write_shim_package(&shim_lib)?;

    Ok(vec![
        renviron,
        root.join(RVENV_GITIGNORE_FILE),
        project_shim_package(root),
    ])
}

// --------------------------------------------------------------------- sync --

/// What an environment was built against, written to `.rvenv/rvenv.cfg`.
///
/// The `pyvenv.cfg` analog, and load bearing in the same way: installed R
/// packages are tied to the R *minor* version, the platform and the
/// architecture, so an environment that is used with a different R than it
/// was solved for is broken, not merely stale.
///
/// `rig proj sync` rewrites this file every time: the lock file decides what
/// the environment is, so there is nothing here to preserve. It carries no
/// timestamp for the same reason -- two syncs in a row write the same bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct RvenvCfg {
    /// The name of the R installation, as `rig list` shows it, e.g. `4.6` or
    /// `4.6-arm64`.
    pub r_version: String,
    /// `<major>.<minor>`, the version R packages are actually tied to.
    pub r_minor: String,
    /// The absolute path of the R binary the wrappers forward to.
    pub r_binary: PathBuf,
    /// The platform the lock file was solved for.
    pub platform: String,
    /// The architecture of the R installation.
    pub r_arch: String,
    /// The version of rig that wrote this file.
    pub rig_version: String,
    /// The shared dev-tools library for `r_version`, `<user library>/__tools`
    /// (see [`crate::library::get_library_path`]). Computed once here,
    /// rather than derived by the shim from `RVENV_R_LIBS_USER` at R
    /// startup, because `rig run`'s wrapper sets `R_LIBS_USER` to the
    /// project library before R starts, and R's own `.Renviron` handling
    /// then captures that already-overridden value into
    /// `RVENV_R_LIBS_USER`, not the genuine per-R-version default.
    pub tools_lib: PathBuf,
}

impl RvenvCfg {
    /// The `rvenv.cfg` body: `key = value` lines, like `pyvenv.cfg`.
    pub fn body(&self) -> String {
        format!(
            "\
# Managed by rig (rig proj sync). Do not edit, `rig proj sync` rewrites it.
#
# R packages are tied to the R minor version, the platform and the
# architecture, so this file records all three.
r-version = {}
r-minor = {}
r-binary = {}
platform = {}
r-arch = {}
rig = {}
tools-lib = {}
",
            self.r_version,
            self.r_minor,
            self.r_binary.display(),
            self.platform,
            self.r_arch,
            self.rig_version,
            self.tools_lib.display()
        )
    }

    /// Parse an `rvenv.cfg` body. Unknown keys are ignored, so that an older
    /// rig can still read a file a newer one wrote.
    pub fn parse(text: &str) -> Result<RvenvCfg, Box<dyn Error>> {
        let mut fields: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            match line.split_once('=') {
                Some((key, value)) => {
                    fields.insert(key.trim(), value.trim());
                }
                None => bail!("Malformed line in {}: {}", RVENV_CFG_FILE, line),
            }
        }
        let get = |key: &str| -> Result<String, Box<dyn Error>> {
            match fields.get(key) {
                Some(value) => Ok((*value).to_string()),
                None => bail!("Missing `{}` from {}", key, RVENV_CFG_FILE),
            }
        };
        Ok(RvenvCfg {
            r_version: get("r-version")?,
            r_minor: get("r-minor")?,
            r_binary: PathBuf::from(get("r-binary")?),
            platform: get("platform")?,
            r_arch: get("r-arch")?,
            rig_version: get("rig")?,
            // Missing, not an error: a file written before this key existed.
            // `rig proj sync` rewrites the file every time anyway.
            tools_lib: PathBuf::from(fields.get("tools-lib").copied().unwrap_or("")),
        })
    }
}

/// Read `<root>/.rvenv/rvenv.cfg`, or `None` if the environment has never
/// been synced.
pub fn read_rvenv_cfg(root: &Path) -> Result<Option<RvenvCfg>, Box<dyn Error>> {
    let path = project_venv(root).join(RVENV_CFG_FILE);
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(RvenvCfg::parse(&fs::read_to_string(&path)?)?))
}

/// The environment variables that make an R session use the project.
///
/// The one source of truth for the activation environment: the Windows
/// `.exe` shims bake this list in verbatim, and the Unix wrapper and the
/// activation scripts spell out the same variables, relative to `$RVENV`.
///
/// `R_LIBS` is deliberately empty, so that the project library stays
/// `.libPaths()[1]`, which is where `install.packages()` writes.
pub fn rvenv_env_vars(venv: &Path) -> Vec<(String, String)> {
    let venv = venv.display().to_string();
    vec![
        ("RVENV".to_string(), venv.clone()),
        (
            "R_LIBS_USER".to_string(),
            format!("{}{}{}", venv, std::path::MAIN_SEPARATOR, RVENV_LIB_SUBDIR),
        ),
        ("R_LIBS".to_string(), String::new()),
        ("R_LIBS_SITE".to_string(), RVENV_NO_SITE.to_string()),
        (
            "R_REPOSITORIES".to_string(),
            format!(
                "{}{}{}{}{}",
                venv,
                std::path::MAIN_SEPARATOR,
                RVENV_ETC_SUBDIR,
                std::path::MAIN_SEPARATOR,
                RVENV_REPOS_FILE
            ),
        ),
    ]
}

/// The repositories the environment installs from, as `R_REPOSITORIES`
/// entries.
///
/// `R_REPOSITORIES` is a plain environment variable R reads at startup, so it
/// survives `--vanilla` and is inherited by child processes -- unlike
/// `options(repos = )`, which neither does.
fn repositories_contents(platform: &str, repos: &[ManifestRepository]) -> RepositoriesContents {
    let entry = |name: &str, url: &str, menu: &str| RepoFileEntry {
        name: name.to_string(),
        description: menu.to_string(),
        url: url.to_string(),
        default: true,
        source: true,
        win_binary: true,
        mac_binary: true,
    };

    // The first entry goes into the file as `CRAN`, keeping its own name as
    // the menu name. R starts with an unresolved `CRAN = "@CRAN@"`
    // placeholder in `getOption("repos")`, and only an entry of that name
    // replaces it -- leave the placeholder in, and `install.packages()`
    // fails with "trying to use CRAN without setting a mirror". Naming the
    // main repository `CRAN` is the usual R idiom for this, the same thing
    // `options(repos = c(CRAN = ...))` does.
    let mut data = vec![];
    match ppm_repo_url(platform) {
        // `rig proj sync` installs P3M binaries for the lock file's target,
        // so an `install.packages()` in the environment should reach the
        // same packages. That means P3M first, at the target's own binary
        // URL -- the project's own repositories keep their names and follow
        // it at lower precedence.
        Some(url) => data.push(entry(RVENV_CRAN_NAME, &url, PPM_MENU_NAME)),
        // A source-only lock file has no P3M target, so there is nothing to
        // prefer over what the project asks for.
        None if repos.is_empty() => {
            data.push(entry(RVENV_CRAN_NAME, RVENV_DEFAULT_REPO_URL, "CRAN"))
        }
        None => {}
    }
    let first_is_ppm = !data.is_empty();
    for (i, r) in repos.iter().enumerate() {
        let name = if i == 0 && !first_is_ppm {
            RVENV_CRAN_NAME
        } else {
            &r.name
        };
        // A repositories file with two entries of the same name is a
        // `getOption("repos")` with a duplicated name, which R handles
        // badly. The higher-precedence entry wins; a project repository
        // called `CRAN` is dropped in favor of P3M, which serves the same
        // packages.
        if data.iter().any(|e: &RepoFileEntry| e.name == name) {
            continue;
        }
        data.push(entry(name, &r.url, &r.name));
    }
    RepositoriesContents {
        data,
        comments: vec![(
            1,
            "# Managed by rig (rig proj sync). Do not edit, `rig proj sync` rewrites it."
                .to_string(),
        )],
    }
}

/// Substitute the placeholders of an `src/data/rvenv` template.
fn render(template: &str, venv: &Path, name: &str, r_binary: &Path) -> String {
    template
        .replace("@RVENV@", &venv.display().to_string())
        .replace("@RVENV_NAME@", name)
        .replace("@R_BINARY@", &r_binary.display().to_string())
        .replace("@RVENV_EXPORTS@", shell_exports().trim_end())
}

/// The `export` lines of the `.rvenv/bin/R` wrapper, from
/// [`rvenv_env_vars`], so that the wrapper and the Windows shims cannot
/// drift apart.
///
/// The paths are relative to `$RVENV`, which the wrapper works out from its
/// own location -- that is what keeps the environment relocatable. `RVENV`
/// itself is left out for the same reason: the wrapper sets it.
#[cfg(not(windows))]
fn shell_exports() -> String {
    rvenv_env_vars(Path::new("$RVENV"))
        .into_iter()
        .filter(|(name, _)| name != "RVENV")
        .map(|(name, value)| format!("export {}=\"{}\"\n", name, value))
        .collect()
}

#[cfg(windows)]
fn shell_exports() -> String {
    // The Windows wrappers are `.exe` shims with the same variables baked
    // into them, so there is no shell wrapper to fill in here.
    String::new()
}

/// The `Rscript` next to an `R` binary.
fn rscript_of(r_binary: &Path) -> PathBuf {
    let name = if cfg!(windows) {
        "Rscript.exe"
    } else {
        "Rscript"
    };
    match r_binary.parent() {
        Some(dir) => dir.join(name),
        None => PathBuf::from(name),
    }
}

/// Write the machine-specific part of the `.rvenv` layout -- `rvenv.cfg`,
/// `etc/repositories`, the `bin/` wrappers and the activation scripts -- and
/// return what was written.
///
/// Everything here is derived from `cfg` and the manifest, so this is
/// idempotent: running it twice writes the same bytes.
pub fn rvenv_sync(
    root: &Path,
    cfg: &RvenvCfg,
    repos: &[ManifestRepository],
) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let venv = project_venv(root);
    let bin = project_bin(root);
    let etc = project_etc(root);
    fs::create_dir_all(&bin)?;
    fs::create_dir_all(&etc)?;

    // The name in the shell prompt of an activated environment. The project
    // directory's name, like Python's, not the manifest's project name: a
    // prompt is about where you are.
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| RVENV_DIR.to_string());

    let mut written = vec![];

    let cfg_path = venv.join(RVENV_CFG_FILE);
    write_atomically(&cfg_path, cfg.body().as_bytes())?;
    written.push(cfg_path);

    let onload_path = venv.join(RVENV_ONLOAD_FILE);
    write_atomically(&onload_path, RVENV_ONLOAD_TEMPLATE.as_bytes())?;
    written.push(onload_path);

    let repos_path = etc.join(RVENV_REPOS_FILE);
    write_repositories_file(
        repositories_contents(&cfg.platform, repos),
        repos_path
            .to_str()
            .ok_or("The project path is not valid Unicode")?,
    )?;
    written.push(repos_path);

    written.extend(write_wrappers(&venv, &bin, &cfg.r_binary)?);

    for (file, template) in ACTIVATE_TEMPLATES {
        let path = bin.join(file);
        write_atomically(
            &path,
            render(template, &venv, &name, &cfg.r_binary).as_bytes(),
        )?;
        written.push(path);
    }

    Ok(written)
}

/// The activation scripts, one per shell. These are sourced, not run, so
/// they do not need the executable bit -- except `activate.bat`, which is
/// run, and which needs no bit on Windows anyway.
const RVENV_ONLOAD_TEMPLATE: &str = include_str!("data/rvenv/onload.R");

const ACTIVATE_TEMPLATES: &[(&str, &str)] = &[
    ("activate", include_str!("data/rvenv/activate.sh")),
    ("activate.csh", include_str!("data/rvenv/activate.csh")),
    ("activate.fish", include_str!("data/rvenv/activate.fish")),
    ("activate.bat", include_str!("data/rvenv/activate.bat")),
    ("deactivate.bat", include_str!("data/rvenv/deactivate.bat")),
    ("Activate.ps1", include_str!("data/rvenv/Activate.ps1")),
];

/// `.rvenv/bin/R` and `.rvenv/bin/Rscript`: wrapper scripts on Unix, `.exe`
/// shims on Windows. Either way they set the environment of
/// [`rvenv_env_vars`] and then hand over to the real R.
///
/// A symlink would not do: it resolves `R_HOME` correctly, but it carries no
/// environment, so putting `.rvenv/bin` on `PATH` would install into the
/// user's own library.
#[cfg(not(windows))]
fn write_wrappers(
    venv: &Path,
    bin: &Path,
    r_binary: &Path,
) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let template = include_str!("data/rvenv/R.sh");
    let mut written = vec![];
    for (file, target) in [
        ("R", r_binary.to_path_buf()),
        ("Rscript", rscript_of(r_binary)),
    ] {
        let path = bin.join(file);
        write_executable(&path, render(template, venv, "", &target).as_bytes())?;
        written.push(path);
    }
    Ok(written)
}

#[cfg(windows)]
fn write_wrappers(
    venv: &Path,
    bin: &Path,
    r_binary: &Path,
) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let envs = rvenv_env_vars(venv);
    let mut written = vec![];
    for (file, target) in [
        ("R.exe", r_binary.to_path_buf()),
        ("Rscript.exe", rscript_of(r_binary)),
    ] {
        let path = bin.join(file);
        // No marker: these shims are not rig's default-version quick links,
        // they belong to one project and one R installation.
        crate::windows::write_shim_link_env(
            &path,
            target
                .to_str()
                .ok_or("The R installation path is not valid Unicode")?,
            "",
            &envs,
        )?;
        written.push(path);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renviron_body_is_what_the_shim_expects() {
        let body = renviron_body();
        // Not the project library: this is the library the shim itself is
        // loaded from, the shim switches to the project library.
        assert!(body.contains("\nR_LIBS_USER=.rvenvlib\n"));
        // Recorded before R_LIBS_USER is overwritten, so that the shim can
        // restore it for a project that has no library yet.
        assert!(body.contains("\nRVENV_R_LIBS_USER=${R_LIBS_USER}\nR_LIBS_USER="));
        // The shim package has to come first, and the rest of R's default
        // package list has to be spelled out.
        assert!(body.contains(
            "\nR_DEFAULT_PACKAGES=rvenv,datasets,utils,grDevices,graphics,stats,methods\n"
        ));
    }

    #[test]
    fn the_gitignore_block_ignores_all_of_rvenv() {
        let root = root_gitignore_block();
        assert!(root.contains("\n/.rvenv/\n"));
        // The shim package lives outside .rvenv entirely, so there is no
        // exception to carve out.
        assert!(!root.contains("!/.rvenv"));
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
        assert!(text.contains("/.rvenv/"));
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
    fn a_gitignore_is_never_a_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        update_root_gitignore(tmp.path()).unwrap();
        assert!(existing_targets(tmp.path()).unwrap().is_empty());

        let path = tmp.path().join(".gitignore");
        fs::write(&path, "*.log\n").unwrap();
        assert!(existing_targets(tmp.path()).unwrap().is_empty());
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

    /// A workspace root at `root` with the given `members` patterns, plus a
    /// manifest in each of `dirs`.
    fn workspace_tree(root: &Path, members: &[&str], dirs: &[&str]) -> Workspace {
        fs::create_dir_all(root).unwrap();
        fs::write(root.join(RPROJ_MANIFEST_FILE), "").unwrap();
        for dir in dirs {
            let dir = root.join(dir);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(RPROJ_MANIFEST_FILE), "").unwrap();
        }
        Workspace {
            members: members.iter().map(|m| m.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn member_globs_match_one_path_component() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let ws = workspace_tree(root, &["packages/*"], &["packages/a", "packages/b/deep"]);
        assert_eq!(
            expand_workspace_members(root, &ws).unwrap(),
            vec![
                root.to_path_buf(),
                root.join("packages/a"),
                root.join("packages/b"),
            ]
        );
    }

    #[test]
    fn a_literal_member_pattern_needs_its_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let ws = workspace_tree(root, &["packages/a", "packages/nope"], &["packages/a"]);
        let err = expand_workspace_members(root, &ws).unwrap_err().to_string();
        assert!(err.contains("packages/nope"), "{}", err);
    }

    #[test]
    fn a_wildcard_member_pattern_may_match_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let ws = workspace_tree(root, &["packages/*"], &[]);
        assert_eq!(
            expand_workspace_members(root, &ws).unwrap(),
            vec![root.to_path_buf()]
        );
    }

    #[test]
    fn exclude_removes_a_matched_member() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mut ws = workspace_tree(root, &["packages/*"], &["packages/a", "packages/old"]);
        ws.exclude = vec!["packages/old".to_string()];
        assert_eq!(
            expand_workspace_members(root, &ws).unwrap(),
            vec![root.to_path_buf(), root.join("packages/a")]
        );
    }

    #[test]
    fn a_member_pattern_reaching_outside_the_root_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("ws");
        let ws = workspace_tree(&root, &["../outside"], &[]);
        let err = expand_workspace_members(&root, &ws)
            .unwrap_err()
            .to_string();
        assert!(err.contains("outside the workspace root"), "{}", err);
    }

    #[test]
    fn the_tooling_directories_are_never_members() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let ws = workspace_tree(root, &["*"], &["a"]);
        fs::create_dir_all(root.join(RVENV_DIR).join("lib")).unwrap();
        fs::create_dir_all(root.join(RVENV_SHIM_DIR)).unwrap();
        assert_eq!(
            expand_workspace_members(root, &ws).unwrap(),
            vec![root.to_path_buf(), root.join("a")]
        );
    }

    #[test]
    fn a_member_without_a_manifest_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let ws = workspace_tree(root, &["packages/*"], &["packages/a"]);
        fs::create_dir_all(root.join("packages/b")).unwrap();
        let err = workspace_members(root, &ws).unwrap_err().to_string();
        assert!(err.contains("packages"), "{}", err);
        assert!(err.contains("b"), "{}", err);
        assert!(err.contains(RPROJ_MANIFEST_FILE), "{}", err);
    }

    /// Write a workspace root manifest that declares `members`.
    fn write_workspace_manifest(root: &Path, members: &[&str]) {
        let patterns = members
            .iter()
            .map(|m| format!("\"{}\"", m))
            .collect::<Vec<_>>()
            .join(", ");
        fs::create_dir_all(root).unwrap();
        fs::write(
            root.join(RPROJ_MANIFEST_FILE),
            format!(
                "[project]\nname = \"ws\"\nversion = \"1.0.0\"\n\n\
                 [workspace]\nmembers = [{}]\n",
                patterns
            ),
        )
        .unwrap();
    }

    #[test]
    fn find_workspace_root_walks_up_past_the_member_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_workspace_manifest(root, &["packages/*"]);
        let member = root.join("packages/a");
        let deep = member.join("R");
        fs::create_dir_all(&deep).unwrap();
        fs::write(member.join(RPROJ_MANIFEST_FILE), "").unwrap();

        // From the member, from below it, and from the root itself.
        assert_eq!(find_workspace_root(&member).unwrap().as_deref(), Some(root));
        assert_eq!(find_workspace_root(&deep).unwrap().as_deref(), Some(root));
        assert_eq!(find_workspace_root(root).unwrap().as_deref(), Some(root));
    }

    #[test]
    fn a_project_outside_every_member_pattern_is_not_in_the_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_workspace_manifest(root, &["packages/*"]);
        let other = root.join("elsewhere/proj");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join(RPROJ_MANIFEST_FILE), "").unwrap();
        assert_eq!(find_workspace_root(&other).unwrap(), None);
    }

    #[test]
    fn a_plain_project_has_no_workspace_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("proj");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(RPROJ_MANIFEST_FILE), "").unwrap();
        assert_eq!(find_workspace_root(&root).unwrap(), None);
    }

    #[test]
    fn the_nearest_workspace_wins() {
        let tmp = tempfile::tempdir().unwrap();
        let outer = tmp.path();
        write_workspace_manifest(outer, &["inner", "inner/packages/*"]);
        let inner = outer.join("inner");
        write_workspace_manifest(&inner, &["packages/*"]);
        let member = inner.join("packages/a");
        fs::create_dir_all(&member).unwrap();
        fs::write(member.join(RPROJ_MANIFEST_FILE), "").unwrap();
        assert_eq!(
            find_workspace_root(&member).unwrap().as_deref(),
            Some(inner.as_path())
        );
    }

    #[test]
    fn the_shim_package_is_a_loadable_installed_package() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = tmp.path().join("lib");
        write_shim_package(&lib).unwrap();
        let pkg = lib.join("rvenv");
        let entries = |dir: &Path| {
            let mut names: Vec<String> = fs::read_dir(dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        };
        assert_eq!(
            entries(&pkg),
            ["DESCRIPTION", "LICENSE", "Meta", "NAMESPACE", "R"]
        );
        assert_eq!(entries(&pkg.join("R")), ["rvenv"]);
        assert_eq!(entries(&pkg.join("Meta")), ["package.rds"]);

        let desc = fs::read_to_string(pkg.join("DESCRIPTION")).unwrap();
        assert!(desc.contains("Package: rvenv\n"));
        // What lets a single committed copy load on every R, see
        // `write_shim_package()`. `library()` reads the same stamp out of
        // Meta/package.rds, which `cargo xtask gen-rvenv-shim --check` checks.
        assert!(desc.contains("\nBuilt: R 4.0.0;"));
        // The R code is the package source verbatim, not a lazy-load stub.
        assert!(fs::read_to_string(pkg.join("R/rvenv"))
            .unwrap()
            .contains(".onLoad <- function("));
    }

    #[test]
    fn writing_the_shim_twice_replaces_it() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = tmp.path().join("lib");
        write_shim_package(&lib).unwrap();
        let stray = lib.join("rvenv/stray-file");
        fs::write(&stray, "x").unwrap();
        write_shim_package(&lib).unwrap();
        assert!(!stray.exists());
        assert!(lib.join("rvenv/DESCRIPTION").exists());
    }

    #[test]
    fn rvenv_init_writes_the_tracked_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let written = rvenv_init(tmp.path()).unwrap();
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
        assert!(tmp.path().join(".rvenvlib/rvenv/DESCRIPTION").exists());
        // The project library is `rig proj sync`'s to create.
        assert!(!project_library(tmp.path()).exists());
    }

    // ------------------------------------------------------------- sync --

    /// The scripts use `@` themselves (`"$@"`, `@echo off`), so look for the
    /// template's placeholders by name rather than for a stray `@`.
    fn assert_no_placeholders(body: &str, file: &str) {
        for placeholder in ["@RVENV@", "@RVENV_NAME@", "@R_BINARY@", "@RVENV_EXPORTS@"] {
            assert!(
                !body.contains(placeholder),
                "{} is still in {}",
                placeholder,
                file
            );
        }
    }

    fn test_cfg() -> RvenvCfg {
        RvenvCfg {
            r_version: "4.6-arm64".to_string(),
            r_minor: "4.6".to_string(),
            r_binary: PathBuf::from("/opt/R/4.6/bin/R"),
            platform: "macos-arm64".to_string(),
            r_arch: "arm64".to_string(),
            rig_version: "0.10.0".to_string(),
            tools_lib: PathBuf::from("/Users/x/Library/R/arm64/4.6/library/__tools"),
        }
    }

    #[test]
    fn rvenv_cfg_roundtrips() {
        let cfg = test_cfg();
        assert_eq!(RvenvCfg::parse(&cfg.body()).unwrap(), cfg);
    }

    #[test]
    fn rvenv_cfg_needs_every_field() {
        assert!(RvenvCfg::parse("r-version = 4.6\n").is_err());
        // An unknown key is not an error: an older rig has to be able to read
        // what a newer one wrote.
        let mut text = test_cfg().body();
        text.push_str("something-new = 1\n");
        assert_eq!(RvenvCfg::parse(&text).unwrap(), test_cfg());
        // but a line that is not a comment and not a key = value is
        assert!(RvenvCfg::parse("r-version\n").is_err());
    }

    #[test]
    fn read_rvenv_cfg_is_none_before_the_first_sync() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_rvenv_cfg(tmp.path()).unwrap().is_none());
    }

    #[test]
    fn rvenv_sync_writes_the_machine_specific_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let written = rvenv_sync(root, &test_cfg(), &[]).unwrap();
        for path in &written {
            assert!(path.exists(), "{} was not written", path.display());
        }

        let wrappers: Vec<&str> = if cfg!(windows) {
            vec!["R.exe", "Rscript.exe"]
        } else {
            vec!["R", "Rscript"]
        };
        let mut expected: Vec<PathBuf> = vec![
            project_venv(root).join(RVENV_CFG_FILE),
            project_venv(root).join(RVENV_ONLOAD_FILE),
            project_etc(root).join(RVENV_REPOS_FILE),
        ];
        expected.extend(wrappers.iter().map(|f| project_bin(root).join(f)));
        expected.extend(
            ACTIVATE_TEMPLATES
                .iter()
                .map(|(f, _)| project_bin(root).join(f)),
        );
        let mut written = written;
        written.sort();
        expected.sort();
        assert_eq!(written, expected);

        assert_eq!(read_rvenv_cfg(root).unwrap(), Some(test_cfg()));
    }

    #[test]
    fn rvenv_sync_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let written = rvenv_sync(root, &test_cfg(), &[]).unwrap();
        let before: Vec<Vec<u8>> = written.iter().map(|p| fs::read(p).unwrap()).collect();
        rvenv_sync(root, &test_cfg(), &[]).unwrap();
        let after: Vec<Vec<u8>> = written.iter().map(|p| fs::read(p).unwrap()).collect();
        assert_eq!(before, after);
    }

    #[cfg(not(windows))]
    #[test]
    fn the_wrapper_sets_the_environment_and_execs_the_real_r() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let cfg = test_cfg();
        rvenv_sync(root, &cfg, &[]).unwrap();

        for (file, binary) in [
            ("R", "/opt/R/4.6/bin/R"),
            ("Rscript", "/opt/R/4.6/bin/Rscript"),
        ] {
            let path = project_bin(root).join(file);
            let body = fs::read_to_string(&path).unwrap();
            // Absolute path, so that the wrapper pins the R version, and
            // "$@" so that `R CMD INSTALL` and friends pass through.
            assert!(
                body.contains(&format!("exec \"{}\" \"$@\"", binary)),
                "{}",
                body
            );
            // The environment comes from `rvenv_env_vars`, relative to
            // $RVENV, which the wrapper works out from its own location.
            assert!(body.contains("RVENV=$(cd \"$(dirname \"$0\")/..\" && pwd)"));
            for (name, value) in rvenv_env_vars(Path::new("$RVENV")) {
                if name == "RVENV" {
                    continue;
                }
                assert!(
                    body.contains(&format!("export {}=\"{}\"", name, value)),
                    "{} is not exported by {}",
                    name,
                    file
                );
            }
            assert_no_placeholders(&body, file);

            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert!(mode & 0o111 != 0, "{} is not executable ({:o})", file, mode);
        }
    }

    #[test]
    fn every_activation_script_sets_the_same_variables() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        rvenv_sync(root, &test_cfg(), &[]).unwrap();
        for (file, _) in ACTIVATE_TEMPLATES {
            if *file == "deactivate.bat" {
                continue;
            }
            let body = fs::read_to_string(project_bin(root).join(file)).unwrap();
            for (name, _) in rvenv_env_vars(Path::new("/x")) {
                assert!(
                    body.contains(name.as_str()),
                    "{} does not set {}",
                    file,
                    name
                );
            }
            // The project path is baked in: a sourced script cannot find its
            // own location portably.
            assert!(body.contains(&project_venv(root).display().to_string()));
            assert_no_placeholders(&body, file);
        }
    }

    /// The data rows of a written repositories file, as
    /// `(name, menu name, url)`. The first two lines are the comment and the
    /// header.
    fn written_repositories(root: &Path) -> Vec<(String, String, String)> {
        fs::read_to_string(project_etc(root).join(RVENV_REPOS_FILE))
            .unwrap()
            .lines()
            .skip(2)
            .map(|line| {
                // Fields with a space in them are quoted in the file, as
                // base R's own `repositories` is.
                let f: Vec<String> = line
                    .split('\t')
                    .map(|v| v.trim_matches('"').to_string())
                    .collect();
                (f[0].clone(), f[1].clone(), f[2].clone())
            })
            .collect()
    }

    /// A [`RvenvCfg`] for another P3M target.
    fn test_cfg_for(platform: &str) -> RvenvCfg {
        RvenvCfg {
            platform: platform.to_string(),
            ..test_cfg()
        }
    }

    #[test]
    fn the_default_repository_is_the_targets_ppm() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        rvenv_sync(root, &test_cfg(), &[]).unwrap();
        assert_eq!(
            written_repositories(root),
            vec![(
                "CRAN".to_string(),
                PPM_MENU_NAME.to_string(),
                format!("{}/cran/latest", ppm_url())
            )]
        );
    }

    #[test]
    fn a_linux_target_gets_its_own_binary_url() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        rvenv_sync(root, &test_cfg_for("jammy-x86_64"), &[]).unwrap();
        assert_eq!(
            written_repositories(root)[0].2,
            format!("{}/cran/__linux__/jammy/latest", ppm_url())
        );
    }

    #[test]
    fn a_source_only_target_falls_back_to_cran() {
        // A source-only lock file records the machine's architecture, not a
        // P3M target, so there are no binaries to install from.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        rvenv_sync(root, &test_cfg_for("x86_64"), &[]).unwrap();
        assert_eq!(
            written_repositories(root),
            vec![(
                "CRAN".to_string(),
                "CRAN".to_string(),
                RVENV_DEFAULT_REPO_URL.to_string()
            )]
        );
    }

    #[test]
    fn the_project_repositories_follow_ppm() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let repos = vec![
            ManifestRepository {
                name: "internal".to_string(),
                url: "https://example.com/internal".to_string(),
            },
            ManifestRepository {
                name: "extra".to_string(),
                url: "https://example.com/extra".to_string(),
            },
        ];
        rvenv_sync(root, &test_cfg(), &repos).unwrap();
        // `rig proj sync` installs P3M binaries, so an `install.packages()`
        // in the environment installs from P3M as well. The project's own
        // repositories keep their names and follow it.
        assert_eq!(
            written_repositories(root),
            vec![
                (
                    "CRAN".to_string(),
                    PPM_MENU_NAME.to_string(),
                    format!("{}/cran/latest", ppm_url())
                ),
                (
                    "internal".to_string(),
                    "internal".to_string(),
                    "https://example.com/internal".to_string()
                ),
                (
                    "extra".to_string(),
                    "extra".to_string(),
                    "https://example.com/extra".to_string()
                ),
            ]
        );
    }

    #[test]
    fn the_first_repository_is_written_as_cran_without_ppm() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let repos = vec![
            ManifestRepository {
                name: "internal".to_string(),
                url: "https://example.com/internal".to_string(),
            },
            ManifestRepository {
                name: "extra".to_string(),
                url: "https://example.com/extra".to_string(),
            },
        ];
        rvenv_sync(root, &test_cfg_for("x86_64"), &repos).unwrap();
        // Without a P3M entry the first project repository is called CRAN in
        // the file, whatever the manifest calls it: R replaces its own
        // `@CRAN@` placeholder with an entry of that name only, and a
        // leftover placeholder breaks `install.packages()`. Its own name
        // survives as the menu name.
        assert_eq!(
            written_repositories(root),
            vec![
                (
                    "CRAN".to_string(),
                    "internal".to_string(),
                    "https://example.com/internal".to_string()
                ),
                (
                    "extra".to_string(),
                    "extra".to_string(),
                    "https://example.com/extra".to_string()
                ),
            ]
        );
    }

    #[test]
    fn a_project_repository_called_cran_does_not_duplicate_the_name() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let repos = vec![ManifestRepository {
            name: "CRAN".to_string(),
            url: "https://cran.r-project.org".to_string(),
        }];
        rvenv_sync(root, &test_cfg(), &repos).unwrap();
        assert_eq!(
            written_repositories(root),
            vec![(
                "CRAN".to_string(),
                PPM_MENU_NAME.to_string(),
                format!("{}/cran/latest", ppm_url())
            )]
        );
    }

    #[test]
    fn the_environment_is_not_the_users_library() {
        // The three things that keep an active session out of the user's own
        // library, all of which have to survive `--vanilla`.
        let vars: std::collections::HashMap<String, String> =
            rvenv_env_vars(Path::new("/p/.rvenv")).into_iter().collect();
        // `rvenv_env_vars()` joins with the native separator, `\` on Windows.
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(vars["RVENV"], "/p/.rvenv");
        assert_eq!(vars["R_LIBS_USER"], format!("/p/.rvenv{}lib", sep));
        // Empty, so that the project library stays .libPaths()[1].
        assert_eq!(vars["R_LIBS"], "");
        // Not empty: an empty R_LIBS_SITE does not disable the site library
        // on every R version.
        assert_eq!(vars["R_LIBS_SITE"], RVENV_NO_SITE);
        assert_eq!(
            vars["R_REPOSITORIES"],
            format!("/p/.rvenv{}etc{}repositories", sep, sep)
        );
    }
}
