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

/// Returns `(key, Value)` for one binding node.
fn lower_binding(node: &SyntaxNode) -> Result<(String, Value), ParseError> {
    // Extract non-trivia tokens and child nodes from the binding.
    let mut key_opt: Option<SyntaxToken> = None;
    let mut after_colon = false;

    for elem in node.children_with_tokens() {
        match elem {
            NodeOrToken::Token(t) if t.kind().is_trivia() => continue,
            NodeOrToken::Token(t) if t.kind() == SyntaxKind::COLON => {
                after_colon = true;
            }
            NodeOrToken::Token(t) if !after_colon => {
                key_opt = Some(t);
            }
            NodeOrToken::Token(t) if after_colon => {
                let key = decode_key(key_opt.as_ref().expect("key before colon"))?;
                let value = decode_scalar(&t)?;
                return Ok((key, value));
            }
            NodeOrToken::Node(n) if after_colon => {
                let key = decode_key(key_opt.as_ref().expect("key before colon"))?;
                let value = lower_composite(&n)?;
                return Ok((key, value));
            }
            _ => {}
        }
    }
    Err(ParseError {
        message: "binding has no value".into(),
        line: 0,
        col: 0,
    })
}

/// Lower a RECORD or LIST node into a `Value`.
fn lower_composite(node: &SyntaxNode) -> Result<Value, ParseError> {
    match node.kind() {
        SyntaxKind::RECORD => {
            let mut map = BTreeMap::new();
            for child in node.children() {
                if child.kind() == SyntaxKind::FIELD {
                    let (key, value) = lower_field(&child)?;
                    map.insert(key, value);
                }
            }
            Ok(Value::Map(map))
        }
        SyntaxKind::LIST => {
            let mut items = Vec::new();
            for elem in node.children_with_tokens() {
                match elem {
                    NodeOrToken::Token(t) if t.kind().is_trivia() => continue,
                    NodeOrToken::Token(t)
                        if matches!(
                            t.kind(),
                            SyntaxKind::L_BRACKET
                                | SyntaxKind::R_BRACKET
                                | SyntaxKind::COMMA
                                | SyntaxKind::NEWLINE
                        ) => {}
                    NodeOrToken::Token(t) => {
                        items.push(decode_scalar(&t)?);
                    }
                    NodeOrToken::Node(n) => {
                        items.push(lower_composite(&n)?);
                    }
                }
            }
            Ok(Value::List(items))
        }
        other => Err(ParseError {
            message: format!("unexpected node kind in value position: {other:?}"),
            line: 0,
            col: 0,
        }),
    }
}

/// Returns `(key, Value)` for one FIELD node inside a RECORD.
fn lower_field(node: &SyntaxNode) -> Result<(String, Value), ParseError> {
    let mut key_opt: Option<SyntaxToken> = None;
    let mut after_colon = false;

    for elem in node.children_with_tokens() {
        match elem {
            NodeOrToken::Token(t) if t.kind().is_trivia() => continue,
            NodeOrToken::Token(t) if t.kind() == SyntaxKind::COLON => {
                after_colon = true;
            }
            NodeOrToken::Token(t) if !after_colon => {
                key_opt = Some(t);
            }
            NodeOrToken::Token(t) if after_colon => {
                let key = decode_key(key_opt.as_ref().expect("key before colon"))?;
                let value = decode_scalar(&t)?;
                return Ok((key, value));
            }
            NodeOrToken::Node(n) if after_colon => {
                let key = decode_key(key_opt.as_ref().expect("key before colon"))?;
                let value = lower_composite(&n)?;
                return Ok((key, value));
            }
            _ => {}
        }
    }
    Err(ParseError {
        message: "field has no value".into(),
        line: 0,
        col: 0,
    })
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
        Tok::Decimal(d) => Ok(Value::Decimal(d.clone())),
        Tok::UnitLit(mantissa, suffix) => Ok(Value::Unit {
            mantissa: mantissa.clone(),
            suffix: suffix.clone(),
        }),
        Tok::InterpStr(parts) => Ok(Value::Interp(parts.clone())),
        Tok::Bytes(b) => Ok(Value::Bytes(b.clone())),
        other => Err(ParseError {
            message: format!("unexpected scalar token: {other:?}"),
            line: 0,
            col: 0,
        }),
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
