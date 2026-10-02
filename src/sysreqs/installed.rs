//! Which OS packages are installed already.

use std::collections::HashSet;
use std::error::Error;

use super::platform::{Family, SysreqsSystem};

/// The packages of `packages` that are not installed.
pub fn missing(system: &SysreqsSystem, packages: &[String]) -> Result<Vec<String>, Box<dyn Error>> {
    match system.family {
        Family::Apt => {
            let out = duct::cmd!(
                "dpkg-query",
                "-W",
                "-f",
                "${db:Status-Abbrev}|${Package}|${Provides}\n"
            )
            .stderr_null()
            .read()?;
            let installed = parse_dpkg_query(&out);
            Ok(packages
                .iter()
                .filter(|p| !installed.contains(p.as_str()))
                .cloned()
                .collect())
        }
        Family::Dnf | Family::Yum | Family::Zypper => {
            let out = duct::cmd!("rpm", "-qa", "--queryformat", "%{NAME}\n")
                .stderr_null()
                .read()?;
            let installed: HashSet<&str> = out.lines().map(|l| l.trim()).collect();
            // A rule may name a capability another package provides, which
            // `rpm -qa` does not list.
            Ok(packages
                .iter()
                .filter(|p| !installed.contains(p.as_str()))
                .filter(|p| {
                    !duct::cmd!("rpm", "-q", "--whatprovides", p.as_str())
                        .stdout_null()
                        .stderr_null()
                        .unchecked()
                        .run()
                        .map(|o| o.status.success())
                        .unwrap_or(false)
                })
                .cloned()
                .collect())
        }
        Family::Apk => {
            let out = duct::cmd!("apk", "info").stderr_null().read()?;
            let installed: HashSet<&str> = out.lines().map(|l| l.trim()).collect();
            Ok(packages
                .iter()
                .filter(|p| !installed.contains(p.as_str()))
                .cloned()
                .collect())
        }
    }
}

/// The installed packages in `dpkg-query -W -f
/// '${db:Status-Abbrev}|${Package}|${Provides}\n'` output, including the
/// virtual packages they provide.
pub fn parse_dpkg_query(out: &str) -> HashSet<&str> {
    let mut installed = HashSet::new();
    for line in out.lines() {
        let mut fields = line.splitn(3, '|');
        let (Some(status), Some(name)) = (fields.next(), fields.next()) else {
            continue;
        };
        // `ii ` is installed; `rc ` is removed with its config files left.
        if !status.starts_with("ii") {
            continue;
        }
        installed.insert(name.trim());
        if let Some(provides) = fields.next() {
            for p in provides.split(',') {
                // `libfoo-dev (= 1.0)`
                if let Some(p) = p.split_whitespace().next() {
                    installed.insert(p);
                }
            }
        }
    }
    installed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dpkg_query_output() {
        let out = "ii |libcurl4-openssl-dev|libcurl-dev (= 8.5.0), libcurl-ssl-dev\n\
                   rc |libxml2-dev|\n\
                   ii |make|\n\
                   un |pandoc|\n";
        let installed = parse_dpkg_query(out);
        assert!(installed.contains("libcurl4-openssl-dev"));
        assert!(installed.contains("libcurl-dev"));
        assert!(installed.contains("libcurl-ssl-dev"));
        assert!(installed.contains("make"));
        assert!(!installed.contains("libxml2-dev"));
        assert!(!installed.contains("pandoc"));
    }
}
