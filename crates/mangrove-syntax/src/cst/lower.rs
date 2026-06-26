//! Lower a lossless CST back into the existing `Document`/`Value` AST for
//! evaluation. Trivia is ignored here (it only matters to the formatter/LSP).

use super::super::parser::{
    Document, FnDef, Param, ParseError, Stmt, TypeDef, UnitDef, Use, parse_fndef_str,
    parse_params_str, parse_stmt_str, parse_type, parse_typedef_str, parse_unitdef_str,
    parse_use_str, parse_value_str,
};
use super::kind::{SyntaxKind, SyntaxNode, SyntaxToken};
use crate::ty::Type;
use mangrove_core::Value;
use rowan::NodeOrToken;
use std::collections::BTreeMap;

pub fn lower(node: &SyntaxNode) -> Result<Document, ParseError> {
    debug_assert_eq!(node.kind(), SyntaxKind::DOCUMENT);
    let mut uses: Vec<Use> = Vec::new();
    let mut typedefs: Vec<TypeDef> = Vec::new();
    let mut unitdefs: Vec<UnitDef> = Vec::new();
    let mut schema: Option<String> = None;
    let mut schema_narrow: Option<Type> = None;
    let mut params: Vec<Param> = Vec::new();
    let mut fns: Vec<FnDef> = Vec::new();
    let mut map: BTreeMap<String, Value> = BTreeMap::new();
    let mut stmts: Vec<Stmt> = Vec::new();

    for child in node.children() {
        match child.kind() {
            SyntaxKind::BINDING => {
                let (key, value) = lower_binding(&child)?;
                stmts.push(Stmt::Bind(key.clone(), value.clone()));
                if !matches!(value, Value::Unset) {
                    map.insert(key, value);
                }
            }
            SyntaxKind::SPREAD => {
                let text = node_text(&child);
                let stmt = parse_stmt_str(text.trim())?;
                stmts.push(stmt);
                // NOT folded into body map
            }
            SyntaxKind::LIST_OP_ITEM => {
                let text = node_text(&child);
                let stmt = parse_stmt_str(text.trim())?;
                stmts.push(stmt);
                // NOT folded into body map
            }
            SyntaxKind::USE_DECL => {
                let text = node_text(&child);
                uses.push(parse_use_str(text.trim())?);
            }
            SyntaxKind::TYPE_DEF => {
                let text = node_text(&child);
                typedefs.push(parse_typedef_str(text.trim())?);
            }
            SyntaxKind::UNIT_DEF => {
                let text = node_text(&child);
                unitdefs.push(parse_unitdef_str(text.trim())?);
            }
            SyntaxKind::PARAM_DECL => {
                let text = node_text(&child);
                params.extend(parse_params_str(text.trim())?);
            }
            SyntaxKind::FN_DEF => {
                let text = node_text(&child);
                fns.push(parse_fndef_str(text.trim())?);
            }
            SyntaxKind::SCHEMA_DECL => {
                let (name, narrow) = lower_schema_decl(&child)?;
                schema = Some(name);
                schema_narrow = narrow;
            }
            _ => {}
        }
    }

    Ok(Document {
        uses,
        typedefs,
        unitdefs,
        schema,
        schema_narrow,
        params,
        fns,
        stmts,
        body: Value::Map(map),
    })
}

/// Extract the full text of a node (concatenation of all token texts in the node).
fn node_text(node: &SyntaxNode) -> String {
    node.text().to_string()
}

/// Lower a SCHEMA_DECL node into `(schema_name, schema_narrow)`.
///
/// The node text is something like `schema Foo\n` or `schema Foo & { bar: str }\n`.
/// We parse the name and optional narrow type from the node's raw text.
fn lower_schema_decl(node: &SyntaxNode) -> Result<(String, Option<Type>), ParseError> {
    use super::super::lexer::{Tok, lex};

    let text = node_text(node);
    let tokens = lex(text.trim()).map_err(|e| ParseError {
        message: e.message,
        line: e.line,
        col: e.col,
    })?;
    // tokens[0] = 'schema', tokens[1] = name, then possibly '.' member, '&', type-expr
    let mut pos = 0usize;

    // skip 'schema'
    pos += 1;

    // read name (possibly qualified: Foo or Foo.Bar)
    let mut name = match tokens.get(pos).map(|t| &t.tok) {
        Some(Tok::Bareword(n)) => {
            let n = n.clone();
            pos += 1;
            n
        }
        other => {
            return Err(ParseError {
                message: format!("expected schema name, found {other:?}"),
                line: 0,
                col: 0,
            });
        }
    };
    // optional qualified: `.member`
    if matches!(tokens.get(pos).map(|t| &t.tok), Some(Tok::Dot))
        && matches!(tokens.get(pos + 1).map(|t| &t.tok), Some(Tok::Bareword(_)))
    {
        pos += 1; // skip '.'
        if let Some(Tok::Bareword(member)) = tokens.get(pos).map(|t| &t.tok) {
            name = format!("{name}.{member}");
            pos += 1;
        }
    }

    let narrow = if matches!(tokens.get(pos).map(|t| &t.tok), Some(Tok::Amp)) {
        // Reconstruct the type-expr source from the original text after the '&'.
        let trimmed = text.trim();
        if let Some(amp_byte) = trimmed.find('&') {
            let after_amp = trimmed[amp_byte + 1..].trim();
            Some(parse_type(after_amp)?)
        } else {
            return Err(ParseError {
                message: "expected type after '&' in schema declaration".into(),
                line: 0,
                col: 0,
            });
        }
    } else {
        None
    };

    Ok((name, narrow))
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

/// Lower a RECORD, LIST, or templating-construct node into a `Value`.
fn lower_composite(node: &SyntaxNode) -> Result<Value, ParseError> {
    match node.kind() {
        SyntaxKind::REF | SyntaxKind::UNSET | SyntaxKind::MATCH_EXPR | SyntaxKind::CALL => {
            let text = node_text(node);
            parse_value_str(text.trim())
        }
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
