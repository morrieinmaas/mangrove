//! Structured validation error (spec §12). Because there is no inference, the
//! `expected` type is a single concrete fact per error (a disjunction only when
//! the type genuinely is a union).

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Position {
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    /// Dotted field path, e.g. `container.port`; the root is `""`.
    pub path: String,
    /// A short rendering of the offending value.
    pub got: String,
    /// The expected type, rendered.
    pub expected: String,
    /// The specific constraint that failed, e.g. `<= 65535`.
    pub failed: Option<String>,
    /// Custom `@message` text (reserved for M2c; `None` in M2a).
    pub message: Option<String>,
    /// Source position, when known.
    pub at: Option<Position>,
}

impl ValidationError {
    pub fn new(
        path: impl Into<String>,
        got: impl Into<String>,
        expected: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            got: got.into(),
            expected: expected.into(),
            failed: None,
            message: None,
            at: None,
        }
    }

    pub fn with_failed(mut self, failed: impl Into<String>) -> Self {
        self.failed = Some(failed.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carries_path_got_expected_and_failed() {
        let e = ValidationError::new("container.port", "70000", "int & <= 65535")
            .with_failed("<= 65535");
        assert_eq!(e.path, "container.port");
        assert_eq!(e.got, "70000");
        assert_eq!(e.expected, "int & <= 65535");
        assert_eq!(e.failed.as_deref(), Some("<= 65535"));
        assert_eq!(e.message, None);
    }
}
