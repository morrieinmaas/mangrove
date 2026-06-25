//! Lossless concrete-syntax tree (CST) for Mangrove, built on `rowan`.
//!
//! Unlike the evaluation-oriented parser in `super::parser`, the CST preserves
//! *every* byte — comments, whitespace, newlines — so the formatter and LSP can
//! round-trip and locate source precisely. Evaluation still goes through
//! `lower()`, which reproduces the existing `Document`/`Value` AST.

mod kind;
mod lex;
mod lower;
mod parse;

#[cfg(test)]
mod tests;
