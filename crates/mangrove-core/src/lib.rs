//! Core types shared across all Mangrove layers. No dependency on any layer.

pub mod error;
pub mod num;
pub mod value;

pub use num::exact_bigint;
pub use value::Value;
