//! Macros used all over rig. This module is declared first, with
//! `#[macro_use]`, so they are in scope everywhere.

/// Return early with an error, the same as `simple_error::bail!`: either
/// any value that converts to the function's error type, or a format string
/// and its arguments. Unlike `simple_error::bail!`, it expands to an
/// expression without a trailing semicolon, so it also works as the last
/// expression of a block or in a `match` arm, without the
/// `semicolon_in_expressions_from_non_local_macros` lint.
macro_rules! bail {
    ($e:expr) => {
        return Err(::std::convert::From::from($e))
    };
    ($fmt:expr, $($arg:tt)+) => {
        return Err(::std::convert::From::from(::simple_error::SimpleError::new(format!(
            $fmt,
            $($arg)+
        ))))
    };
}
