//! `rig cache info` and `rig cache clean`.
//!
//! rig's producers (binary indexes, built packages, downloaded package files,
//! CRAN-like databases, package manifests, ...) each write into their own
//! corner of `real_cache_dir()`, see [`crate::cache`]. Rather than teach this
//! module every producer's file-naming scheme, categories are derived from the
//! *name of the top-level entry* in the cache directory: `built`, `packages`
//! and `p3m` are their own category, `metadata` collects every other kind of
//! package metadata (binary indexes, CRAN-like databases, package manifests,
//! repo data), and anything unrecognized left over at the cache root (e.g.
//! stray files from an older rig's layout) falls back to `metadata` too, so
//! this module stays correct without an update here.

use std::error::Error;
use std::fs;
use std::path::Path;

use clap::ArgMatches;
use tabular::{row, Table};

use crate::cache::real_cache_dir;
use crate::output::OUTPUT;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CacheCategory {
    Built,
    Packages,
    Metadata,
    P3m,
}

impl CacheCategory {
    const ALL: [CacheCategory; 4] = [
        CacheCategory::Built,
        CacheCategory::Packages,
        CacheCategory::Metadata,
        CacheCategory::P3m,
    ];

    fn label(self) -> &'static str {
        match self {
            CacheCategory::Built => "Built",
            CacheCategory::Packages => "Packages",
            CacheCategory::Metadata => "Metadata",
            CacheCategory::P3m => "P3M status",
        }
    }

    // Also the top-level directory shown to the user in `rig cache info`:
    // every category has a real, single directory of its own.
    fn key(self) -> &'static str {
        match self {
            CacheCategory::Built => "built",
            CacheCategory::Packages => "packages",
            CacheCategory::Metadata => "metadata",
            CacheCategory::P3m => "p3m",
        }
    }
}

// Anything not recognized (e.g. a stray file left over from an older rig's
// cache layout) falls back to `Metadata`, the catch-all category.
fn classify_entry(name: &str) -> CacheCategory {
    match name {
        "built" => CacheCategory::Built,
        "packages" => CacheCategory::Packages,
        "p3m" => CacheCategory::P3m,
        _ => CacheCategory::Metadata,
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct Usage {
    size: u64,
    count: u64,
}

impl Usage {
    fn add(&mut self, other: Usage) {
        self.size += other.size;
        self.count += other.count;
    }
}

// Recurses into `path`, which is a directory. Missing/unreadable entries are
// skipped rather than failing the whole command: a cache directory is not
// something rig needs to be consistent, and a mid-walk race (another rig
// process cleaning up a stale file) is not an error either.
fn dir_usage(path: &Path) -> Usage {
    let mut usage = Usage::default();
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(_) => return usage,
    };
    for entry in entries.flatten() {
        // `DirEntry::metadata()` does not follow symlinks, so a symlink is
        // counted as a (small) file rather than recursed into.
        let meta = match entry.metadata() {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        if meta.is_dir() {
            usage.add(dir_usage(&entry.path()));
        } else {
            usage.size += meta.len();
            usage.count += 1;
        }
    }
    usage
}

fn cache_breakdown(cache_dir: &Path) -> [Usage; 4] {
    let mut totals = [Usage::default(); 4];
    let entries = match fs::read_dir(cache_dir) {
        Ok(entries) => entries,
        Err(_) => return totals,
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let category = classify_entry(&name.to_string_lossy());
        let meta = match entry.metadata() {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        let usage = if meta.is_dir() {
            dir_usage(&entry.path())
        } else {
            Usage {
                size: meta.len(),
                count: 1,
            }
        };
        let idx = CacheCategory::ALL
            .iter()
            .position(|c| *c == category)
            .expect("classify_entry() only returns values from CacheCategory::ALL");
        totals[idx].add(usage);
    }
    totals
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{:.1} {}", size, UNITS[unit])
    }
}

#[derive(serde::Serialize)]
struct CacheInfo {
    cache_dir: String,
    built_size: u64,
    built_count: u64,
    packages_size: u64,
    packages_count: u64,
    metadata_size: u64,
    metadata_count: u64,
    p3m_size: u64,
    p3m_count: u64,
    total_size: u64,
    total_count: u64,
}

pub fn sc_cache_info(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let json = args.get_flag("json");
    let cache_dir = real_cache_dir()?;
    let totals = cache_breakdown(&cache_dir);
    let mut total = Usage::default();
    for usage in totals {
        total.add(usage);
    }

    if json {
        let info = CacheInfo {
            cache_dir: cache_dir.display().to_string(),
            built_size: totals[0].size,
            built_count: totals[0].count,
            packages_size: totals[1].size,
            packages_count: totals[1].count,
            metadata_size: totals[2].size,
            metadata_count: totals[2].count,
            p3m_size: totals[3].size,
            p3m_count: totals[3].count,
            total_size: total.size,
            total_count: total.count,
        };
        println!("{}", serde_json::to_string_pretty(&info)?);
    } else {
        let mut tab = Table::new("{:<}  {:<}  {:>}  {:>}");
        tab.add_row(row!("Category", "Directory", "Size", "Files"));
        for (cat, usage) in CacheCategory::ALL.iter().zip(totals.iter()) {
            tab.add_row(row!(
                cat.label(),
                cat.key(),
                human_size(usage.size),
                usage.count.to_string()
            ));
        }
        tab.add_row(row!(
            "Total",
            "",
            human_size(total.size),
            total.count.to_string()
        ));
        let rendered = tab.to_string();
        let header_width = rendered.lines().next().unwrap_or("").len();
        let mut lines = rendered.lines();
        println!("{}", lines.next().unwrap_or(""));
        println!("{}", "-".repeat(header_width));
        for line in lines {
            println!("{}", line);
        }
        println!("Cache directory: {}", cache_dir.display());
    }

    Ok(())
}

pub fn sc_cache_clean(args: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let category = args.get_one::<String>("category").map(|s| s.as_str());
    let cache_dir = real_cache_dir()?;

    let entries = match fs::read_dir(&cache_dir) {
        Ok(entries) => entries,
        Err(_) => {
            OUTPUT.status("Cache directory does not exist, nothing to clean.");
            return Ok(());
        }
    };

    let mut removed = 0u32;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(category) = category {
            if classify_entry(&name).key() != category {
                continue;
            }
        }

        let path = entry.path();
        let result = if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        match result {
            Ok(()) => removed += 1,
            Err(err) => OUTPUT.warn(&format!("Cannot remove {}: {}", path.display(), err)),
        }
    }

    match category {
        Some(category) => OUTPUT.status(&format!(
            "Removed {} cache {} from {} ({})",
            removed,
            if removed == 1 { "entry" } else { "entries" },
            cache_dir.display(),
            category
        )),
        None => OUTPUT.status(&format!(
            "Removed {} cache {} from {}",
            removed,
            if removed == 1 { "entry" } else { "entries" },
            cache_dir.display()
        )),
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_known_and_unknown_entries() {
        assert_eq!(classify_entry("built"), CacheCategory::Built);
        assert_eq!(classify_entry("packages"), CacheCategory::Packages);
        assert_eq!(classify_entry("metadata"), CacheCategory::Metadata);
        assert_eq!(classify_entry("p3m"), CacheCategory::P3m);
        // Anything unrecognized (e.g. left over from an older rig's cache
        // layout) falls back to the metadata catch-all.
        assert_eq!(classify_entry("leftover-file"), CacheCategory::Metadata);
    }

    #[test]
    fn dir_usage_counts_files_recursively() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.txt"), b"hello").unwrap();
        let sub = tmp.path().join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("b.txt"), b"world!").unwrap();

        let usage = dir_usage(tmp.path());
        assert_eq!(usage.count, 2);
        assert_eq!(usage.size, 5 + 6);
    }

    #[test]
    fn cache_breakdown_groups_by_top_level_name() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("packages")).unwrap();
        fs::write(tmp.path().join("packages").join("pkg.tar.gz"), b"1234567").unwrap();
        fs::create_dir(tmp.path().join("metadata")).unwrap();
        fs::create_dir(tmp.path().join("metadata").join("binaries")).unwrap();
        fs::write(
            tmp.path().join("metadata").join("binaries").join("pkg.rbi"),
            b"12345",
        )
        .unwrap();
        fs::write(tmp.path().join("metadata").join("repo-xyz.data"), b"123").unwrap();
        fs::create_dir(tmp.path().join("p3m")).unwrap();
        fs::write(tmp.path().join("p3m").join("p3m-status.json"), b"12").unwrap();

        let totals = cache_breakdown(tmp.path());
        assert_eq!(totals[0].size, 0); // built
        assert_eq!(totals[1].size, 7); // packages
        assert_eq!(totals[2].size, 5 + 3); // metadata (binaries/ + repo-xyz.data)
        assert_eq!(totals[2].count, 2);
        assert_eq!(totals[3].size, 2); // p3m
        assert_eq!(totals[3].count, 1);
    }

    #[test]
    fn human_size_formats_units() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
    }
}
