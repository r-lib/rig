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

    // On macOS, P3M is not enabled by default, but it can be enabled with --with-repos=p3m
    #[cfg(target_os = "macos")]
    if let ReposSetupArgs::Default {
        whitelist,
        blacklist,
    } = &mut setup
    {
        if !whitelist.contains(&"p3m".to_string()) {
            blacklist.push("p3m".to_string());
        }
    }

    setup
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

        #[cfg(target_os = "macos")]
        assert_eq!(
            result,
            ReposSetupArgs::Default {
                whitelist: vec![],
                blacklist: vec!["p3m".to_string()],
            }
        );

        #[cfg(not(target_os = "macos"))]
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

        #[cfg(target_os = "macos")]
        assert_eq!(
            result,
            ReposSetupArgs::Default {
                whitelist: vec![],
                blacklist: vec!["cran".to_string(), "p3m".to_string(), "p3m".to_string()],
            }
        );

        #[cfg(not(target_os = "macos"))]
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

        #[cfg(target_os = "macos")]
        assert_eq!(
            result,
            ReposSetupArgs::Default {
                whitelist: vec!["bioc".to_string(), "custom".to_string()],
                blacklist: vec!["p3m".to_string()],
            }
        );

        #[cfg(not(target_os = "macos"))]
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

        #[cfg(target_os = "macos")]
        assert_eq!(
            result,
            ReposSetupArgs::Default {
                whitelist: vec!["bioc".to_string()],
                blacklist: vec!["p3m".to_string(), "p3m".to_string()],
            }
        );

        #[cfg(not(target_os = "macos"))]
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

        #[cfg(target_os = "macos")]
        assert_eq!(
            result,
            ReposSetupArgs::Default {
                whitelist: vec![],
                blacklist: vec!["cran".to_string(), "p3m".to_string()],
            }
        );

        #[cfg(not(target_os = "macos"))]
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

        #[cfg(target_os = "macos")]
        assert_eq!(
            result,
            ReposSetupArgs::Default {
                whitelist: vec![],
                blacklist: vec!["p3m".to_string(), "p3m".to_string()],
            }
        );

        #[cfg(not(target_os = "macos"))]
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

        #[cfg(target_os = "macos")]
        assert_eq!(
            result,
            ReposSetupArgs::Default {
                whitelist: vec![],
                blacklist: vec!["cran".to_string(), "p3m".to_string(), "p3m".to_string()],
            }
        );

        #[cfg(not(target_os = "macos"))]
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

        #[cfg(target_os = "macos")]
        assert_eq!(
            result,
            ReposSetupArgs::Default {
                whitelist: vec!["cran".to_string(), "bioc".to_string()],
                blacklist: vec!["p3m".to_string()],
            }
        );

        #[cfg(not(target_os = "macos"))]
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

        #[cfg(target_os = "macos")]
        assert_eq!(
            result,
            ReposSetupArgs::Default {
                whitelist: vec!["bioc".to_string(), "custom".to_string()],
                blacklist: vec!["cran".to_string(), "p3m".to_string(), "p3m".to_string()],
            }
        );

        #[cfg(not(target_os = "macos"))]
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

        #[cfg(target_os = "macos")]
        assert_eq!(
            result,
            ReposSetupArgs::Default {
                whitelist: vec!["bioc".to_string()],
                blacklist: vec![
                    "cran".to_string(),
                    "cran".to_string(),
                    "p3m".to_string(),
                    "p3m".to_string()
                ],
            }
        );

        #[cfg(not(target_os = "macos"))]
        assert_eq!(
            result,
            ReposSetupArgs::Default {
                whitelist: vec!["bioc".to_string()],
                blacklist: vec!["cran".to_string(), "cran".to_string(), "p3m".to_string()],
            }
        );
    }
}
