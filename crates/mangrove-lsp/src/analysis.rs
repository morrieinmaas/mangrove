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

// ---- completions ----

/// Top-level language keywords offered as completion items.
const KEYWORDS: &[&str] = &[
    "type", "schema", "unit", "use", "params", "fn", "match", "unset",
];

/// Return completion items for a cursor at `offset` in `src`.
///
/// Pragmatic v1: returns the union of
/// - declared type names (TYPE_DEF / UNIT_DEF) — Kind=TypeName
/// - top-level keywords — Kind=Keyword
/// - record-field names from the document's bound schema type — Kind=Field
///
/// No precise context detection; over-offering is intentional (better than
/// mis-resolving).  Cross-file completion is out of scope.
pub fn completions(src: &str, _offset: usize) -> Vec<CompletionItem> {
    let root = parse_cst(src).syntax();
    let mut items: Vec<CompletionItem> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // Helper to dedup-insert.
    let mut push = |item: CompletionItem| {
        if seen.insert(item.label.clone()) {
            items.push(item);
        }
    };

    // 1. Declared type/unit names.
    for child in root.children() {
        match child.kind() {
            SyntaxKind::TYPE_DEF | SyntaxKind::UNIT_DEF => {
                if let Some(name) = node_decl_name(&child) {
                    push(CompletionItem {
                        label: name,
                        kind: CompletionKind::TypeName,
                    });
                }
            }
            _ => {}
        }
    }

    // 2. Keywords.
    for &kw in KEYWORDS {
        push(CompletionItem {
            label: kw.to_string(),
            kind: CompletionKind::Keyword,
        });
    }

    // 3. Record-field names from the bound schema type (if any).
    //    We lower the document to get the schema name, then resolve the type
    //    definition from the local typedefs.
    if let Some(fields) = schema_record_fields(src) {
        for field in fields {
            push(CompletionItem {
                label: field,
                kind: CompletionKind::Field,
            });
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
        let src = "type Server = { host: str }\nschema Server\nhost: \"x\"\n";
        let items = completions(src, 0);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"Server"),
            "expected 'Server' in completions, got {labels:?}"
        );
        let server = items.iter().find(|i| i.label == "Server").unwrap();
        assert_eq!(server.kind, CompletionKind::TypeName);
    }

    #[test]
    fn completions_include_keywords() {
        let src = "type Server = { host: str }\nschema Server\n";
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
        // Document with a bound record schema — field names should appear.
        let src =
            "type Server = { host: str, port: int }\nschema Server\nhost: \"x\"\nport: 8080\n";
        let items = completions(src, 0);
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
}
