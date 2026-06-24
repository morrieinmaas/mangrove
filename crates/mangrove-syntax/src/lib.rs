//! L0 surface syntax: lexer and parser producing a `mangrove_core::Value`.

pub mod lexer;
pub mod parser;

pub use parser::{ParseError, parse};
