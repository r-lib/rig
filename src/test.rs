use std::error::Error;

use clap::ArgMatches;

use crate::rds::read_rds_file;
use crate::repos::cranlike_metadata::parse_packages_from_rds;

pub fn sc_test(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        Some(("read-rds", s)) => sc_test_read_rds(s, args, mainargs),
        Some(("read-packages-rds", s)) => sc_test_read_packages_rds(s, args, mainargs),
        Some(("parse-platform-string", s)) => sc_test_parse_platform_string(s, args, mainargs),
        Some(("platform-to-pkg-type", s)) => sc_test_platform_to_pkg_type(s, args, mainargs),
        Some(("download-lockfile", s)) => sc_test_download_lockfile(s, args, mainargs),
        Some(("binary-index", s)) => sc_test_binary_index(s, args, mainargs),
        _ => Ok(()), // unreachable
    }
}

fn sc_test_read_rds(
    args: &ArgMatches,
    _subargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let path = args.get_one::<String>("path").unwrap();
    read_rds_file(&std::path::PathBuf::from(path))?;
    Ok(())
}

fn sc_test_read_packages_rds(
    args: &ArgMatches,
    _subargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let path = args.get_one::<String>("path").unwrap();
    parse_packages_from_rds(&std::path::PathBuf::from(path))?;
    Ok(())
}

fn sc_test_parse_platform_string(
    args: &ArgMatches,
    _subargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let platform = args.get_one::<String>("platform").unwrap();
    let parsed = crate::platform::parse_platform_string(platform);
    println!("Parsed platform string: {:#?}", parsed);
    Ok(())
}

fn sc_test_platform_to_pkg_type(
    args: &ArgMatches,
    _subargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let platform = args.get_one::<String>("platform").unwrap();
    let r_version = args.get_one::<String>("r-version").unwrap();
    let pkg_type = crate::platform::platform_to_pkg_type(
        &crate::platform::parse_platform_string(platform)?,
        r_version,
    );
    println!("Package type: {:?}", pkg_type);
    Ok(())
}

fn sc_test_download_lockfile(
    _args: &ArgMatches,
    _subargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    crate::proj::proj_download()?;
    Ok(())
}

fn sc_test_binary_index(
    args: &ArgMatches,
    _subargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    use crate::repos::binaries::*;

    let package = args.get_one::<String>("package").unwrap();
    // Before echoing the name back in a URL or a path.
    validate_package_name(package)?;

    println!("URL:        {}", binary_index_url(package));
    println!(
        "Cache file: {}",
        binary_index_local_file(package)?.display()
    );

    let cached = match ensure_binary_index_cached(package, None)? {
        None => {
            println!("Result:     no binary index for '{}' (404)", package);
            return Ok(());
        }
        Some(c) => c,
    };
    println!(
        "Result:     {}",
        if cached.downloaded {
            "downloaded"
        } else {
            "used cached copy"
        }
    );

    let index = BinaryIndex::from_file(package, &cached.path)?;
    println!("Package:    {}", index.package());
    println!("Rows:       {}", index.rows().len());
    println!("Versions:   {}", index.versions().len());

    // Resolve the target we are asking about. Default to this machine.
    let platform = match args.get_one::<String>("platform") {
        Some(p) => crate::platform::parse_platform_string(p)?,
        None => crate::platform::detect_platform()?,
    };
    let status = PpmStatus::load(None)?;
    let ppm = status.ppm_platform(&platform);
    match &ppm {
        Some((p, a)) => println!("Target:     platform={} arch={}", p, a),
        None => println!("Target:     no P3M builds for this platform"),
    }

    let version = match args.get_one::<String>("version") {
        Some(v) => v.to_string(),
        None => match index.latest_version() {
            Some(v) => v.to_string(),
            None => {
                println!("Index is empty.");
                return Ok(());
            }
        },
    };
    println!("Version:    {}", version);

    match index.source_row(&version) {
        Some(row) => println!("\nSource:\n  {}", row.url),
        None => println!("\nSource:\n  (none)"),
    }

    println!("Platforms:  {}", index.platforms_for(&version).join(" "));

    if let Some((ppm_plat, ppm_arch)) = ppm {
        let r_version = args.get_one::<String>("r-version");
        let r_minor = match r_version {
            Some(v) => crate::repos::cranlike_metadata::minor_r_version(v)?,
            None => {
                println!("\nPass --r-version to look up binaries.");
                return Ok(());
            }
        };
        let rows = index.binary_rows(&version, &ppm_plat, &ppm_arch, &r_minor);
        println!("\nBinaries for R {} ({} candidates):", r_minor, rows.len());
        for row in rows.iter() {
            println!("  {}", row.url);
            if !row.linkingto.is_empty() {
                let lt: Vec<String> = row
                    .linkingto
                    .iter()
                    .map(|l| format!("{}@{}", l.package, l.version))
                    .collect();
                println!("    built against: {}", lt.join(", "));
            }
        }
        // Several candidates mean several builds against different LinkingTo
        // versions; nothing here knows which one is right.
        if rows.len() > 1 {
            if let Some(row) = index.latest_binary_row(&version, &ppm_plat, &ppm_arch, &r_minor) {
                println!(
                    "\nNewest snapshot (arbitrary among the above):\n  {}",
                    row.url
                );
            }
        }
    }

    Ok(())
}
