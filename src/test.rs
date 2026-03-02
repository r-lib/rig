use std::error::Error;

use clap::ArgMatches;

use crate::rds::read_rds_file;
use crate::repos::cranlike_metadata::parse_packages_from_rds;

pub fn sc_test(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        Some(("read-rds", s)) => sc_test_read_rds(s, args, mainargs),
        Some(("read-packages-rds", s)) => sc_test_read_packages_rds(s, args, mainargs),
        Some(("parse-platform-string", s)) => sc_test_parse_platform_string(s, args, mainargs),
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
