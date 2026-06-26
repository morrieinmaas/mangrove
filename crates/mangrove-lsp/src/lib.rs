//! Mangrove language server: a read-only, network-free LSP over the lossless
//! CST and the existing type pipeline.
//!
//! - [`analysis`] — pure, protocol-independent document analysis (diagnostics,
//!   symbols, hover, semantic tokens). Unit-testable without a client.
//! - [`line_index`] — byte offset ↔ UTF-16 `line:character` mapping.
//! - [`server`] — the `lsp-server` stdio event loop, driven by `mangrove lsp`.

pub mod analysis;
pub mod line_index;
pub mod server;
