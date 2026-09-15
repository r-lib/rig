use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rds2rust::RObject;

pub fn read_rds(data: &[u8]) -> Result<RObject, Box<dyn Error>> {
    let ps = rds2rust::read_rds(data)?;
    Ok(ps.object)
}

pub fn read_rds_file(path: &PathBuf) -> Result<RObject, Box<dyn Error>> {
    let ps = rds2rust::read_rds_from_path(path)?;
    Ok(ps.object)
}

/// Write `obj` to `path` as an RDS file, replacing it atomically.
pub fn write_rds_file(obj: &RObject, path: &Path) -> Result<(), Box<dyn Error>> {
    rds2rust::write_rds_atomic(obj, path)?;
    Ok(())
}

/// A length-1 character vector: `"value"`.
pub fn character_scalar(value: &str) -> RObject {
    RObject::Character(vec![Arc::from(value)].into())
}

/// A named character vector: `c(key1 = value1, key2 = value2, ...)`.
pub fn named_character_vector<I, K, V>(entries: I) -> RObject
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<Arc<str>>,
    V: Into<Arc<str>>,
{
    let mut names = Vec::new();
    let mut values = Vec::new();
    for (k, v) in entries {
        names.push(k.into());
        values.push(v.into());
    }
    let mut attrs = rds2rust::Attributes::new();
    attrs.insert(Arc::from("names"), RObject::Character(names.into()));
    RObject::WithAttributes {
        object: Box::new(RObject::Character(values.into())),
        attributes: attrs,
    }
}

/// An R version as R itself represents one: a `numeric_version`/
/// `package_version`/`R_system_version`-classed object, i.e. `unclass()`d a
/// one-element list holding an integer vector of the dot-separated parts
/// (`"4.4.0"` becomes `list(c(4L, 4L, 0L))`). This is the exact shape
/// `Meta/package.rds`'s `Built$R` has in a real `R CMD INSTALL`-produced
/// package; `loadNamespace()` checks compiled-code packages against it more
/// strictly than R-only ones, and rejects a plain unclassed value as
/// "installed by an R version with different internals".
pub fn r_system_version(version: &str) -> RObject {
    let parts: Vec<i32> = version
        .split(['.', '-'])
        .filter_map(|part| part.parse().ok())
        .collect();
    let mut attrs = rds2rust::Attributes::new();
    attrs.insert(
        Arc::from("class"),
        RObject::Character(
            vec![
                Arc::from("R_system_version"),
                Arc::from("package_version"),
                Arc::from("numeric_version"),
            ]
            .into(),
        ),
    );
    RObject::WithAttributes {
        object: Box::new(RObject::List(vec![RObject::Integer(parts.into())])),
        attributes: attrs,
    }
}

/// A named list: `list(key1 = value1, key2 = value2, ...)`.
pub fn named_list<I, K>(entries: I) -> RObject
where
    I: IntoIterator<Item = (K, RObject)>,
    K: Into<Arc<str>>,
{
    let mut names = Vec::new();
    let mut values = Vec::new();
    for (k, v) in entries {
        names.push(k.into());
        values.push(v);
    }
    let mut attrs = rds2rust::Attributes::new();
    attrs.insert(Arc::from("names"), RObject::Character(names.into()));
    RObject::WithAttributes {
        object: Box::new(RObject::List(values)),
        attributes: attrs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_rds_packages() {
        let path = PathBuf::from("tests/fixtures/cran-metadata/src/PACKAGES.rds");
        let result = read_rds_file(&path);

        assert!(result.is_ok(), "Failed to read PACKAGES.rds file");

        let obj = result.unwrap();

        // Use snapshot testing to verify the exact contents
        insta::assert_debug_snapshot!(obj);
    }
}
