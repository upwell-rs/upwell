//! Helpers for tests that drive fallible APIs with work that cannot fail.

use std::convert::Infallible;

/// A successful result of work that cannot fail, for APIs that expect a `Result`.
pub fn ok<T>(value: T) -> Result<T, Infallible> {
    Ok(value)
}
