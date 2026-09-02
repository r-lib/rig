// `rproj.lock`: the multi-target project lockfile written by `rig proj lock`
// and read by `rig proj sync`. TOML, unlike the JSON `pkg.lock` written by
// `rig pkg install` (`src/pak.rs`), which stays as-is because it mirrors the R
// `pak` package's own lockfile schema for interop with `pak::lockfile_*()`.
//
// `rproj.lock` is not interop with anything external; it is rig's own format,
// designed to hold the solve for *several* `(R version, platform)` targets in
// one file — e.g. solving once on a laptop for both macOS and a Linux
// deployment target. Each target's package list reuses `PakLockfilePackage`
// as-is (verified it round-trips cleanly through the `toml` crate, table
// fields and all), so a target's dependency data is exactly what `pkg.lock`
// would have recorded for that one target, just nested under it instead of
// being the whole file.
//
// For now (first implementation slice) `rig proj lock` only ever writes one
// target, and `rig proj sync` always installs `targets[0]`; the multi-target
// solve loop and the "pick the entry matching this machine" logic in `sync`
// are follow-up work.

use serde::{Deserialize, Serialize};

use crate::pak::PakLockfilePackage;

pub const RPROJ_LOCK_VERSION: usize = 1;

#[derive(Serialize, Deserialize, Debug)]
pub struct RprojLock {
    pub version: usize,
    pub targets: Vec<RprojLockTarget>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct RprojLockTarget {
    pub r_version: String,
    pub platform: String,
    pub packages: Vec<PakLockfilePackage>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn sample_package() -> PakLockfilePackage {
        PakLockfilePackage {
            r#ref: "cli".to_string(),
            package: "cli".to_string(),
            version: "3.6.0".to_string(),
            r#type: "standard".to_string(),
            direct: true,
            binary: true,
            dependencies: vec!["rlang".to_string()],
            vignettes: false,
            metadata: HashMap::from([("RemoteSha".to_string(), "abc123".to_string())]),
            sources: vec!["https://example.com/cli.tgz".to_string()],
            target: "cli.tgz".to_string(),
            platform: "aarch64-apple-darwin".to_string(),
            rversion: "4.6".to_string(),
            directpkg: true,
            license: "MIT".to_string(),
            dep_types: vec!["Imports".to_string()],
            params: vec![],
            install_args: "".to_string(),
            sysreqs: "".to_string(),
        }
    }

    #[test]
    fn roundtrips_through_toml() {
        let lock = RprojLock {
            version: RPROJ_LOCK_VERSION,
            targets: vec![RprojLockTarget {
                r_version: "4.6".to_string(),
                platform: "aarch64-apple-darwin".to_string(),
                packages: vec![sample_package()],
            }],
        };
        let text = toml::to_string_pretty(&lock).unwrap();
        let parsed: RprojLock = toml::from_str(&text).unwrap();
        assert_eq!(parsed.version, 1);
        assert_eq!(parsed.targets.len(), 1);
        assert_eq!(parsed.targets[0].r_version, "4.6");
        assert_eq!(parsed.targets[0].packages[0].r#ref, "cli");
        assert_eq!(
            parsed.targets[0].packages[0].metadata.get("RemoteSha"),
            Some(&"abc123".to_string())
        );
    }
}
