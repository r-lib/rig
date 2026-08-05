use std::env;
use std::error::Error;
use std::io::IsTerminal;

use clap::ArgMatches;
use simple_error::*;
use tabular::*;

use super::config::{Enabled, RepoEntry, Repository};
use super::{get_repos_config, print_field, reflow, wrap};

pub fn sc_repos_available(
    args: &ArgMatches,
    libargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let config = get_repos_config()?;

    // `--json` is accepted at every level: `rig --json repos available`,
    // `rig repos --json available` and `rig repos available --json`.
    let json = args.get_flag("json") || libargs.get_flag("json") || mainargs.get_flag("json");

    match args.get_one::<String>("name") {
        // `rig repos available <name>`: the details of a single repository.
        Some(name) => {
            let repo = find_repo(&config, name)?;
            if json {
                println!("{}", serde_json::to_string_pretty(repo)?);
            } else {
                print_repo_info(repo);
            }
        }
        // `rig repos available`: an overview of the whole catalog.
        None => {
            if json {
                println!("{}", serde_json::to_string_pretty(&config)?);
            } else {
                print_repo_list(&config);
            }
        }
    }

    Ok(())
}

/// Look up a repository by name, case insensitively, the same way
/// `--with-repos` matches repository names.
fn find_repo<'a>(config: &'a [Repository], name: &str) -> Result<&'a Repository, Box<dyn Error>> {
    let needle = name.to_lowercase();
    if let Some(repo) = config.iter().find(|r| r.name.to_lowercase() == needle) {
        return Ok(repo);
    }

    let mut names: Vec<&str> = config.iter().map(|r| r.name.as_str()).collect();
    names.sort_by_key(|n| n.to_lowercase());
    bail!(
        "Invalid repository name: {}. Valid repositories are: {}",
        name,
        names.join(", ")
    );
}

/// Whether a repository is part of the default repository set, as stated by the
/// catalog. This is deliberately not resolved against the current machine
/// (`rig repos available` also works without R installed), so anything the
/// catalog makes conditional is `Depends`.
#[derive(Debug, PartialEq, Eq)]
enum DefaultState {
    /// A default everywhere, on every platform, architecture and R version.
    Yes,
    /// A default, but only where some URL of it applies.
    Depends,
    /// Never a default; it has to be enabled with `--with-repos`.
    No,
}

/// Whether an URL of a repository is unconditional, i.e. it applies to every
/// platform, architecture and R version.
fn entry_is_unconditional(entry: &RepoEntry) -> bool {
    [&entry.platforms, &entry.archs, &entry.rversions]
        .iter()
        .all(|values| values.as_ref().is_none_or(|v| v.is_empty()))
}

/// Whether a repository is a default, mirroring what `repos_setup` does per URL:
/// an URL is part of the default set when its default-enabled rule says so
/// (`entry.enabled` overriding `repo.enabled`) *and* its platform, architecture
/// and R version conditions match, so the repository is an unconditional default
/// only if one of its URLs is.
fn default_state(repo: &Repository) -> DefaultState {
    let mut conditional = false;
    for entry in &repo.repos {
        match entry.enabled.as_ref().unwrap_or(&repo.enabled) {
            // This URL is never part of the default set.
            Enabled::Always(false) => continue,
            Enabled::Always(true) => {
                if entry_is_unconditional(entry) {
                    return DefaultState::Yes;
                }
                conditional = true;
            }
            Enabled::OnPlatforms { .. } => conditional = true,
        }
    }

    if conditional {
        DefaultState::Depends
    } else {
        DefaultState::No
    }
}

/// The `Default` column of the listing.
fn default_column(repo: &Repository) -> &'static str {
    match default_state(repo) {
        DefaultState::Yes => "yes",
        DefaultState::Depends => "depends",
        DefaultState::No => "no",
    }
}

/// The same verdict for the detail view, which has the room to say what it
/// depends on and to spell out the platform patterns of a platform dependent
/// rule. The conditions themselves are in the per-URL blocks below it.
fn default_detail(repo: &Repository) -> String {
    match default_state(repo) {
        DefaultState::Yes => "yes, on all platforms".to_string(),
        DefaultState::No => "no, enable it with `--with-repos`".to_string(),
        DefaultState::Depends => match &repo.enabled {
            Enabled::OnPlatforms { platforms } => {
                format!("only on platforms matching {}", platforms.join(", "))
            }
            // The repository itself is a default, but each of its URLs is
            // restricted to some platforms, architectures or R versions.
            _ => {
                let which = if repo.repos.len() == 1 {
                    "the URL"
                } else {
                    "one of the URLs"
                };
                format!("yes, but only where {} below applies", which)
            }
        },
    }
}

/// The default-enabled rule of a single URL, for the detail view, used only when
/// the URL overrides its repository's rule.
fn entry_default_detail(enabled: &Enabled) -> String {
    match enabled {
        Enabled::Always(true) => "yes, for this URL".to_string(),
        Enabled::Always(false) => "no, not for this URL".to_string(),
        Enabled::OnPlatforms { platforms } => format!(
            "for this URL only on platforms matching {}",
            platforms.join(", ")
        ),
    }
}

/// The `label`, `value` pairs shown for a single repository URL.
///
/// `Name` appears only when the URL has a name of its own (Bioconductor's five
/// URLs do: `BioCsoft`, `BioCann`, ...), and `Default` only when the URL
/// overrides the repository's default-enabled rule.
fn entry_fields(repo: &Repository, entry: &RepoEntry) -> Vec<(&'static str, String)> {
    let mut fields: Vec<(&'static str, String)> = vec![];

    if entry.name != repo.name {
        fields.push(("Name", entry.name.clone()));
    }
    fields.push(("URL", entry.url.clone()));
    for (label, value) in [("Title", &entry.title), ("Description", &entry.description)] {
        if let Some(value) = value.as_deref().map(reflow).filter(|v| !v.is_empty()) {
            fields.push((label, value));
        }
    }
    if let Some(enabled) = &entry.enabled {
        fields.push(("Default", entry_default_detail(enabled)));
    }
    for (label, values) in [
        ("Platforms", &entry.platforms),
        ("Archs", &entry.archs),
        ("R versions", &entry.rversions),
    ] {
        if let Some(values) = values.as_ref().filter(|v| !v.is_empty()) {
            fields.push((label, values.join(", ")));
        }
    }

    fields
}

/// Pretty-print the repository catalog for `rig repos available`.
///
/// A colored header line names the number of repositories; the table then lists
/// each one with its default-enabled rule and title, in catalog order, which is
/// the order rig writes them into R's `repositories` file. The URLs and the
/// platform / architecture / R version conditions are in the detail view,
/// `rig repos available <name>`.
fn print_repo_list(config: &[Repository]) {
    use owo_colors::OwoColorize;

    let tty = std::io::stdout().is_terminal();
    let color = tty && env::var_os("NO_COLOR").is_none();

    // -- Header ------------------------------------------------------------
    let count = config.len();
    let repo_word = if count == 1 {
        "repository"
    } else {
        "repositories"
    };
    if color {
        println!("{} {}", count.cyan().bold(), repo_word);
    } else {
        println!("{} {}", count, repo_word);
    }
    if count == 0 {
        return;
    }
    println!();

    // -- Table -------------------------------------------------------------
    let mut tab: Table = Table::new("{:<}   {:<}   {:<}");
    tab.add_row(row!("Name", "Default", "Title"));
    tab.add_heading(
        "-----------------------------------------------------------------------------",
    );
    for repo in config {
        tab.add_row(row!(
            &repo.name,
            default_column(repo),
            repo.title.as_deref().unwrap_or("")
        ));
    }
    print!("{}", tab);

    // -- Footer ------------------------------------------------------------
    // Navigation hints only, so they are skipped when the output is redirected
    // and scripts and saved listings get the table alone.
    if tty {
        let mut hints = vec!["Use `rig repos available <name>` to see a repository's URLs."];
        if config
            .iter()
            .any(|r| default_state(r) == DefaultState::Depends)
        {
            hints.push("`depends`: a default only on some platforms, architectures or R versions.");
        }
        println!();
        for hint in hints {
            if color {
                println!("{}", hint.dimmed());
            } else {
                println!("{}", hint);
            }
        }
    }
}

/// Pretty-print a single repository for `rig repos available <name>`.
///
/// A colored header names the repository and how many URLs it has, followed by
/// its title and description, its default-enabled rule, and one block per URL
/// listing the platforms, architectures and R versions that URL applies to.
fn print_repo_info(repo: &Repository) {
    use owo_colors::OwoColorize;

    let color = std::io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none();
    let label_width = 13;

    // -- Header ------------------------------------------------------------
    let count = repo.repos.len();
    let tag = format!("({} {})", count, if count == 1 { "URL" } else { "URLs" });
    let mut header = if color {
        repo.name.cyan().bold().to_string()
    } else {
        repo.name.clone()
    };
    header.push(' ');
    header.push_str(&if color { tag.dimmed().to_string() } else { tag });
    println!("{}", header);

    if let Some(title) = repo.title.as_deref().map(reflow).filter(|t| !t.is_empty()) {
        if color {
            println!("{}", title.italic());
        } else {
            println!("{}", title);
        }
    }

    if let Some(desc) = repo
        .description
        .as_deref()
        .map(reflow)
        .filter(|d| !d.is_empty())
    {
        println!();
        for line in wrap(&desc, 78) {
            println!("{}", line);
        }
    }

    // -- Default-enabled rule ----------------------------------------------
    println!();
    print_field("Default", &default_detail(repo), label_width, color);

    // -- One block per URL -------------------------------------------------
    for entry in &repo.repos {
        println!();
        for (label, value) in entry_fields(repo, entry) {
            print_field(label, &value, label_width, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_repo(name: &str, enabled: Enabled, repos: Vec<RepoEntry>) -> Repository {
        Repository {
            name: name.to_string(),
            title: Some("A title".to_string()),
            description: None,
            enabled,
            repos,
        }
    }

    fn make_entry(name: &str) -> RepoEntry {
        RepoEntry {
            name: name.to_string(),
            title: None,
            description: None,
            url: "https://example.com".to_string(),
            platforms: None,
            archs: None,
            rversions: None,
            enabled: None,
        }
    }

    fn on_platforms(patterns: &[&str]) -> Enabled {
        Enabled::OnPlatforms {
            platforms: patterns.iter().map(|p| p.to_string()).collect(),
        }
    }

    /// An URL restricted to a platform, i.e. one that cannot make its
    /// repository an unconditional default.
    fn conditional_entry(name: &str) -> RepoEntry {
        let mut entry = make_entry(name);
        entry.platforms = Some(vec!["windows".to_string()]);
        entry
    }

    #[test]
    fn default_yes_needs_an_unconditional_url() {
        let repo = make_repo("CRAN", Enabled::Always(true), vec![make_entry("CRAN")]);
        assert_eq!(default_state(&repo), DefaultState::Yes);
        assert_eq!(default_column(&repo), "yes");
        assert_eq!(default_detail(&repo), "yes, on all platforms");
    }

    #[test]
    fn default_depends_when_every_url_is_conditional() {
        // The repository itself is a default, but each of its URLs is limited to
        // some platforms, architectures or R versions, the way P3M's and
        // CRAN-archive's are, so it is not a default everywhere.
        let repo = make_repo(
            "P3M",
            Enabled::Always(true),
            vec![conditional_entry("P3M"), conditional_entry("P3M")],
        );
        assert_eq!(default_state(&repo), DefaultState::Depends);
        assert_eq!(default_column(&repo), "depends");
        assert_eq!(
            default_detail(&repo),
            "yes, but only where one of the URLs below applies"
        );

        // CRAN-archive has a single, conditional URL.
        let repo = make_repo(
            "CRAN-archive",
            Enabled::Always(true),
            vec![conditional_entry("CRAN-archive")],
        );
        assert_eq!(
            default_detail(&repo),
            "yes, but only where the URL below applies"
        );
    }

    #[test]
    fn default_depends_on_platform_rule() {
        let repo = make_repo(
            "P3M-manylinux",
            on_platforms(&["*-linux-gnu-manylinux-*"]),
            vec![make_entry("P3M-manylinux")],
        );
        assert_eq!(default_state(&repo), DefaultState::Depends);
        assert_eq!(default_column(&repo), "depends");
        assert_eq!(
            default_detail(&repo),
            "only on platforms matching *-linux-gnu-manylinux-*"
        );
    }

    #[test]
    fn default_no_when_no_url_is_ever_enabled() {
        let repo = make_repo("RHUB", Enabled::Always(false), vec![make_entry("RHUB")]);
        assert_eq!(default_state(&repo), DefaultState::No);
        assert_eq!(default_column(&repo), "no");
        assert_eq!(default_detail(&repo), "no, enable it with `--with-repos`");
    }

    #[test]
    fn default_state_honors_url_level_overrides() {
        // An URL's own `enabled` overrides its repository's, in both directions.
        let mut off = make_entry("P3M");
        off.enabled = Some(Enabled::Always(false));
        let repo = make_repo("P3M", Enabled::Always(true), vec![off]);
        assert_eq!(default_state(&repo), DefaultState::No);

        let mut on = make_entry("RHUB");
        on.enabled = Some(Enabled::Always(true));
        let repo = make_repo("RHUB", Enabled::Always(false), vec![on]);
        assert_eq!(default_state(&repo), DefaultState::Yes);
    }

    #[test]
    fn default_state_of_a_repo_without_urls() {
        let repo = make_repo("Empty", Enabled::Always(true), vec![]);
        assert_eq!(default_state(&repo), DefaultState::No);
    }

    #[test]
    fn entry_is_unconditional_ignores_empty_lists() {
        let mut entry = make_entry("CRAN");
        entry.platforms = Some(vec![]);
        assert!(entry_is_unconditional(&entry));
        entry.archs = Some(vec!["x86_64".to_string()]);
        assert!(!entry_is_unconditional(&entry));
    }

    #[test]
    fn catalog_default_states() {
        // The catalog's own verdicts, so that the `Default` column cannot start
        // claiming that a conditional repository is a default everywhere. Only
        // CRAN is: every P3M URL is limited to a platform and architecture, and
        // CRAN-archive's is limited to Windows and macOS and R older than 4.0.0.
        let config = get_repos_config().unwrap();
        let states: Vec<(&str, &str)> = config
            .iter()
            .map(|r| (r.name.as_str(), default_column(r)))
            .collect();
        assert_eq!(
            states,
            vec![
                ("P3M", "depends"),
                ("P3M-manylinux", "depends"),
                ("RHUB", "no"),
                ("r-universe/cran", "depends"),
                ("r-universe/bioc", "no"),
                ("CRAN", "yes"),
                ("CRAN-archive", "depends"),
                ("Bioconductor", "no"),
            ]
        );
    }

    #[test]
    fn entry_fields_hides_redundant_name() {
        let repo = make_repo("P3M", Enabled::Always(true), vec![make_entry("P3M")]);
        let fields = entry_fields(&repo, &repo.repos[0]);
        assert_eq!(fields, vec![("URL", "https://example.com".to_string())]);
    }

    #[test]
    fn entry_fields_shows_own_name() {
        // Bioconductor's URLs have names of their own.
        let repo = make_repo(
            "Bioconductor",
            Enabled::Always(false),
            vec![make_entry("BioCsoft")],
        );
        let fields = entry_fields(&repo, &repo.repos[0]);
        assert_eq!(fields[0], ("Name", "BioCsoft".to_string()));
    }

    #[test]
    fn entry_fields_shows_constraints_and_override() {
        let mut entry = make_entry("P3M");
        entry.enabled = Some(Enabled::Always(false));
        entry.platforms = Some(vec!["macos".to_string(), "*-apple-*".to_string()]);
        entry.archs = Some(vec!["x86_64".to_string(), "aarch64".to_string()]);
        entry.rversions = Some(vec!["< 4.0.0".to_string()]);
        let repo = make_repo("P3M", Enabled::Always(true), vec![entry]);
        let fields = entry_fields(&repo, &repo.repos[0]);
        assert_eq!(
            fields,
            vec![
                ("URL", "https://example.com".to_string()),
                ("Default", "no, not for this URL".to_string()),
                ("Platforms", "macos, *-apple-*".to_string()),
                ("Archs", "x86_64, aarch64".to_string()),
                ("R versions", "< 4.0.0".to_string()),
            ]
        );
    }

    #[test]
    fn entry_fields_skips_empty_values() {
        let mut entry = make_entry("P3M");
        entry.title = Some("".to_string());
        entry.platforms = Some(vec![]);
        let repo = make_repo("P3M", Enabled::Always(true), vec![entry]);
        let fields = entry_fields(&repo, &repo.repos[0]);
        assert_eq!(fields, vec![("URL", "https://example.com".to_string())]);
    }

    #[test]
    fn find_repo_is_case_insensitive() {
        let config = vec![
            make_repo("P3M", Enabled::Always(true), vec![]),
            make_repo("CRAN", Enabled::Always(true), vec![]),
        ];
        assert_eq!(find_repo(&config, "p3m").unwrap().name, "P3M");
        assert_eq!(find_repo(&config, "CRAN").unwrap().name, "CRAN");
    }

    #[test]
    fn find_repo_error_lists_valid_names() {
        let config = vec![
            make_repo("P3M", Enabled::Always(true), vec![]),
            make_repo("CRAN", Enabled::Always(true), vec![]),
        ];
        let err = find_repo(&config, "nope").unwrap_err().to_string();
        assert!(err.contains("nope"));
        assert!(err.contains("CRAN, P3M"));
    }

    #[test]
    fn catalog_names_are_unique_case_insensitively() {
        // The case insensitive lookup (and `--with-repos`) would be ambiguous
        // otherwise.
        let config = get_repos_config().unwrap();
        let mut names: Vec<String> = config.iter().map(|r| r.name.to_lowercase()).collect();
        names.sort();
        let len = names.len();
        names.dedup();
        assert_eq!(names.len(), len);
    }
}
