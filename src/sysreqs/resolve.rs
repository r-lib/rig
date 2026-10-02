//! Match `SystemRequirements` fields against the rules, for one system.

use log::warn;

use super::platform::SysreqsSystem;
use super::rules::RuleDb;

/// One rule that applies to at least one of the R packages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchedRule {
    pub rule: String,
    /// The OS packages the rule needs on the system.
    pub packages: Vec<String>,
    /// Shell commands to run before and after installing `packages`.
    pub pre_install: Vec<String>,
    pub post_install: Vec<String>,
    /// The R packages that need it.
    pub r_packages: Vec<String>,
}

/// Every rule that applies to `packages`, given as `(R package,
/// SystemRequirements)`, in rule name order. A rule that matches but has no
/// entry for the system is left out: there is nothing rig could install.
pub fn resolve(
    db: &RuleDb,
    system: &SysreqsSystem,
    packages: &[(String, String)],
) -> Vec<MatchedRule> {
    let mut out = vec![];
    for rule in &db.rules {
        let r_packages: Vec<String> = packages
            .iter()
            .filter(|(_, sysreqs)| rule.matches(sysreqs))
            .map(|(name, _)| name.clone())
            .collect();
        if r_packages.is_empty() {
            continue;
        }
        let Some(dep) = rule.dependency_for(&system.distribution, &system.version) else {
            continue;
        };
        let steps = |steps: &[super::rules::RuleStep]| -> Vec<String> {
            steps
                .iter()
                .filter_map(|s| {
                    if s.command.is_none() {
                        warn!(
                            "Not running script {:?} of system requirements rule {}",
                            s.script, rule.name
                        );
                    }
                    s.command.clone()
                })
                .collect()
        };
        out.push(MatchedRule {
            rule: rule.name.clone(),
            packages: dep.packages.clone(),
            pre_install: steps(&dep.pre_install),
            post_install: steps(&dep.post_install),
            r_packages,
        });
    }
    out
}

/// The distinct values of `f` over `rules`, in order of first appearance.
pub fn collect<'a>(
    rules: &[&'a MatchedRule],
    f: impl Fn(&'a MatchedRule) -> &'a [String],
) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for rule in rules {
        for x in f(rule) {
            if !out.contains(x) {
                out.push(x.clone());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_against_the_embedded_rules() {
        let db = RuleDb::embedded().unwrap();
        let packages = vec![
            (
                "curl".to_string(),
                "libcurl: libcurl-devel (rpm) or libcurl4-openssl-dev (deb)".to_string(),
            ),
            (
                "xml2".to_string(),
                "libxml2: libxml2-dev (deb), libxml2-devel (rpm)".to_string(),
            ),
            ("cli".to_string(), "C++11".to_string()),
        ];

        let ubuntu = SysreqsSystem::new("ubuntu", "24.04").unwrap();
        let matched = resolve(&db, &ubuntu, &packages);
        let rules: Vec<&MatchedRule> = matched.iter().collect();
        let os = collect(&rules, |r| &r.packages);
        assert!(os.contains(&"libcurl4-openssl-dev".to_string()));
        assert!(os.contains(&"libxml2-dev".to_string()));
        let curl = matched.iter().find(|r| r.rule == "libcurl").unwrap();
        assert_eq!(curl.r_packages, vec!["curl"]);

        let fedora = SysreqsSystem::new("fedora", "42").unwrap();
        let matched = resolve(&db, &fedora, &packages);
        let rules: Vec<&MatchedRule> = matched.iter().collect();
        let os = collect(&rules, |r| &r.packages);
        assert!(os.contains(&"libcurl-devel".to_string()));
        assert!(os.contains(&"libxml2-devel".to_string()));
    }

    #[test]
    fn rhel_rules_bring_their_pre_install_commands() {
        let db = RuleDb::embedded().unwrap();
        let packages = vec![(
            "sf".to_string(),
            "GDAL (>= 2.0.1), GEOS (>= 3.4.0), PROJ (>= 4.8.0)".to_string(),
        )];
        let rhel = SysreqsSystem::new("rhel", "8").unwrap();
        let matched = resolve(&db, &rhel, &packages);
        let gdal = matched.iter().find(|r| r.rule == "gdal").unwrap();
        assert!(gdal.packages.contains(&"gdal-devel".to_string()));
        assert!(gdal.pre_install.iter().any(|c| c.contains("epel")));
    }
}
