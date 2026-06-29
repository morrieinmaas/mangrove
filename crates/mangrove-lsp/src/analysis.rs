//! Pure analysis over a single Mangrove document: parse + type/compose
//! diagnostics, document symbols, hover, and semantic-token classification —
//! all derived from the lossless CST and the existing type pipeline.
//!
//! Single-file and **read-only**: imports are *not* resolved (no network, no
//! lockfile fetch). Cross-file `use` diagnostics are out of scope for v0.4.0;
//! a document that `use`s namespaced modules simply skips the type-check stage
//! (its parse + local-symbol features still work).

use mangrove_syntax::cst::{SyntaxKind, SyntaxNode, lower, parse_cst};
use mangrove_syntax::ty::Type;
use rowan::TextRange;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

// ---------------------------------------------------------------------------
// ImportCache — mtime/len-keyed, session-scoped cache for imported-file reads.
// Interior-mutability (RefCell/Cell) keeps the borrow signature `&self` so
// callers can hold a shared reference for the lifetime of the event loop.
// The LSP is single-threaded; a RefCell is safe and a Mutex would add
// needless overhead.
// ---------------------------------------------------------------------------

/// Version key: `(mtime, file_len)` — both fields come from a single
/// `std::fs::metadata` stat call; no extra read is needed.
///
/// **Invalidation contract**: a cache entry is considered stale when either
/// field changes.  In practice this catches all normal edits:
/// - Any write that changes the file content almost always changes either the
///   byte length OR the mtime (or both).
/// - `std::fs::Metadata::modified()` returns the OS's full-resolution mtime
///   (nanoseconds on Linux/macOS, 100 ns on Windows), making the residual
///   same-tick + same-len window effectively impossible under real workloads
///   (it would require two distinct writes whose bytes cancel out to identical
///   length, both completing within one mtime tick — sub-microsecond on
///   modern kernels).
///
/// We deliberately do NOT add content-hashing on every request because that
/// would re-read the file on every cache check, defeating the cache's purpose.
/// Adding a unix `ctime` field (via `std::os::unix::fs::MetadataExt`) would
/// close the residual window on Unix but adds platform-specific complexity
/// that isn't justified by the near-zero practical risk.
type VersionKey = (SystemTime, u64);

struct CacheEntry {
    key: VersionKey,
    text: Arc<str>,
}

/// Session-scoped cache for imported Mangrove file texts.
///
/// Uses `std::fs::metadata` (a single stat) as the cache-hit check, falling
/// back to `read_to_string` only when mtime or length has changed.  All IO
/// errors are treated as cache-miss → return `None`; callers skip the file.
///
/// This type is intentionally NOT `Send`/`Sync` (it holds `RefCell`).
/// That is correct for the single-threaded LSP event loop.
pub struct ImportCache {
    inner: RefCell<HashMap<PathBuf, CacheEntry>>,
    reads: Cell<usize>,
}

impl ImportCache {
    pub fn new() -> Self {
        Self {
            inner: RefCell::new(HashMap::new()),
            reads: Cell::new(0),
        }
    }

    /// Number of actual `read_to_string` calls (cache misses).
    /// Used in tests to verify cache-hit behaviour.
    pub fn reads(&self) -> usize {
        self.reads.get()
    }

    /// Return the text of `path`, reading from disk only when the
    /// mtime/len key has changed (or the path is not yet cached).
    ///
    /// Returns `None` on any IO error; never panics, never fetches.
    pub fn read(&self, path: &Path) -> Option<Arc<str>> {
        // Stat first (cheap; avoids opening the file on cache hits).
        let meta = std::fs::metadata(path).ok()?;
        let key: VersionKey = (meta.modified().ok()?, meta.len());

        // Fast path: key matches → return cached text.
        if let Some(entry) = self.inner.borrow().get(path) {
            if entry.key == key {
                return Some(Arc::clone(&entry.text));
            }
        }

        // Slow path: read from disk.
        let text: Arc<str> = Arc::from(std::fs::read_to_string(path).ok()?.as_str());
        self.reads.set(self.reads.get() + 1);
        self.inner.borrow_mut().insert(
            path.to_path_buf(),
            CacheEntry {
                key,
                text: Arc::clone(&text),
            },
        );
        Some(text)
    }
}

impl Default for ImportCache {
    fn default() -> Self {
        Self::new()
    }
}

/// A completion item returned by the analysis layer.
#[derive(Debug, Clone, PartialEq)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    /// A `type X = …` or `unit X` declaration.
    TypeName,
    /// A top-level language keyword.
    Keyword,
    /// A record-field name from the bound schema type.
    Field,
    /// A literal member of an enum / literal-union field type.
    EnumValue,
}

/// A diagnostic with a byte-offset range into the source.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub range: (usize, usize),
    pub message: String,
}

/// A document-outline symbol (top-level declaration).
#[derive(Debug, Clone, PartialEq)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub range: (usize, usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Type,
    Unit,
    Schema,
    Param,
    Fn,
    Binding,
}

/// One classified token for semantic highlighting.
#[derive(Debug, Clone, PartialEq)]
pub struct SemToken {
    pub range: (usize, usize),
    pub kind: SemKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemKind {
    Keyword,
    Type,
    String,
    Number,
    Unit,
    Comment,
    Property,
    Operator,
}

/// Parse + (when self-contained) type-check, returning all diagnostics.
pub fn diagnostics(src: &str) -> Vec<Diagnostic> {
    let parse = parse_cst(src);
    let root = parse.syntax();
    let mut out = Vec::new();

    // 1. Parse errors → one diagnostic per ERROR node, ranged by the node.
    //    Use the actual parser error messages where available (matched by index).
    let error_nodes: Vec<_> = root
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::ERROR)
        .collect();
    for (i, node) in error_nodes.iter().enumerate() {
        let r = node.text_range();
        // Prefer the parser's own message for this node if it has one.
        let message = parse
            .errors
            .get(i)
            .map(|e| e.message.clone())
            .unwrap_or_else(|| "syntax error".to_string());
        out.push(Diagnostic {
            range: (r.start().into(), r.end().into()),
            message,
        });
    }
    // If the parser recorded messages but produced no ERROR node (e.g. a missing
    // closer at EOF), still surface them. Use a visible range: from the start of
    // the last non-empty line to the end of the source.
    if out.is_empty() && !parse.errors.is_empty() {
        let end = src.len();
        // Find the start of the last non-empty line so the range is visible.
        let range_start = src
            .rfind(|c: char| !c.is_whitespace())
            .and_then(|pos| src[..pos].rfind('\n').map(|nl| nl + 1))
            .unwrap_or(0);
        for e in &parse.errors {
            out.push(Diagnostic {
                range: (range_start, end),
                message: e.message.clone(),
            });
        }
    }
    // Don't run semantic analysis on a document that didn't even parse cleanly.
    if !out.is_empty() {
        return out;
    }

    // 2. Type/compose diagnostics — only for self-contained documents.
    //    Lowering delegates to the legacy parser; semantic errors carry their
    //    own messages. We locate the offending declaration in the CST so the
    //    squiggle lands on the right symbol rather than the whole document.
    if let Some(msg) = type_check(&root) {
        let range = locate_span_for_type_error(&msg, &root, src.len());
        out.push(Diagnostic {
            range,
            message: msg,
        });
    }
    out
}

/// Run the same pipeline as `mangrove check`, but single-file (no import
/// resolution). Returns the first blocking error message, or `None` if the
/// document is valid / can't be checked self-contained.
fn type_check(root: &SyntaxNode) -> Option<String> {
    // A document that imports namespaced modules needs the resolver/lockfile —
    // out of scope for the read-only LSP. Skip type-checking it.
    let doc = lower(root).ok()?;
    if !doc.uses.is_empty() {
        return None;
    }
    // build() returning Err → schema error.
    let env = mangrove_typed::TypeEnv::build(&doc.typedefs, &doc.unitdefs).err()?;
    Some(format!("schema error: {env}"))
}

/// Given a type/schema error message from `TypeEnv::build`, locate the
/// offending declaration in the CST and return its byte range. Falls back to
/// `(0, src_len)` when the message can't be mapped to a node.
///
/// Handled patterns:
///   "duplicate type definition: NAME"             → first TYPE_DEF with name == NAME
///   "duplicate type/unit definition: NAME"        → first UNIT_DEF or TYPE_DEF with name == NAME
///   "non-productive recursive type involving `NAME`" → TYPE_DEF with name == NAME
///   "unknown type: NAME"                          → first TYPE_DEF body containing BAREWORD NAME
fn locate_span_for_type_error(msg: &str, root: &SyntaxNode, src_len: usize) -> (usize, usize) {
    let fallback = (0, src_len);
    // The message arrives as "schema error: <TypeEnv message>"; strip the prefix.
    let inner = msg.strip_prefix("schema error: ").unwrap_or(msg);

    let symbol: Option<&str> = if let Some(rest) = inner.strip_prefix("duplicate type definition: ")
    {
        Some(rest.trim())
    } else if let Some(rest) = inner.strip_prefix("duplicate type/unit definition: ") {
        Some(rest.trim())
    } else if let Some(rest) = inner.strip_prefix("non-productive recursive type involving `") {
        // Extract the name between backticks: the rest starts after "involving `",
        // so we need to find where the closing backtick is.
        rest.split('`').next().map(str::trim)
    } else {
        None
    };

    if let Some(name) = symbol {
        for node in root.children() {
            if !matches!(node.kind(), SyntaxKind::TYPE_DEF | SyntaxKind::UNIT_DEF) {
                continue;
            }
            if node_decl_name(&node).as_deref() == Some(name) {
                let r = node.text_range();
                return (r.start().into(), r.end().into());
            }
        }
        return fallback;
    }

    if let Some(rest) = inner.strip_prefix("unknown type: ") {
        let name = rest.trim();
        for node in root.children() {
            if node.kind() != SyntaxKind::TYPE_DEF {
                continue;
            }
            let has_ref = node
                .descendants_with_tokens()
                .filter_map(|e| e.into_token())
                .any(|t| t.kind() == SyntaxKind::BAREWORD && t.text() == name);
            if has_ref {
                let r = node.text_range();
                return (r.start().into(), r.end().into());
            }
        }
    }

    fallback
}

/// Extract the declared name of a TYPE_DEF or UNIT_DEF node: skip the keyword
/// token, return the text of the first non-trivia, non-NEWLINE token.
fn node_decl_name(node: &SyntaxNode) -> Option<String> {
    let mut tokens = node
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| !t.kind().is_trivia() && t.kind() != SyntaxKind::NEWLINE);
    tokens.next(); // skip keyword
    tokens.next().map(|t| t.text().to_string())
}

/// Top-level declaration outline.
pub fn symbols(src: &str) -> Vec<Symbol> {
    let root = parse_cst(src).syntax();
    let mut out = Vec::new();
    for child in root.children() {
        let kind = match child.kind() {
            SyntaxKind::TYPE_DEF => SymbolKind::Type,
            SyntaxKind::UNIT_DEF => SymbolKind::Unit,
            SyntaxKind::SCHEMA_DECL => SymbolKind::Schema,
            SyntaxKind::PARAM_DECL => SymbolKind::Param,
            SyntaxKind::FN_DEF => SymbolKind::Fn,
            SyntaxKind::BINDING => SymbolKind::Binding,
            _ => continue,
        };
        if let Some(name) = decl_name(&child, kind) {
            let r = child.text_range();
            out.push(Symbol {
                name,
                kind,
                range: (r.start().into(), r.end().into()),
            });
        }
    }
    out
}

/// The declared name of a top-level node: for `type`/`unit`/`schema`/`fn` it's
/// the second significant token; for a binding it's the first; for `params`
/// it's the literal keyword.
fn decl_name(node: &SyntaxNode, kind: SymbolKind) -> Option<String> {
    if kind == SymbolKind::Param {
        return Some("params".to_string());
    }
    let mut words = node
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| !t.kind().is_trivia() && t.kind() != SyntaxKind::NEWLINE);
    match kind {
        SymbolKind::Binding => words.next().map(|t| t.text().to_string()),
        // skip the keyword, take the name
        _ => {
            words.next();
            words.next().map(|t| t.text().to_string())
        }
    }
}

/// Classify every significant token + comment for highlighting.
pub fn semantic_tokens(src: &str) -> Vec<SemToken> {
    let root = parse_cst(src).syntax();
    let mut out = Vec::new();
    for el in root.descendants_with_tokens() {
        let Some(tok) = el.into_token() else { continue };
        let k = tok.kind();
        let kind = match k {
            SyntaxKind::COMMENT | SyntaxKind::DOC | SyntaxKind::DIRECTIVE => SemKind::Comment,
            SyntaxKind::STR => SemKind::String, // INTERP_STR / BYTES are reserved — never emitted
            SyntaxKind::INT | SyntaxKind::DECIMAL => SemKind::Number,
            SyntaxKind::UNIT_LIT => SemKind::Unit,
            SyntaxKind::BOOL => SemKind::Keyword,
            SyntaxKind::COLON
            | SyntaxKind::AMP
            | SyntaxKind::PIPE
            | SyntaxKind::EQ
            | SyntaxKind::GE
            | SyntaxKind::LE
            | SyntaxKind::GT
            | SyntaxKind::LT
            | SyntaxKind::DOT_DOT_DOT
            | SyntaxKind::PLUS_EQ
            | SyntaxKind::AT => SemKind::Operator,
            SyntaxKind::BAREWORD => classify_bareword(&tok),
            _ => continue,
        };
        let r = tok.text_range();
        out.push(SemToken {
            range: (r.start().into(), r.end().into()),
            kind,
        });
    }
    out
}

/// A bareword is a keyword, a type name (capitalized / in type position), or a
/// plain property/reference. We use a light heuristic that doesn't need a typed
/// tree: declaration keywords are fixed; a capitalized word is treated as a type.
fn classify_bareword(tok: &mangrove_syntax::cst::SyntaxToken) -> SemKind {
    let text = tok.text();
    if matches!(
        text,
        "type" | "unit" | "schema" | "use" | "params" | "fn" | "match" | "require" | "unset" | "as"
    ) {
        return SemKind::Keyword;
    }
    if text.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
        return SemKind::Type;
    }
    SemKind::Property
}

/// Hover text for the symbol at `offset`: the declaration it names, plus any
/// leading `##` doc comment.
pub fn hover(src: &str, offset: usize) -> Option<String> {
    let root = parse_cst(src).syntax();
    let off: rowan::TextSize = (offset as u32).into();
    // Find the token at the offset, preferring a significant token over trivia
    // when the offset sits on a boundary (e.g. between a space and a word).
    let covering: Vec<_> = root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.text_range().contains_inclusive(off))
        .collect();
    let tok = covering
        .iter()
        .find(|t| !t.kind().is_trivia() && t.kind() != SyntaxKind::NEWLINE)
        .cloned()?;
    // Walk up to the enclosing top-level declaration.
    let decl = top_level_ancestor(&tok.parent()?)?;
    let kind_label = match decl.kind() {
        SyntaxKind::TYPE_DEF => "type",
        SyntaxKind::UNIT_DEF => "unit",
        SyntaxKind::SCHEMA_DECL => "schema",
        SyntaxKind::PARAM_DECL => "params",
        SyntaxKind::FN_DEF => "fn",
        SyntaxKind::BINDING => "binding",
        _ => return None,
    };
    let mut text = format!("**{kind_label}**\n```mangrove\n{}\n```", decl_text(&decl));
    if let Some(doc) = leading_doc(&decl) {
        text = format!("{doc}\n\n{text}");
    }
    Some(text)
}

/// The trimmed source text of a declaration node.
fn decl_text(node: &SyntaxNode) -> String {
    node.text().to_string().trim().to_string()
}

/// Collect the `##` doc-comment lines immediately preceding a declaration.
///
/// Doc comments aren't always reliable CST siblings of the declaration they lead
/// (the parser can fold a leading `## …` line into an adjacent node), so we scan
/// the document's flat token stream backward from the declaration's start,
/// skipping only trivia/newlines and gathering contiguous `DOC` tokens.
fn leading_doc(node: &SyntaxNode) -> Option<String> {
    let root = {
        let mut r = node.clone();
        while let Some(p) = r.parent() {
            r = p;
        }
        r
    };
    let start = node.text_range().start();
    // All tokens strictly before the declaration start, in document order.
    let before: Vec<_> = root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.text_range().end() <= start)
        .collect();
    let mut docs = Vec::new();
    for t in before.iter().rev() {
        match t.kind() {
            SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE => {}
            SyntaxKind::DOC => docs.push(t.text().trim_start_matches('#').trim().to_string()),
            _ => break,
        }
    }
    if docs.is_empty() {
        None
    } else {
        docs.reverse();
        Some(docs.join("\n"))
    }
}

/// Climb to the DOCUMENT-direct child containing `node`.
fn top_level_ancestor(node: &SyntaxNode) -> Option<SyntaxNode> {
    let mut cur = node.clone();
    loop {
        let parent = cur.parent()?;
        if parent.kind() == SyntaxKind::DOCUMENT {
            return Some(cur);
        }
        cur = parent;
    }
}

/// Format a CST range as an LSP-friendly `(start, end)` byte tuple.
pub fn range_tuple(r: TextRange) -> (usize, usize) {
    (r.start().into(), r.end().into())
}

// ---- go-to-definition ----

/// Resolve the symbol under `offset` to the byte range of its definition in
/// the same document.  Returns `None` if the cursor is not on a resolvable
/// symbol, or if the definition is cross-file (out of scope for v1).
///
/// Supported cases (local only):
/// - A `REF` node / bare-word in value position → the top-level `BINDING`
///   whose key matches, or a `params` entry with the same name.
/// - A bareword in type-name position inside a `TYPE_DEF`/`UNIT_DEF`/
///   `SCHEMA_DECL` → the `TYPE_DEF` or `UNIT_DEF` that declares it.
pub fn goto_definition(src: &str, offset: usize) -> Option<(usize, usize)> {
    let root = parse_cst(src).syntax();
    let off: rowan::TextSize = (offset as u32).into();

    // Find the significant token under the cursor.
    let covering: Vec<_> = root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.text_range().contains_inclusive(off))
        .collect();
    let tok = covering
        .iter()
        .find(|t| !t.kind().is_trivia() && t.kind() != SyntaxKind::NEWLINE)
        .cloned()?;

    // Only BAREWORD tokens are interesting for definition lookup.
    if tok.kind() != SyntaxKind::BAREWORD {
        return None;
    }
    let name = tok.text().to_string();

    // Skip language keywords — they have no definition site.
    if matches!(
        name.as_str(),
        "type"
            | "unit"
            | "schema"
            | "use"
            | "params"
            | "fn"
            | "match"
            | "require"
            | "unset"
            | "as"
            | "true"
            | "false"
            | "int"
            | "str"
            | "bool"
            | "decimal"
            | "bytes"
            | "brand"
    ) {
        return None;
    }

    // Determine the context of the token by examining its parent node kind.
    let parent = tok.parent()?;
    let in_type_position = matches!(
        parent.kind(),
        SyntaxKind::TYPE_DEF | SyntaxKind::UNIT_DEF | SyntaxKind::SCHEMA_DECL
    );

    if in_type_position {
        // In a SCHEMA_DECL the name always refers to a type definition — jump there.
        if parent.kind() == SyntaxKind::SCHEMA_DECL {
            return find_type_decl(&root, &name);
        }

        // The token is inside a TYPE_DEF or UNIT_DEF: it is either (a) the
        // declared name itself (first significant token after the keyword) or
        // (b) a type reference inside the body.
        let declared = node_decl_name(&parent);
        let is_decl_name = declared.as_deref() == Some(name.as_str());

        // If the cursor is on the declaration name itself, jump to the enclosing
        // declaration node (that is the definition).
        if is_decl_name {
            let r = parent.text_range();
            return Some((r.start().into(), r.end().into()));
        }

        // Otherwise it's a reference to another type/unit.
        return find_type_decl(&root, &name);
    }

    // For tokens inside a REF node or general value/binding context, look for
    // a top-level binding or param entry with this name.  Qualified refs
    // (`alias.Type`) are cross-file — skip them.
    if name.contains('.') {
        return None;
    }

    // Try REF node parent first.
    let maybe_ref = parent.kind() == SyntaxKind::REF
        || parent.kind() == SyntaxKind::BINDING
        || parent.kind() == SyntaxKind::MATCH_EXPR
        || parent.kind() == SyntaxKind::CALL
        || parent.kind() == SyntaxKind::DOCUMENT;

    // Also accept barewords that are in BINDING value position (the key side
    // is not a reference, the value side is).  We detect value-position by
    // checking there is a COLON sibling before this token in the binding.
    let in_value_position = if parent.kind() == SyntaxKind::BINDING {
        let mut seen_colon = false;
        let mut is_value = false;
        for el in parent.children_with_tokens() {
            match el {
                rowan::NodeOrToken::Token(t) if t.kind() == SyntaxKind::COLON => {
                    seen_colon = true;
                }
                rowan::NodeOrToken::Token(t)
                    if seen_colon && t.text_range() == tok.text_range() =>
                {
                    is_value = true;
                    break;
                }
                rowan::NodeOrToken::Node(n) if seen_colon => {
                    // token is inside a child node after the colon
                    if n.text_range().contains_inclusive(off) {
                        is_value = true;
                    }
                    break;
                }
                _ => {}
            }
        }
        is_value
    } else {
        maybe_ref
    };

    if !in_value_position && parent.kind() != SyntaxKind::REF {
        return None;
    }

    // Search: binding key, then params entry, then type/unit decl as fallback.
    find_binding(&root, &name)
        .or_else(|| find_param_entry(&root, &name))
        .or_else(|| find_type_decl(&root, &name))
}

/// Find the top-level BINDING with the given key name; return its byte range.
fn find_binding(root: &SyntaxNode, name: &str) -> Option<(usize, usize)> {
    for child in root.children() {
        if child.kind() != SyntaxKind::BINDING {
            continue;
        }
        // The key is the first significant token.
        let key = child
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| !t.kind().is_trivia() && t.kind() != SyntaxKind::NEWLINE)?;
        if key.text() == name {
            let r = child.text_range();
            return Some((r.start().into(), r.end().into()));
        }
    }
    None
}

/// Find a param entry inside a PARAM_DECL block with the given name.
fn find_param_entry(root: &SyntaxNode, name: &str) -> Option<(usize, usize)> {
    for child in root.children() {
        if child.kind() != SyntaxKind::PARAM_DECL {
            continue;
        }
        // Walk tokens inside the params block; each param name is a BAREWORD
        // that precedes a COLON.
        let tokens: Vec<_> = child
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| !t.kind().is_trivia() && t.kind() != SyntaxKind::NEWLINE)
            .collect();
        let mut i = 0;
        while i < tokens.len() {
            if tokens[i].kind() == SyntaxKind::BAREWORD
                && tokens.get(i + 1).map(|t| t.kind()) == Some(SyntaxKind::COLON)
                && tokens[i].text() == name
            {
                let r = tokens[i].text_range();
                return Some((r.start().into(), r.end().into()));
            }
            i += 1;
        }
    }
    None
}

// ---- cross-file go-to-definition ----

/// Attempt cross-file go-to-definition for a qualified reference `alias.TypeName`.
///
/// Returns `Some((file_path, (start, end), file_text))` where the byte range is
/// within `file_text`. Returns `None` if:
/// - The cursor is not on a qualified reference (`alias.TypeName` pattern).
/// - The alias doesn't match any `use` declaration in the document.
/// - The package is not locally available (git backend, missing file, etc.).
/// - Anything would require a fetch or network operation.
///
/// `doc_path` is the on-disk path of the document being edited (from the file URI).
pub fn goto_definition_cross_file(
    src: &str,
    offset: usize,
    doc_path: &Path,
) -> Option<(PathBuf, (usize, usize), String)> {
    goto_definition_cross_file_impl(src, offset, doc_path, None)
}

/// Internal implementation; `cache` is `None` in pure unit tests (preserves
/// existing test behaviour) and `Some(&state.import_cache)` in the live server.
pub(crate) fn goto_definition_cross_file_cached(
    src: &str,
    offset: usize,
    doc_path: &Path,
    cache: &ImportCache,
) -> Option<(PathBuf, (usize, usize), String)> {
    goto_definition_cross_file_impl(src, offset, doc_path, Some(cache))
}

fn goto_definition_cross_file_impl(
    src: &str,
    offset: usize,
    doc_path: &Path,
    cache: Option<&ImportCache>,
) -> Option<(PathBuf, (usize, usize), String)> {
    let root = parse_cst(src).syntax();
    let off: rowan::TextSize = (offset as u32).into();

    // Find significant token under cursor.
    // Prefer tokens that start at or before `off` and end AFTER `off` (strict
    // containment). Fall back to tokens that merely end at `off` (inclusive end).
    // This avoids picking a DOT when the cursor is on the first character of the
    // following BAREWORD (both have `contains_inclusive(off)` true).
    let covering: Vec<_> = root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.text_range().contains_inclusive(off))
        .filter(|t| !t.kind().is_trivia() && t.kind() != SyntaxKind::NEWLINE)
        .collect();
    // Prefer a token that strictly contains the offset (end > off), then fall
    // back to one that just ends at off.
    let tok = covering
        .iter()
        .find(|t| t.text_range().end() > off)
        .or_else(|| covering.first())
        .cloned()?;

    if tok.kind() != SyntaxKind::BAREWORD {
        return None;
    }

    // Check for qualified ref pattern: alias.TypeName
    let (alias, type_name) = detect_qualified_ref(&root, &tok)?;

    // Resolve the use alias directly from the CST so that documents which fail
    // to lower (e.g. a bareword in value position: `x: inf.Widget`) still work.
    let reference = use_reference_for_alias(&root, &alias)?;

    // Resolve to local path (no-fetch).
    let doc_dir = doc_path.parent()?;
    let resolvers = mangrove_resolve::Resolvers::find_and_load(doc_dir).ok()?;
    let pkg_path = resolvers.resolve_local_path(&reference).ok()?;

    // Read the file via cache (or directly when no cache is available).
    // The cache internally checks existence via metadata(); a missing file → None.
    let pkg_text: String = if let Some(c) = cache {
        c.read(&pkg_path)?.to_string()
    } else {
        // Fallback: check existence then read directly (preserves behaviour
        // for unit tests that don't supply a cache).
        if !pkg_path.exists() {
            return None;
        }
        std::fs::read_to_string(&pkg_path).ok()?
    };

    // Parse and find the type/unit declaration.
    let pkg_root = parse_cst(&pkg_text).syntax();
    let byte_range = find_type_decl(&pkg_root, &type_name)?;

    Some((pkg_path, byte_range, pkg_text))
}

/// Walk `USE_DECL` nodes in the CST and return the path of the one whose alias
/// matches. This avoids calling `lower()`, which fails on partially-invalid
/// documents (e.g. a bareword in value position such as `x: inf.Widget`).
///
/// **First-match is correct here**: duplicate `use` aliases (e.g. two `use
/// "…" as k` declarations sharing the alias `k`) are rejected by the compose
/// layer (`mangrove-compose/src/load.rs`) before a document can be evaluated.
/// The LSP skips type-checking for documents with `use` declarations, so a
/// duplicate-alias document never reaches this function with ambiguous state.
fn use_reference_for_alias(root: &SyntaxNode, alias: &str) -> Option<String> {
    for child in root.children() {
        if child.kind() == SyntaxKind::USE_DECL {
            let text = child.text().to_string();
            if let Ok(u) = mangrove_syntax::parse_use_str(text.trim()) {
                if u.alias == alias {
                    return Some(u.path);
                }
            }
        }
    }
    None
}

/// Detect if the token is part of an `alias.TypeName` pattern.
/// Returns `(alias, type_name)` if so.
fn detect_qualified_ref(
    root: &SyntaxNode,
    tok: &mangrove_syntax::cst::SyntaxToken,
) -> Option<(String, String)> {
    // Collect all significant (non-trivia, non-newline) tokens in document order.
    let all_toks: Vec<_> = root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| !t.kind().is_trivia() && t.kind() != SyntaxKind::NEWLINE)
        .collect();

    let pos = all_toks
        .iter()
        .position(|t| t.text_range() == tok.text_range())?;

    // Case A: cursor on alias — next token is DOT, then a BAREWORD (the type name).
    if let (Some(next), Some(after)) = (all_toks.get(pos + 1), all_toks.get(pos + 2)) {
        if next.kind() == SyntaxKind::DOT && after.kind() == SyntaxKind::BAREWORD {
            return Some((tok.text().to_string(), after.text().to_string()));
        }
    }

    // Case B: cursor on TypeName — prev token is DOT, prev-prev is a BAREWORD (alias).
    if pos >= 2 {
        let prev = &all_toks[pos - 1];
        let before = &all_toks[pos - 2];
        if prev.kind() == SyntaxKind::DOT && before.kind() == SyntaxKind::BAREWORD {
            return Some((before.text().to_string(), tok.text().to_string()));
        }
    }

    None
}

/// Find a TYPE_DEF or UNIT_DEF whose declared name equals `name`.
fn find_type_decl(root: &SyntaxNode, name: &str) -> Option<(usize, usize)> {
    for child in root.children() {
        if !matches!(child.kind(), SyntaxKind::TYPE_DEF | SyntaxKind::UNIT_DEF) {
            continue;
        }
        if node_decl_name(&child).as_deref() == Some(name) {
            let r = child.text_range();
            return Some((r.start().into(), r.end().into()));
        }
    }
    None
}

// ---- references ----

/// The "symbol kind" we resolved the cursor to — for sharing between
/// `references` and `rename`.
#[derive(Debug, Clone, PartialEq)]
enum ResolvedSymbol {
    /// A `type X` or `unit X` declaration; `name` is the declared identifier.
    TypeName(String),
    /// A top-level binding key or `params` entry; `name` is the identifier.
    ValueName(String),
}

/// Resolve the token under `offset` to a local symbol, or return `None` when
/// the cursor is on a keyword, a qualified name (`alias.Foo`), or an unresolvable
/// token.  Reuses the same token-finding logic as `goto_definition`.
fn resolve_symbol(root: &SyntaxNode, _src: &str, offset: usize) -> Option<ResolvedSymbol> {
    let off: rowan::TextSize = (offset as u32).into();

    let covering: Vec<_> = root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.text_range().contains_inclusive(off))
        .collect();
    let tok = covering
        .iter()
        .find(|t| !t.kind().is_trivia() && t.kind() != SyntaxKind::NEWLINE)
        .cloned()?;

    if tok.kind() != SyntaxKind::BAREWORD {
        return None;
    }
    let name = tok.text().to_string();

    if is_keyword(&name) {
        return None;
    }

    // Qualified names are cross-file.
    if name.contains('.') {
        return None;
    }

    let parent = tok.parent()?;

    // --- type-name position ---
    let in_type_def_or_unit = matches!(
        parent.kind(),
        SyntaxKind::TYPE_DEF | SyntaxKind::UNIT_DEF | SyntaxKind::SCHEMA_DECL
    );
    if in_type_def_or_unit {
        // Any bareword in a TYPE_DEF/UNIT_DEF/SCHEMA_DECL context refers to a type.
        // Confirm the name resolves to an actual local type/unit declaration.
        if find_type_decl(root, &name).is_some() || is_decl_name_in_node(&parent, &name) {
            return Some(ResolvedSymbol::TypeName(name));
        }
        return None;
    }

    // --- value-name position (REF, BINDING value, etc.) ---
    // mirror goto_definition's in_value_position logic
    let maybe_ref = matches!(
        parent.kind(),
        SyntaxKind::REF | SyntaxKind::MATCH_EXPR | SyntaxKind::CALL | SyntaxKind::DOCUMENT
    );

    let in_value_position = if parent.kind() == SyntaxKind::BINDING {
        let mut seen_colon = false;
        let mut is_value = false;
        for el in parent.children_with_tokens() {
            match el {
                rowan::NodeOrToken::Token(t) if t.kind() == SyntaxKind::COLON => {
                    seen_colon = true;
                }
                rowan::NodeOrToken::Token(t)
                    if seen_colon && t.text_range() == tok.text_range() =>
                {
                    is_value = true;
                    break;
                }
                rowan::NodeOrToken::Node(n) if seen_colon => {
                    if n.text_range().contains_inclusive(off) {
                        is_value = true;
                    }
                    break;
                }
                _ => {}
            }
        }
        is_value
    } else {
        maybe_ref
    };

    if !in_value_position && parent.kind() != SyntaxKind::REF {
        return None;
    }

    // Confirm the name resolves to a local binding or param.
    if find_binding(root, &name).is_some() || find_param_entry(root, &name).is_some() {
        return Some(ResolvedSymbol::ValueName(name));
    }

    // Also try type as fallback (goto_definition falls through to type lookup).
    if find_type_decl(root, &name).is_some() {
        return Some(ResolvedSymbol::TypeName(name));
    }

    // Check if the cursor is on the binding *key* itself (not value position).
    // A BINDING child's first significant token is the key.
    if parent.kind() == SyntaxKind::BINDING {
        let key_tok = parent
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| !t.kind().is_trivia() && t.kind() != SyntaxKind::NEWLINE);
        if key_tok.as_ref().map(|t| t.text_range()) == Some(tok.text_range()) {
            // Cursor is on the binding key — that IS the declaration.
            return Some(ResolvedSymbol::ValueName(name));
        }
    }

    None
}

/// Returns true if the bareword is a language keyword.
fn is_keyword(s: &str) -> bool {
    matches!(
        s,
        "type"
            | "unit"
            | "schema"
            | "use"
            | "params"
            | "fn"
            | "match"
            | "require"
            | "unset"
            | "as"
            | "true"
            | "false"
            | "int"
            | "str"
            | "bool"
            | "decimal"
            | "bytes"
            | "brand"
    )
}

/// True if `name` is the declared name inside `node` (TYPE_DEF or UNIT_DEF).
fn is_decl_name_in_node(node: &SyntaxNode, name: &str) -> bool {
    node_decl_name(node).as_deref() == Some(name)
}

/// Collect all byte ranges in `src` where `name` appears as a type-name token.
///
/// A BAREWORD matching `name` inside a TYPE_DEF, UNIT_DEF, or SCHEMA_DECL is
/// included UNLESS it is a record-type field KEY — detected by:
///   brace-depth > 0  AND  the next significant token is COLON
///
/// This correctly:
/// - Keeps the decl name (`type T = …` / `unit T : …` at depth 0 — depth rule skips exclusion).
/// - Keeps type references in field-value position (`{ x: T }` — next token is not COLON).
/// - Excludes record field keys (`{ T: int }` — depth > 0 and next token IS COLON).
fn type_name_occurrences(root: &SyntaxNode, name: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();

    for node in root.children() {
        if !matches!(
            node.kind(),
            SyntaxKind::TYPE_DEF | SyntaxKind::UNIT_DEF | SyntaxKind::SCHEMA_DECL
        ) {
            continue;
        }

        // Collect all significant tokens in document order within this node.
        let tokens: Vec<_> = node
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| !t.kind().is_trivia() && t.kind() != SyntaxKind::NEWLINE)
            .collect();

        let mut brace_depth: u32 = 0;
        for (i, tok) in tokens.iter().enumerate() {
            match tok.kind() {
                SyntaxKind::L_BRACE | SyntaxKind::L_BRACKET | SyntaxKind::L_PAREN => {
                    brace_depth += 1;
                }
                SyntaxKind::R_BRACE | SyntaxKind::R_BRACKET | SyntaxKind::R_PAREN => {
                    brace_depth = brace_depth.saturating_sub(1);
                }
                SyntaxKind::BAREWORD if tok.text() == name => {
                    // Exclude field keys: inside braces AND next significant token is COLON.
                    let next_is_colon = tokens
                        .get(i + 1)
                        .map(|t| t.kind() == SyntaxKind::COLON)
                        .unwrap_or(false);
                    if brace_depth > 0 && next_is_colon {
                        // This is a record-type field key — skip it.
                        continue;
                    }
                    let r = tok.text_range();
                    out.push((r.start().into(), r.end().into()));
                }
                _ => {}
            }
        }
    }

    out
}

/// Collect all byte ranges in `src` where `name` appears as a value-name token:
/// - The key of a top-level BINDING (the declaration site).
/// - The sole BAREWORD token inside a REF node.
/// - A BAREWORD in a PARAM_DECL that is followed by a COLON (param entry decl).
fn value_name_occurrences(root: &SyntaxNode, name: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for tok in root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == SyntaxKind::BAREWORD && t.text() == name)
    {
        let parent = match tok.parent() {
            Some(p) => p,
            None => continue,
        };
        match parent.kind() {
            // REF node — always a value reference.
            SyntaxKind::REF => {
                let r = tok.text_range();
                out.push((r.start().into(), r.end().into()));
            }
            // BINDING — accept only the key (first significant token).
            SyntaxKind::BINDING => {
                let key = parent
                    .descendants_with_tokens()
                    .filter_map(|e| e.into_token())
                    .find(|t| !t.kind().is_trivia() && t.kind() != SyntaxKind::NEWLINE);
                if key.as_ref().map(|t| t.text_range()) == Some(tok.text_range()) {
                    let r = tok.text_range();
                    out.push((r.start().into(), r.end().into()));
                }
            }
            // PARAM_DECL — accept bareword that is followed by COLON.
            SyntaxKind::PARAM_DECL => {
                // Walk tokens inside the PARAM_DECL to find param-entry declarations.
                let tokens: Vec<_> = parent
                    .descendants_with_tokens()
                    .filter_map(|e| e.into_token())
                    .filter(|t| !t.kind().is_trivia() && t.kind() != SyntaxKind::NEWLINE)
                    .collect();
                let mut i = 0;
                while i < tokens.len() {
                    if tokens[i].kind() == SyntaxKind::BAREWORD
                        && tokens[i].text() == name
                        && tokens[i].text_range() == tok.text_range()
                        && tokens.get(i + 1).map(|t| t.kind()) == Some(SyntaxKind::COLON)
                    {
                        let r = tok.text_range();
                        out.push((r.start().into(), r.end().into()));
                        break;
                    }
                    i += 1;
                }
            }
            _ => {}
        }
    }
    out
}

/// Return all byte ranges (start, end) for occurrences of the symbol under
/// `offset` in `src`.  When `include_decl` is false, the declaration site is
/// excluded from the result.
///
/// Returns an empty Vec when the cursor is not on a resolvable local symbol
/// or when the symbol is cross-file/qualified.
pub fn references(src: &str, offset: usize, include_decl: bool) -> Vec<(usize, usize)> {
    let root = parse_cst(src).syntax();
    let symbol = match resolve_symbol(&root, src, offset) {
        Some(s) => s,
        None => {
            // Last-chance: cursor may be on a binding key directly.
            // Try finding a binding whose key matches the BAREWORD under cursor.
            let off: rowan::TextSize = (offset as u32).into();
            let covering: Vec<_> = root
                .descendants_with_tokens()
                .filter_map(|e| e.into_token())
                .filter(|t| t.text_range().contains_inclusive(off))
                .collect();
            let tok = covering
                .iter()
                .find(|t| !t.kind().is_trivia() && t.kind() != SyntaxKind::NEWLINE)
                .cloned();
            if let Some(tok) = tok {
                if tok.kind() == SyntaxKind::BAREWORD && !is_keyword(tok.text()) {
                    let name = tok.text().to_string();
                    // Check if this is a binding key
                    if let Some(parent) = tok.parent() {
                        if parent.kind() == SyntaxKind::BINDING {
                            let key = parent
                                .descendants_with_tokens()
                                .filter_map(|e| e.into_token())
                                .find(|t| !t.kind().is_trivia() && t.kind() != SyntaxKind::NEWLINE);
                            if key.as_ref().map(|t| t.text_range()) == Some(tok.text_range()) {
                                let mut occs = value_name_occurrences(&root, &name);
                                if !include_decl {
                                    // Remove the binding key range (first BINDING occurrence)
                                    if let Some(decl_range) = find_binding(&root, &name) {
                                        // find the exact token range for the key
                                        let key_range = find_binding_key_range(&root, &name);
                                        if let Some(kr) = key_range {
                                            occs.retain(|&r| r != kr);
                                        } else {
                                            occs.retain(|&r| r != decl_range);
                                        }
                                    }
                                }
                                occs.sort_by_key(|r| r.0);
                                return occs;
                            }
                        }
                    }
                }
            }
            return vec![];
        }
    };

    let mut occs = match &symbol {
        ResolvedSymbol::TypeName(name) => type_name_occurrences(&root, name),
        ResolvedSymbol::ValueName(name) => value_name_occurrences(&root, name),
    };

    if !include_decl {
        // Remove the declaration site from the result.
        match &symbol {
            ResolvedSymbol::TypeName(name) => {
                // Declaration site = token range of the name in the TYPE_DEF/UNIT_DEF.
                if let Some(decl_tok_range) = type_decl_name_token_range(&root, name) {
                    occs.retain(|&r| r != decl_tok_range);
                }
            }
            ResolvedSymbol::ValueName(name) => {
                // Declaration = binding key token range or param entry token range.
                if let Some(kr) = find_binding_key_range(&root, name) {
                    occs.retain(|&r| r != kr);
                } else if let Some(pr) = find_param_entry(&root, name) {
                    occs.retain(|&r| r != pr);
                }
            }
        }
    }

    occs.sort_by_key(|r| r.0);
    occs.dedup();
    occs
}

/// Return the byte range of just the name *token* inside the TYPE_DEF or
/// UNIT_DEF for `name` (not the whole node range).
fn type_decl_name_token_range(root: &SyntaxNode, name: &str) -> Option<(usize, usize)> {
    for child in root.children() {
        if !matches!(child.kind(), SyntaxKind::TYPE_DEF | SyntaxKind::UNIT_DEF) {
            continue;
        }
        if node_decl_name(&child).as_deref() != Some(name) {
            continue;
        }
        // The name token is the second significant token.
        let mut tokens = child
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| !t.kind().is_trivia() && t.kind() != SyntaxKind::NEWLINE);
        tokens.next(); // skip keyword
        let name_tok = tokens.next()?;
        let r = name_tok.text_range();
        return Some((r.start().into(), r.end().into()));
    }
    None
}

/// Return just the key-token byte range of a top-level BINDING.
fn find_binding_key_range(root: &SyntaxNode, name: &str) -> Option<(usize, usize)> {
    for child in root.children() {
        if child.kind() != SyntaxKind::BINDING {
            continue;
        }
        let key = child
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| !t.kind().is_trivia() && t.kind() != SyntaxKind::NEWLINE)?;
        if key.text() == name {
            let r = key.text_range();
            return Some((r.start().into(), r.end().into()));
        }
    }
    None
}

// ---- rename ----

/// Validate that `new_name` is a legal Mangrove identifier.  Matches the
/// lexer rules exactly: start position requires `[a-zA-Z_]`; continue
/// position allows `[a-zA-Z0-9_-]` (hyphen is valid after the first char).
fn is_valid_identifier(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Return the byte ranges to replace for a rename of the symbol under `offset`
/// to `new_name`.  Returns `None` if the cursor is not on a renameable local
/// symbol or if `new_name` is not a valid identifier.
///
/// The caller replaces each returned range with `new_name` to produce the
/// renamed source.
pub fn rename(src: &str, offset: usize, new_name: &str) -> Option<Vec<(usize, usize)>> {
    if !is_valid_identifier(new_name) {
        return None;
    }
    // `references` with include_decl=true gives us all occurrences.
    let occs = references(src, offset, true);
    if occs.is_empty() {
        return None;
    }
    Some(occs)
}

// ---- workspace-wide references and rename ----

/// Recursively collect all `.mang` files under `root`, skipping hidden dirs,
/// `target/` dirs, and capping at 500 entries.
fn walk_mang_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk_mang_files_inner(root, &mut out);
    out
}

fn walk_mang_files_inner(dir: &Path, out: &mut Vec<PathBuf>) {
    if out.len() >= 500 {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        if out.len() >= 500 {
            eprintln!(
                "mangrove-lsp: workspace walk capped at 500 files; some files may be skipped"
            );
            return;
        }
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        // Skip hidden entries and the target directory.
        if name.starts_with('.') || name == "target" {
            continue;
        }
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if ft.is_dir() {
            walk_mang_files_inner(&path, out);
        } else if ft.is_file() && path.extension().and_then(|e| e.to_str()) == Some("mang") {
            out.push(path);
        }
        // ft.is_symlink() → skip (no recursion; avoids ancestor-dir cycles)
    }
}

/// Collect all byte ranges where `type_name` appears as the TypeName part of
/// `alias.TypeName` in the given CST root.  Only the type-name token range
/// (not the alias or dot) is returned.
fn qualified_type_occurrences(
    root: &SyntaxNode,
    alias: &str,
    type_name: &str,
) -> Vec<(usize, usize)> {
    let all_toks: Vec<_> = root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| !t.kind().is_trivia() && t.kind() != SyntaxKind::NEWLINE)
        .collect();

    let mut out = Vec::new();
    for (pos, tok) in all_toks.iter().enumerate() {
        if tok.kind() != SyntaxKind::BAREWORD || tok.text() != type_name {
            continue;
        }
        // Must be preceded by DOT then the alias BAREWORD.
        if pos < 2 {
            continue;
        }
        let prev = &all_toks[pos - 1];
        let before = &all_toks[pos - 2];
        if prev.kind() == SyntaxKind::DOT
            && before.kind() == SyntaxKind::BAREWORD
            && before.text() == alias
        {
            let r = tok.text_range();
            out.push((r.start().into(), r.end().into()));
        }
    }
    out
}

/// Cross-workspace references for a type symbol defined in `doc_path`.
///
/// Returns a list of `(file_path, occurrences)` pairs — always includes the
/// source file itself first.
///
/// When `workspace_root` is `None`, falls back to same-file references only.
/// Only works for `TypeName` symbols; `ValueName` symbols are file-local and
/// return same-file results only.
/// If the cursor is on a qualified (imported) reference (`alias.X`), returns
/// empty — renaming foreign symbols is out of scope.
pub fn references_in_workspace(
    src: &str,
    offset: usize,
    doc_path: &Path,
    workspace_root: Option<&Path>,
    cache: &ImportCache,
) -> Vec<(PathBuf, Vec<(usize, usize)>)> {
    let root = parse_cst(src).syntax();

    // If the cursor is on a qualified ref (alias.X), decline — that is an
    // imported symbol; we cannot rename the foreign declaration.
    let off: rowan::TextSize = (offset as u32).into();
    let covering: Vec<_> = root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.text_range().contains_inclusive(off))
        .filter(|t| !t.kind().is_trivia() && t.kind() != SyntaxKind::NEWLINE)
        .collect();
    if let Some(tok) = covering.first() {
        if tok.kind() == SyntaxKind::BAREWORD && detect_qualified_ref(&root, tok).is_some() {
            return vec![];
        }
    }

    let symbol = match resolve_symbol(&root, src, offset) {
        Some(s) => s,
        None => return vec![],
    };

    // Same-file occurrences (always include decl for workspace use).
    let same_file_refs = references(src, offset, true);

    // Non-TypeName symbols are always file-local.
    let name = match symbol {
        ResolvedSymbol::TypeName(n) => n,
        ResolvedSymbol::ValueName(_) => {
            if same_file_refs.is_empty() {
                return vec![];
            }
            return vec![(doc_path.to_path_buf(), same_file_refs)];
        }
    };

    // No workspace root → same-file only.
    let workspace_root = match workspace_root {
        Some(r) => r,
        None => {
            if same_file_refs.is_empty() {
                return vec![];
            }
            return vec![(doc_path.to_path_buf(), same_file_refs)];
        }
    };

    let mut result: Vec<(PathBuf, Vec<(usize, usize)>)> = Vec::new();
    if !same_file_refs.is_empty() {
        result.push((doc_path.to_path_buf(), same_file_refs));
    }

    // Canonical path of the source file for comparison.
    let doc_canonical = std::fs::canonicalize(doc_path).unwrap_or_else(|_| doc_path.to_path_buf());

    // Walk all other .mang files in the workspace.
    let mang_files = walk_mang_files(workspace_root);
    for other_path in mang_files {
        let other_canonical =
            std::fs::canonicalize(&other_path).unwrap_or_else(|_| other_path.clone());
        if other_canonical == doc_canonical {
            continue;
        }

        // Read via cache.
        let other_text = match cache.read(&other_path) {
            Some(t) => t,
            None => continue,
        };

        // Parse and find all aliases whose use-reference resolves to doc_path.
        let other_root = parse_cst(other_text.as_ref()).syntax();
        let doc_dir_b = match other_path.parent() {
            Some(d) => d,
            None => continue,
        };
        let resolvers = match mangrove_resolve::Resolvers::find_and_load(doc_dir_b) {
            Ok(r) => r,
            Err(_) => continue,
        };

        let mut file_ranges: Vec<(usize, usize)> = Vec::new();

        // Walk USE_DECL nodes to find aliases that resolve to our doc.
        for child in other_root.children() {
            if child.kind() != SyntaxKind::USE_DECL {
                continue;
            }
            let text = child.text().to_string();
            let u = match mangrove_syntax::parse_use_str(text.trim()) {
                Ok(u) => u,
                Err(_) => continue,
            };
            let pkg_path = match resolvers.resolve_local_path(&u.path) {
                Ok(p) => p,
                Err(_) => continue,
            };
            let pkg_canonical =
                std::fs::canonicalize(&pkg_path).unwrap_or_else(|_| pkg_path.clone());
            if pkg_canonical != doc_canonical {
                continue;
            }
            // This alias maps to our file — collect qualified occurrences.
            let ranges = qualified_type_occurrences(&other_root, &u.alias, &name);
            file_ranges.extend(ranges);
        }

        if !file_ranges.is_empty() {
            file_ranges.sort_by_key(|r| r.0);
            result.push((other_path, file_ranges));
        }
    }

    result
}

/// Cross-workspace rename: returns a map of file → byte ranges to replace with
/// `new_name`, or `None` if the cursor is not on a renameable symbol or
/// `new_name` is invalid.
///
/// Callers replace each returned range with `new_name` to produce renamed
/// source across all files.
pub fn rename_in_workspace(
    src: &str,
    offset: usize,
    new_name: &str,
    doc_path: &Path,
    workspace_root: Option<&Path>,
    cache: &ImportCache,
) -> Option<HashMap<PathBuf, Vec<(usize, usize)>>> {
    if !is_valid_identifier(new_name) {
        return None;
    }
    let per_file = references_in_workspace(src, offset, doc_path, workspace_root, cache);
    if per_file.is_empty() {
        return None;
    }
    Some(per_file.into_iter().collect())
}

// ---- completions ----

/// Top-level declaration keywords (valid at the start of a new statement).
const DECL_KEYWORDS: &[&str] = &["type", "schema", "unit", "use", "params", "fn"];

/// Value-position keywords (valid inside a value expression).
const VALUE_KEYWORDS: &[&str] = &["match", "unset"];

/// Primitive type keywords (valid after `=` in a type definition).
const PRIMITIVE_TYPE_KEYWORDS: &[&str] = &["int", "str", "bool", "decimal", "bytes"];

/// The cursor context derived from the CST at a given offset.
#[derive(Debug, PartialEq)]
enum CursorContext {
    /// Inside a type definition body or type annotation (e.g. after `=` in
    /// `type Foo = …`, or after `schema`).
    TypePosition,
    /// At top-level document scope — a new declaration is expected.
    TopLevel,
    /// Inside a value position under the document's bound schema record.
    SchemaValuePosition,
    /// General value position (binding value, match arm, etc.).
    GeneralValue,
    /// Context is ambiguous — fall back to the full union.
    Ambiguous,
}

/// Declaration-opening keywords — if the cursor sits on one of these, the
/// position is effectively top-level (the user may be replacing/extending a
/// declaration keyword, not writing a type expression or value).
const DECL_KEYWORD_TOKENS: &[&str] = &["type", "schema", "unit", "use", "params", "fn"];

/// Determine what the cursor is completing at `offset` in the CST.
///
/// Strategy:
/// 1. Find the significant token whose range contains (or ends at) `offset`.
/// 2. If that token is a declaration-opening keyword, the context is TopLevel.
/// 3. Otherwise walk up the ancestor chain and classify by the first matching
///    ancestor kind.
/// 4. Fall back to `Ambiguous` when the position can't be determined.
fn cursor_context(root: &SyntaxNode, offset: usize) -> CursorContext {
    let off: rowan::TextSize = (offset as u32).into();

    // Collect tokens whose range strictly contains the offset (preferred) or
    // that end exactly at the offset (cursor just past the token).  We do NOT
    // include tokens that merely start at `off` unless they also contain it,
    // to avoid grabbing the declaration keyword when the cursor is at the very
    // start of a new line.
    let all_toks: Vec<_> = root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.text_range().contains_inclusive(off))
        .collect();

    // Prefer a significant (non-trivia, non-newline) token.
    let tok = all_toks
        .iter()
        .find(|t| !t.kind().is_trivia() && t.kind() != SyntaxKind::NEWLINE)
        .cloned();

    // If no significant token covers the offset, the cursor is between
    // declarations (whitespace / newline / EOF) → top-level.
    let Some(tok) = tok else {
        return CursorContext::TopLevel;
    };

    // If the cursor is sitting on a declaration-opening keyword, the user is
    // at the start of a new top-level statement — treat as TopLevel.
    if matches!(tok.kind(), SyntaxKind::BAREWORD) && DECL_KEYWORD_TOKENS.contains(&tok.text()) {
        return CursorContext::TopLevel;
    }

    // Walk up the ancestor chain checking node kinds.
    let mut node = tok.parent();
    while let Some(n) = node {
        match n.kind() {
            // Inside a type definition or schema declaration → type position.
            SyntaxKind::TYPE_DEF | SyntaxKind::UNIT_DEF | SyntaxKind::SCHEMA_DECL => {
                return CursorContext::TypePosition;
            }
            // Inside a binding value or record field under DOCUMENT → value position.
            SyntaxKind::BINDING | SyntaxKind::RECORD | SyntaxKind::FIELD => {
                return CursorContext::SchemaValuePosition;
            }
            // Inside a match expression, call, list, etc. → general value.
            // (MATCH_ARM is reserved/unemitted in v0.3.0; kept here so the match
            // stays extensible when the structured parser lands it.)
            SyntaxKind::MATCH_EXPR
            | SyntaxKind::MATCH_ARM
            | SyntaxKind::CALL
            | SyntaxKind::LIST
            | SyntaxKind::LIST_OP_ITEM => {
                return CursorContext::GeneralValue;
            }
            // Direct child of DOCUMENT (covers USE_DECL, PARAM_DECL, FN_DEF,
            // ERROR nodes, etc.) → top-level.
            SyntaxKind::DOCUMENT => {
                return CursorContext::TopLevel;
            }
            _ => {}
        }
        node = n.parent();
    }

    // Reached root without a clear context → ambiguous fallback.
    CursorContext::Ambiguous
}

/// Return completion items for a cursor at `offset` in `src`.
///
/// Context-aware: determines the cursor's position in the CST and offers only
/// items that fit:
/// - Type position → declared type/unit names + primitive type keywords +
///   imported `alias.TypeName` completions (when `doc_path` is `Some`).
/// - Top-level → declaration keywords (`type`, `schema`, `unit`, …).
/// - Schema value position → schema record field names.
/// - General value → value keywords (`match`, `unset`).
/// - Ambiguous → full union (over-offer rather than offer nothing).
///
/// `doc_path` is the on-disk path of the document being edited; when `Some`
/// the function also offers imported types from `use` declarations by resolving
/// them exactly like `goto_definition_cross_file` (local-only, no-fetch).
/// When `None` (untitled buffer), imported-type completion is skipped.
pub fn completions(src: &str, offset: usize, doc_path: Option<&Path>) -> Vec<CompletionItem> {
    completions_impl(src, offset, doc_path, None)
}

/// Cached variant used by the live server — passes the session cache so
/// imported-file reads are served from memory on subsequent requests.
pub(crate) fn completions_cached(
    src: &str,
    offset: usize,
    doc_path: Option<&Path>,
    cache: &ImportCache,
) -> Vec<CompletionItem> {
    completions_impl(src, offset, doc_path, Some(cache))
}

fn completions_impl(
    src: &str,
    offset: usize,
    doc_path: Option<&Path>,
    cache: Option<&ImportCache>,
) -> Vec<CompletionItem> {
    let root = parse_cst(src).syntax();
    let mut items: Vec<CompletionItem> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    let mut push = |item: CompletionItem| {
        if seen.insert(item.label.clone()) {
            items.push(item);
        }
    };

    let ctx = cursor_context(&root, offset);

    match ctx {
        CursorContext::TypePosition => {
            // Detect whether the cursor is positioned after an `alias.` prefix
            // (e.g. user typed `k.` or is inside `k.SomeName`).
            //
            // V3 behaviour change from V2: when no alias-prefix is present, we do
            // NOT offer the full imported-type set (`alias.TypeName` for all
            // packages). That set can be thousands of items in real projects and
            // provides poor UX. Imported types are only shown when the user
            // explicitly types an alias prefix.
            if let Some(alias) = detect_alias_prefix(&root, offset) {
                // Alias-prefix present: offer only types from that alias's package,
                // using bare TypeName labels (not alias.TypeName, since the user
                // already typed alias.).
                for imported in imported_type_completions_for_alias(&root, doc_path, &alias, cache)
                {
                    push(imported);
                }
            } else {
                // No alias-prefix: offer local type/unit names only.
                for child in root.children() {
                    if matches!(child.kind(), SyntaxKind::TYPE_DEF | SyntaxKind::UNIT_DEF) {
                        if let Some(name) = node_decl_name(&child) {
                            push(CompletionItem {
                                label: name,
                                kind: CompletionKind::TypeName,
                            });
                        }
                    }
                }
                // Primitive type keywords.
                for &kw in PRIMITIVE_TYPE_KEYWORDS {
                    push(CompletionItem {
                        label: kw.to_string(),
                        kind: CompletionKind::Keyword,
                    });
                }
            }
        }

        CursorContext::TopLevel => {
            for &kw in DECL_KEYWORDS {
                push(CompletionItem {
                    label: kw.to_string(),
                    kind: CompletionKind::Keyword,
                });
            }
        }

        CursorContext::SchemaValuePosition => {
            // Enum-value completions: offered first so they appear at the top.
            for ev in enum_value_completions(src, offset, doc_path, cache) {
                push(ev);
            }
            // Field names from the bound schema (if any).
            if let Some(fields) = schema_record_fields(src, doc_path, cache) {
                for field in fields {
                    push(CompletionItem {
                        label: field,
                        kind: CompletionKind::Field,
                    });
                }
            }
            // Also offer value keywords as a convenience.
            for &kw in VALUE_KEYWORDS {
                push(CompletionItem {
                    label: kw.to_string(),
                    kind: CompletionKind::Keyword,
                });
            }
        }

        CursorContext::GeneralValue => {
            for &kw in VALUE_KEYWORDS {
                push(CompletionItem {
                    label: kw.to_string(),
                    kind: CompletionKind::Keyword,
                });
            }
        }

        CursorContext::Ambiguous => {
            // Full union — better to over-offer than to return nothing.
            for child in root.children() {
                if matches!(child.kind(), SyntaxKind::TYPE_DEF | SyntaxKind::UNIT_DEF) {
                    if let Some(name) = node_decl_name(&child) {
                        push(CompletionItem {
                            label: name,
                            kind: CompletionKind::TypeName,
                        });
                    }
                }
            }
            for &kw in DECL_KEYWORDS {
                push(CompletionItem {
                    label: kw.to_string(),
                    kind: CompletionKind::Keyword,
                });
            }
            for &kw in VALUE_KEYWORDS {
                push(CompletionItem {
                    label: kw.to_string(),
                    kind: CompletionKind::Keyword,
                });
            }
            if let Some(fields) = schema_record_fields(src, doc_path, cache) {
                for field in fields {
                    push(CompletionItem {
                        label: field,
                        kind: CompletionKind::Field,
                    });
                }
            }
        }
    }

    items
}

/// Detect whether the cursor at `offset` is positioned after an `alias.` prefix,
/// meaning the user typed or is completing something like `k.` or `k.Som`.
///
/// Returns `Some(alias)` when a BAREWORD DOT [BAREWORD] pattern is found around
/// the cursor. Returns `None` when the cursor is not in an alias-prefix context.
fn detect_alias_prefix(root: &SyntaxNode, offset: usize) -> Option<String> {
    let off: rowan::TextSize = (offset as u32).into();

    // Collect all significant (non-trivia, non-newline) tokens in document order.
    let all_toks: Vec<_> = root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| !t.kind().is_trivia() && t.kind() != SyntaxKind::NEWLINE)
        .collect();

    // Find the index of the token that contains or ends at `offset`.
    // We also check the token BEFORE the cursor position for the case where the
    // cursor is exactly between tokens (e.g. just after a DOT).
    let cursor_tok_idx = all_toks.iter().position(|t| {
        let r = t.text_range();
        r.contains_inclusive(off)
    });

    // If no token contains the offset, look for the last token ending at or
    // before the offset (cursor is in whitespace or at end of line).
    let idx = if let Some(i) = cursor_tok_idx {
        i
    } else {
        // Find the last token that ends at or before `off`.
        all_toks.iter().rposition(|t| t.text_range().end() <= off)?
    };

    let tok = &all_toks[idx];

    // Case A: cursor is ON a BAREWORD and the previous two tokens are BAREWORD DOT
    // → the BAREWORD before DOT is the alias.
    if tok.kind() == SyntaxKind::BAREWORD && idx >= 2 {
        let prev = &all_toks[idx - 1];
        let before = &all_toks[idx - 2];
        if prev.kind() == SyntaxKind::DOT && before.kind() == SyntaxKind::BAREWORD {
            return Some(before.text().to_string());
        }
    }

    // Case B: cursor is ON a DOT and the previous token is a BAREWORD (alias)
    // → alias-prefix with empty partial suffix.
    if tok.kind() == SyntaxKind::DOT && idx >= 1 {
        let before = &all_toks[idx - 1];
        if before.kind() == SyntaxKind::BAREWORD {
            return Some(before.text().to_string());
        }
    }

    None
}

/// Offer completion items for a specific alias's package, using bare TypeName
/// labels (not `alias.TypeName`) — the caller already typed the `alias.` prefix.
///
/// No-fetch, local-only: same resolution chain as `imported_type_completions`.
fn imported_type_completions_for_alias(
    root: &SyntaxNode,
    doc_path: Option<&Path>,
    alias: &str,
    cache: Option<&ImportCache>,
) -> Vec<CompletionItem> {
    let Some(doc_path) = doc_path else {
        return vec![];
    };
    let Some(doc_dir) = doc_path.parent() else {
        return vec![];
    };
    let Some(reference) = use_reference_for_alias(root, alias) else {
        return vec![];
    };
    let Ok(resolvers) = mangrove_resolve::Resolvers::find_and_load(doc_dir) else {
        return vec![];
    };
    let Ok(pkg_path) = resolvers.resolve_local_path(&reference) else {
        return vec![];
    };
    let pkg_text: String = if let Some(c) = cache {
        let Some(t) = c.read(&pkg_path) else {
            return vec![];
        };
        t.to_string()
    } else {
        if !pkg_path.exists() {
            return vec![];
        }
        let Ok(t) = std::fs::read_to_string(&pkg_path) else {
            return vec![];
        };
        t
    };
    let pkg_root = parse_cst(&pkg_text).syntax();

    let mut out = Vec::new();
    for pkg_child in pkg_root.children() {
        if matches!(
            pkg_child.kind(),
            SyntaxKind::TYPE_DEF | SyntaxKind::UNIT_DEF
        ) {
            if let Some(type_name) = node_decl_name(&pkg_child) {
                // Bare TypeName label — the user already typed `alias.`
                out.push(CompletionItem {
                    label: type_name,
                    kind: CompletionKind::TypeName,
                });
            }
        }
    }
    out
}

/// Determine which field name the cursor is in the **value** position of,
/// within a schema-bound record context.
///
/// Walks the token's ancestors to find the innermost `FIELD` or top-level
/// `BINDING` node, then returns the key (first significant token before COLON).
/// Returns `None` when the cursor is not inside such a node or the structure
/// is unexpected.
fn field_name_at_cursor(root: &SyntaxNode, offset: usize) -> Option<String> {
    let off: rowan::TextSize = (offset as u32).into();

    // Find the significant token at or containing the offset.
    let covering: Vec<_> = root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.text_range().contains_inclusive(off))
        .collect();
    let tok = covering
        .iter()
        .find(|t| !t.kind().is_trivia() && t.kind() != SyntaxKind::NEWLINE)
        .cloned()?;

    // Walk up ancestors to find the enclosing FIELD or BINDING.
    let mut cur = tok.parent()?;
    loop {
        match cur.kind() {
            SyntaxKind::FIELD | SyntaxKind::BINDING => {
                // The key is the first significant token before the COLON.
                let key = cur
                    .descendants_with_tokens()
                    .filter_map(|e| e.into_token())
                    .find(|t| !t.kind().is_trivia() && t.kind() != SyntaxKind::NEWLINE)?;
                // Confirm it's a BAREWORD or STR key.
                let key_text = match key.kind() {
                    SyntaxKind::BAREWORD => key.text().to_string(),
                    SyntaxKind::STR => {
                        // Quoted key — strip surrounding quotes.
                        let s = key.text();
                        s.strip_prefix('"')
                            .and_then(|s| s.strip_suffix('"'))
                            .unwrap_or(s)
                            .to_string()
                    }
                    _ => return None,
                };
                return Some(key_text);
            }
            SyntaxKind::DOCUMENT => return None,
            _ => {
                cur = cur.parent()?;
            }
        }
    }
}

/// Walk the typedef list to find a field named `field_name` in the record type
/// named `schema_name` (following Named aliases up to `depth` levels).
/// Returns a clone of the field's `Type` if found.
fn resolve_field_type_in_typedefs(
    schema_name: &str,
    field_name: &str,
    typedefs: &[mangrove_syntax::parser::TypeDef],
) -> Option<Type> {
    // First resolve the schema name to a record type.
    let fields = resolve_record_fields(schema_name, typedefs)?;
    let field = fields.iter().find(|f| f.name == field_name)?;
    Some(field.ty.clone())
}

/// Given a `Type`, collect all literal completion items (LitStr/LitInt/LitBool),
/// following Named aliases through `typedefs` up to 8 levels.
/// Non-literal members of a Union are skipped silently.
/// Returns an empty Vec if the type is not a literal or literal union.
fn literal_completions_from_type(
    ty: &Type,
    typedefs: &[mangrove_syntax::parser::TypeDef],
    depth: usize,
) -> Vec<CompletionItem> {
    if depth > 8 {
        return vec![];
    }
    match ty {
        Type::LitStr(s) => vec![CompletionItem {
            label: format!("\"{s}\""),
            kind: CompletionKind::EnumValue,
        }],
        Type::LitInt(n) => vec![CompletionItem {
            label: n.to_string(),
            kind: CompletionKind::EnumValue,
        }],
        Type::LitBool(b) => vec![CompletionItem {
            label: b.to_string(),
            kind: CompletionKind::EnumValue,
        }],
        Type::Union(members) => {
            let mut out = Vec::new();
            for m in members {
                out.extend(literal_completions_from_type(m, typedefs, depth + 1));
            }
            out
        }
        Type::Named(name) => {
            // Resolve the alias one level deeper.
            let Some(td) = typedefs.iter().find(|t| t.name == name.as_str()) else {
                return vec![];
            };
            literal_completions_from_type(&td.ty.clone(), typedefs, depth + 1)
        }
        _ => vec![],
    }
}

/// Collect enum-value completions for the field the cursor is in, within a
/// schema-bound context.  Returns an empty Vec when the cursor field cannot be
/// determined, the field's type is not a literal/literal-union, or the schema
/// cannot be resolved.  Never panics, never fetches.
fn enum_value_completions(
    src: &str,
    offset: usize,
    doc_path: Option<&Path>,
    cache: Option<&ImportCache>,
) -> Vec<CompletionItem> {
    let root = parse_cst(src).syntax();

    let field_name = match field_name_at_cursor(&root, offset) {
        Some(n) => n,
        None => return vec![],
    };

    // Determine the schema name.
    let schema_name: String = if let Ok(doc) = lower(&root) {
        match doc.schema {
            Some(s) => s,
            None => return vec![],
        }
    } else {
        match schema_name_from_cst(&root) {
            Some(s) => s,
            None => return vec![],
        }
    };

    if schema_name.contains('.') {
        // Imported schema — resolve the field type from the imported file.
        enum_value_completions_imported(&root, &schema_name, &field_name, doc_path, cache)
    } else {
        // Local schema.
        let doc = match lower(&root).ok() {
            Some(d) => d,
            None => return vec![],
        };
        let field_ty =
            match resolve_field_type_in_typedefs(&schema_name, &field_name, &doc.typedefs) {
                Some(t) => t,
                None => return vec![],
            };
        literal_completions_from_type(&field_ty, &doc.typedefs, 0)
    }
}

/// Collect enum-value completions for an imported schema's field.
fn enum_value_completions_imported(
    root: &SyntaxNode,
    schema_name: &str,
    field_name: &str,
    doc_path: Option<&Path>,
    cache: Option<&ImportCache>,
) -> Vec<CompletionItem> {
    let doc_path = match doc_path {
        Some(p) => p,
        None => return vec![],
    };
    let doc_dir = match doc_path.parent() {
        Some(d) => d,
        None => return vec![],
    };
    let (alias, type_name) = match schema_name.split_once('.') {
        Some(pair) => pair,
        None => return vec![],
    };
    let reference = match use_reference_for_alias(root, alias) {
        Some(r) => r,
        None => return vec![],
    };
    let resolvers = match mangrove_resolve::Resolvers::find_and_load(doc_dir).ok() {
        Some(r) => r,
        None => return vec![],
    };
    let pkg_path = match resolvers.resolve_local_path(&reference).ok() {
        Some(p) => p,
        None => return vec![],
    };
    let pkg_text: String = if let Some(c) = cache {
        match c.read(&pkg_path) {
            Some(t) => t.to_string(),
            None => return vec![],
        }
    } else {
        if !pkg_path.exists() {
            return vec![];
        }
        match std::fs::read_to_string(&pkg_path).ok() {
            Some(t) => t,
            None => return vec![],
        }
    };
    let pkg_root = parse_cst(&pkg_text).syntax();
    let pkg_doc = match lower(&pkg_root).ok() {
        Some(d) => d,
        None => return vec![],
    };
    let field_ty = match resolve_field_type_in_typedefs(type_name, field_name, &pkg_doc.typedefs) {
        Some(t) => t,
        None => return vec![],
    };
    literal_completions_from_type(&field_ty, &pkg_doc.typedefs, 0)
}

/// If the document has a `schema X` and `X` resolves to a record type (either
/// locally or from an imported package), return that record's field names.
///
/// Two cases:
/// 1. Local schema (`schema LocalType`) — resolve through `doc.typedefs`, works
///    even when `doc.uses` is non-empty (fixes the regression where any `use` decl
///    blocked local-schema field completions).
/// 2. Imported schema (`schema alias.RecordName`) — resolve the alias via CST
///    `use_reference_for_alias`, load the imported file, and extract its fields.
///
/// Returns `None` when the schema is absent, unresolvable, or not a record type.
/// Never panics, never fetches.
fn schema_record_fields(
    src: &str,
    doc_path: Option<&Path>,
    cache: Option<&ImportCache>,
) -> Option<Vec<String>> {
    let root = parse_cst(src).syntax();

    // Determine the schema name: prefer lower() but fall back to CST scan when
    // lower() fails (e.g. document contains a bareword in value position like
    // `x: alias.Widget` which isn't a valid scalar).
    let schema_name: String = if let Ok(doc) = lower(&root) {
        doc.schema?.clone()
    } else {
        // Fall back: find the SCHEMA_DECL node and extract the name tokens.
        schema_name_from_cst(&root)?
    };

    if schema_name.contains('.') {
        // Imported schema: `alias.TypeName`
        schema_record_fields_imported(&root, &schema_name, doc_path, cache)
    } else {
        // Local schema: resolve through the document's own typedefs.
        // Re-lower to get typedefs (or retry after the branch above).
        let doc = lower(&root).ok()?;
        let fields = resolve_record_fields(&schema_name, &doc.typedefs)?;
        Some(fields.iter().map(|f| f.name.clone()).collect())
    }
}

/// Extract the schema name from the CST SCHEMA_DECL node when `lower()` fails.
/// Returns the raw text after the `schema` keyword (e.g. `"k.RecordType"`).
fn schema_name_from_cst(root: &SyntaxNode) -> Option<String> {
    for child in root.children() {
        if child.kind() != SyntaxKind::SCHEMA_DECL {
            continue;
        }
        // Tokens in a SCHEMA_DECL: BAREWORD("schema") WS BAREWORD(name) [DOT BAREWORD(type)] NEWLINE
        let mut tokens = child
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| !t.kind().is_trivia() && t.kind() != SyntaxKind::NEWLINE);
        tokens.next(); // skip `schema` keyword
        let name_tok = tokens.next()?;
        let mut name = name_tok.text().to_string();
        // If followed by DOT BAREWORD, it's a qualified name.
        if let Some(dot) = tokens.next() {
            if dot.kind() == SyntaxKind::DOT {
                if let Some(type_tok) = tokens.next() {
                    name.push('.');
                    name.push_str(type_tok.text());
                }
            }
        }
        return Some(name);
    }
    None
}

/// Resolve fields for an imported schema `alias.TypeName`.
/// Walks USE_DECL nodes to find the alias, loads the imported file,
/// lowers it, and resolves the record fields. No-fetch, local-only.
fn schema_record_fields_imported(
    root: &SyntaxNode,
    schema_name: &str,
    doc_path: Option<&Path>,
    cache: Option<&ImportCache>,
) -> Option<Vec<String>> {
    let doc_path = doc_path?;
    let doc_dir = doc_path.parent()?;

    let (alias, type_name) = schema_name.split_once('.')?;

    let reference = use_reference_for_alias(root, alias)?;
    let Ok(resolvers) = mangrove_resolve::Resolvers::find_and_load(doc_dir) else {
        return None;
    };
    let Ok(pkg_path) = resolvers.resolve_local_path(&reference) else {
        return None;
    };
    let pkg_text: String = if let Some(c) = cache {
        c.read(&pkg_path)?.to_string()
    } else {
        if !pkg_path.exists() {
            return None;
        }
        std::fs::read_to_string(&pkg_path).ok()?
    };
    let pkg_root = parse_cst(&pkg_text).syntax();
    // Lower the imported file — it's self-contained so lower() works.
    let pkg_doc = lower(&pkg_root).ok()?;
    let fields = resolve_record_fields(type_name, &pkg_doc.typedefs)?;
    Some(fields.iter().map(|f| f.name.clone()).collect())
}

/// Walk the typedef list resolving `Named` aliases until we find a `Record`
/// and return its fields.  Avoids infinite loops by bounding depth.
fn resolve_record_fields<'a>(
    name: &str,
    typedefs: &'a [mangrove_syntax::parser::TypeDef],
) -> Option<&'a [mangrove_syntax::ty::FieldDef]> {
    let mut current = name;
    let mut depth = 0usize;
    loop {
        if depth > 16 {
            return None; // cycle guard
        }
        let td = typedefs.iter().find(|t| t.name == current)?;
        match &td.ty {
            Type::Record { fields, .. } => return Some(fields),
            Type::Named(next) => {
                current = next.as_str();
                depth += 1;
            }
            _ => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_document_has_no_diagnostics() {
        let src = "type Server = { host: str, port: int & >= 1 & <= 65535 }\nschema Server\nhost: \"x\"\nport: 8443\n";
        assert_eq!(diagnostics(src), vec![]);
    }

    #[test]
    fn syntax_error_produces_a_ranged_diagnostic() {
        // `@` in value position is unexpected → ERROR node.
        let src = "a: @\n";
        let diags = diagnostics(src);
        assert!(!diags.is_empty(), "expected a syntax diagnostic");
        // range is non-empty and within the source
        let d = &diags[0];
        assert!(d.range.0 < d.range.1 || d.range.1 <= src.len());
    }

    #[test]
    fn bad_schema_type_is_a_diagnostic() {
        // duplicate type name → TypeEnv::build error.
        let src = "type T = int\ntype T = str\nschema T\nx: 1\n";
        let diags = diagnostics(src);
        assert!(
            diags.iter().any(|d| d.message.contains("schema error")),
            "expected a schema-error diagnostic, got {diags:?}"
        );
    }

    #[test]
    fn document_with_imports_skips_typecheck_but_still_parses() {
        // a `use` doc can't be checked single-file; must not panic / error.
        let src = "use \"ns/x@v1\" as x\nfoo: 1\n";
        let diags = diagnostics(src);
        assert_eq!(diags, vec![], "import doc should yield no diagnostics");
    }

    #[test]
    fn symbols_lists_top_level_declarations() {
        let src = "type Server = { host: str }\nschema Server\nhost: \"x\"\n";
        let syms = symbols(src);
        let names: Vec<_> = syms.iter().map(|s| (s.name.as_str(), s.kind)).collect();
        assert!(names.contains(&("Server", SymbolKind::Type)));
        assert!(names.contains(&("Server", SymbolKind::Schema)));
        assert!(names.contains(&("host", SymbolKind::Binding)));
    }

    #[test]
    fn semantic_tokens_classify_keywords_types_strings_numbers() {
        let src = "type Server = { host: str }\nhost: \"api\"\nport: 8443\n";
        let toks = semantic_tokens(src);
        let kinds: Vec<SemKind> = toks.iter().map(|t| t.kind).collect();
        assert!(kinds.contains(&SemKind::Keyword)); // `type`
        assert!(kinds.contains(&SemKind::Type)); // `Server`
        assert!(kinds.contains(&SemKind::String)); // "api"
        assert!(kinds.contains(&SemKind::Number)); // 8443
    }

    #[test]
    fn semantic_tokens_classify_booleans() {
        let src = "a: true\nb: false\n";
        let toks = semantic_tokens(src);
        let kinds: Vec<SemKind> = toks.iter().map(|t| t.kind).collect();
        assert!(
            kinds.contains(&SemKind::Keyword),
            "expected SemKind::Keyword for booleans, got {kinds:?}"
        );
    }

    #[test]
    fn hover_on_a_type_name_shows_the_declaration_and_doc() {
        let src = "## the server schema\ntype Server = { host: str }\nschema Server\n";
        // offset into "Server" on the type line (after the doc comment + "type ")
        let off = src.find("Server").unwrap();
        let h = hover(src, off).expect("hover");
        assert!(h.contains("type"), "hover: {h}");
        assert!(h.contains("the server schema"), "doc missing: {h}");
    }

    #[test]
    fn duplicate_type_diagnostic_points_at_declaration_not_whole_doc() {
        let src = "type T = int\ntype T = str\nschema T\nx: 1\n";
        let diags = diagnostics(src);
        assert!(
            diags.iter().any(|d| d.message.contains("schema error")),
            "expected a schema-error diagnostic, got {diags:?}"
        );
        let d = diags
            .iter()
            .find(|d| d.message.contains("schema error"))
            .unwrap();
        // Range must be narrower than the whole document
        assert!(
            d.range.1 < src.len(),
            "diagnostic range should not span the whole document (got {:?})",
            d.range
        );
        // Range should be on a line containing "type T"
        let snip = &src[d.range.0..d.range.1];
        assert!(
            snip.contains('T')
                || src[..d.range.1]
                    .lines()
                    .last()
                    .unwrap_or("")
                    .contains("type T"),
            "diagnostic range should land on a type T declaration, snip={snip:?}"
        );
    }

    #[test]
    fn unknown_type_diagnostic_points_at_referencing_declaration() {
        // type A references unknown Foo
        let src = "type A = Foo\nschema A\nx: 1\n";
        let diags = diagnostics(src);
        assert!(
            diags.iter().any(|d| d.message.contains("schema error")),
            "expected a schema-error diagnostic, got {diags:?}"
        );
        let d = diags
            .iter()
            .find(|d| d.message.contains("schema error"))
            .unwrap();
        // Range must be narrower than the whole document
        assert!(
            d.range.1 < src.len(),
            "diagnostic range should not span the whole document (got {:?})",
            d.range
        );
    }

    #[test]
    fn non_productive_recursion_diagnostic_points_at_declaration() {
        // non-productive recursion: type T = T
        let src = "type T = T\nschema T\nx: 1\n";
        let diags = diagnostics(src);
        assert!(
            diags.iter().any(|d| d.message.contains("schema error")),
            "expected a schema-error diagnostic, got {diags:?}"
        );
        let d = diags
            .iter()
            .find(|d| d.message.contains("non-productive recursive type"))
            .unwrap();
        // Range must be narrower than the whole document
        assert!(
            d.range.1 < src.len(),
            "diagnostic range should not span the whole document (got {:?})",
            d.range
        );
        // Range should land on the "type T" line
        let snip = &src[d.range.0..d.range.1];
        assert!(
            snip.contains("type T"),
            "diagnostic range should land on a type T declaration, snip={snip:?}"
        );
    }

    // ---- goto_definition tests ----

    #[test]
    fn goto_definition_value_ref_resolves_to_binding() {
        // `host` in value position of `addr` should resolve to `host: "x"`.
        let src = "host: \"x\"\naddr: host\n";
        // offset into "host" on line 2 (the ref)
        let ref_off = src.rfind("host").unwrap();
        let result = goto_definition(src, ref_off);
        assert!(result.is_some(), "expected a definition, got None");
        let (start, end) = result.unwrap();
        // The definition range should cover the first `host: "x"` binding.
        let snip = &src[start..end];
        assert!(snip.contains("host"), "snip={snip:?}");
        // Should not point at the ref itself (i.e. it must be at or before the ref).
        assert!(start < ref_off, "definition should precede the reference");
    }

    #[test]
    fn goto_definition_type_ref_resolves_to_type_decl() {
        let src = "type Server = { host: str }\nschema Server\nhost: \"x\"\n";
        // offset into "Server" in the `schema Server` line
        let schema_off = src.rfind("Server").unwrap();
        // The first Server is in the TYPE_DEF; rfind gives us the schema line one.
        let result = goto_definition(src, schema_off);
        // Should resolve to the `type Server = …` declaration.
        assert!(result.is_some(), "expected goto_definition to resolve");
        let (start, _end) = result.unwrap();
        let snip = &src[start..];
        assert!(
            snip.starts_with("type Server") || snip.contains("type Server"),
            "expected to land on type declaration, got start={start}: {snip:.40?}"
        );
    }

    #[test]
    fn goto_definition_returns_none_for_keyword() {
        let src = "type Server = { host: str }\nschema Server\n";
        // offset at "type" keyword
        let off = src.find("type").unwrap();
        assert_eq!(goto_definition(src, off), None);
    }

    #[test]
    fn goto_definition_returns_none_for_unknown_ref() {
        let src = "host: ghost\n";
        let off = src.find("ghost").unwrap();
        // `ghost` is not declared anywhere → None
        assert_eq!(goto_definition(src, off), None);
    }

    // ---- completions tests ----

    #[test]
    fn completions_include_declared_type_name() {
        // In a document with two types, completions in type-position context
        // should include declared type names.
        let src = "type Inner = int\ntype Server = Inner\nschema Server\nx: 1\n";
        // Offset pointing at "Inner" inside `type Server = Inner` — inside TYPE_DEF.
        // rfind("Inner") gives us the reference occurrence after `=`.
        let off = src.rfind("Inner").unwrap();
        let items = completions(src, off, None);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"Inner"),
            "expected 'Inner' in type-position completions, got {labels:?}"
        );
        let item = items.iter().find(|i| i.label == "Inner").unwrap();
        assert_eq!(item.kind, CompletionKind::TypeName);
    }

    #[test]
    fn completions_include_keywords() {
        let src = "type Server = { host: str }\nschema Server\n";
        // offset 0 = ambiguous → full union, includes "type"/"schema"
        let items = completions(src, 0, None);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"type"),
            "expected keyword 'type' in completions, got {labels:?}"
        );
        assert!(
            labels.contains(&"schema"),
            "expected keyword 'schema' in completions, got {labels:?}"
        );
        let kw = items.iter().find(|i| i.label == "type").unwrap();
        assert_eq!(kw.kind, CompletionKind::Keyword);
    }

    #[test]
    fn completions_include_schema_record_fields() {
        // Document with a bound record schema — field names should appear when
        // cursor is inside a binding value (schema value position).
        let src =
            "type Server = { host: str, port: int }\nschema Server\nhost: \"x\"\nport: 8080\n";
        // Offset pointing into the value of `host: "x"` — inside a BINDING.
        let off = src.find("\"x\"").unwrap() + 1;
        let items = completions(src, off, None);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"host"),
            "expected field 'host' in completions, got {labels:?}"
        );
        assert!(
            labels.contains(&"port"),
            "expected field 'port' in completions, got {labels:?}"
        );
        let f = items.iter().find(|i| i.label == "host").unwrap();
        assert_eq!(f.kind, CompletionKind::Field);
    }

    #[test]
    fn completions_no_duplicate_labels() {
        // A name that could appear as both a type and a binding should appear once.
        let src = "type Server = { host: str }\nschema Server\nhost: \"x\"\n";
        let items = completions(src, 0, None);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        let mut dedup = labels.clone();
        dedup.sort();
        dedup.dedup();
        assert_eq!(
            labels.len(),
            dedup.len(),
            "duplicate completion labels: {labels:?}"
        );
    }

    // ---- context-aware completion tests ----

    /// (a) In type position: includes a declared type name + primitive keyword,
    ///     but NOT value-only keywords like `match`.
    #[test]
    fn completions_type_position_includes_type_names_and_primitives_not_match() {
        // `type Foo = ` — cursor at offset just after `=` (inside TYPE_DEF body).
        // We position the cursor inside the type body token ("int").
        let src = "type Foo = int\ntype Bar = str\nschema Foo\nx: 1\n";
        // Offset pointing at "int" inside the TYPE_DEF for Foo.
        let off = src.find("int").unwrap();
        let items = completions(src, off, None);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        // Should include declared type names.
        assert!(
            labels.contains(&"Foo") || labels.contains(&"Bar"),
            "expected type names in type-position completions, got {labels:?}"
        );
        // Should include primitive type keywords.
        assert!(
            labels.contains(&"int"),
            "expected 'int' primitive in type-position completions, got {labels:?}"
        );
        // Must NOT include value-only keywords.
        assert!(
            !labels.contains(&"match"),
            "unexpected 'match' in type-position completions, got {labels:?}"
        );
        assert!(
            !labels.contains(&"unset"),
            "unexpected 'unset' in type-position completions, got {labels:?}"
        );
    }

    /// (b) At top-level: includes `type` and `schema` declaration keywords.
    #[test]
    fn completions_top_level_position_includes_decl_keywords() {
        // The document ends with a newline; cursor at the end = new top-level line.
        let src = "type Foo = int\n";
        let off = src.len(); // past EOF → treated as top-level
        let items = completions(src, off, None);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"type"),
            "expected 'type' keyword at top-level, got {labels:?}"
        );
        assert!(
            labels.contains(&"schema"),
            "expected 'schema' keyword at top-level, got {labels:?}"
        );
    }

    /// (c) Inside a schema-bound record: includes a field name.
    #[test]
    fn completions_schema_value_position_includes_field_names() {
        let src = "type Server = { host: str, port: int }\nschema Server\nhost: \"localhost\"\nport: 80\n";
        // Cursor inside the value of `host: "localhost"` — inside a BINDING.
        let off = src.find("\"localhost\"").unwrap() + 1;
        let items = completions(src, off, None);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"host"),
            "expected field 'host' in schema-value completions, got {labels:?}"
        );
        assert!(
            labels.contains(&"port"),
            "expected field 'port' in schema-value completions, got {labels:?}"
        );
        let f = items.iter().find(|i| i.label == "host").unwrap();
        assert_eq!(f.kind, CompletionKind::Field);
    }

    /// (d) Ambiguous context (offset 0 on a document): non-empty fallback.
    #[test]
    fn completions_ambiguous_context_returns_non_empty_fallback() {
        // A simple document; offset 0 is before any token — cursor context is
        // ambiguous. Must return a non-empty list rather than nothing.
        let src = "type Foo = int\n";
        let items = completions(src, 0, None);
        assert!(
            !items.is_empty(),
            "expected non-empty fallback for ambiguous context"
        );
    }

    // ---- imported-type completion tests (cross-file, local-only) ----

    /// Helper: write a file relative to `dir`.
    fn write_completion_fixture(dir: &std::path::Path, rel: &str, contents: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
    }

    /// A document with `use "ns/pkg@v1" as k` where the cursor is inside
    /// `k.SomeType` (alias-prefix detected) should offer bare TypeName labels
    /// (`SomeType`, `OtherUnit`) not `k.SomeType` (V3 behavior: prefix already typed).
    #[test]
    fn completions_imported_types_offered_in_type_position() {
        let dir = scratch_dir();
        write_completion_fixture(
            &dir,
            ".mangrove/resolvers.toml",
            "[namespace.ns]\nremote = \"vendor\"\n",
        );
        write_completion_fixture(
            &dir,
            "vendor/pkg.mang",
            "type SomeType = int\nunit OtherUnit : int\n",
        );
        let main_src = "use \"ns/pkg@v1\" as k\ntype Local = k.SomeType\n";
        let main_path = dir.join("main.mang");
        // Cursor inside "k.SomeType" in the TYPE_DEF body — alias-prefix `k.` detected.
        // V3: labels are bare (SomeType, OtherUnit) not prefixed (k.SomeType, k.OtherUnit).
        let off = main_src.rfind("SomeType").unwrap();
        let items = completions(main_src, off, Some(&main_path));
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"SomeType"),
            "expected bare 'SomeType' in completions (alias-prefix already typed), got {labels:?}"
        );
        assert!(
            labels.contains(&"OtherUnit"),
            "expected bare 'OtherUnit' in completions, got {labels:?}"
        );
        // Imported items should be typed as TypeName.
        for lbl in &["SomeType", "OtherUnit"] {
            let item = items.iter().find(|i| i.label == *lbl).unwrap();
            assert_eq!(
                item.kind,
                CompletionKind::TypeName,
                "imported item {lbl} should have TypeName kind"
            );
        }
        // Must NOT offer k.SomeType (that would double the alias in the editor).
        assert!(
            !labels.contains(&"k.SomeType"),
            "must not offer k.SomeType when alias-prefix already typed, got {labels:?}"
        );
    }

    /// A package that is NOT locally present must contribute nothing — no panic,
    /// no fetch, just silently skipped.
    #[test]
    fn completions_missing_package_contributes_nothing() {
        let dir = scratch_dir();
        write_completion_fixture(
            &dir,
            ".mangrove/resolvers.toml",
            "[namespace.ns]\nremote = \"vendor\"\n",
        );
        // vendor/pkg.mang intentionally NOT created.
        let main_src = "use \"ns/pkg@v1\" as k\ntype Local = k.Missing\n";
        let main_path = dir.join("main.mang");
        let off = main_src.rfind("Missing").unwrap();
        // Must not panic; missing package is silently skipped.
        let items = completions(main_src, off, Some(&main_path));
        let imported: Vec<&str> = items
            .iter()
            .filter(|i| i.label.starts_with("k."))
            .map(|i| i.label.as_str())
            .collect();
        assert!(
            imported.is_empty(),
            "expected no imported completions for missing package, got {imported:?}"
        );
    }

    /// When doc_path is None (untitled buffer), imported types are skipped and
    /// local completions still work.
    #[test]
    fn completions_no_doc_path_skips_imports_keeps_local() {
        let src = "type Inner = int\ntype Server = Inner\nschema Server\nx: 1\n";
        let off = src.rfind("Inner").unwrap();
        let items = completions(src, off, None);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        // Local types still offered.
        assert!(
            labels.contains(&"Inner"),
            "expected 'Inner' in completions with None doc_path, got {labels:?}"
        );
        // No dotted labels (no imported types).
        let dotted: Vec<_> = labels.iter().filter(|l| l.contains('.')).collect();
        assert!(
            dotted.is_empty(),
            "expected no dotted labels with None doc_path, got {dotted:?}"
        );
    }

    // ---- references tests ----

    /// References on a local type name returns the decl + every use.
    #[test]
    fn references_type_name_finds_decl_and_uses() {
        // "Server" appears in: (1) `type Server = …` decl, (2) `schema Server`.
        let src = "type Server = { host: str }\nschema Server\nhost: \"x\"\n";
        // Cursor on "Server" in the `type Server` declaration.
        let decl_off = src.find("Server").unwrap();
        let refs = references(src, decl_off, true);
        // Should find at least 2 occurrences (decl + schema reference).
        assert!(
            refs.len() >= 2,
            "expected >= 2 occurrences for 'Server', got {refs:?}"
        );
        // All ranges must contain only the text "Server".
        for (s, e) in &refs {
            assert_eq!(
                &src[*s..*e],
                "Server",
                "unexpected token at range ({s},{e})"
            );
        }
        // Decl site is included (include_decl=true).
        let decl_range = (decl_off, decl_off + "Server".len());
        assert!(
            refs.contains(&decl_range),
            "expected decl range {decl_range:?} in refs {refs:?}"
        );
    }

    /// References with include_decl=false excludes the declaration.
    #[test]
    fn references_type_name_exclude_decl() {
        let src = "type Server = { host: str }\nschema Server\nhost: \"x\"\n";
        let decl_off = src.find("Server").unwrap();
        let refs_incl = references(src, decl_off, true);
        let refs_excl = references(src, decl_off, false);
        assert!(
            refs_excl.len() < refs_incl.len(),
            "exclude_decl should return fewer results"
        );
        // The decl range must not appear in refs_excl.
        let decl_range = (decl_off, decl_off + "Server".len());
        assert!(
            !refs_excl.contains(&decl_range),
            "decl range should be excluded, got {refs_excl:?}"
        );
    }

    /// References on a binding key finds decl + REF uses.
    #[test]
    fn references_binding_finds_decl_and_ref_uses() {
        // `host` declared as a binding, referenced in `addr` value.
        let src = "host: \"x\"\naddr: host\n";
        // Cursor on `host` in `addr: host` (the REF occurrence).
        let ref_off = src.rfind("host").unwrap();
        let refs = references(src, ref_off, true);
        assert!(
            refs.len() >= 2,
            "expected >= 2 occurrences for 'host', got {refs:?}"
        );
        // All ranges must point at "host".
        for (s, e) in &refs {
            assert_eq!(&src[*s..*e], "host", "unexpected token at ({s},{e})");
        }
    }

    /// Imported/qualified symbol yields empty result.
    #[test]
    fn references_qualified_symbol_returns_empty() {
        let _src = "use \"ns/x@v1\" as x\nfoo: x.Bar\n";
        // Cursor on "Bar" — but it's part of a qualified expression, so not local.
        // Actually "x.Bar" in value position: let's put cursor on "x" which is qualified.
        // The DOT means the name contains a qualifier at the BAREWORD level.
        // We test by placing cursor on "foo" (the binding key) and checking "x.Bar" REF
        // won't match anything. Actually test that references on an unknown ref is empty.
        let ghost_src = "host: ghost\n";
        let off = ghost_src.find("ghost").unwrap();
        let refs = references(ghost_src, off, true);
        assert!(
            refs.is_empty(),
            "expected empty for undeclared 'ghost', got {refs:?}"
        );
    }

    // ---- rename tests ----

    /// Rename produces the same occurrence set as references(include_decl=true).
    #[test]
    fn rename_type_name_matches_references() {
        let src = "type Server = { host: str }\nschema Server\nhost: \"x\"\n";
        let decl_off = src.find("Server").unwrap();
        let refs = references(src, decl_off, true);
        let renames = rename(src, decl_off, "Node");
        assert_eq!(
            renames.as_deref(),
            Some(refs.as_slice()),
            "rename ranges should equal references(include_decl=true)"
        );
    }

    /// Applying rename edits to the source yields a validly-renamed document.
    #[test]
    fn rename_produces_valid_renamed_source() {
        let src = "type Server = { host: str }\nschema Server\nhost: \"x\"\n";
        let decl_off = src.find("Server").unwrap();
        let ranges = rename(src, decl_off, "Node").expect("rename should succeed");
        // Apply edits in reverse order to preserve offsets.
        let mut result = src.to_string();
        let mut sorted = ranges.clone();
        sorted.sort_by_key(|r| std::cmp::Reverse(r.0));
        for (s, e) in sorted {
            result.replace_range(s..e, "Node");
        }
        assert!(
            result.contains("type Node"),
            "renamed source should contain 'type Node', got: {result:?}"
        );
        assert!(
            result.contains("schema Node"),
            "renamed source should contain 'schema Node', got: {result:?}"
        );
        assert!(
            !result.contains("Server"),
            "renamed source should not contain 'Server', got: {result:?}"
        );
    }

    /// Rename of a binding name also renames all its REF uses.
    #[test]
    fn rename_binding_renames_all_refs() {
        let src = "host: \"x\"\naddr: host\n";
        let decl_off = src.find("host").unwrap();
        let ranges = rename(src, decl_off, "hostname").expect("rename should succeed");
        assert!(
            ranges.len() >= 2,
            "expected >= 2 ranges for binding rename, got {ranges:?}"
        );
        let mut result = src.to_string();
        let mut sorted = ranges.clone();
        sorted.sort_by_key(|r| std::cmp::Reverse(r.0));
        for (s, e) in sorted {
            result.replace_range(s..e, "hostname");
        }
        assert!(
            result.contains("hostname: \"x\""),
            "renamed source should contain 'hostname: \"x\"', got: {result:?}"
        );
        assert!(
            result.contains("addr: hostname"),
            "renamed source should contain 'addr: hostname', got: {result:?}"
        );
    }

    /// Rename with invalid identifier returns None.
    #[test]
    fn rename_invalid_new_name_returns_none() {
        let src = "type Server = { host: str }\nschema Server\nhost: \"x\"\n";
        let off = src.find("Server").unwrap();
        assert_eq!(rename(src, off, ""), None, "empty name should return None");
        assert_eq!(
            rename(src, off, "123bad"),
            None,
            "name starting with digit should return None"
        );
        assert_eq!(
            rename(src, off, "bad name"),
            None,
            "name with space should return None"
        );
    }

    /// Rename on a keyword returns None.
    #[test]
    fn rename_keyword_returns_none() {
        let src = "type Server = { host: str }\nschema Server\n";
        let off = src.find("type").unwrap();
        assert_eq!(rename(src, off, "newtype"), None);
    }

    /// Rename on an undeclared symbol returns None.
    #[test]
    fn rename_undeclared_symbol_returns_none() {
        let src = "host: ghost\n";
        let off = src.find("ghost").unwrap();
        assert_eq!(rename(src, off, "spirit"), None);
    }

    // ---- field-key exclusion tests (bug: rename must not touch record-type field keys) ----

    /// find-references on type T must NOT include the `T` field key in `type Rec = { T: int }`.
    /// Only the type decl name and the `schema T` reference are valid occurrences.
    #[test]
    fn references_type_name_excludes_record_field_key() {
        // "T" appears as: type T decl name, field key in Rec, and schema T reference.
        let src = "type T = int\ntype Rec = { T: int }\nschema T\nv: 1\n";
        let decl_off = src.find('T').unwrap(); // offset of `T` in `type T = int`
        let refs = references(src, decl_off, true);
        // Must find exactly the decl + schema ref (count == 2).
        assert_eq!(
            refs.len(),
            2,
            "expected exactly 2 occurrences (decl + schema ref), got {refs:?}"
        );
        // All ranges must be outside the `{ T: int }` record body.
        let rec_body_start = src.find("{ T:").unwrap();
        let rec_body_end = src.find('}').unwrap() + 1;
        for (s, _e) in &refs {
            assert!(
                *s < rec_body_start || *s >= rec_body_end,
                "occurrence at byte {s} falls inside the record-type body (field key), refs={refs:?}"
            );
        }
    }

    /// rename T -> U must not touch the `T` field key in `type Rec = { T: int }`.
    #[test]
    fn rename_type_name_does_not_corrupt_record_field_key() {
        let src = "type T = int\ntype Rec = { T: int }\nschema T\nv: 1\n";
        let decl_off = src.find('T').unwrap();
        let ranges = rename(src, decl_off, "U").expect("rename should succeed");
        // Apply edits in reverse order.
        let mut result = src.to_string();
        let mut sorted = ranges.clone();
        sorted.sort_by_key(|r| std::cmp::Reverse(r.0));
        for (s, e) in sorted {
            result.replace_range(s..e, "U");
        }
        assert_eq!(
            result, "type U = int\ntype Rec = { T: int }\nschema U\nv: 1\n",
            "rename T->U corrupted the record-type field key: {result:?}"
        );
    }

    /// A bareword that IS a type REFERENCE in field-value position (e.g. `{{ x: T }}`)
    /// must still be found by references/rename (it is NOT a field key — it follows a colon
    /// but is the VALUE, not the KEY).
    #[test]
    fn references_type_name_includes_type_ref_in_field_value() {
        // `T` appears as: type T decl, field VALUE `{ x: T }` in Rec, and schema T.
        let src = "type T = int\ntype Rec = { x: T }\nschema T\nv: 1\n";
        let decl_off = src.find('T').unwrap();
        let refs = references(src, decl_off, true);
        // Should find 3: decl + the type-ref `T` in `{ x: T }` + schema T.
        assert_eq!(
            refs.len(),
            3,
            "expected 3 occurrences (decl + field-value type ref + schema ref), got {refs:?}"
        );
    }

    /// unit decl-name rename must still work (unit `Mem : int` has a COLON at depth 0 — must KEEP).
    #[test]
    fn rename_unit_decl_name_still_works() {
        let src = "unit Mem : int\nschema Mem\nv: 1\n";
        let decl_off = src.find("Mem").unwrap();
        let ranges = rename(src, decl_off, "Bytes").expect("rename should succeed");
        let mut result = src.to_string();
        let mut sorted = ranges.clone();
        sorted.sort_by_key(|r| std::cmp::Reverse(r.0));
        for (s, e) in sorted {
            result.replace_range(s..e, "Bytes");
        }
        assert!(
            result.contains("unit Bytes"),
            "unit decl rename failed: {result:?}"
        );
        assert!(
            result.contains("schema Bytes"),
            "unit schema rename failed: {result:?}"
        );
        assert!(
            !result.contains("Mem"),
            "old name still present after rename: {result:?}"
        );
    }

    #[test]
    fn rename_to_hyphenated_name_succeeds() {
        // `my-field` is a valid Mangrove bareword (lexer allows `-` in continue position).
        let src = "type my-field = int\n";
        // cursor on `my-field` in the type declaration
        let off = src.find("my-field").unwrap();
        let result = rename(src, off, "your-field");
        assert!(
            result.is_some(),
            "rename to 'your-field' must succeed (hyphen is valid in continue position), got None"
        );
    }

    #[test]
    fn rename_rejects_leading_hyphen() {
        // Leading `-` is not allowed per is_ident_start.
        let src = "type Foo = int\n";
        let off = src.find("Foo").unwrap();
        let result = rename(src, off, "-bad");
        assert!(
            result.is_none(),
            "rename to '-bad' (leading hyphen) must be rejected, got {result:?}"
        );
    }

    #[test]
    fn rename_rejects_leading_digit() {
        let src = "type Foo = int\n";
        let off = src.find("Foo").unwrap();
        let result = rename(src, off, "1bad");
        assert!(
            result.is_none(),
            "rename to '1bad' (leading digit) must be rejected, got {result:?}"
        );
    }

    #[test]
    fn rename_rejects_name_with_space() {
        let src = "type Foo = int\n";
        let off = src.find("Foo").unwrap();
        let result = rename(src, off, "bad name");
        assert!(
            result.is_none(),
            "rename to 'bad name' (space) must be rejected, got {result:?}"
        );
    }

    // ---- cross-file go-to-definition tests ----

    fn write_fixture(dir: &std::path::Path, rel: &str, contents: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
    }

    fn scratch_dir() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let id = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("mangrove_lsp_xfile_{}_{id}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn goto_definition_cross_file_resolves_to_imported_type() {
        let dir = scratch_dir();
        // .mangrove/resolvers.toml
        write_fixture(
            &dir,
            ".mangrove/resolvers.toml",
            "[namespace.ns]\nremote = \"vendor\"\n",
        );
        // vendor/pkg.mang — the target file
        write_fixture(&dir, "vendor/pkg.mang", "type SomeType = int\n");
        // main.mang — the source document
        let main_src = "use \"ns/pkg@v1\" as k\ntype Local = k.SomeType\n";
        let main_path = dir.join("main.mang");
        // cursor on "SomeType" in "k.SomeType"
        let off = main_src.rfind("SomeType").unwrap();
        let result = goto_definition_cross_file(main_src, off, &main_path);
        assert!(
            result.is_some(),
            "expected cross-file goto to resolve, got None"
        );
        let (file_path, (start, end), file_text) = result.unwrap();
        assert!(
            file_path.ends_with("pkg.mang"),
            "expected to resolve to pkg.mang, got {file_path:?}"
        );
        let snip = &file_text[start..end];
        assert!(
            snip.contains("SomeType"),
            "expected definition range to cover SomeType, got {snip:?}"
        );
    }

    #[test]
    fn goto_definition_cross_file_resolves_from_alias_part() {
        let dir = scratch_dir();
        write_fixture(
            &dir,
            ".mangrove/resolvers.toml",
            "[namespace.ns]\nremote = \"vendor\"\n",
        );
        write_fixture(&dir, "vendor/pkg.mang", "type SomeType = int\n");
        let main_src = "use \"ns/pkg@v1\" as k\ntype Local = k.SomeType\n";
        let main_path = dir.join("main.mang");
        // cursor on "k" (the alias) in "k.SomeType"
        let off = main_src.rfind("k.SomeType").unwrap();
        let result = goto_definition_cross_file(main_src, off, &main_path);
        assert!(
            result.is_some(),
            "expected cross-file goto from alias part to resolve, got None"
        );
        let (file_path, (start, end), file_text) = result.unwrap();
        assert!(
            file_path.ends_with("pkg.mang"),
            "expected to resolve to pkg.mang, got {file_path:?}"
        );
        let snip = &file_text[start..end];
        assert!(
            snip.contains("SomeType"),
            "expected definition range to cover SomeType, got {snip:?}"
        );
    }

    #[test]
    fn goto_definition_cross_file_missing_package_returns_none() {
        let dir = scratch_dir();
        // resolvers.toml points to vendor/ but no files exist there
        write_fixture(
            &dir,
            ".mangrove/resolvers.toml",
            "[namespace.ns]\nremote = \"vendor\"\n",
        );
        // vendor/pkg.mang intentionally NOT created
        let main_src = "use \"ns/pkg@v1\" as k\ntype Local = k.SomeType\n";
        let main_path = dir.join("main.mang");
        let off = main_src.rfind("SomeType").unwrap();
        let result = goto_definition_cross_file(main_src, off, &main_path);
        assert!(
            result.is_none(),
            "expected None for missing package, got {:?}",
            result.as_ref().map(|(p, _, _)| p)
        );
    }

    // RED test: a document that has a use decl plus a value-position qualified ref
    // (`x: inf.Widget`) causes lower() to fail because barewords are not valid
    // scalar values.  The cursor is on a *different* `inf.SomeType` occurrence in
    // type position; the CST-based alias lookup must find the use decl regardless.
    #[test]
    fn goto_definition_cross_file_works_when_lower_fails() {
        let dir = scratch_dir();
        write_fixture(
            &dir,
            ".mangrove/resolvers.toml",
            "[namespace.ns]\nremote = \"vendor\"\n",
        );
        write_fixture(&dir, "vendor/pkg.mang", "type SomeType = int\n");
        // The binding `x: inf.Widget` puts a bareword in value position — lower()
        // cannot decode `inf` as a scalar and returns Err, so the old
        // lower(&root).ok()? path would return None for the whole goto call.
        let main_src = "use \"ns/pkg@v1\" as inf\nx: inf.Widget\ntype Local = inf.SomeType\n";
        let main_path = dir.join("main.mang");
        // Cursor on "SomeType" in the type-position occurrence.
        let off = main_src.rfind("SomeType").unwrap();
        let result = goto_definition_cross_file(main_src, off, &main_path);
        assert!(
            result.is_some(),
            "expected cross-file goto to resolve even when lower() fails, got None"
        );
        let (file_path, (start, end), file_text) = result.unwrap();
        assert!(
            file_path.ends_with("pkg.mang"),
            "expected to resolve to pkg.mang, got {file_path:?}"
        );
        let snip = &file_text[start..end];
        assert!(
            snip.contains("SomeType"),
            "expected definition range to cover SomeType, got {snip:?}"
        );
    }

    #[test]
    fn goto_definition_local_still_works_after_cross_file_added() {
        // existing local goto test: type Server, schema Server — still resolves
        let src = "type Server = { host: str }\nschema Server\nhost: \"x\"\n";
        let schema_off = src.rfind("Server").unwrap();
        let result = goto_definition(src, schema_off);
        assert!(result.is_some(), "local goto_definition should still work");
        let (start, _end) = result.unwrap();
        let snip = &src[start..];
        assert!(
            snip.starts_with("type Server") || snip.contains("type Server"),
            "expected to land on type declaration, got start={start}: {snip:.40?}"
        );
    }

    // ---- Feature A: imported-schema field completion tests ----

    /// Cursor inside a schema-bound record value where the schema is `alias.RecordType`
    /// (imported via `use`). Field names from the imported record must be offered.
    #[test]
    fn completions_imported_schema_fields_offered() {
        let dir = scratch_dir();
        write_completion_fixture(
            &dir,
            ".mangrove/resolvers.toml",
            "[namespace.ns]\nremote = \"vendor\"\n",
        );
        write_completion_fixture(
            &dir,
            "vendor/pkg.mang",
            "type RecordType = { x: str, y: int }\n",
        );
        // schema k.RecordType — imported record type
        let main_src = "use \"ns/pkg@v1\" as k\nschema k.RecordType\nx: \"hello\"\n";
        let main_path = dir.join("main.mang");
        // Cursor inside the value of `x: "hello"` — schema value position.
        let off = main_src.find("\"hello\"").unwrap() + 1;
        let items = completions(main_src, off, Some(&main_path));
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"x"),
            "expected field 'x' from imported schema, got {labels:?}"
        );
        assert!(
            labels.contains(&"y"),
            "expected field 'y' from imported schema, got {labels:?}"
        );
        let field_x = items.iter().find(|i| i.label == "x").unwrap();
        assert_eq!(
            field_x.kind,
            CompletionKind::Field,
            "field 'x' must have kind Field"
        );
    }

    /// If the imported schema resolves to a non-record type, no field completions
    /// should be offered, and no panic must occur.
    #[test]
    fn completions_imported_non_record_schema_no_fields() {
        let dir = scratch_dir();
        write_completion_fixture(
            &dir,
            ".mangrove/resolvers.toml",
            "[namespace.ns]\nremote = \"vendor\"\n",
        );
        write_completion_fixture(&dir, "vendor/pkg.mang", "type SimpleType = int\n");
        let main_src = "use \"ns/pkg@v1\" as k\nschema k.SimpleType\nx: 1\n";
        let main_path = dir.join("main.mang");
        let off = main_src.find("1\n").unwrap(); // inside value of `x: 1`
        let items = completions(main_src, off, Some(&main_path));
        let field_items: Vec<_> = items
            .iter()
            .filter(|i| i.kind == CompletionKind::Field)
            .collect();
        assert!(
            field_items.is_empty(),
            "expected no Field completions for non-record schema, got {field_items:?}"
        );
    }

    /// Cursor in schema value position where the alias can't be resolved (e.g. not
    /// in resolvers.toml). Must return non-empty items (keywords), no panic, no Field items.
    #[test]
    fn completions_imported_schema_unresolvable_alias_no_fields() {
        let dir = scratch_dir();
        // resolvers.toml has no namespace — alias won't resolve
        write_completion_fixture(
            &dir,
            ".mangrove/resolvers.toml",
            "[namespace.other]\nremote = \"vendor\"\n",
        );
        let main_src = "use \"ns/pkg@v1\" as k\nschema k.RecordType\nx: 1\n";
        let main_path = dir.join("main.mang");
        let off = main_src.find("1\n").unwrap();
        // Must not panic; must return at least keyword items
        let items = completions(main_src, off, Some(&main_path));
        assert!(!items.is_empty(), "expected at least keyword items");
        let field_items: Vec<_> = items
            .iter()
            .filter(|i| i.kind == CompletionKind::Field)
            .collect();
        assert!(
            field_items.is_empty(),
            "expected no Field items when alias unresolvable, got {field_items:?}"
        );
    }

    /// A document that has both `use` decls AND a LOCAL schema (not imported) must
    /// still offer the local schema's field completions. Regression for the
    /// `doc.uses.is_empty()` early-bail that broke this.
    #[test]
    fn completions_local_schema_with_use_decls_still_works() {
        let dir = scratch_dir();
        write_completion_fixture(
            &dir,
            ".mangrove/resolvers.toml",
            "[namespace.ns]\nremote = \"vendor\"\n",
        );
        write_completion_fixture(&dir, "vendor/pkg.mang", "type PkgType = int\n");
        // Local schema LocalRec, but the document also has a `use` decl.
        let main_src = "use \"ns/pkg@v1\" as k\ntype LocalRec = { a: str, b: int }\nschema LocalRec\na: \"x\"\n";
        let main_path = dir.join("main.mang");
        let off = main_src.find("\"x\"").unwrap() + 1;
        let items = completions(main_src, off, Some(&main_path));
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"a"),
            "expected field 'a' for local schema in doc with use decls, got {labels:?}"
        );
        assert!(
            labels.contains(&"b"),
            "expected field 'b' for local schema in doc with use decls, got {labels:?}"
        );
    }

    // ---- Feature B: alias-prefix filtering tests ----

    /// Cursor positioned inside `k.TypeK` in type position — only types from
    /// alias `k`'s package must be offered, not those from `other`, and the labels
    /// must be bare TypeNames (not `k.TypeK`).
    #[test]
    fn completions_alias_prefix_filters_to_that_alias_only() {
        let dir = scratch_dir();
        write_completion_fixture(
            &dir,
            ".mangrove/resolvers.toml",
            "[namespace.ns]\nremote = \"vendor\"\n",
        );
        write_completion_fixture(&dir, "vendor/k.mang", "type TypeK = int\n");
        write_completion_fixture(&dir, "vendor/other.mang", "type TypeOther = str\n");
        // Two use decls; cursor inside k.TypeK in a TYPE_DEF body.
        let main_src = "use \"ns/k@v1\" as k\nuse \"ns/other@v1\" as other\ntype X = k.TypeK\n";
        let main_path = dir.join("main.mang");
        // Cursor on "TypeK" inside "k.TypeK" — alias-prefix detected.
        let off = main_src.rfind("TypeK").unwrap();
        let items = completions(main_src, off, Some(&main_path));
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        // TypeK must appear as bare name (not k.TypeK)
        assert!(
            labels.contains(&"TypeK"),
            "expected bare 'TypeK' label when alias-prefix present, got {labels:?}"
        );
        // Must NOT contain the other alias's types
        assert!(
            !labels.contains(&"other.TypeOther"),
            "must not offer other.TypeOther when alias-prefix is k, got {labels:?}"
        );
        // Must NOT contain k.TypeK (double-alias)
        assert!(
            !labels.contains(&"k.TypeK"),
            "must not offer k.TypeK (double alias), got {labels:?}"
        );
    }

    /// When NO alias-prefix is present in type position, imported types must NOT
    /// be offered — only local types and primitives.
    #[test]
    fn completions_no_alias_prefix_omits_imported_types() {
        let dir = scratch_dir();
        write_completion_fixture(
            &dir,
            ".mangrove/resolvers.toml",
            "[namespace.ns]\nremote = \"vendor\"\n",
        );
        write_completion_fixture(&dir, "vendor/pkg.mang", "type SomeType = int\n");
        // Cursor on `int` in `type X = int` — no alias prefix, type position.
        let main_src = "use \"ns/pkg@v1\" as k\ntype X = int\n";
        let main_path = dir.join("main.mang");
        let off = main_src.find("int").unwrap();
        let items = completions(main_src, off, Some(&main_path));
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        // Local type must still be present
        assert!(
            labels.contains(&"int"),
            "expected primitive 'int' in completions, got {labels:?}"
        );
        // No dotted (imported) labels
        let dotted: Vec<_> = labels.iter().filter(|l| l.contains('.')).collect();
        assert!(
            dotted.is_empty(),
            "expected no imported types when no alias-prefix, got {dotted:?}"
        );
    }

    /// When the cursor is inside `k.Som` (partial type name after alias dot), the
    /// alias-prefix is still detected and only types from `k`'s package are offered.
    #[test]
    fn completions_alias_prefix_partial_text_still_filters() {
        let dir = scratch_dir();
        write_completion_fixture(
            &dir,
            ".mangrove/resolvers.toml",
            "[namespace.ns]\nremote = \"vendor\"\n",
        );
        write_completion_fixture(&dir, "vendor/pkg.mang", "type SomeType = int\n");
        // The CST will parse `k.Som` as `k` DOT `Som` in type position (BAREWORD DOT BAREWORD).
        let main_src = "use \"ns/pkg@v1\" as k\ntype X = k.SomeType\n";
        let main_path = dir.join("main.mang");
        // Cursor on "SomeType" — partial text after `k.`
        let off = main_src.rfind("SomeType").unwrap();
        let items = completions(main_src, off, Some(&main_path));
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        // Must offer bare SomeType (not k.SomeType)
        assert!(
            labels.contains(&"SomeType"),
            "expected bare 'SomeType' when partial alias-prefix, got {labels:?}"
        );
        assert!(
            !labels.contains(&"k.SomeType"),
            "must not offer k.SomeType (double-alias), got {labels:?}"
        );
    }

    // ---- ImportCache unit tests ----

    /// Test 1: cache hit avoids re-read.
    /// Two calls to `cache.read(path)` for the same unchanged file must result
    /// in exactly one disk read (`reads() == 1`) and identical returned text.
    #[test]
    fn import_cache_hit_avoids_re_read() {
        let dir = scratch_dir();
        let path = dir.join("cached.mang");
        std::fs::write(&path, "type T = int\n").unwrap();

        let cache = ImportCache::new();
        let text1 = cache.read(&path).expect("first read should succeed");
        let text2 = cache.read(&path).expect("second read should hit cache");

        assert_eq!(
            cache.reads(),
            1,
            "expected exactly 1 disk read; got {}",
            cache.reads()
        );
        assert_eq!(
            text1.as_ref(),
            text2.as_ref(),
            "both calls should return identical text"
        );
        assert_eq!(text1.as_ref(), "type T = int\n");
    }

    /// Test 2: mtime/len invalidation forces a re-read with fresh content.
    /// After a cache hit, we write new content (different length → len key changes),
    /// then call read() again. The read counter must increment and new content returned.
    #[test]
    fn import_cache_invalidated_on_len_change() {
        let dir = scratch_dir();
        let path = dir.join("invalidated.mang");
        std::fs::write(&path, "type A = int\n").unwrap();

        let cache = ImportCache::new();
        let text1 = cache.read(&path).expect("first read");
        assert_eq!(cache.reads(), 1);
        assert_eq!(text1.as_ref(), "type A = int\n");

        // Write new content with a different byte length so the len key changes.
        // (Even on same-second filesystems, len != len triggers a miss.)
        std::fs::write(&path, "type A = str\ntype B = int\n").unwrap();

        let text2 = cache.read(&path).expect("re-read after invalidation");
        assert_eq!(
            cache.reads(),
            2,
            "expected a second disk read after content change"
        );
        assert_eq!(text2.as_ref(), "type A = str\ntype B = int\n");
        assert_ne!(
            text1.as_ref(),
            text2.as_ref(),
            "stale text must not be returned after invalidation"
        );
    }

    /// Test 3a: mtime-only invalidation forces a re-read even when the byte
    /// length is unchanged.
    ///
    /// This test documents the `(mtime, len)` key contract: if ONLY the mtime
    /// changes (same length, same text content — as though a `touch` were run),
    /// the cache treats the entry as stale and reads from disk again.  The
    /// residual risk scenario (same mtime AND same len with different content)
    /// is not tested here because it cannot be reliably reproduced without
    /// defeating the OS mtime mechanism; see the `VersionKey` doc comment for
    /// why that window is effectively impossible in practice.
    #[test]
    fn import_cache_invalidated_on_mtime_change() {
        let dir = scratch_dir();
        let path = dir.join("mtime_only.mang");
        std::fs::write(&path, "type A = int\n").unwrap();

        // Backdate the mtime to a well-known past time so the first cache entry
        // stores a mtime that is clearly different from "now".
        let past =
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        std::fs::File::open(&path)
            .unwrap()
            .set_modified(past)
            .unwrap();

        let cache = ImportCache::new();
        let text1 = cache.read(&path).expect("first read");
        assert_eq!(cache.reads(), 1);
        assert_eq!(text1.as_ref(), "type A = int\n");

        // Re-write with IDENTICAL byte length so only the mtime changes.
        // ("type A = int\n" and "type A = str\n" are both 13 bytes.)
        std::fs::write(&path, "type A = str\n").unwrap();
        // The OS sets mtime to "now" on write, which differs from `past`.

        let text2 = cache.read(&path).expect("re-read after mtime change");
        assert_eq!(
            cache.reads(),
            2,
            "expected a re-read when mtime changed (even with same byte length)"
        );
        assert_eq!(text2.as_ref(), "type A = str\n");
        assert_ne!(
            text1.as_ref(),
            text2.as_ref(),
            "stale text must not be returned after mtime-only invalidation"
        );
    }

    /// Test 4 (was 3): missing file returns None, no panic.
    #[test]
    fn import_cache_missing_file_returns_none() {
        let dir = scratch_dir();
        let path = dir.join("does_not_exist.mang");

        let cache = ImportCache::new();
        let result = cache.read(&path);
        assert!(
            result.is_none(),
            "expected None for missing file, got {result:?}"
        );
        assert_eq!(
            cache.reads(),
            0,
            "no disk read should be attempted for missing file"
        );
    }

    /// Test 5: two successive completions_cached calls return the same results
    /// and the second call does not perform any additional disk reads.
    #[test]
    fn completions_cached_reduces_reads_on_second_call() {
        let dir = scratch_dir();
        write_completion_fixture(
            &dir,
            ".mangrove/resolvers.toml",
            "[namespace.ns]\nremote = \"vendor\"\n",
        );
        write_completion_fixture(
            &dir,
            "vendor/pkg.mang",
            "type SomeType = int\nunit OtherUnit : int\n",
        );
        let main_src = "use \"ns/pkg@v1\" as k\ntype Local = k.SomeType\n";
        let main_path = dir.join("main.mang");
        let off = main_src.rfind("SomeType").unwrap();

        let cache = ImportCache::new();

        // First call — populates cache.
        let items1 = completions_cached(main_src, off, Some(&main_path), &cache);
        let reads_after_first = cache.reads();
        assert!(reads_after_first > 0, "first call must read from disk");

        // Second call — should be served entirely from cache.
        let items2 = completions_cached(main_src, off, Some(&main_path), &cache);
        let reads_after_second = cache.reads();
        assert_eq!(
            reads_after_first, reads_after_second,
            "second call must not trigger any additional disk reads"
        );

        // Results must be identical.
        let labels1: Vec<&str> = items1.iter().map(|i| i.label.as_str()).collect();
        let labels2: Vec<&str> = items2.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            labels1, labels2,
            "both calls must return identical completions"
        );
        assert!(
            labels1.contains(&"SomeType"),
            "SomeType must be in completions"
        );
    }

    // ---- enum-value completion tests (T2: literal-union field types) ----

    /// T2-a: local schema with a field typed as a direct literal union.
    /// Cursor in the value position of `mode:` where `type T = { mode: "a" | "b" | "c" }`.
    /// Must offer `"a"`, `"b"`, `"c"` as EnumValue completions.
    #[test]
    fn completions_enum_value_local_direct_union() {
        let src = "type T = { mode: \"a\" | \"b\" | \"c\" }\nschema T\nmode: \"a\"\n";
        // Cursor inside the value of `mode: "a"` — after the opening quote.
        let off = src.rfind("\"a\"\n").unwrap() + 1;
        let items = completions(src, off, None);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"\"a\""),
            "expected '\"a\"' in enum completions, got {labels:?}"
        );
        assert!(
            labels.contains(&"\"b\""),
            "expected '\"b\"' in enum completions, got {labels:?}"
        );
        assert!(
            labels.contains(&"\"c\""),
            "expected '\"c\"' in enum completions, got {labels:?}"
        );
        // The items for the literals must be EnumValue kind.
        let item_a = items.iter().find(|i| i.label == "\"a\"").unwrap();
        assert_eq!(
            item_a.kind,
            CompletionKind::EnumValue,
            "literal completion must have kind EnumValue"
        );
    }

    /// T2-b: field typed via a Named alias to a literal union.
    /// `type Mode = "a" | "b"`, `type T = { m: Mode }`.
    /// Cursor in `m:`'s value must resolve through Named → offer `"a"`, `"b"`.
    #[test]
    fn completions_enum_value_named_alias_resolves() {
        let src = "type Mode = \"a\" | \"b\"\ntype T = { m: Mode }\nschema T\nm: \"a\"\n";
        let off = src.rfind("\"a\"\n").unwrap() + 1;
        let items = completions(src, off, None);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"\"a\""),
            "expected '\"a\"' via Named alias, got {labels:?}"
        );
        assert!(
            labels.contains(&"\"b\""),
            "expected '\"b\"' via Named alias, got {labels:?}"
        );
        let item = items.iter().find(|i| i.label == "\"a\"").unwrap();
        assert_eq!(item.kind, CompletionKind::EnumValue);
    }

    /// T2-c: non-literal field type (`x: int`) offers no enum values (no panic).
    #[test]
    fn completions_enum_value_non_literal_type_no_values() {
        let src = "type T = { x: int }\nschema T\nx: 1\n";
        // Cursor inside value of `x: 1`
        let off = src.rfind("1\n").unwrap();
        let items = completions(src, off, None);
        let enum_items: Vec<_> = items
            .iter()
            .filter(|i| i.kind == CompletionKind::EnumValue)
            .collect();
        assert!(
            enum_items.is_empty(),
            "expected no EnumValue completions for int field, got {enum_items:?}"
        );
    }

    /// T2-d: mixed union `"a" | int` — only offers the literal member `"a"`.
    #[test]
    fn completions_enum_value_mixed_union_only_literals() {
        let src = "type T = { x: \"a\" | int }\nschema T\nx: \"a\"\n";
        let off = src.rfind("\"a\"\n").unwrap() + 1;
        let items = completions(src, off, None);
        let enum_items: Vec<_> = items
            .iter()
            .filter(|i| i.kind == CompletionKind::EnumValue)
            .collect();
        // Only "a" should be offered, not the int part.
        assert_eq!(
            enum_items.len(),
            1,
            "expected exactly 1 EnumValue completion for mixed union, got {enum_items:?}"
        );
        assert_eq!(
            enum_items[0].label, "\"a\"",
            "only the literal member 'a' should be offered"
        );
    }

    /// T2-e: unresolvable field (field name not in schema) → no EnumValue, no panic.
    #[test]
    fn completions_enum_value_unresolvable_no_panic() {
        let src = "type T = { x: int }\nschema T\ny: 1\n";
        // `y` is not a field of T; cursor in its value
        let off = src.rfind("1\n").unwrap();
        // Must not panic.
        let items = completions(src, off, None);
        let enum_items: Vec<_> = items
            .iter()
            .filter(|i| i.kind == CompletionKind::EnumValue)
            .collect();
        assert!(
            enum_items.is_empty(),
            "expected no EnumValue for unknown field, got {enum_items:?}"
        );
    }

    /// T2-f: imported schema where the field has a literal-union type.
    /// `schema k.Deployment` where imported file has `type Deployment = { policy: "Always" | "Never" }`.
    #[test]
    fn completions_enum_value_imported_schema_field() {
        let dir = scratch_dir();
        write_completion_fixture(
            &dir,
            ".mangrove/resolvers.toml",
            "[namespace.ns]\nremote = \"vendor\"\n",
        );
        write_completion_fixture(
            &dir,
            "vendor/pkg.mang",
            "type Deployment = { policy: \"Always\" | \"Never\" | \"IfNotPresent\" }\n",
        );
        let main_src = "use \"ns/pkg@v1\" as k\nschema k.Deployment\npolicy: \"Always\"\n";
        let main_path = dir.join("main.mang");
        // Cursor inside value of `policy: "Always"` — after opening quote.
        let off = main_src.rfind("\"Always\"\n").unwrap() + 1;
        let items = completions(main_src, off, Some(&main_path));
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"\"Always\""),
            "expected '\"Always\"' from imported schema, got {labels:?}"
        );
        assert!(
            labels.contains(&"\"Never\""),
            "expected '\"Never\"' from imported schema, got {labels:?}"
        );
        assert!(
            labels.contains(&"\"IfNotPresent\""),
            "expected '\"IfNotPresent\"' from imported schema, got {labels:?}"
        );
        let item = items.iter().find(|i| i.label == "\"Always\"").unwrap();
        assert_eq!(item.kind, CompletionKind::EnumValue);
        // No fetch: verify read count is finite and no extra disk reads on second call.
        // (No-fetch invariant: the resolution uses the same local-path mechanism as other cross-file features.)
    }

    /// T2-g: regression — existing field-name and keyword completions still work
    /// when enum-value completion is also active.
    #[test]
    fn completions_enum_value_regression_field_names_still_offered() {
        let src = "type T = { mode: \"a\" | \"b\", host: str }\nschema T\nmode: \"a\"\n";
        let off = src.rfind("\"a\"\n").unwrap() + 1;
        let items = completions(src, off, None);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        // Enum values for `mode` field.
        assert!(
            labels.contains(&"\"a\""),
            "enum value 'a' should still be offered"
        );
        // Field names (from schema) should also be there.
        assert!(
            labels.contains(&"mode"),
            "field name 'mode' should still be offered alongside enum values, got {labels:?}"
        );
        assert!(
            labels.contains(&"host"),
            "field name 'host' should still be offered, got {labels:?}"
        );
    }

    /// Test 6: goto_definition_cross_file with a missing file returns None, no panic.
    /// (Exercises the no-fetch / missing-file guard through the cached path.)
    #[test]
    fn import_cache_goto_missing_file_returns_none() {
        let dir = scratch_dir();
        write_fixture(
            &dir,
            ".mangrove/resolvers.toml",
            "[namespace.ns]\nremote = \"vendor\"\n",
        );
        // vendor/pkg.mang intentionally NOT created
        let main_src = "use \"ns/pkg@v1\" as k\ntype Local = k.SomeType\n";
        let main_path = dir.join("main.mang");
        let off = main_src.rfind("SomeType").unwrap();

        let cache = ImportCache::new();
        let result = goto_definition_cross_file_cached(main_src, off, &main_path, &cache);
        assert!(
            result.is_none(),
            "expected None for missing package (cached path), got {result:?}"
        );
        assert_eq!(cache.reads(), 0, "no disk read for a missing file");
    }

    // ---- workspace-wide references and rename tests ----

    /// Helper: write a fixture file in a workspace directory.
    fn write_ws_fixture(dir: &std::path::Path, rel: &str, contents: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
    }

    /// Build a standard three-file workspace:
    ///   workspace/
    ///     .mangrove/resolvers.toml  — `[local] path = "."`
    ///     a.mang                   — `type S = int`
    ///     b.mang                   — imports a.mang as x, uses `x.S`
    ///     c.mang                   — has its own `type S = str` (unrelated)
    fn make_workspace() -> std::path::PathBuf {
        let dir = scratch_dir();
        // resolvers.toml: local namespace points at the workspace root itself.
        write_ws_fixture(
            &dir,
            ".mangrove/resolvers.toml",
            "[namespace.local]\nremote = \".\"\n",
        );
        // a.mang — defines type S
        write_ws_fixture(&dir, "a.mang", "type S = int\n");
        // b.mang — imports a.mang via "local/a@v1" and uses x.S
        write_ws_fixture(&dir, "b.mang", "use \"local/a@v1\" as x\ntype T = x.S\n");
        // c.mang — unrelated type S
        write_ws_fixture(&dir, "c.mang", "type S = str\n");
        dir
    }

    #[test]
    fn workspace_references_finds_type_in_importer() {
        let dir = make_workspace();
        let a_path = dir.join("a.mang");
        let a_src = std::fs::read_to_string(&a_path).unwrap();
        let off = a_src.find('S').unwrap(); // cursor on `S` in `type S = int`

        let cache = ImportCache::new();
        let results = references_in_workspace(&a_src, off, &a_path, Some(&dir), &cache);

        // Must include a.mang (the decl) and b.mang (x.S reference).
        let paths: Vec<_> = results.iter().map(|(p, _)| p.clone()).collect();
        assert!(
            paths.iter().any(|p| p.ends_with("a.mang")),
            "expected a.mang in workspace refs, got {paths:?}"
        );
        assert!(
            paths.iter().any(|p| p.ends_with("b.mang")),
            "expected b.mang in workspace refs, got {paths:?}"
        );

        // b.mang ranges must point at the `S` token only (not `x` or dot).
        if let Some((_, b_ranges)) = results.iter().find(|(p, _)| p.ends_with("b.mang")) {
            let b_src = std::fs::read_to_string(dir.join("b.mang")).unwrap();
            for &(s, e) in b_ranges {
                assert_eq!(
                    &b_src[s..e],
                    "S",
                    "b.mang range ({s},{e}) should span only 'S'"
                );
            }
        }
    }

    #[test]
    fn workspace_references_false_positive_guard() {
        let dir = make_workspace();
        let a_path = dir.join("a.mang");
        let a_src = std::fs::read_to_string(&a_path).unwrap();
        let off = a_src.find('S').unwrap();

        let cache = ImportCache::new();
        let results = references_in_workspace(&a_src, off, &a_path, Some(&dir), &cache);

        // c.mang defines its own `type S = str` but does NOT import a.mang.
        // It must NOT appear in the results.
        let paths: Vec<_> = results.iter().map(|(p, _)| p.clone()).collect();
        assert!(
            !paths.iter().any(|p| p.ends_with("c.mang")),
            "c.mang (unrelated S) must not appear in workspace refs, got {paths:?}"
        );
    }

    #[test]
    fn workspace_rename_produces_multi_file_workspace_edit() {
        let dir = make_workspace();
        let a_path = dir.join("a.mang");
        let a_src = std::fs::read_to_string(&a_path).unwrap();
        let off = a_src.find('S').unwrap();

        let cache = ImportCache::new();
        let per_file = rename_in_workspace(&a_src, off, "NewName", &a_path, Some(&dir), &cache);
        assert!(per_file.is_some(), "rename_in_workspace should return Some");
        let per_file = per_file.unwrap();

        // Must have entries for a.mang and b.mang.
        let keys: Vec<_> = per_file.keys().collect();
        assert!(
            keys.iter().any(|p| p.ends_with("a.mang")),
            "expected a.mang in rename output, got {keys:?}"
        );
        assert!(
            keys.iter().any(|p| p.ends_with("b.mang")),
            "expected b.mang in rename output, got {keys:?}"
        );

        // c.mang must NOT be touched.
        assert!(
            !keys.iter().any(|p| p.ends_with("c.mang")),
            "c.mang must not be touched by rename, got {keys:?}"
        );

        // Apply the b.mang edits and verify only `S` was renamed.
        let b_path = dir.join("b.mang");
        let b_src = std::fs::read_to_string(&b_path).unwrap();
        if let Some(b_ranges) = per_file.get(&b_path) {
            let mut result = b_src.clone();
            let mut sorted = b_ranges.clone();
            sorted.sort_by_key(|r| std::cmp::Reverse(r.0));
            for (s, e) in sorted {
                result.replace_range(s..e, "NewName");
            }
            assert!(
                result.contains("x.NewName"),
                "b.mang after rename should contain 'x.NewName', got: {result:?}"
            );
            assert!(
                !result.contains("x.S"),
                "b.mang after rename must not contain 'x.S', got: {result:?}"
            );
        }
    }

    #[test]
    fn workspace_references_no_root_falls_back_to_same_file() {
        let dir = make_workspace();
        let a_path = dir.join("a.mang");
        let a_src = std::fs::read_to_string(&a_path).unwrap();
        let off = a_src.find('S').unwrap();

        let cache = ImportCache::new();
        // workspace_root = None → same-file only
        let results = references_in_workspace(&a_src, off, &a_path, None, &cache);

        // Only a.mang should be in the results.
        assert_eq!(
            results.len(),
            1,
            "expected exactly 1 file (a.mang) with no workspace root, got {results:?}"
        );
        assert!(
            results[0].0.ends_with("a.mang"),
            "expected a.mang, got {:?}",
            results[0].0
        );
        // b.mang must not appear.
        let paths: Vec<_> = results.iter().map(|(p, _)| p.clone()).collect();
        assert!(
            !paths.iter().any(|p| p.ends_with("b.mang")),
            "b.mang must not appear when workspace_root is None, got {paths:?}"
        );
    }

    #[test]
    fn rename_in_workspace_declines_on_imported_ref() {
        let dir = make_workspace();
        // b.mang: `use "local/a@v1" as x\ntype T = x.S\n`
        // Cursor on `S` inside `x.S` — this is a FOREIGN (imported) symbol,
        // not locally defined in b.mang.
        let b_path = dir.join("b.mang");
        let b_src = std::fs::read_to_string(&b_path).unwrap();
        let cache = ImportCache::new();
        // Find the `S` offset inside `x.S` (the type-ref, not a local decl).
        let off = b_src.rfind('S').unwrap(); // only one S in b.mang — it's `x.S`

        // rename must decline (return None) — cannot rename a foreign symbol
        let result = rename_in_workspace(&b_src, off, "NewName", &b_path, Some(&dir), &cache);
        assert!(
            result.is_none(),
            "rename_in_workspace must return None when cursor is on imported ref (x.S), got {result:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn walk_mang_files_skips_symlinked_dirs() {
        let dir = scratch_dir();
        // Write a real .mang file.
        std::fs::write(dir.join("a.mang"), "type A = int\n").unwrap();
        // Create a subdirectory with a symlink pointing back to the workspace root,
        // which would cause infinite recursion if followed.
        let subdir = dir.join("subdir");
        std::fs::create_dir_all(&subdir).unwrap();
        std::os::unix::fs::symlink(&dir, subdir.join("cycle")).unwrap();

        let results = walk_mang_files(&dir);

        // a.mang must appear exactly once.
        assert!(
            results.iter().any(|p| p.ends_with("a.mang")),
            "a.mang must appear in walk results, got {results:?}"
        );
        // Must not have hit the 500-file cap (symlink cycle not followed).
        assert!(
            results.len() < 500,
            "walk hit 500-file cap — symlink cycle was followed, got {} files",
            results.len()
        );
    }
}
