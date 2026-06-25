//! L1 semantics: resolve named types and validate values against types.

pub mod env;
pub mod eval;
pub mod predicate;
pub mod resolve;
pub mod validate;

pub use env::TypeEnv;
pub use eval::eval;
pub use resolve::resolve;
pub use validate::{deprecations, validate};
