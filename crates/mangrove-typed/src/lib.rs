//! L1 semantics: resolve named types and validate values against types.

pub mod env;
pub mod validate;

pub use env::TypeEnv;
pub use validate::validate;
