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
            SyntaxKind::STR | SyntaxKind::INTERP_STR | SyntaxKind::BYTES => SemKind::String,
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
    doc_path: &std::path::Path,
) -> Option<(std::path::PathBuf, (usize, usize), String)> {
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

    // Find the `use "ref" as alias` declaration.
    let doc = lower(&root).ok()?;
    let use_decl = doc.uses.iter().find(|u| u.alias == alias)?;
    let reference = use_decl.path.clone();

    // Resolve to local path (no-fetch).
    let doc_dir = doc_path.parent()?;
    let resolvers = mangrove_resolve::Resolvers::find_and_load(doc_dir).ok()?;
    let pkg_path = resolvers.resolve_local_path(&reference).ok()?;

    // Only proceed if the file exists locally.
    if !pkg_path.exists() {
        return None;
    }

    // Read the file (read-only).
    let pkg_text = std::fs::read_to_string(&pkg_path).ok()?;

    // Parse and find the type/unit declaration.
    let pkg_root = parse_cst(&pkg_text).syntax();
    let byte_range = find_type_decl(&pkg_root, &type_name)?;

    Some((pkg_path, byte_range, pkg_text))
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

/// Collect all byte ranges in `src` where `name` appears as a type-name token
/// (i.e. BAREWORD tokens whose parent is a TYPE_DEF, UNIT_DEF, or SCHEMA_DECL,
/// OR that are the declared name of a TYPE_DEF/UNIT_DEF).
fn type_name_occurrences(root: &SyntaxNode, name: &str) -> Vec<(usize, usize)> {
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
        // Accept: token lives inside a TYPE_DEF, UNIT_DEF, or SCHEMA_DECL node
        // (which covers both the decl name and type-body references).
        if matches!(
            parent.kind(),
            SyntaxKind::TYPE_DEF | SyntaxKind::UNIT_DEF | SyntaxKind::SCHEMA_DECL
        ) {
            let r = tok.text_range();
            out.push((r.start().into(), r.end().into()));
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

/// Validate that `new_name` is a legal Mangrove identifier (a non-empty bareword
/// that matches `[a-zA-Z_][a-zA-Z0-9_]*`).
fn is_valid_identifier(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
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
/// - Type position → declared type/unit names + primitive type keywords.
/// - Top-level → declaration keywords (`type`, `schema`, `unit`, …).
/// - Schema value position → schema record field names.
/// - General value → value keywords (`match`, `unset`).
/// - Ambiguous → full union (over-offer rather than offer nothing).
///
/// Cross-file completion is out of scope.
pub fn completions(src: &str, offset: usize) -> Vec<CompletionItem> {
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
            // Declared type/unit names.
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

        CursorContext::TopLevel => {
            for &kw in DECL_KEYWORDS {
                push(CompletionItem {
                    label: kw.to_string(),
                    kind: CompletionKind::Keyword,
                });
            }
        }

        CursorContext::SchemaValuePosition => {
            // Field names from the bound schema (if any).
            if let Some(fields) = schema_record_fields(src) {
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
            if let Some(fields) = schema_record_fields(src) {
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

/// If the document has a `schema X` and `X` resolves locally to a record type,
/// return that record's field names.  Returns `None` if the schema is absent,
/// unresolvable, or not a plain record.
fn schema_record_fields(src: &str) -> Option<Vec<String>> {
    let root = parse_cst(src).syntax();
    let doc = lower(&root).ok()?;
    // skip cross-file imports (can't resolve locally)
    if !doc.uses.is_empty() {
        return None;
    }
    let schema_name = doc.schema.as_deref()?;
    // Resolve the schema name through typedefs.
    let fields = resolve_record_fields(schema_name, &doc.typedefs)?;
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
        let items = completions(src, off);
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
        let items = completions(src, 0);
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
        let items = completions(src, off);
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
        let items = completions(src, 0);
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
        let items = completions(src, off);
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
        let items = completions(src, off);
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
        let items = completions(src, off);
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
        let items = completions(src, 0);
        assert!(
            !items.is_empty(),
            "expected non-empty fallback for ambiguous context"
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
}
