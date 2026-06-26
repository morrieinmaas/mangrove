//! Lossless concrete-syntax tree (CST) for Mangrove, built on `rowan`.
//!
//! Unlike the evaluation-oriented parser in `super::parser`, the CST preserves
//! *every* byte — comments, whitespace, newlines — so the formatter and LSP can
//! round-trip and locate source precisely.
//!
//! # Architecture: two validated-equivalent front-ends
//!
//! - **Evaluation** (`hash`, `check`, compose) keeps using the hand-written
//!   `super::parser` (`parse`/`parse_document`) — fast, no tree allocation.
//! - **Tooling** (formatter, LSP) uses [`parse_cst`], walking the lossless
//!   [`SyntaxNode`] tree for navigation/formatting and calling [`lower`] when it
//!   needs a `Document` (e.g. to run type/compose diagnostics) — one parse yields
//!   both the tree and the AST.
//!
//! [`lower`] reconstructs the `Document` from the CST (delegating leaf decoding
//! to the legacy lexer/parser on node text), and the test suite's
//! `assert_document_equivalent` oracle proves the two front-ends agree on every
//! fixture and the whole example corpus — so the CST never silently drifts from
//! evaluation semantics. (Evaluation is intentionally *not* routed through the
//! CST: `lower` delegates back to the legacy parser, so doing so would parse
//! twice for no eval benefit.)

mod kind;
mod lex;
mod lower;
mod parse;

#[cfg(test)]
mod tests;

pub use kind::{MangroveLang, SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken};
pub use lower::lower;
pub use parse::{Parse, parse_cst};
