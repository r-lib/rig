//! `rig pkg list`: the packages installed in a package library.
//!
//! Unlike the other `rig pkg` subcommands, which read package metadata from the
//! repositories, this one reads a library directory on disk: one subdirectory
//! per installed package, each with a `DESCRIPTION` file.
//!
//! The parsing is deliberately lenient. A library holds whatever was installed
//! into it, including packages built from a local source tree, so a version
//! like `0.1-alpha` or a `Built:` field R itself would not write must not stop
//! the listing — an unreadable package is skipped, and a field that cannot be
//! parsed is shown as unknown.

use std::env;
use std::error::Error;
use std::fs::File;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use clap::ArgMatches;
use deb822_fast::Deb822;
use log::debug;
use simple_error::*;
use tabular::*;

use crate::dcf::DCFBuilt;
use crate::install::{parse_linkingto, REMOTE_HASH_FIELD, REMOTE_LINKINGTO_FIELD};
use crate::library::{library_rver, sc_library_get_default, sc_library_get_list};
use crate::textfmt::reflow;

pub fn sc_pkg_list(
    args: &ArgMatches,
    pkgargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let json = args.get_flag("json") || pkgargs.get_flag("json") || mainargs.get_flag("json");

    let lib = resolve_library(args)?;
    let mut pkgs = read_installed(&lib.path)?;
    pkgs.sort_by(|a, b| {
        a.package
            .to_lowercase()
            .cmp(&b.package.to_lowercase())
            .then_with(|| a.package.cmp(&b.package))
    });

    if json {
        print_installed_json(&pkgs)?;
    } else {
        print_installed(&lib, &pkgs);
    }

    Ok(())
}

// ------------------------------------------------------------------------
// Which library

/// The library `rig pkg list` was asked about.
///
/// `name` and `rversion` are only known for a library of an R installation, and
/// are unset when `--library` named a directory directly: rig does not need to
/// know which R version, if any, that directory belongs to, and never asks.
pub(super) struct ResolvedLibrary {
    pub(super) name: Option<String>,
    pub(super) path: PathBuf,
    pub(super) rversion: Option<String>,
}

impl ResolvedLibrary {
    /// How to name this library in the header line of a listing: the R version
    /// and library name it belongs to, when it has them, and always its path.
    pub(super) fn tag(&self) -> String {
        match (&self.rversion, &self.name) {
            (Some(rver), Some(name)) => format!("(R {}, {}: {})", rver, name, self.path.display()),
            _ => format!("({})", self.path.display()),
        }
    }
}

/// Resolve `--library`, in three cases:
///
/// * a directory path is used as it is, without consulting R at all, so this
///   also works with no R version installed;
/// * any other `--library` value is a library name of the R version, as
///   `rig library list` prints them;
/// * without `--library` it is the default library of the R version, i.e. the
///   path `rig library default --json` reports.
pub(super) fn resolve_library(args: &ArgMatches) -> Result<ResolvedLibrary, Box<dyn Error>> {
    let lib = args.get_one::<String>("library");

    if let Some(lib) = lib {
        if Path::new(lib).is_dir() || looks_like_path(lib) {
            debug!("Using library directory {}", lib);
            return Ok(ResolvedLibrary {
                name: None,
                path: PathBuf::from(lib),
                rversion: None,
            });
        }
    }

    let rver = library_rver(args)?;

    let lib = match lib {
        None => sc_library_get_default(&rver)?,
        Some(name) => {
            let libs = sc_library_get_list(Some(rver.to_string()), true)?;
            match libs.iter().find(|lib| &lib.name == name) {
                Some(lib) => lib.clone(),
                None => {
                    let known: Vec<&str> = libs.iter().map(|lib| lib.name.as_str()).collect();
                    bail!(
                        "No such library: {}, for R {}. Known libraries: {}. \
                        (`--library` also takes the path of a library directory.)",
                        name,
                        rver,
                        known.join(", ")
                    )
                }
            }
        }
    };

    Ok(ResolvedLibrary {
        name: Some(lib.name),
        path: lib.path,
        rversion: Some(rver),
    })
}

/// Whether a `--library` value is meant as a path rather than a library name.
///
/// An existing directory is unambiguous, but a directory rig is about to create
/// is not there to be looked at yet, and a mistyped path should not be reported
/// as an unknown library name. A library name is a single path component — that
/// is how `rig library add` creates them — so anything with a separator in it,
/// or anchored to a root, is a path.
fn looks_like_path(lib: &str) -> bool {
    let path = Path::new(lib);
    path.is_absolute() || path.components().count() > 1 || lib.starts_with('~')
}

// ------------------------------------------------------------------------
// Reading the library

/// One installed package, as its `DESCRIPTION` describes it.
///
/// `version` is the string from the file, not an [`crate::dcf::RPackageVersion`]:
/// this is a report on what is installed, so a version rig cannot parse is
/// still shown as it is.
#[derive(Debug)]
pub(crate) struct InstalledPackage {
    pub(crate) package: String,
    pub(crate) version: String,
    /// The directory the package is installed in, i.e. the one holding its
    /// `DESCRIPTION`. Usually named after the package, but the `Package` field
    /// of the `DESCRIPTION` is what `package` reports, so the two can differ.
    pub(super) path: PathBuf,
    built_r: Option<String>,
    platform: Option<String>,
    /// Where the package came from: the repository name (`CRAN`) for a
    /// repository install, otherwise the remote type (`github`, `git`, …).
    source: Option<String>,
    /// Which remote, for a package that was not installed from a repository:
    /// the `user/repo` of a GitHub install, the URL of a git one, etc.
    remote: Option<String>,
    /// The `RemoteHash` field, i.e. which upstream CRAN artifact this package
    /// was installed from. Only `rig pkg install` writes it, so it is unset for
    /// anything installed by R, pak or renv.
    pub(crate) hash: Option<String>,
    /// The `RemoteLinkingToHashes` field: what the package was compiled against,
    /// as `(package, version, sha256)`.
    pub(crate) linkingto: Vec<(String, String, String)>,
}

#[cfg(test)]
impl InstalledPackage {
    /// An installed package with only the fields the install planner looks at,
    /// so that its tests do not need a library on disk.
    pub(super) fn for_test(
        package: &str,
        version: &str,
        hash: Option<&str>,
        linkingto: Vec<(String, String, String)>,
    ) -> InstalledPackage {
        InstalledPackage {
            package: package.to_string(),
            version: version.to_string(),
            path: PathBuf::from(package),
            built_r: None,
            platform: None,
            source: None,
            remote: None,
            hash: hash.map(|x| x.to_string()),
            linkingto,
        }
    }
}

/// The packages installed in the library at `path`, unordered.
///
/// Everything that is not a readable package is skipped, and only logged:
/// rig's own `__<name>` sibling libraries and the `___default` marker file
/// (see [`crate::library::sc_library_get_list`]) live in the main library
/// directory, and a package whose installation was interrupted has no
/// `DESCRIPTION` yet.
pub(crate) fn read_installed(path: &Path) -> Result<Vec<InstalledPackage>, Box<dyn Error>> {
    debug!("Listing packages in {}", path.display());

    let entries = match std::fs::read_dir(path) {
        Ok(x) => x,
        Err(err) => bail!("Cannot read library at {}: {}", path.display(), err),
    };

    let mut pkgs = Vec::new();

    for entry in entries {
        let entry = entry?;
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }

        // A package name cannot start with `.` or `_`, so anything that does is
        // not one: dot-directories, and rig's own `__<name>` libraries.
        let name = match dir.file_name().and_then(|x| x.to_str()) {
            Some(x) if !x.starts_with('.') && !x.starts_with('_') => x.to_string(),
            _ => continue,
        };

        match read_package(&dir, &name) {
            Ok(Some(pkg)) => pkgs.push(pkg),
            Ok(None) => {}
            Err(err) => debug!("Skipping {}: {}", dir.display(), err),
        }
    }

    Ok(pkgs)
}

/// Read the `DESCRIPTION` of the package installed at `dir`.
///
/// `Ok(None)` means the directory is not a package, i.e. has no `DESCRIPTION`;
/// an `Err` means it has one that could not be read or parsed. Both are skipped
/// by the caller, and told apart only in the log.
fn read_package(dir: &Path, dir_name: &str) -> Result<Option<InstalledPackage>, Box<dyn Error>> {
    let desc_path = dir.join("DESCRIPTION");
    if !desc_path.exists() {
        debug!("No DESCRIPTION in {}, not a package", dir.display());
        return Ok(None);
    }

    let desc = Deb822::from_reader(File::open(&desc_path)?)?;
    let para = match desc.iter().next() {
        Some(x) => x,
        None => bail!("empty DESCRIPTION file"),
    };

    // A package directory is named after the package, so the directory name is
    // a good enough fallback for a DESCRIPTION without a `Package` field.
    let package = match para.get("Package") {
        Some(x) => reflow(x),
        None => dir_name.to_string(),
    };
    let version = match para.get("Version") {
        Some(x) => reflow(x),
        None => "?".to_string(),
    };

    // R writes `Built` when it installs a package, but a package copied into
    // the library by hand may not have it, and a malformed one is no reason to
    // leave the package out of the listing.
    let built = para
        .get("Built")
        .map(|x| DCFBuilt::from_str(&reflow(x)))
        .and_then(|x| match x {
            Ok(built) => Some(built),
            Err(err) => {
                debug!("Ignoring unparseable Built field of {}: {}", package, err);
                None
            }
        });

    let (source, remote) = read_source(para);

    // Written by `rig pkg install`, absent from anything else, which is exactly
    // what makes a package without them a candidate for reinstallation.
    let hash = para.get(REMOTE_HASH_FIELD).map(reflow);
    let linkingto = para
        .get(REMOTE_LINKINGTO_FIELD)
        .map(|x| parse_linkingto(&reflow(x)))
        .unwrap_or_default();

    Ok(Some(InstalledPackage {
        package,
        version,
        path: dir.to_path_buf(),
        built_r: built.as_ref().map(|x| x.r.clone()),
        platform: built.and_then(|x| x.platform),
        source,
        remote,
        hash,
        linkingto,
    }))
}

/// Where a package was installed from, as `(source, remote)`.
///
/// A package installed from a repository has a `Repository` field naming it,
/// e.g. `CRAN`, and that is all there is to say. pak and remotes write a
/// `RemoteType` instead, plus fields describing the remote itself, and those
/// are what tell two GitHub installs of the same package apart, so they are
/// worth reporting: `remote` is the repository of a GitHub (GitLab, Bitbucket)
/// install, and the URL or path of a git, url or local one.
fn read_source(para: &deb822_fast::Paragraph) -> (Option<String>, Option<String>) {
    if let Some(repo) = para.get("Repository") {
        return (Some(reflow(repo)), None);
    }

    let field = |name: &str| para.get(name).map(reflow).filter(|x| !x.is_empty());

    let rtype = match field("RemoteType") {
        Some(x) => x,
        // Old devtools and remotes versions wrote only the `Github*` fields,
        // without a `RemoteType`, so those are all an old GitHub install has.
        None => match (field("GithubUsername"), field("GithubRepo")) {
            (Some(user), Some(repo)) => {
                return (
                    Some("github".to_string()),
                    Some(format!("{}/{}", user, repo)),
                )
            }
            _ => return (None, None),
        },
    };

    // The `user/repo` of the code hosts pak knows, with the host itself in
    // front of it when the package came from a self-hosted instance, e.g. a
    // GitHub Enterprise one.
    let hosted = || match (field("RemoteUsername"), field("RemoteRepo")) {
        (Some(user), Some(repo)) => {
            let repo = match field("RemoteHost") {
                Some(host) if !is_default_host(&host) => format!("{}/{}/{}", host, user, repo),
                _ => format!("{}/{}", user, repo),
            };
            Some(repo)
        }
        _ => None,
    };

    // `RemotePkgRef` is the package reference the install was requested with,
    // e.g. `r-lib/cli` or `git::https://github.com/r-lib/cli.git`. It is the
    // fallback for a remote type rig does not know: it always describes the
    // remote, if not always tersely.
    let pkg_ref = || {
        field("RemotePkgRef").map(|r| match r.split_once("::") {
            Some((prefix, rest)) if prefix == rtype => rest.to_string(),
            _ => r,
        })
    };

    let remote = match rtype.as_str() {
        "github" | "gitlab" | "bitbucket" => hosted().or_else(pkg_ref),
        "git" | "url" => field("RemoteUrl").or_else(pkg_ref),
        "local" => field("RemoteUrl").or_else(pkg_ref),
        _ => field("RemoteUrl").or_else(hosted).or_else(pkg_ref),
    };

    (Some(rtype), remote)
}

/// Whether a `RemoteHost` is the code host's own, and so not worth naming: only
/// a self-hosted instance is. pak writes the API host for GitHub.
fn is_default_host(host: &str) -> bool {
    matches!(
        host,
        "api.github.com" | "github.com" | "gitlab.com" | "bitbucket.org"
    )
}

// ------------------------------------------------------------------------
// Output

/// A table cell for a field the `DESCRIPTION` did not have.
fn cell(value: Option<&String>) -> &str {
    match value {
        Some(x) => x,
        None => "-",
    }
}

/// The `Source` cell of a row: the repository the package came from, or, for a
/// package installed from a remote, the remote type and the remote itself, in
/// pak's package reference syntax, e.g. `github::r-lib/cli`.
fn source_cell(pkg: &InstalledPackage) -> String {
    match (&pkg.source, &pkg.remote) {
        (Some(source), Some(remote)) => format!("{}::{}", source, remote),
        (Some(source), None) => source.clone(),
        _ => "-".to_string(),
    }
}

/// Pretty-print the packages installed in a library.
///
/// A colored header line names the number of packages and the library they were
/// found in; the table then lists each package with its version, the R version
/// and platform it was built for, and where it was installed from.
fn print_installed(lib: &ResolvedLibrary, pkgs: &[InstalledPackage]) {
    use owo_colors::OwoColorize;

    let color = std::io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none();

    // -- Header ------------------------------------------------------------
    let count = pkgs.len();
    let pkg_word = if count == 1 { "package" } else { "packages" };
    let head = if color {
        format!("{} {}", count.cyan().bold(), pkg_word)
    } else {
        format!("{} {}", count, pkg_word)
    };
    let tag = lib.tag();
    println!(
        "{} {}",
        head,
        if color { tag.dimmed().to_string() } else { tag }
    );
    if count == 0 {
        return;
    }
    println!();

    // -- Table -------------------------------------------------------------
    let mut tab: Table = Table::new("{:<}   {:<}   {:<}   {:<}   {:<}");
    tab.add_row(row!("Package", "Version", "Built", "Platform", "Source"));
    for pkg in pkgs {
        tab.add_row(row!(
            &pkg.package,
            &pkg.version,
            cell(pkg.built_r.as_ref()),
            cell(pkg.platform.as_ref()),
            source_cell(pkg)
        ));
    }

    print_table(&tab);
}

/// Print a table whose first row is its header, with a rule under that header
/// spanning the table.
///
/// The five columns here are wide and their widths only known once `tabular`
/// has laid them out, so the rule is measured from the rendered table rather
/// than being added as a fixed-width heading row.
fn print_table(tab: &Table) {
    let rendered = tab.to_string();
    let width = rendered
        .lines()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    let mut lines = rendered.lines();
    if let Some(header) = lines.next() {
        println!("{}", header);
        println!("{}", "-".repeat(width));
        for line in lines {
            println!("{}", line);
        }
    }
}

/// Print the installed packages as a JSON array, one object per package.
fn print_installed_json(pkgs: &[InstalledPackage]) -> Result<(), Box<dyn Error>> {
    #[derive(serde::Serialize)]
    struct InstalledEntry<'a> {
        package: &'a str,
        version: &'a str,
        built: Option<&'a str>,
        platform: Option<&'a str>,
        source: Option<&'a str>,
        remote: Option<&'a str>,
    }

    let entries: Vec<InstalledEntry> = pkgs
        .iter()
        .map(|pkg| InstalledEntry {
            package: &pkg.package,
            version: &pkg.version,
            built: pkg.built_r.as_deref(),
            platform: pkg.platform.as_deref(),
            source: pkg.source.as_deref(),
            remote: pkg.remote.as_deref(),
        })
        .collect();

    println!("{}", serde_json::to_string_pretty(&entries)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a package directory with the given `DESCRIPTION` contents.
    fn add_package(lib: &Path, dir: &str, desc: &str) {
        let dir = lib.join(dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("DESCRIPTION"), desc).unwrap();
    }

    /// The packages of a library, ordered as `sc_pkg_list` orders them.
    fn sorted(path: &Path) -> Vec<InstalledPackage> {
        let mut pkgs = read_installed(path).unwrap();
        pkgs.sort_by(|a, b| {
            a.package
                .to_lowercase()
                .cmp(&b.package.to_lowercase())
                .then_with(|| a.package.cmp(&b.package))
        });
        pkgs
    }

    fn names(pkgs: &[InstalledPackage]) -> Vec<&str> {
        pkgs.iter().map(|p| p.package.as_str()).collect()
    }

    #[test]
    fn binary_package_has_all_fields() {
        let tmp = tempfile::tempdir().unwrap();
        add_package(
            tmp.path(),
            "cli",
            "Package: cli\n\
             Version: 3.6.3\n\
             Repository: CRAN\n\
             Built: R 4.4.0; aarch64-apple-darwin20; 2024-06-21 20:16:33 UTC; unix\n",
        );

        let pkgs = sorted(tmp.path());
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].package, "cli");
        assert_eq!(pkgs[0].version, "3.6.3");
        assert_eq!(pkgs[0].built_r.as_deref(), Some("4.4.0"));
        assert_eq!(pkgs[0].platform.as_deref(), Some("aarch64-apple-darwin20"));
        assert_eq!(pkgs[0].source.as_deref(), Some("CRAN"));
    }

    #[test]
    fn source_install_has_no_platform() {
        let tmp = tempfile::tempdir().unwrap();
        add_package(
            tmp.path(),
            "mypkg",
            "Package: mypkg\n\
             Version: 0.0.1\n\
             Built: R 4.4.1; ; 2024-08-01 10:00:00 UTC; unix\n",
        );

        let pkgs = sorted(tmp.path());
        assert_eq!(pkgs[0].built_r.as_deref(), Some("4.4.1"));
        assert_eq!(pkgs[0].platform, None);
        assert_eq!(pkgs[0].source, None);
    }

    /// A version rig cannot parse as an [`crate::dcf::RPackageVersion`], and a
    /// `Built` field that is not R's, are both shown rather than rejected.
    #[test]
    fn unparseable_fields_do_not_drop_the_package() {
        let tmp = tempfile::tempdir().unwrap();
        add_package(
            tmp.path(),
            "weird",
            "Package: weird\n\
             Version: 0.1-alpha\n\
             Built: whatever\n",
        );

        let pkgs = sorted(tmp.path());
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].version, "0.1-alpha");
        assert_eq!(pkgs[0].built_r, None);
        assert_eq!(pkgs[0].platform, None);
    }

    #[test]
    fn missing_version_is_unknown() {
        let tmp = tempfile::tempdir().unwrap();
        add_package(tmp.path(), "noversion", "Package: noversion\n");

        let pkgs = sorted(tmp.path());
        assert_eq!(pkgs[0].version, "?");
    }

    #[test]
    fn remote_type_is_the_source() {
        let tmp = tempfile::tempdir().unwrap();
        add_package(
            tmp.path(),
            "gh",
            "Package: gh\n\
             Version: 1.4.1\n\
             RemoteType: github\n",
        );

        let pkgs = sorted(tmp.path());
        assert_eq!(pkgs[0].source.as_deref(), Some("github"));
        assert_eq!(pkgs[0].remote, None);
        assert_eq!(source_cell(&pkgs[0]), "github");
    }

    /// A GitHub install names the repository it came from, as
    /// `github::<user>/<repo>`.
    #[test]
    fn github_install_names_the_repository() {
        let tmp = tempfile::tempdir().unwrap();
        add_package(
            tmp.path(),
            "asciicast",
            "Package: asciicast\n\
             Version: 2.3.1.9000\n\
             RemoteType: github\n\
             RemoteHost: api.github.com\n\
             RemoteUsername: r-lib\n\
             RemoteRepo: asciicast\n\
             RemoteRef: HEAD\n\
             RemoteSha: 0d1e0f0\n\
             RemotePkgRef: r-lib/asciicast\n",
        );

        let pkgs = sorted(tmp.path());
        assert_eq!(pkgs[0].source.as_deref(), Some("github"));
        assert_eq!(pkgs[0].remote.as_deref(), Some("r-lib/asciicast"));
        assert_eq!(source_cell(&pkgs[0]), "github::r-lib/asciicast");
    }

    /// A self-hosted code host is worth naming, the public one is not.
    #[test]
    fn a_non_default_host_is_part_of_the_remote() {
        let tmp = tempfile::tempdir().unwrap();
        add_package(
            tmp.path(),
            "internal",
            "Package: internal\n\
             Version: 1.0.0\n\
             RemoteType: github\n\
             RemoteHost: github.acme.com/api/v3\n\
             RemoteUsername: tools\n\
             RemoteRepo: internal\n",
        );

        let pkgs = sorted(tmp.path());
        assert_eq!(
            pkgs[0].remote.as_deref(),
            Some("github.acme.com/api/v3/tools/internal")
        );
    }

    /// A git install names its URL.
    #[test]
    fn git_install_names_the_url() {
        let tmp = tempfile::tempdir().unwrap();
        add_package(
            tmp.path(),
            "cli",
            "Package: cli\n\
             Version: 3.6.3\n\
             RemoteType: git\n\
             RemoteUrl: https://github.com/r-lib/cli.git\n\
             RemoteRef: main\n",
        );

        let pkgs = sorted(tmp.path());
        assert_eq!(pkgs[0].source.as_deref(), Some("git"));
        assert_eq!(
            pkgs[0].remote.as_deref(),
            Some("https://github.com/r-lib/cli.git")
        );
        assert_eq!(
            source_cell(&pkgs[0]),
            "git::https://github.com/r-lib/cli.git"
        );
    }

    /// An install by an old devtools or remotes, which wrote no `RemoteType`.
    #[test]
    fn old_github_fields_are_understood() {
        let tmp = tempfile::tempdir().unwrap();
        add_package(
            tmp.path(),
            "old",
            "Package: old\n\
             Version: 0.1.0\n\
             GithubRepo: old\n\
             GithubUsername: someone\n\
             GithubRef: master\n\
             GithubSHA1: 0d1e0f0\n",
        );

        let pkgs = sorted(tmp.path());
        assert_eq!(pkgs[0].source.as_deref(), Some("github"));
        assert_eq!(pkgs[0].remote.as_deref(), Some("someone/old"));
    }

    /// Without the fields that describe the remote, the package reference the
    /// install was requested with stands in for it, without its redundant
    /// `<type>::` prefix.
    #[test]
    fn pkg_ref_stands_in_for_the_remote() {
        let tmp = tempfile::tempdir().unwrap();
        add_package(
            tmp.path(),
            "mypkg",
            "Package: mypkg\n\
             Version: 0.0.1\n\
             RemoteType: local\n\
             RemotePkgRef: local::/Users/gaborcsardi/works/mypkg\n",
        );

        let pkgs = sorted(tmp.path());
        assert_eq!(pkgs[0].source.as_deref(), Some("local"));
        assert_eq!(
            pkgs[0].remote.as_deref(),
            Some("/Users/gaborcsardi/works/mypkg")
        );
    }

    /// A `Repository` field wins over a `RemoteType` one, which pak also writes
    /// for a package it installed from a repository.
    #[test]
    fn repository_wins_over_remote_type() {
        let tmp = tempfile::tempdir().unwrap();
        add_package(
            tmp.path(),
            "glue",
            "Package: glue\n\
             Version: 1.8.0\n\
             Repository: CRAN\n\
             RemoteType: standard\n",
        );

        let pkgs = sorted(tmp.path());
        assert_eq!(pkgs[0].source.as_deref(), Some("CRAN"));
        assert_eq!(pkgs[0].remote, None);
    }

    #[test]
    fn other_libraries_and_files_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        add_package(
            tmp.path(),
            "cli",
            "Package: cli\nVersion: 3.6.3\nRepository: CRAN\n",
        );
        // A sibling rig library, with a package in it.
        add_package(
            tmp.path(),
            "__other/glue",
            "Package: glue\nVersion: 1.8.0\n",
        );
        // The marker file of rig's default library, and a dot-directory.
        std::fs::write(tmp.path().join("___default"), "main\n").unwrap();
        std::fs::create_dir(tmp.path().join(".Rcache")).unwrap();
        // A file, not a directory.
        std::fs::write(tmp.path().join("R.css"), "").unwrap();
        // An interrupted installation, without a DESCRIPTION yet.
        std::fs::create_dir(tmp.path().join("00LOCK-cli")).unwrap();
        std::fs::create_dir(tmp.path().join("half")).unwrap();

        assert_eq!(names(&sorted(tmp.path())), vec!["cli"]);
    }

    #[test]
    fn empty_library_is_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_installed(tmp.path()).unwrap().is_empty());
    }

    #[test]
    fn missing_library_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let err = read_installed(&tmp.path().join("nope")).unwrap_err();
        assert!(err.to_string().contains("Cannot read library at"));
    }

    #[test]
    fn sorting_is_case_insensitive() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["zoo", "Matrix", "abc"] {
            add_package(
                tmp.path(),
                name,
                &format!("Package: {}\nVersion: 1.0.0\n", name),
            );
        }

        assert_eq!(names(&sorted(tmp.path())), vec!["abc", "Matrix", "zoo"]);
    }

    /// The package name comes from the `DESCRIPTION`, not from the directory,
    /// but a `DESCRIPTION` without one falls back to the directory name.
    #[test]
    fn package_name_falls_back_to_the_directory() {
        let tmp = tempfile::tempdir().unwrap();
        add_package(tmp.path(), "nameless", "Version: 1.0.0\n");

        let pkgs = sorted(tmp.path());
        assert_eq!(pkgs[0].package, "nameless");
    }
}
