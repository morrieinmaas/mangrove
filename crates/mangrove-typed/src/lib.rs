//! L1 semantics: resolve named types and validate values against types.

pub mod env;
pub mod resolve;
pub mod validate;

pub use env::TypeEnv;
pub use resolve::resolve;
pub use validate::validate;
