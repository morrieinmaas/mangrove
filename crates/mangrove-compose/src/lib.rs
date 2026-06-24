//! L2 composition: local `use`, spread + deep-merge, `unset`, `@key` list ops,
//! and subtype redefinition. Produces a single merged value that validates and
//! hashes exactly like a hand-written one (D12).

pub mod merge;

pub use merge::merge;
