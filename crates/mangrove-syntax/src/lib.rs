//! L0 surface syntax: lexer and parser producing a `mangrove_core::Value`.

pub mod lexer;
pub mod parser;
pub mod ty;

pub use parser::{Document, ParseError, UnitDef, parse, parse_document, parse_type};
pub use ty::{FieldDef, Type};
