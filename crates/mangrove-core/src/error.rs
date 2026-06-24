//! Structured validation error (spec §12). Fleshed out in later milestones;
//! at M0 it carries just a field path and a message.

/// A single structured validation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    /// Dotted field path, e.g. `container.port`.
    pub path: String,
    /// Human- and machine-readable failure message.
    pub message: String,
}

impl ValidationError {
    pub fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carries_path_and_message() {
        let e = ValidationError::new("container.port", "out of range");
        assert_eq!(e.path, "container.port");
        assert_eq!(e.message, "out of range");
    }
}
