//! Full DESCRIPTION files of Bioconductor packages, from the `VIEWS` files of
//! the Bioconductor repositories.
//!
//! Every Bioconductor repository (software, annotation, experiment data,
//! workflows, books) has a `VIEWS` file next to its `src/contrib` directory:
//!
//! ```text
//! https://bioconductor.org/packages/<biocver>/<repo>/VIEWS
//! ```
//!
//! It is a DCF file with one record per current package, holding every
//! DESCRIPTION field plus some Bioconductor specific ones, e.g. `biocViews`,
//! `git_url` and `dependsOnMe`. It only covers the current version of each
//! package.

use std::error::Error;
use std::path::PathBuf;
use std::time::Duration;

use log::debug;
use serde_json::{Map, Value};

use crate::cache::get_cache_dir;
use crate::dcf::{parse_dcf, Package};
use crate::download::download_if_newer_;
use crate::repos::bioc_mirror;
use crate::repos::feed::RepoId;
use crate::utils::*;

/// How long a downloaded `VIEWS` file is used without asking the server, the
/// same as for `PACKAGES` files.
const VIEWS_TTL: Duration = Duration::from_hours(24);

/// The paths of the Bioconductor repositories under
/// `<mirror>/packages/<biocver>`, in the order of R's `repositories` file.
const BIOC_REPO_PATHS: [&str; 5] = [
    "bioc",
    "data/annotation",
    "data/experiment",
    "workflows",
    "books",
];

/// The `VIEWS` URLs that may have a package version from `repo`, in the order
/// to try them. Empty if `repo` is not a Bioconductor repository.
///
/// The `bioc/<version>` metadata feed covers all Bioconductor repositories
/// and does not record which one a package is from, so it gets the `VIEWS`
/// files of all of them. The other Bioconductor repositories are CRAN-like
/// repositories with their resolved URL, each with its own `VIEWS` file.
pub(crate) fn views_urls(repo: &RepoId) -> Vec<String> {
    match repo {
        RepoId::Bioc(ver) => BIOC_REPO_PATHS
            .iter()
            .map(|path| format!("{}/packages/{}/{}/VIEWS", bioc_mirror(), ver, path))
            .collect(),
        RepoId::Cranlike { name, url } if super::is_bioc_entry(name, url) => {
            vec![format!("{}/VIEWS", url.trim_end_matches('/'))]
        }
        _ => vec![],
    }
}

/// The cache file of the `VIEWS` file at `url`.
fn views_cache_file(url: &str) -> Result<PathBuf, Box<dyn Error>> {
    let name: String = url
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut local = get_cache_dir()?;
    local.push("metadata");
    local.push("views");
    local.push(format!("VIEWS-{}", name));
    Ok(local)
}

/// Download the `VIEWS` file at `url` to `local`, unless the cached copy is
/// recent, and return its contents.
fn fetch_views(url: &str, local: &PathBuf) -> Result<String, Box<dyn Error>> {
    debug!("Fetching Bioconductor VIEWS from {}", url);
    create_parent_dir_if_needed(local)?;
    let (downloaded, _etag) = download_if_newer_(url, local, Some(VIEWS_TTL), None)?;
    let contents = read_file_string(local)?;
    match parse_dcf(&contents) {
        Ok(_) => Ok(contents),
        // Only the cached copy is worth a second try; a fresh download that
        // does not parse is the server's answer and will not change.
        Err(err) if !downloaded => {
            debug!(
                "Cached VIEWS {} is corrupt ({}), downloading it again",
                local.display(),
                err
            );
            std::fs::remove_file(local)?;
            let _ = download_if_newer_(url, local, Some(VIEWS_TTL), None)?;
            read_file_string(local)
        }
        Err(err) => Err(err),
    }
}

/// The record of `package` `version` in the `VIEWS` file `views`, as a JSON
/// object of field name to value, or `None` if it is not there. Field values
/// keep their DCF line wrapping; the printer reflows the ones it shows.
fn views_description(
    views: &str,
    package: &str,
    version: &str,
) -> Result<Option<Value>, Box<dyn Error>> {
    let views = parse_dcf(views)?;
    let para = views
        .iter()
        .find(|p| p.get("Package") == Some(package) && p.get("Version") == Some(version));
    Ok(para.map(|para| {
        let mut map = Map::new();
        for (key, value) in para.iter() {
            map.insert(key.to_string(), Value::String(value.to_string()));
        }
        Value::Object(map)
    }))
}

/// The full DESCRIPTION of `pkg` from the `VIEWS` file of its Bioconductor
/// repository. `None` if `pkg` is not from Bioconductor, or if no `VIEWS`
/// file can be downloaded that has this version.
pub(crate) fn bioc_description(pkg: &Package) -> Option<Value> {
    for url in views_urls(pkg.repository.as_ref()?) {
        let result = views_cache_file(&url)
            .and_then(|local| fetch_views(&url, &local))
            .and_then(|views| views_description(&views, &pkg.name, &pkg.version.original));
        match result {
            Ok(Some(desc)) => return Some(desc),
            Ok(None) => debug!("{} {} is not in {}", pkg.name, pkg.version.original, url),
            Err(err) => debug!("Failed to use Bioconductor VIEWS from {}: {}", url, err),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEWS: &str = "\
Package: limma
Version: 3.64.0
Title: Linear Models for Microarray and Omics Data
Description: Data analysis, linear models and differential
        expression for omics data.
Maintainer: Gordon Smyth <smyth@wehi.edu.au>
biocViews: ExonArray, GeneExpression

Package: org.Hs.eg.db
Version: 3.21.0
Title: Genome wide annotation for Human
Maintainer: Bioconductor Package Maintainer <maintainer@bioconductor.org>
";

    fn cranlike(name: &str, url: &str) -> RepoId {
        RepoId::Cranlike {
            name: name.to_string(),
            url: url.to_string(),
        }
    }

    #[test]
    fn views_urls_of_bioc_repos() {
        // Only meaningful with the default mirror.
        if std::env::var_os("R_BIOC_MIRROR").is_some() {
            return;
        }
        assert_eq!(
            views_urls(&RepoId::Bioc("3.22".to_string())),
            vec![
                "https://bioconductor.org/packages/3.22/bioc/VIEWS",
                "https://bioconductor.org/packages/3.22/data/annotation/VIEWS",
                "https://bioconductor.org/packages/3.22/data/experiment/VIEWS",
                "https://bioconductor.org/packages/3.22/workflows/VIEWS",
                "https://bioconductor.org/packages/3.22/books/VIEWS",
            ]
        );
        for (name, path) in [
            ("BioCsoft", "bioc"),
            ("BioCann", "data/annotation"),
            ("BioCexp", "data/experiment"),
            ("BioCworkflows", "workflows"),
            ("BioCbooks", "books"),
        ] {
            let url = format!("https://bioconductor.org/packages/3.22/{}", path);
            assert_eq!(
                views_urls(&cranlike(name, &url)),
                vec![format!("{}/VIEWS", url)]
            );
        }
        // By URL, whatever the name.
        assert_eq!(
            views_urls(&cranlike(
                "mybioc",
                "https://bioconductor.org/packages/3.22/data/annotation/"
            )),
            vec!["https://bioconductor.org/packages/3.22/data/annotation/VIEWS"]
        );
    }

    #[test]
    fn views_urls_of_other_repos() {
        assert!(views_urls(&RepoId::Cran).is_empty());
        assert!(views_urls(&cranlike("CRAN", "https://cloud.r-project.org")).is_empty());
        assert!(views_urls(&cranlike("r-universe/acme", "https://acme.r-universe.dev")).is_empty());
    }

    #[test]
    fn views_description_picks_the_record() {
        let desc = views_description(VIEWS, "limma", "3.64.0")
            .unwrap()
            .unwrap();
        assert_eq!(desc["Package"], "limma");
        assert_eq!(desc["Title"], "Linear Models for Microarray and Omics Data");
        assert!(desc["Description"]
            .as_str()
            .unwrap()
            .contains("expression for omics data."));
        assert_eq!(desc["biocViews"], "ExonArray, GeneExpression");

        let desc = views_description(VIEWS, "org.Hs.eg.db", "3.21.0")
            .unwrap()
            .unwrap();
        assert_eq!(desc["Title"], "Genome wide annotation for Human");
    }

    #[test]
    fn views_description_missing() {
        assert_eq!(views_description(VIEWS, "limma", "3.62.0").unwrap(), None);
        assert_eq!(views_description(VIEWS, "edgeR", "4.0.0").unwrap(), None);
    }

    #[test]
    fn fetch_views_uses_the_cache() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let rt = tokio::runtime::Runtime::new().unwrap();
        let server = rt.block_on(MockServer::start());
        rt.block_on(
            Mock::given(method("GET"))
                .and(path("/packages/3.22/data/annotation/VIEWS"))
                .respond_with(ResponseTemplate::new(200).set_body_string(VIEWS))
                .expect(1)
                .mount(&server),
        );

        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join("views").join("VIEWS-test");
        let url = format!("{}/packages/3.22/data/annotation/VIEWS", server.uri());

        assert_eq!(fetch_views(&url, &local).unwrap(), VIEWS);
        // The second call is served from the cache, `expect(1)` checks it.
        assert_eq!(fetch_views(&url, &local).unwrap(), VIEWS);
        rt.block_on(server.verify());
    }
}
