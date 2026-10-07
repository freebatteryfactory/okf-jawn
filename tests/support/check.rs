//! Shared test idiom: tests return `TestResult`, use `?`, assertion macros and these helpers.
//!
//! Each test target includes this file with `#[path]` and declares it as `mod check;` at its
//! crate root, so sibling support modules can name `crate::check`. The unit tests below run in
//! every including target; they also keep each helper used, so no includer sees a dead-code
//! warning for a helper it does not call itself.

/// Result of a test that reports failure through `?` instead of panicking.
pub type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The error of a result that must have failed.
///
/// # Errors
/// Returns a description of the unexpected success.
pub fn err_of<T: std::fmt::Debug, E>(result: Result<T, E>) -> Result<E, String> {
    match result {
        Ok(value) => Err(format!("expected an error, got Ok({value:?})")),
        Err(error) => Ok(error),
    }
}

/// The value of an option that must be present; `what` names it in the failure.
///
/// # Errors
/// Returns a message naming `what` when the option is empty.
pub fn some<T>(value: Option<T>, what: &str) -> Result<T, String> {
    value.ok_or_else(|| format!("expected {what} to be present"))
}

#[cfg(test)]
mod tests {
    use super::{TestResult, err_of, some};

    #[test]
    fn err_of_returns_the_error_of_a_failed_result() -> TestResult {
        let failed: Result<u8, &str> = Err("boom");
        assert_eq!(err_of(failed)?, "boom");
        Ok(())
    }

    #[test]
    fn err_of_describes_an_unexpected_success() {
        let succeeded: Result<u8, &str> = Ok(7);
        assert_eq!(
            err_of(succeeded),
            Err("expected an error, got Ok(7)".to_owned())
        );
    }

    #[test]
    fn some_returns_the_present_value() -> TestResult {
        assert_eq!(some(Some(3), "count")?, 3);
        Ok(())
    }

    #[test]
    fn some_names_the_missing_value() {
        assert_eq!(
            some(None::<u8>, "count"),
            Err("expected count to be present".to_owned())
        );
    }
}
