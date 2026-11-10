//! The policy of a repository about a package version: whether it blocks
//! the version, and the known vulnerabilities of the version.
//!
//! A repository with extended metadata may add these fields to the records
//! of its `ALLPACKAGES` file (see tidyup 12):
//!
//! * `Blocked`: the kind of rule that blocks the version, e.g. `package`,
//!   `license`, `vulnerability` or `deleted`. Other values are accepted, too.
//! * `BlockedRule`: an identifier of the rule.
//! * `BlockedReason`: a description of the rule, for people.
//! * `BlockedUntil`: the version is blocked before this time, RFC 3339.
//! * `Vulnerabilities`: a comma separated list of vulnerability ids, each
//!   followed by its highest CVSS base score in parentheses, if it has one,
//!   e.g. `CVE-2024-27322 (8.8), GHSA-xxxx-xxxx-xxxx`.
//!
//! A repository blocks a version by refusing to serve it, so rig never picks
//! a blocked version. A vulnerable version is not blocked, but rig warns
//! about it, and `rig audit` reports it.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The policy fields of one package version, `None` in
/// [`crate::dcf::Package::policy`] if it has none.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VersionPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_rule: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_until: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vulnerabilities: Vec<Vulnerability>,
}

/// A known vulnerability of a package version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Vulnerability {
    /// E.g. `CVE-2024-27322` or `RSEC-2023-3`.
    pub id: String,
    /// The highest CVSS base score of the vulnerability, if it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
}

impl fmt::Display for Vulnerability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.score {
            Some(score) => write!(f, "{} ({})", self.id, score),
            None => write!(f, "{}", self.id),
        }
    }
}

impl VersionPolicy {
    /// The policy fields of a DCF record, from its fields by `get`. `None`
    /// if it has none of them.
    pub fn from_fields<'a>(get: impl Fn(&str) -> Option<&'a str>) -> Option<VersionPolicy> {
        let field = |name: &str| {
            get(name)
                .map(|v| v.trim())
                .filter(|v| !v.is_empty())
                .map(|v| v.to_string())
        };
        let policy = VersionPolicy {
            blocked: field("Blocked"),
            blocked_rule: field("BlockedRule"),
            blocked_reason: field("BlockedReason"),
            blocked_until: field("BlockedUntil"),
            vulnerabilities: get("Vulnerabilities")
                .map(parse_vulnerabilities)
                .unwrap_or_default(),
        };
        if policy == VersionPolicy::default() {
            None
        } else {
            Some(policy)
        }
    }

    /// Whether the repository blocks the version now.
    pub fn is_blocked(&self) -> bool {
        self.is_blocked_at(jiff::Timestamp::now())
    }

    /// Whether the repository blocks the version at time `now`. A version
    /// with a `BlockedUntil` that is not a valid time is blocked.
    pub fn is_blocked_at(&self, now: jiff::Timestamp) -> bool {
        if self.blocked.is_some() {
            return true;
        }
        match &self.blocked_until {
            Some(until) => match until.parse::<jiff::Timestamp>() {
                Ok(until) => now < until,
                Err(_) => true,
            },
            None => false,
        }
    }

    pub fn is_vulnerable(&self) -> bool {
        !self.vulnerabilities.is_empty()
    }

    /// Why the version is blocked, e.g. `package rule 9: Block otel 0.3.0`,
    /// or `too new, available from 2026-04-17T00:00:00Z`.
    pub fn block_reason(&self) -> String {
        let mut parts: Vec<String> = vec![];
        if let Some(kind) = &self.blocked {
            let mut rule = format!("{} rule", kind);
            if let Some(id) = &self.blocked_rule {
                rule.push_str(&format!(" {}", id));
            }
            if let Some(reason) = &self.blocked_reason {
                rule.push_str(&format!(": {}", reason));
            }
            parts.push(rule);
        }
        if self.blocked.is_none() {
            if let Some(until) = &self.blocked_until {
                parts.push(format!("too new, available from {}", until));
            }
        }
        parts.join(", ")
    }
}

/// The items of a `Vulnerabilities` field, e.g.
/// `CVE-2024-27322 (8.8), GHSA-xxxx-xxxx-xxxx`. A score that is not a
/// number is dropped.
pub fn parse_vulnerabilities(field: &str) -> Vec<Vulnerability> {
    field
        .split(',')
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
        .map(|item| match item.split_once('(') {
            Some((id, rest)) => Vulnerability {
                id: id.trim().to_string(),
                score: rest.trim_end_matches(')').trim().parse::<f64>().ok(),
            },
            None => Vulnerability {
                id: item.to_string(),
                score: None,
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn policy(fields: &[(&str, &str)]) -> Option<VersionPolicy> {
        let map: HashMap<&str, &str> = fields.iter().cloned().collect();
        VersionPolicy::from_fields(|name| map.get(name).copied())
    }

    #[test]
    fn no_policy_fields_is_none() {
        assert_eq!(policy(&[("Package", "cli")]), None);
        assert_eq!(policy(&[("Vulnerabilities", " ")]), None);
    }

    #[test]
    fn blocked_fields_are_parsed() {
        let p = policy(&[
            ("Blocked", "package"),
            ("BlockedRule", "9"),
            ("BlockedReason", "Block otel 0.3.0"),
        ])
        .unwrap();
        assert!(p.is_blocked());
        assert!(!p.is_vulnerable());
        assert_eq!(p.block_reason(), "package rule 9: Block otel 0.3.0");

        // Unknown kinds are blocked, too.
        let p = policy(&[("Blocked", "embargo")]).unwrap();
        assert!(p.is_blocked());
        assert_eq!(p.block_reason(), "embargo rule");
    }

    #[test]
    fn blocked_until_is_compared_to_now() {
        let p = policy(&[("BlockedUntil", "2026-04-17T00:00:00Z")]).unwrap();
        let before: jiff::Timestamp = "2026-04-16T23:59:59Z".parse().unwrap();
        let after: jiff::Timestamp = "2026-04-17T00:00:00Z".parse().unwrap();
        assert!(p.is_blocked_at(before));
        assert!(!p.is_blocked_at(after));
        assert_eq!(
            p.block_reason(),
            "too new, available from 2026-04-17T00:00:00Z"
        );

        // Blocked stays blocked after `BlockedUntil`.
        let p = policy(&[
            ("Blocked", "license"),
            ("BlockedUntil", "2026-04-17T00:00:00Z"),
        ])
        .unwrap();
        assert!(p.is_blocked_at(after));

        // An invalid time is blocked.
        let p = policy(&[("BlockedUntil", "soon")]).unwrap();
        assert!(p.is_blocked_at(after));
    }

    #[test]
    fn vulnerabilities_are_parsed() {
        let v = parse_vulnerabilities("CVE-2024-27322 (8.8), GHSA-xxxx-xxxx-xxxx, RSEC-1 (n/a)");
        assert_eq!(
            v,
            vec![
                Vulnerability {
                    id: "CVE-2024-27322".to_string(),
                    score: Some(8.8)
                },
                Vulnerability {
                    id: "GHSA-xxxx-xxxx-xxxx".to_string(),
                    score: None
                },
                Vulnerability {
                    id: "RSEC-1".to_string(),
                    score: None
                },
            ]
        );
        let p = policy(&[("Vulnerabilities", "RSEC-2023-6, RSEC-2023-7 (7.5)")]).unwrap();
        assert!(p.is_vulnerable());
        assert!(!p.is_blocked());
        assert_eq!(p.vulnerabilities[1].to_string(), "RSEC-2023-7 (7.5)");
    }
}
