//! Lower a lossless CST back into the existing `Document`/`Value` AST for
//! evaluation. Trivia is ignored here (it only matters to the formatter/LSP).

use super::super::parser::{Document, ParseError, Stmt};
use super::kind::{SyntaxKind, SyntaxNode, SyntaxToken};
use mangrove_core::Value;
use rowan::NodeOrToken;
use std::collections::BTreeMap;

pub fn lower(node: &SyntaxNode) -> Result<Document, ParseError> {
    debug_assert_eq!(node.kind(), SyntaxKind::DOCUMENT);
    let mut map: BTreeMap<String, Value> = BTreeMap::new();
    let mut stmts: Vec<Stmt> = Vec::new();
    for child in node.children() {
        if child.kind() == SyntaxKind::BINDING {
            let (key, value) = lower_binding(&child)?;
            stmts.push(Stmt::Bind(key.clone(), value.clone()));
            map.insert(key, value);
        }
    }
    Ok(Document {
        uses: Vec::new(),
        typedefs: Vec::new(),
        unitdefs: Vec::new(),
        schema: None,
        schema_narrow: None,
        params: Vec::new(),
        fns: Vec::new(),
        stmts,
        body: Value::Map(map),
    })
}

/// Returns `(key, Value)` for one simple binding.
fn lower_binding(node: &SyntaxNode) -> Result<(String, Value), ParseError> {
    let mut toks = node.children_with_tokens().filter_map(|e| match e {
        NodeOrToken::Token(t) if !t.kind().is_trivia() => Some(t),
        _ => None,
    });
    let key_tok = toks.next().expect("binding has a key");
    let key = decode_key(&key_tok)?;
    // skip COLON
    let val_tok = toks
        .find(|t| t.kind() != SyntaxKind::COLON)
        .expect("binding has a value");
    let value = decode_scalar(&val_tok)?;
    Ok((key, value))
}

fn decode_key(t: &SyntaxToken) -> Result<String, ParseError> {
    match t.kind() {
        SyntaxKind::STR => decode_str_tok(t),
        _ => Ok(t.text().to_string()), // BAREWORD
    }
}

fn decode_scalar(t: &SyntaxToken) -> Result<Value, ParseError> {
    use super::super::lexer::{Tok, lex};
    let text = t.text();
    let tokens = lex(text).map_err(|e| ParseError {
        message: e.message,
        line: e.line,
        col: e.col,
    })?;
    match &tokens[0].tok {
        Tok::Int(n) => Ok(Value::Int(n.clone())),
        Tok::Bool(b) => Ok(Value::Bool(*b)),
        Tok::Str(s) => Ok(Value::Str(s.clone())),
        // DECIMAL/UNIT_LIT/INTERP_STR/BYTES handled in later tasks.
        other => unreachable!("decode_scalar not yet handling {other:?}"),
    }
}

fn decode_str_tok(t: &SyntaxToken) -> Result<String, ParseError> {
    use super::super::lexer::{Tok, lex};
    let text = t.text();
    let tokens = lex(text).map_err(|e| ParseError {
        message: e.message,
        line: e.line,
        col: e.col,
    })?;
    match &tokens[0].tok {
        Tok::Str(s) => Ok(s.clone()),
        other => Err(ParseError {
            message: format!("expected string key, got {other:?}"),
            line: 0,
            col: 0,
        }),
    }
}
