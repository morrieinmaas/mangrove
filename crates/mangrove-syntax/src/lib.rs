//! L0 surface syntax: lexer and parser producing a `mangrove_core::Value`.

pub mod cst;
pub mod lexer;
pub mod parser;
pub mod ty;

pub use parser::{
    Document, FnDef, ListOpItem, Param, ParseError, Stmt, TypeDef, UnitDef, Use, parse,
    parse_document, parse_type,
};
pub use ty::{Annotation, CmpOp, FieldDef, Operand, Pred, Require, Type};
