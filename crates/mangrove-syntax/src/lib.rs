//! L0 surface syntax: lexer and parser producing a `mangrove_core::Value`.

pub mod lexer;
pub mod parser;
pub mod ty;

pub use parser::{ParseError, parse, parse_type};
pub use ty::{FieldDef, Type};
