//! Which protocol rig uses for a repository, from its type, see
//! [`crate::repos::detect`].
//!
//! * `extended`: the extended metadata (`ALLPACKAGES.zst`) of CRAN or of a
//!   Bioconductor release. CRAN mirrors, PPM's CRAN and Bioconductor
//!   repositories, and Bioconductor mirrors use it, unless `extend` is false.
//!   A CRAN mirror stays the first place to download source packages from.
//! * `r-universe`: the `PACKAGES` files, plus the r-universe API for the git
//!   commit of each package.
//! * `cran`: the `PACKAGES` files, for everything else.

use super::detect::{DetectedRepo, RepoType};
use super::feed::{CranlikeRepo, MetadataFeed, RepoId};

/// The repositories of a Bioconductor release, below `packages/<version>`,
/// with the names R gives them in its `repositories` file.
pub const BIOC_SUBREPOS: [(&str, &str); 5] = [
    ("BioCsoft", "bioc"),
    ("BioCann", "data/annotation"),
    ("BioCexp", "data/experiment"),
    ("BioCworkflows", "workflows"),
    ("BioCbooks", "books"),
];

/// The repositories of a solve for the repository `name` at `repo.url`, of
/// type `repo.repo_type`. Usually one, but the root of a Bioconductor mirror
/// or a PPM instance may have several, and a Bioconductor repository has none
/// if there is no Bioconductor version for the target.
///
/// `extend = false` turns off the extended metadata, and `bioc_version` is the
/// Bioconductor version of the target, if any. Without the extended metadata,
/// a Bioconductor root has R's repositories, `BioCsoft`, `BioCann`, etc.
pub fn repos_for(
    name: &str,
    repo: &DetectedRepo,
    extend: bool,
    bioc_version: Option<&str>,
) -> Vec<super::feed::PkgRepo> {
    use super::feed::PkgRepo;

    let url = repo.url.trim_end_matches('/');
    let cranlike = || vec![PkgRepo::Cranlike(CranlikeRepo::new(name, url))];

    match repo.repo_type {
        RepoType::Cran if extend => vec![PkgRepo::Extended(MetadataFeed::cran().with_mirror(url))],
        RepoType::PpmCran if extend => vec![PkgRepo::Extended(
            MetadataFeed::cran().with_mirror(&ppm_source_url(repo)),
        )],

        RepoType::PpmBioc | RepoType::Bioc if repo.bioc_subrepo.is_none() => {
            // A URL ending in `packages/<version>` is that version only.
            let (version, base) = match &repo.bioc_version {
                Some(v) => (Some(v.as_str()), url.to_string()),
                None => (bioc_version, String::new()),
            };
            let Some(version) = version else {
                return vec![];
            };
            if extend {
                return vec![PkgRepo::Extended(MetadataFeed::bioc(version))];
            }
            let base = if base.is_empty() {
                format!("{}/packages/{}", url, version)
            } else {
                base
            };
            BIOC_SUBREPOS
                .iter()
                .map(|(subname, sub)| {
                    PkgRepo::Cranlike(CranlikeRepo::new(subname, &format!("{}/{}", base, sub)))
                })
                .collect()
        }

        RepoType::Extended => {
            let feed = match (repo.extended_flavor.as_deref(), &repo.bioc_version) {
                (Some("bioc"), Some(v)) => Some(MetadataFeed::at(RepoId::Bioc(v.clone()), url)),
                (Some("bioc"), None) => {
                    MetadataFeed::from_metadata_url(&format!("{}/%v", url), bioc_version)
                }
                _ => MetadataFeed::from_metadata_url(url, None),
            };
            feed.into_iter().map(PkgRepo::Extended).collect()
        }

        RepoType::RUniverse => vec![PkgRepo::Cranlike(CranlikeRepo::r_universe(name, url))],

        RepoType::Ppm => repo
            .ppm_repos
            .iter()
            .filter(|r| !r.hidden)
            .flat_map(|r| {
                let mut sub = DetectedRepo::new(&r.url, r.repo_type);
                sub.ppm_base = Some(url.to_string());
                sub.ppm_repo = Some(r.name.clone());
                sub.snapshot = Some("latest".to_string());
                repos_for(&format!("{}/{}", name, r.name), &sub, extend, bioc_version)
            })
            .collect(),

        // A CRAN mirror or a PPM CRAN repository with `extend = false`, a
        // single Bioconductor repository, other PPM repositories and PPM
        // snapshots, and any other CRAN-like repository.
        RepoType::Cran
        | RepoType::PpmCran
        | RepoType::PpmBioc
        | RepoType::Bioc
        | RepoType::PpmOther
        | RepoType::PpmSnapshot
        | RepoType::CranLike => cranlike(),
    }
}

/// The URL of the source packages of a PPM repository: without the
/// `__linux__/<distro>` part, which serves Linux binaries instead.
fn ppm_source_url(repo: &DetectedRepo) -> String {
    match (&repo.ppm_base, &repo.ppm_repo) {
        (Some(base), Some(name)) => format!(
            "{}/{}/{}",
            base,
            name,
            repo.snapshot.as_deref().unwrap_or("latest")
        ),
        _ => repo.url.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::detect::PpmRepoInfo;
    use super::super::feed::{CranlikeProtocol, PkgRepo};
    use super::*;

    fn detected(url: &str, rtype: RepoType) -> DetectedRepo {
        DetectedRepo::new(url, rtype)
    }

    fn cranlike(name: &str, url: &str) -> PkgRepo {
        PkgRepo::Cranlike(CranlikeRepo::new(name, url))
    }

    fn feed_mirror(repos: &[PkgRepo]) -> Option<String> {
        match repos {
            [PkgRepo::Extended(f)] => f.mirror.clone(),
            _ => panic!("not one feed: {:?}", repos),
        }
    }

    #[test]
    fn test_cran() {
        let d = detected("https://cran.acme.com/", RepoType::Cran);
        let repos = repos_for("acme", &d, true, None);
        assert_eq!(repos, vec![PkgRepo::Extended(MetadataFeed::cran())]);
        assert_eq!(
            feed_mirror(&repos).as_deref(),
            Some("https://cran.acme.com")
        );

        assert_eq!(
            repos_for("acme", &d, false, None),
            vec![cranlike("acme", "https://cran.acme.com")]
        );
    }

    #[test]
    fn test_ppm_cran() {
        let mut d = detected(
            "https://p3m.dev/cran/__linux__/jammy/latest",
            RepoType::PpmCran,
        );
        d.ppm_base = Some("https://p3m.dev".to_string());
        d.ppm_repo = Some("cran".to_string());
        d.distro = Some("jammy".to_string());
        d.snapshot = Some("latest".to_string());
        let repos = repos_for("p3m", &d, true, None);
        assert_eq!(
            feed_mirror(&repos).as_deref(),
            Some("https://p3m.dev/cran/latest")
        );

        // No snapshot is `latest`.
        d.snapshot = None;
        let repos = repos_for("p3m", &d, true, None);
        assert_eq!(
            feed_mirror(&repos).as_deref(),
            Some("https://p3m.dev/cran/latest")
        );

        // Without the extended metadata the Linux binaries are kept.
        assert_eq!(
            repos_for("p3m", &d, false, None),
            vec![cranlike(
                "p3m",
                "https://p3m.dev/cran/__linux__/jammy/latest"
            )]
        );
    }

    #[test]
    fn test_bioc_root() {
        let d = detected("https://bioconductor.org", RepoType::Bioc);
        assert_eq!(
            repos_for("bioc", &d, true, Some("3.22")),
            vec![PkgRepo::Extended(MetadataFeed::bioc("3.22"))]
        );
        assert_eq!(repos_for("bioc", &d, true, None), vec![]);

        let repos = repos_for("bioc", &d, false, Some("3.22"));
        assert_eq!(repos.len(), 5);
        assert_eq!(
            repos[1],
            cranlike(
                "BioCann",
                "https://bioconductor.org/packages/3.22/data/annotation"
            )
        );

        // A fixed version in the URL wins over the target's.
        let mut d = detected("https://bioconductor.org/packages/3.20", RepoType::Bioc);
        d.bioc_version = Some("3.20".to_string());
        assert_eq!(
            repos_for("bioc", &d, true, Some("3.22")),
            vec![PkgRepo::Extended(MetadataFeed::bioc("3.20"))]
        );
        assert_eq!(
            repos_for("bioc", &d, false, Some("3.22"))[0],
            cranlike("BioCsoft", "https://bioconductor.org/packages/3.20/bioc")
        );
    }

    #[test]
    fn test_bioc_subrepo() {
        let mut d = detected(
            "https://bioconductor.org/packages/3.22/bioc",
            RepoType::Bioc,
        );
        d.bioc_version = Some("3.22".to_string());
        d.bioc_subrepo = Some("bioc".to_string());
        assert_eq!(
            repos_for("BioCsoft", &d, true, Some("3.22")),
            vec![cranlike(
                "BioCsoft",
                "https://bioconductor.org/packages/3.22/bioc"
            )]
        );

        let mut d = detected(
            "https://p3m.dev/bioconductor/latest/packages/3.22/bioc",
            RepoType::PpmBioc,
        );
        d.bioc_subrepo = Some("bioc".to_string());
        assert_eq!(repos_for("b", &d, true, Some("3.22")).len(), 1);
        assert!(matches!(
            repos_for("b", &d, true, Some("3.22"))[0],
            PkgRepo::Cranlike(_)
        ));
    }

    #[test]
    fn test_ppm_bioc() {
        let d = detected("https://p3m.dev/bioconductor/latest", RepoType::PpmBioc);
        assert_eq!(
            repos_for("pb", &d, true, Some("3.22")),
            vec![PkgRepo::Extended(MetadataFeed::bioc("3.22"))]
        );
        assert_eq!(
            repos_for("pb", &d, false, Some("3.22"))[0],
            cranlike(
                "BioCsoft",
                "https://p3m.dev/bioconductor/latest/packages/3.22/bioc"
            )
        );
    }

    #[test]
    fn test_extended() {
        let mut d = detected("https://ppm.r-pkg.org", RepoType::Extended);
        d.extended_flavor = Some("cran".to_string());
        assert_eq!(
            repos_for("x", &d, true, None),
            vec![PkgRepo::Extended(MetadataFeed::cran())]
        );
        // `extend` does not matter, this is the extended metadata itself.
        assert_eq!(repos_for("x", &d, false, None).len(), 1);

        let mut d = detected("https://ppm-bioc.r-pkg.org", RepoType::Extended);
        d.extended_flavor = Some("bioc".to_string());
        assert_eq!(
            repos_for("x", &d, true, Some("3.22")),
            vec![PkgRepo::Extended(MetadataFeed::bioc("3.22"))]
        );
        assert_eq!(repos_for("x", &d, true, None), vec![]);

        let mut d = detected("https://ppm-bioc.r-pkg.org/3.21", RepoType::Extended);
        d.extended_flavor = Some("bioc".to_string());
        d.bioc_version = Some("3.21".to_string());
        assert_eq!(
            repos_for("x", &d, true, Some("3.22")),
            vec![PkgRepo::Extended(MetadataFeed::bioc("3.21"))]
        );
    }

    #[test]
    fn test_r_universe() {
        let d = detected("https://r-lib.r-universe.dev", RepoType::RUniverse);
        match repos_for("rlib", &d, true, None).as_slice() {
            [PkgRepo::Cranlike(r)] => {
                assert_eq!(r.url, "https://r-lib.r-universe.dev");
                assert_eq!(r.protocol, CranlikeProtocol::RUniverse);
            }
            other => panic!("{:?}", other),
        }
    }

    #[test]
    fn test_plain_cranlike() {
        for rtype in [
            RepoType::PpmOther,
            RepoType::PpmSnapshot,
            RepoType::CranLike,
        ] {
            let d = detected("https://x.org/repo", rtype);
            assert_eq!(
                repos_for("x", &d, true, None),
                vec![cranlike("x", "https://x.org/repo")]
            );
        }
    }

    #[test]
    fn test_ppm_root() {
        let info = |name: &str, rtype: RepoType, hidden: bool| PpmRepoInfo {
            name: name.to_string(),
            ppm_type: "R".to_string(),
            hidden,
            repo_type: rtype,
            extensible: false,
            metadata: None,
            url: format!("https://p3m.dev/{}/latest", name),
            guessed: false,
        };
        let mut d = detected("https://p3m.dev", RepoType::Ppm);
        d.ppm_repos = vec![
            info("all", RepoType::PpmOther, true),
            info("bioconductor", RepoType::PpmBioc, false),
            info("cran", RepoType::PpmCran, false),
            info("internal", RepoType::PpmOther, false),
        ];
        let repos = repos_for("p3m", &d, true, Some("3.22"));
        assert_eq!(
            repos,
            vec![
                PkgRepo::Extended(MetadataFeed::bioc("3.22")),
                PkgRepo::Extended(MetadataFeed::cran()),
                cranlike("p3m/internal", "https://p3m.dev/internal/latest"),
            ]
        );
        assert_eq!(
            feed_mirror(&repos[1..2]).as_deref(),
            Some("https://p3m.dev/cran/latest")
        );
    }
}
