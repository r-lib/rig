use std::error::Error;

use clap::ArgMatches;

use crate::rds::read_rds;

pub fn sc_test(args: &ArgMatches, mainargs: &ArgMatches) -> Result<(), Box<dyn Error>> {
    match args.subcommand() {
        Some(("read-rds", s)) => sc_test_read_rds(s, args, mainargs),
        _ => Ok(()), // unreachable
    }
}

fn sc_test_read_rds(
    args: &ArgMatches,
    _subargs: &ArgMatches,
    _mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let path = args.get_one::<String>("path").unwrap();
    read_rds(&std::path::PathBuf::from(path))?;
    Ok(())
}
