use std::error::Error;
use std::path::PathBuf;

use rds2rust::read_rds_from_path;
use rds2rust::RObject;

pub fn read_rds(path: &PathBuf) -> Result<RObject, Box<dyn Error>> {
    let ps = read_rds_from_path(path)?;
    Ok(ps.object)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_rds_packages() {
        let path = PathBuf::from("tests/fixtures/cran-metadata/src/PACKAGES.rds");
        let result = read_rds(&path);

        assert!(result.is_ok(), "Failed to read PACKAGES.rds file");

        let obj = result.unwrap();

        // Use snapshot testing to verify the exact contents
        insta::assert_debug_snapshot!(obj);
    }
}
