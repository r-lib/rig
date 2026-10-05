use clap::ArgMatches;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReposSetupArgs {
    Default {
        whitelist: Vec<String>,
        blacklist: Vec<String>,
    },
    Empty {
        whitelist: Vec<String>,
    },
}

pub fn interpret_repos_args(args: &ArgMatches, deprecated: bool) -> ReposSetupArgs {
    let mut setup;

    let without_repos = args.get_one::<String>("without-repos");

    match without_repos {
        Some(value) if value == "ALL REPOSITORIES" => {
            // Specified without a value: --without-repos
            setup = ReposSetupArgs::Empty {
                whitelist: Vec::new(),
            };
        }
        _ => {
            // Not specified at all, or specified with a value: --without-repos=repo1,repo2
            setup = ReposSetupArgs::Default {
                whitelist: Vec::new(),
                blacklist: Vec::new(),
            };

            if deprecated {
                if args.get_flag("without-cran-mirror") {
                    if let ReposSetupArgs::Default { blacklist, .. } = &mut setup {
                        blacklist.push("cran".to_string());
                    }
                }
                if args.get_flag("without-p3m") {
                    if let ReposSetupArgs::Default { blacklist, .. } = &mut setup {
                        blacklist.push("p3m".to_string());
                    }
                }
            }
        }
    }

    if let Some(without_repos) = without_repos {
        if without_repos != "ALL REPOSITORIES" {
            let repos: Vec<String> = without_repos
                .split(',')
                .map(|s| s.trim().to_string().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect();
            if let ReposSetupArgs::Default { blacklist, .. } = &mut setup {
                blacklist.extend(repos);
            }
        }
    }

    if let Some(with_repos) = args.get_one::<String>("with-repos") {
        let repos: Vec<String> = with_repos
            .split(',')
            .map(|s| s.trim().to_string().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        match &mut setup {
            ReposSetupArgs::Default { whitelist, .. } => whitelist.extend(repos),
            ReposSetupArgs::Empty { whitelist } => whitelist.extend(repos),
        }
    }

    setup
}

/// The repository arguments of a `rig pkg` command, `--with-repos`
/// (`--index`) and `--without-repos` (`--no-index`). They change the
/// repositories for this command only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PkgReposArgs {
    /// The repository names, lowercase, to apply on top of the configured
    /// repositories.
    pub setup: ReposSetupArgs,
    /// Extra repositories given by URL, `(name, url)`, in the order given.
    pub urls: Vec<(String, String)>,
}

impl PkgReposArgs {
    /// Whether all repositories that are not named or given by URL are off.
    pub fn is_empty_base(&self) -> bool {
        matches!(self.setup, ReposSetupArgs::Empty { .. })
    }

    /// The repository names to use, lowercase.
    pub fn enabled_names(&self) -> &[String] {
        match &self.setup {
            ReposSetupArgs::Default { whitelist, .. } => whitelist,
            ReposSetupArgs::Empty { whitelist } => whitelist,
        }
    }
}

/// Interpret `--with-repos` and `--without-repos` of a `rig pkg` command.
/// `None` if neither is given, or the command does not have them.
///
/// Both can be repeated, and take comma-separated lists. `--without-repos`
/// without a value turns off all configured repositories. An item of
/// `--with-repos` is a repository name, a URL (it has `://`), named after
/// its host, or `name=URL`. A name cannot be both used and not used.
pub fn interpret_pkg_repos_args(
    args: &ArgMatches,
) -> Result<Option<PkgReposArgs>, Box<dyn std::error::Error>> {
    let with: Vec<String> = args
        .try_get_many::<String>("with-repos")
        .ok()
        .flatten()
        .map(|v| v.cloned().collect())
        .unwrap_or_default();
    let without: Vec<String> = args
        .try_get_many::<String>("without-repos")
        .ok()
        .flatten()
        .map(|v| v.cloned().collect())
        .unwrap_or_default();
    if with.is_empty() && without.is_empty() {
        return Ok(None);
    }

    let empty = without.iter().any(|v| v == "ALL REPOSITORIES");
    let blacklist: Vec<String> = without
        .iter()
        .filter(|v| *v != "ALL REPOSITORIES")
        .flat_map(|v| split_repos_list(v))
        .map(|s| s.to_lowercase())
        .collect();

    let mut whitelist: Vec<String> = vec![];
    let mut urls: Vec<(String, String)> = vec![];
    for item in with.iter().flat_map(|v| split_repos_list(v)) {
        match parse_repo_url(&item)? {
            Some(nu) => urls.push(nu),
            None => whitelist.push(item.to_lowercase()),
        }
    }

    let mut both: Vec<&String> = whitelist.iter().filter(|x| blacklist.contains(x)).collect();
    if !both.is_empty() {
        both.sort();
        both.dedup();
        let both: Vec<&str> = both.iter().map(|x| x.as_str()).collect();
        bail!(
            "Repositories are both in --with-repos and --without-repos: {}",
            both.join(", ")
        );
    }

    let setup = if empty {
        ReposSetupArgs::Empty { whitelist }
    } else {
        ReposSetupArgs::Default {
            whitelist,
            blacklist,
        }
    };
    Ok(Some(PkgReposArgs { setup, urls }))
}

// The items of a comma-separated list, trimmed, without empty ones.
pub(crate) fn split_repos_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

// `Some((name, url))` if `item` is a URL or `name=URL`, `None` if it is a
// repository name. A URL is named after its host.
pub(crate) fn parse_repo_url(
    item: &str,
) -> Result<Option<(String, String)>, Box<dyn std::error::Error>> {
    let Some(pos) = item.find("://") else {
        return Ok(None);
    };
    let (name, url) = match item[..pos].find('=') {
        Some(eq) => (item[..eq].trim().to_string(), item[eq + 1..].trim()),
        None => (String::new(), item),
    };
    let url = url.trim_end_matches('/').to_string();
    let rest = &url[url.find("://").unwrap_or(0) + 3..];
    let host = rest.split('/').next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    if host.is_empty() && !url.starts_with("file://") {
        bail!("Invalid repository URL: {}", url);
    }
    let name = if !name.is_empty() {
        name
    } else if !host.is_empty() {
        host.to_string()
    } else {
        "local".to_string()
    };
    Ok(Some((name, url)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{Arg, Command};

    fn create_test_command() -> Command {
        Command::new("test")
            .arg(
                Arg::new("with-repos")
                    .long("with-repos")
                    .num_args(1)
                    .require_equals(true)
                    .required(false),
            )
            .arg(
                Arg::new("without-repos")
                    .long("without-repos")
                    .num_args(0..=1)
                    .require_equals(true)
                    .default_missing_value("ALL REPOSITORIES")
                    .required(false),
            )
            .arg(
                Arg::new("without-cran-mirror")
                    .long("without-cran-mirror")
                    .num_args(0)
                    .required(false)
                    .action(clap::ArgAction::SetTrue),
            )
            .arg(
                Arg::new("without-p3m")
                    .long("without-p3m")
                    .num_args(0)
                    .required(false)
                    .action(clap::ArgAction::SetTrue),
            )
    }

    #[test]
    fn test_no_args() {
        let cmd = create_test_command();
        let matches = cmd.try_get_matches_from(vec!["test"]).unwrap();
        let result = interpret_repos_args(&matches, true);

        assert_eq!(
            result,
            ReposSetupArgs::Default {
                whitelist: vec![],
                blacklist: vec![],
            }
        );
    }

    #[test]
    fn test_without_repos_no_value() {
        let cmd = create_test_command();
        let matches = cmd
            .try_get_matches_from(vec!["test", "--without-repos"])
            .unwrap();
        let result = interpret_repos_args(&matches, true);

        assert_eq!(result, ReposSetupArgs::Empty { whitelist: vec![] });
    }

    #[test]
    fn test_without_repos_with_value() {
        let cmd = create_test_command();
        let matches = cmd
            .try_get_matches_from(vec!["test", "--without-repos=cran,p3m"])
            .unwrap();
        let result = interpret_repos_args(&matches, true);

        assert_eq!(
            result,
            ReposSetupArgs::Default {
                whitelist: vec![],
                blacklist: vec!["cran".to_string(), "p3m".to_string()],
            }
        );
    }

    #[test]
    fn test_with_repos() {
        let cmd = create_test_command();
        let matches = cmd
            .try_get_matches_from(vec!["test", "--with-repos=bioc,custom"])
            .unwrap();
        let result = interpret_repos_args(&matches, true);

        assert_eq!(
            result,
            ReposSetupArgs::Default {
                whitelist: vec!["bioc".to_string(), "custom".to_string()],
                blacklist: vec![],
            }
        );
    }

    #[test]
    fn test_with_repos_and_without_repos_value() {
        let cmd = create_test_command();
        let matches = cmd
            .try_get_matches_from(vec!["test", "--with-repos=bioc", "--without-repos=p3m"])
            .unwrap();
        let result = interpret_repos_args(&matches, true);

        assert_eq!(
            result,
            ReposSetupArgs::Default {
                whitelist: vec!["bioc".to_string()],
                blacklist: vec!["p3m".to_string()],
            }
        );
    }

    #[test]
    fn test_with_repos_and_without_repos_no_value() {
        let cmd = create_test_command();
        let matches = cmd
            .try_get_matches_from(vec!["test", "--with-repos=cran,bioc", "--without-repos"])
            .unwrap();
        let result = interpret_repos_args(&matches, true);

        // When --without-repos has no value, it creates Empty variant
        assert_eq!(
            result,
            ReposSetupArgs::Empty {
                whitelist: vec!["cran".to_string(), "bioc".to_string()],
            }
        );
    }

    #[test]
    fn test_deprecated_without_cran_mirror() {
        let cmd = create_test_command();
        let matches = cmd
            .try_get_matches_from(vec!["test", "--without-cran-mirror"])
            .unwrap();
        let result = interpret_repos_args(&matches, true);

        assert_eq!(
            result,
            ReposSetupArgs::Default {
                whitelist: vec![],
                blacklist: vec!["cran".to_string()],
            }
        );
    }

    #[test]
    fn test_deprecated_without_p3m() {
        let cmd = create_test_command();
        let matches = cmd
            .try_get_matches_from(vec!["test", "--without-p3m"])
            .unwrap();
        let result = interpret_repos_args(&matches, true);

        assert_eq!(
            result,
            ReposSetupArgs::Default {
                whitelist: vec![],
                blacklist: vec!["p3m".to_string()],
            }
        );
    }

    #[test]
    fn test_both_deprecated_flags() {
        let cmd = create_test_command();
        let matches = cmd
            .try_get_matches_from(vec!["test", "--without-cran-mirror", "--without-p3m"])
            .unwrap();
        let result = interpret_repos_args(&matches, true);

        assert_eq!(
            result,
            ReposSetupArgs::Default {
                whitelist: vec![],
                blacklist: vec!["cran".to_string(), "p3m".to_string()],
            }
        );
    }

    #[test]
    fn test_whitespace_trimming() {
        let cmd = create_test_command();
        let matches = cmd
            .try_get_matches_from(vec!["test", "--with-repos= cran , p3m "])
            .unwrap();
        let result = interpret_repos_args(&matches, true);

        // p3m is in whitelist, so it should NOT be in blacklist on macOS
        assert_eq!(
            result,
            ReposSetupArgs::Default {
                whitelist: vec!["cran".to_string(), "p3m".to_string()],
                blacklist: vec![],
            }
        );
    }

    #[test]
    fn test_lowercase_conversion() {
        let cmd = create_test_command();
        let matches = cmd
            .try_get_matches_from(vec!["test", "--with-repos=CRAN,BiOc"])
            .unwrap();
        let result = interpret_repos_args(&matches, true);

        assert_eq!(
            result,
            ReposSetupArgs::Default {
                whitelist: vec!["cran".to_string(), "bioc".to_string()],
                blacklist: vec![],
            }
        );
    }

    #[test]
    fn test_empty_values_filtered() {
        let cmd = create_test_command();
        let matches = cmd
            .try_get_matches_from(vec!["test", "--with-repos=cran,,p3m"])
            .unwrap();
        let result = interpret_repos_args(&matches, true);

        // p3m is in whitelist, so it should NOT be in blacklist on macOS
        assert_eq!(
            result,
            ReposSetupArgs::Default {
                whitelist: vec!["cran".to_string(), "p3m".to_string()],
                blacklist: vec![],
            }
        );
    }

    #[test]
    fn test_complex_combination() {
        let cmd = create_test_command();
        let matches = cmd
            .try_get_matches_from(vec![
                "test",
                "--with-repos=bioc,custom",
                "--without-repos=cran,p3m",
            ])
            .unwrap();
        let result = interpret_repos_args(&matches, true);

        assert_eq!(
            result,
            ReposSetupArgs::Default {
                whitelist: vec!["bioc".to_string(), "custom".to_string()],
                blacklist: vec!["cran".to_string(), "p3m".to_string()],
            }
        );
    }

    #[test]
    fn test_macos_p3m_in_whitelist() {
        // On macOS, explicitly adding p3m to whitelist should prevent it from being blacklisted
        let cmd = create_test_command();
        let matches = cmd
            .try_get_matches_from(vec!["test", "--with-repos=p3m"])
            .unwrap();
        let result = interpret_repos_args(&matches, true);

        // p3m is in whitelist, so it should NOT be in blacklist on macOS
        assert_eq!(
            result,
            ReposSetupArgs::Default {
                whitelist: vec!["p3m".to_string()],
                blacklist: vec![],
            }
        );
    }

    #[test]
    fn test_deprecated_flag_adds_duplicate() {
        // This test documents current behavior: deprecated flags can add duplicates
        // In practice, clap conflicts_with_all prevents this combination
        let cmd = create_test_command();
        let matches = cmd
            .try_get_matches_from(vec![
                "test",
                "--with-repos=bioc",
                "--without-repos=cran,p3m",
                "--without-cran-mirror",
            ])
            .unwrap();
        let result = interpret_repos_args(&matches, true);

        assert_eq!(
            result,
            ReposSetupArgs::Default {
                whitelist: vec!["bioc".to_string()],
                blacklist: vec!["cran".to_string(), "cran".to_string(), "p3m".to_string()],
            }
        );
    }

    fn create_pkg_command() -> Command {
        Command::new("test")
            .arg(
                Arg::new("with-repos")
                    .long("with-repos")
                    .visible_alias("index")
                    .num_args(1)
                    .action(clap::ArgAction::Append),
            )
            .arg(
                Arg::new("without-repos")
                    .long("without-repos")
                    .visible_alias("no-index")
                    .num_args(0..=1)
                    .require_equals(true)
                    .default_missing_value("ALL REPOSITORIES")
                    .action(clap::ArgAction::Append),
            )
            .arg(Arg::new("package").num_args(0..))
    }

    fn pkg_args(argv: &[&str]) -> Option<PkgReposArgs> {
        let mut v = vec!["test"];
        v.extend_from_slice(argv);
        let matches = create_pkg_command().try_get_matches_from(v).unwrap();
        interpret_pkg_repos_args(&matches).unwrap()
    }

    fn s(x: &[&str]) -> Vec<String> {
        x.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn test_pkg_no_args() {
        assert_eq!(pkg_args(&["cli"]), None);
        let matches = Command::new("test")
            .try_get_matches_from(vec!["test"])
            .unwrap();
        assert_eq!(interpret_pkg_repos_args(&matches).unwrap(), None);
    }

    #[test]
    fn test_pkg_repeated_and_aliases() {
        assert_eq!(
            pkg_args(&[
                "--with-repos",
                "Bioc",
                "--index=p3m,cranextra",
                "--without-repos=CRAN",
                "--no-index=r-forge"
            ]),
            Some(PkgReposArgs {
                setup: ReposSetupArgs::Default {
                    whitelist: s(&["bioc", "p3m", "cranextra"]),
                    blacklist: s(&["cran", "r-forge"]),
                },
                urls: vec![],
            })
        );
    }

    #[test]
    fn test_pkg_no_index_does_not_take_package() {
        let res = pkg_args(&["--no-index", "cli", "--index=bioc"]).unwrap();
        assert_eq!(
            res.setup,
            ReposSetupArgs::Empty {
                whitelist: s(&["bioc"])
            }
        );
        assert!(res.is_empty_base());
        assert_eq!(res.enabled_names(), &s(&["bioc"])[..]);
    }

    #[test]
    fn test_pkg_urls() {
        let res = pkg_args(&[
            "--index",
            "https://R-Lib.r-universe.dev/",
            "--index=cran,mine=https://user:pw@example.org:8080/cran,file:///tmp/repo",
        ])
        .unwrap();
        assert_eq!(
            res.urls,
            vec![
                (
                    "R-Lib.r-universe.dev".to_string(),
                    "https://R-Lib.r-universe.dev".to_string()
                ),
                (
                    "mine".to_string(),
                    "https://user:pw@example.org:8080/cran".to_string()
                ),
                ("local".to_string(), "file:///tmp/repo".to_string()),
            ]
        );
        assert_eq!(res.enabled_names(), &s(&["cran"])[..]);
    }

    #[test]
    fn test_pkg_both_is_error() {
        let matches = create_pkg_command()
            .try_get_matches_from(vec!["test", "--index=cran,p3m", "--no-index=P3M"])
            .unwrap();
        let err = interpret_pkg_repos_args(&matches).unwrap_err();
        assert!(err.to_string().contains("p3m"));
    }

    #[test]
    fn test_pkg_invalid_url() {
        let matches = create_pkg_command()
            .try_get_matches_from(vec!["test", "--index=https:///foo"])
            .unwrap();
        assert!(interpret_pkg_repos_args(&matches).is_err());
    }
}
