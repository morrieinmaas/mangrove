//! Pure analysis over a single Mangrove document: parse + type/compose
//! diagnostics, document symbols, hover, and semantic-token classification —
//! all derived from the lossless CST and the existing type pipeline.
//!
//! Single-file and **read-only**: imports are *not* resolved (no network, no
//! lockfile fetch). Cross-file `use` diagnostics are out of scope for v0.4.0;
//! a document that `use`s namespaced modules simply skips the type-check stage
//! (its parse + local-symbol features still work).

use mangrove_syntax::cst::{SyntaxKind, SyntaxNode, lower, parse_cst};
use rowan::TextRange;

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
    //    own messages. We attach them to the whole document range (precise span
    //    mapping for nested paths is a future enhancement).
    if let Some(msg) = type_check(&root) {
        let end = src.len();
        out.push(Diagnostic {
            range: (0, end),
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
    fn hover_on_a_type_name_shows_the_declaration_and_doc() {
        let src = "## the server schema\ntype Server = { host: str }\nschema Server\n";
        // offset into "Server" on the type line (after the doc comment + "type ")
        let off = src.find("Server").unwrap();
        let h = hover(src, off).expect("hover");
        assert!(h.contains("type"), "hover: {h}");
        assert!(h.contains("the server schema"), "doc missing: {h}");
    }
}
