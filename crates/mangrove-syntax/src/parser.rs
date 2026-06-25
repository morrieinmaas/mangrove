//! L0 recursive-descent parser: token stream → `mangrove_core::Value`.
//!
//! A document is a sequence of `key: value` bindings forming the root map
//! (Decision D2). `{ … }` is a map, `[ … ]` a list. Entries are separated by a
//! newline or comma, trailing separators allowed. `##`/`#!` tokens are treated
//! as separators here (not folded into the value — Decision D5).

use crate::lexer::{Tok, Token, lex};
use crate::ty::{Annotation, FieldDef, Type};
use bigdecimal::BigDecimal;
use mangrove_core::Value;
use num_bigint::BigInt;
use std::collections::BTreeMap;
use std::fmt;

/// Parse a single type expression (test/embedding entrypoint).
pub fn parse_type(src: &str) -> Result<Type, ParseError> {
    let tokens = lex(src).map_err(|e| ParseError {
        message: e.message,
        line: e.line,
        col: e.col,
    })?;
    let mut p = Parser { tokens, pos: 0 };
    let ty = p.parse_type_expr(0)?;
    if !p.at_eof() {
        return Err(p.error(format!("unexpected token after type: {:?}", p.peek().tok)));
    }
    Ok(ty)
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub line: usize,
    pub col: usize,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.col, self.message)
    }
}

impl std::error::Error for ParseError {}

/// Maximum container nesting depth. Past this, parsing errors instead of
/// recursing — guards against stack-overflow (SIGABRT) on adversarial input
/// like `[[[[…`. Generous for real config, far below the overflow threshold.
const MAX_DEPTH: usize = 128;

/// A unit type definition (§4.5): each member mapped to its base integer value
/// (e.g. `Mi -> 1048576`), in declaration order.
#[derive(Debug, Clone, PartialEq)]
pub struct UnitDef {
    pub name: String,
    pub members: Vec<(String, BigInt)>,
}

/// A named type definition with its metadata annotations (§4.9).
#[derive(Debug, Clone, PartialEq)]
pub struct TypeDef {
    pub name: String,
    pub ty: Type,
    pub annotations: Vec<Annotation>,
}

/// A local import (`use ./path.mang as alias`). M3a is local-path-only.
#[derive(Debug, Clone, PartialEq)]
pub struct Use {
    pub path: String,
    pub alias: String,
}

/// One L3 parameter (`name: <type> [= <default>]`, §6.1). A param with a default
/// is optional; one without is required (D34).
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub ty: Type,
    pub default: Option<Value>,
}

/// A body statement (L2), ordered so composition folds them left-to-right.
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// `key: value`
    Bind(String, Value),
    /// `...alias`
    Spread(String),
    /// `key += [ … ]` — append to the inherited list.
    Append(String, Value),
    /// `key { patch …, append: …, remove: … }` — `@key` list operations (§5.3).
    ListOp(String, Vec<ListOpItem>),
}

/// One operation inside a `@key` list-op block (§5.3).
#[derive(Debug, Clone, PartialEq)]
pub enum ListOpItem {
    /// `patch "k": { … }` — deep-merge into the element whose key field == "k".
    Patch(String, Value),
    /// `append: { … }` — add an element (error if its key already exists).
    Append(Value),
    /// `remove: "k"` — drop the element whose key field == "k".
    Remove(String),
}

/// A parsed document: local `use`s, any `type`/`unit` definitions, an optional
/// `schema` binding, the ordered body statements, and — for the common
/// spread-free case — the folded body value. A pure L0 document has empty defs,
/// no uses/spreads, and `schema == None`.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    pub uses: Vec<Use>,
    pub typedefs: Vec<TypeDef>,
    pub unitdefs: Vec<UnitDef>,
    pub schema: Option<String>,
    /// Subtype-redefinition narrowing record (`schema Base & { … }`, §5.5).
    pub schema_narrow: Option<Type>,
    /// L3 `params` block (§6.1); empty for L0–L2 documents.
    pub params: Vec<Param>,
    /// Ordered body statements (binds + spreads), for the compose driver.
    pub stmts: Vec<Stmt>,
    /// The folded body of plain bindings (spread-free path; used by `hash`
    /// until composition runs). Spreads do not contribute here.
    pub body: Value,
}

/// Parse a complete document (typedefs + schema + body).
pub fn parse_document(src: &str) -> Result<Document, ParseError> {
    let tokens = lex(src).map_err(|e| ParseError {
        message: e.message,
        line: e.line,
        col: e.col,
    })?;
    Parser { tokens, pos: 0 }.parse_doc()
}

/// Parse a document and return just its data body (L0 entrypoint; used by `hash`).
pub fn parse(src: &str) -> Result<Value, ParseError> {
    parse_document(src).map(|d| d.body)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Token {
        // The lexer always terminates the stream with a `Tok::Eof`.
        &self.tokens[self.pos]
    }

    fn advance(&mut self) {
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
    }

    fn at_eof(&self) -> bool {
        matches!(self.peek().tok, Tok::Eof)
    }

    fn check(&self, t: &Tok) -> bool {
        &self.peek().tok == t
    }

    /// Whether the token *after* the current one equals `t`.
    fn next_is(&self, t: &Tok) -> bool {
        self.tokens.get(self.pos + 1).map(|x| &x.tok) == Some(t)
    }

    fn at_sep(&self) -> bool {
        matches!(
            self.peek().tok,
            Tok::Newline | Tok::Comma | Tok::Doc(_) | Tok::Directive(_)
        )
    }

    fn skip_seps(&mut self) {
        while self.at_sep() {
            self.advance();
        }
    }

    fn expect(&mut self, t: &Tok) -> Result<(), ParseError> {
        if self.check(t) {
            self.advance();
            Ok(())
        } else {
            Err(self.error(format!("expected {:?}, found {:?}", t, self.peek().tok)))
        }
    }

    fn error(&self, message: String) -> ParseError {
        let tok = self.peek();
        ParseError {
            message,
            line: tok.line,
            col: tok.col,
        }
    }

    /// Walk top-level statements: `type X = …` definitions, one `schema X`
    /// binding, and ordinary `key: value` bindings forming the body.
    fn parse_doc(&mut self) -> Result<Document, ParseError> {
        let mut uses = Vec::new();
        let mut typedefs = Vec::new();
        let mut unitdefs = Vec::new();
        let mut schema: Option<String> = None;
        let mut schema_narrow: Option<Type> = None;
        let mut params: Vec<Param> = Vec::new();
        let mut stmts: Vec<Stmt> = Vec::new();
        let mut body = BTreeMap::new();
        self.skip_seps();
        loop {
            if self.at_eof() {
                break;
            }
            if matches!(&self.peek().tok, Tok::Bareword(b) if b == "use")
                && matches!(
                    self.tokens.get(self.pos + 1).map(|t| &t.tok),
                    Some(Tok::Str(_))
                )
            {
                uses.push(self.parse_use()?);
            } else if self.is_keyword_stmt("type") {
                typedefs.push(self.parse_typedef()?);
            } else if self.is_keyword_stmt("unit") {
                unitdefs.push(self.parse_unitdef()?);
            } else if matches!(&self.peek().tok, Tok::Bareword(b) if b == "params")
                && self.next_is(&Tok::LBrace)
            {
                if !params.is_empty() {
                    return Err(self.error("duplicate `params` block".into()));
                }
                params = self.parse_params()?;
            } else if self.is_keyword_stmt("schema") {
                self.advance(); // 'schema'
                let name = match self.peek().tok.clone() {
                    Tok::Bareword(n) => {
                        self.advance();
                        n
                    }
                    other => {
                        return Err(self.error(format!("expected a schema name, found {other:?}")));
                    }
                };
                if schema.is_some() {
                    return Err(self.error("duplicate `schema` statement".into()));
                }
                // optional subtype redefinition: `schema Base & { … }` (§5.5)
                if self.check(&Tok::Amp) {
                    self.advance();
                    schema_narrow = Some(self.parse_type_expr(0)?);
                }
                schema = Some(name);
            } else if self.check(&Tok::DotDotDot) {
                // `...alias` spread
                self.advance();
                let alias = match self.peek().tok.clone() {
                    Tok::Bareword(n) => {
                        self.advance();
                        n
                    }
                    other => {
                        return Err(self.error(format!("expected a spread alias, found {other:?}")));
                    }
                };
                stmts.push(Stmt::Spread(alias));
            } else {
                // A keyed body statement: `k: v`, `k += [..]`, or `k { ops }`.
                let key = match self.peek().tok.clone() {
                    Tok::Bareword(n) => {
                        self.advance();
                        n
                    }
                    Tok::Str(n) => {
                        self.advance();
                        n
                    }
                    other => return Err(self.error(format!("expected a key, found {other:?}"))),
                };
                match self.peek().tok {
                    Tok::Colon => {
                        self.advance();
                        let value = self.parse_value(0)?;
                        // The folded body is the spread-free convenience view
                        // (used by `hash` until composition runs). A duplicate
                        // plain key is a typo error; `unset` with no base → absent.
                        if !matches!(value, Value::Unset) {
                            if body.contains_key(&key) {
                                return Err(self.error(format!("duplicate key {key:?}")));
                            }
                            body.insert(key.clone(), value.clone());
                        }
                        stmts.push(Stmt::Bind(key, value));
                    }
                    Tok::PlusEq => {
                        self.advance();
                        let value = self.parse_value(0)?;
                        stmts.push(Stmt::Append(key, value));
                    }
                    Tok::LBrace => {
                        let items = self.parse_list_op_block()?;
                        stmts.push(Stmt::ListOp(key, items));
                    }
                    ref other => {
                        return Err(self.error(format!(
                            "expected ':', '+=', or '{{' after key, found {other:?}"
                        )));
                    }
                }
            }

            let had_sep = self.at_sep();
            self.skip_seps();
            if !had_sep && !self.at_eof() {
                return Err(self.error("expected ',' or newline between statements".into()));
            }
        }
        Ok(Document {
            uses,
            typedefs,
            unitdefs,
            schema,
            schema_narrow,
            params,
            stmts,
            body: Value::Map(body),
        })
    }

    /// `params { name: <type> [= <default>], … }` (§6.1). A default makes the
    /// param optional (D34). Entries are newline/comma separated.
    fn parse_params(&mut self) -> Result<Vec<Param>, ParseError> {
        self.advance(); // 'params'
        self.expect(&Tok::LBrace)?;
        let mut out: Vec<Param> = Vec::new();
        self.skip_seps();
        while !self.check(&Tok::RBrace) {
            if self.at_eof() {
                return Err(self.error("unterminated `params` block".into()));
            }
            let name = match self.peek().tok.clone() {
                Tok::Bareword(n) => {
                    self.advance();
                    n
                }
                other => return Err(self.error(format!("expected a param name, found {other:?}"))),
            };
            if out.iter().any(|p| p.name == name) {
                return Err(self.error(format!("duplicate param {name:?}")));
            }
            self.expect(&Tok::Colon)?;
            let ty = self.parse_type_expr(0)?;
            let default = if self.check(&Tok::Eq) {
                self.advance();
                Some(self.parse_value(0)?)
            } else {
                None
            };
            out.push(Param { name, ty, default });
            let had_sep = self.at_sep();
            self.skip_seps();
            if !had_sep && !self.check(&Tok::RBrace) {
                return Err(self.error("expected ',' or newline between params".into()));
            }
        }
        self.expect(&Tok::RBrace)?;
        Ok(out)
    }

    /// `use ./path.mang as alias` (M3a: local relative path only).
    fn parse_use(&mut self) -> Result<Use, ParseError> {
        self.advance(); // 'use'
        let path = match self.peek().tok.clone() {
            Tok::Str(s) => {
                self.advance();
                s
            }
            other => {
                return Err(self.error(format!(
                    "expected a quoted path after `use`, found {other:?}"
                )));
            }
        };
        // expect `as alias`
        match self.peek().tok.clone() {
            Tok::Bareword(b) if b == "as" => self.advance(),
            other => {
                return Err(self.error(format!("expected `as` after use path, found {other:?}")));
            }
        }
        let alias = match self.peek().tok.clone() {
            Tok::Bareword(n) => {
                self.advance();
                n
            }
            other => return Err(self.error(format!("expected an alias, found {other:?}"))),
        };
        Ok(Use { path, alias })
    }

    /// `{ patch "k": value, append: value, remove: "k", … }` (§5.3).
    fn parse_list_op_block(&mut self) -> Result<Vec<ListOpItem>, ParseError> {
        self.expect(&Tok::LBrace)?;
        let mut items = Vec::new();
        self.skip_seps();
        loop {
            if self.check(&Tok::RBrace) {
                self.advance();
                break;
            }
            if self.at_eof() {
                return Err(self.error("unterminated list-op block".into()));
            }
            let verb = match self.peek().tok.clone() {
                Tok::Bareword(b) => {
                    self.advance();
                    b
                }
                other => {
                    return Err(
                        self.error(format!("expected patch/append/remove, found {other:?}"))
                    );
                }
            };
            let item = match verb.as_str() {
                "patch" => {
                    let key = match self.peek().tok.clone() {
                        Tok::Str(s) => {
                            self.advance();
                            s
                        }
                        other => {
                            return Err(self.error(format!(
                                "expected a key string after patch, found {other:?}"
                            )));
                        }
                    };
                    self.expect(&Tok::Colon)?;
                    ListOpItem::Patch(key, self.parse_value(0)?)
                }
                "append" => {
                    self.expect(&Tok::Colon)?;
                    ListOpItem::Append(self.parse_value(0)?)
                }
                "remove" => {
                    self.expect(&Tok::Colon)?;
                    match self.peek().tok.clone() {
                        Tok::Str(s) => {
                            self.advance();
                            ListOpItem::Remove(s)
                        }
                        other => {
                            return Err(self.error(format!(
                                "expected a key string after remove, found {other:?}"
                            )));
                        }
                    }
                }
                other => {
                    return Err(self.error(format!("unknown list op `{other}`")));
                }
            };
            items.push(item);
            let had_sep = self.at_sep();
            self.skip_seps();
            if self.check(&Tok::RBrace) {
                self.advance();
                break;
            }
            if !had_sep {
                return Err(self.error("expected ',' or newline in list-op block".into()));
            }
        }
        Ok(items)
    }

    /// True if the current statement is the keyword `kw` followed by a bareword
    /// (a `type`/`schema` statement), as opposed to a field named `kw` (`kw:`).
    fn is_keyword_stmt(&self, kw: &str) -> bool {
        matches!(&self.peek().tok, Tok::Bareword(b) if b == kw)
            && matches!(
                self.tokens.get(self.pos + 1).map(|t| &t.tok),
                Some(Tok::Bareword(_))
            )
    }

    fn parse_typedef(&mut self) -> Result<TypeDef, ParseError> {
        self.advance(); // 'type'
        let name = match self.peek().tok.clone() {
            Tok::Bareword(n) => {
                self.advance();
                n
            }
            other => return Err(self.error(format!("expected a type name, found {other:?}"))),
        };
        self.expect(&Tok::Eq)?;
        let ty = self.parse_type_expr(0)?;
        // A top-level `type X = brand …` takes its identity from the typedef name.
        let ty = match ty {
            Type::Brand { inner, .. } => Type::Brand {
                name: name.clone(),
                inner,
            },
            other => other,
        };
        let annotations = self.parse_annotations()?;
        Ok(TypeDef {
            name,
            ty,
            annotations,
        })
    }

    /// Parse a run of `@name(arg)` annotations (§4.9), possibly empty.
    fn parse_annotations(&mut self) -> Result<Vec<Annotation>, ParseError> {
        let mut anns = Vec::new();
        while self.check(&Tok::At) {
            self.advance(); // @
            let name = match self.peek().tok.clone() {
                Tok::Bareword(n) => {
                    self.advance();
                    n
                }
                other => {
                    return Err(self.error(format!("expected an annotation name, found {other:?}")));
                }
            };
            let arg = if self.check(&Tok::LParen) {
                self.advance(); // (
                // arg is a string (`@message("…")`) or a bareword (`@key(name)`).
                let s = match self.peek().tok.clone() {
                    Tok::Str(s) | Tok::Bareword(s) => {
                        self.advance();
                        s
                    }
                    other => {
                        return Err(
                            self.error(format!("expected an annotation argument, found {other:?}"))
                        );
                    }
                };
                self.expect(&Tok::RParen)?;
                Some(s)
            } else {
                None
            };
            anns.push(Annotation { name, arg });
        }
        Ok(anns)
    }

    /// `unit Name : int { member = value, … }` (§4.5). Each member value is an
    /// integer or `<coefficient><earlier-member>` (Decision D13), evaluated to a
    /// base integer.
    fn parse_unitdef(&mut self) -> Result<UnitDef, ParseError> {
        self.advance(); // 'unit'
        let name = match self.peek().tok.clone() {
            Tok::Bareword(n) => {
                self.advance();
                n
            }
            other => return Err(self.error(format!("expected a unit name, found {other:?}"))),
        };
        self.expect(&Tok::Colon)?;
        match self.peek().tok.clone() {
            Tok::Bareword(b) if b == "int" => self.advance(),
            other => {
                return Err(self.error(format!("unit base type must be `int`, found {other:?}")));
            }
        }
        self.expect(&Tok::LBrace)?;
        let mut members: Vec<(String, BigInt)> = Vec::new();
        self.skip_seps();
        loop {
            if self.check(&Tok::RBrace) {
                self.advance();
                break;
            }
            if self.at_eof() {
                return Err(self.error("unterminated unit declaration".into()));
            }
            let mname = match self.peek().tok.clone() {
                Tok::Bareword(n) => {
                    self.advance();
                    n
                }
                other => return Err(self.error(format!("expected a unit member, found {other:?}"))),
            };
            self.expect(&Tok::Eq)?;
            let base = match self.peek().tok.clone() {
                Tok::Int(n) => {
                    self.advance();
                    n
                }
                Tok::UnitLit(coeff, suffix) => {
                    self.advance();
                    let c = mangrove_core::exact_bigint(&coeff).ok_or_else(|| {
                        self.error(format!(
                            "unit member coefficient must be an integer: {coeff}"
                        ))
                    })?;
                    let earlier = members
                        .iter()
                        .find(|(n, _)| *n == suffix)
                        .map(|(_, b)| b.clone())
                        .ok_or_else(|| {
                            self.error(format!("unknown earlier unit member `{suffix}`"))
                        })?;
                    c * earlier
                }
                other => {
                    return Err(self.error(format!(
                        "unit member value must be an integer or <int><member>, found {other:?}"
                    )));
                }
            };
            if members.iter().any(|(n, _)| *n == mname) {
                return Err(self.error(format!("duplicate unit member `{mname}`")));
            }
            members.push((mname, base));

            let had_sep = self.at_sep();
            self.skip_seps();
            if self.check(&Tok::RBrace) {
                self.advance();
                break;
            }
            if !had_sep {
                return Err(self.error("expected ',' or newline in unit declaration".into()));
            }
        }
        Ok(UnitDef { name, members })
    }

    /// Parse a sequence of bindings. `top_level` documents end at EOF; nested
    /// maps end at `}` (which this consumes). `depth` is the current container
    /// nesting level (see [`MAX_DEPTH`]).
    fn parse_bindings(
        &mut self,
        top_level: bool,
        depth: usize,
    ) -> Result<BTreeMap<String, Value>, ParseError> {
        let mut map = BTreeMap::new();
        self.skip_seps();
        loop {
            if top_level && self.at_eof() {
                break;
            }
            if !top_level && self.check(&Tok::RBrace) {
                self.advance();
                break;
            }
            if self.at_eof() {
                return Err(self.error("unexpected end of input, expected '}'".into()));
            }
            let (key, value) = self.parse_binding(depth)?;
            if map.contains_key(&key) {
                return Err(self.error(format!("duplicate key {key:?}")));
            }
            map.insert(key, value);

            let had_sep = self.at_sep();
            self.skip_seps();
            let at_terminator =
                (top_level && self.at_eof()) || (!top_level && self.check(&Tok::RBrace));
            if !had_sep && !at_terminator {
                return Err(self.error("expected ',' or newline between entries".into()));
            }
        }
        Ok(map)
    }

    fn parse_binding(&mut self, depth: usize) -> Result<(String, Value), ParseError> {
        let key = match &self.peek().tok {
            Tok::Bareword(s) => s.clone(),
            Tok::Str(s) => s.clone(),
            other => return Err(self.error(format!("expected a key, found {other:?}"))),
        };
        self.advance();
        self.expect(&Tok::Colon)?;
        let value = self.parse_value(depth)?;
        Ok((key, value))
    }

    /// `match <name> { <pat>: <value>, … }` (§6.1). A pattern is a literal value
    /// (a bareword is a string literal) or `_` (wildcard → `None`).
    fn parse_match(&mut self, depth: usize) -> Result<Value, ParseError> {
        if depth >= MAX_DEPTH {
            return Err(self.error("nesting too deep".into()));
        }
        self.advance(); // 'match'
        let scrutinee = match self.peek().tok.clone() {
            Tok::Bareword(n) => {
                self.advance();
                Value::Ref(n)
            }
            other => {
                return Err(self.error(format!("expected a name to match on, found {other:?}")));
            }
        };
        self.expect(&Tok::LBrace)?;
        let mut arms: Vec<(Option<Value>, Value)> = Vec::new();
        self.skip_seps();
        while !self.check(&Tok::RBrace) {
            if self.at_eof() {
                return Err(self.error("unterminated `match`".into()));
            }
            let pat = match self.peek().tok.clone() {
                Tok::Bareword(b) if b == "_" => {
                    self.advance();
                    None
                }
                Tok::Bareword(b) => {
                    self.advance();
                    Some(Value::Str(b))
                }
                Tok::Str(s) => {
                    self.advance();
                    Some(Value::Str(s))
                }
                Tok::Int(n) => {
                    self.advance();
                    Some(Value::Int(n))
                }
                Tok::Bool(x) => {
                    self.advance();
                    Some(Value::Bool(x))
                }
                other => {
                    return Err(self.error(format!("expected a match pattern, found {other:?}")));
                }
            };
            self.expect(&Tok::Colon)?;
            let val = self.parse_value(depth + 1)?;
            arms.push((pat, val));
            let had_sep = self.at_sep();
            self.skip_seps();
            if !had_sep && !self.check(&Tok::RBrace) {
                return Err(self.error("expected ',' or newline between match arms".into()));
            }
        }
        self.expect(&Tok::RBrace)?;
        if arms.is_empty() {
            return Err(self.error("`match` needs at least one arm".into()));
        }
        Ok(Value::Match {
            scrutinee: Box::new(scrutinee),
            arms,
        })
    }

    fn parse_value(&mut self, depth: usize) -> Result<Value, ParseError> {
        match self.peek().tok.clone() {
            Tok::Int(n) => {
                self.advance();
                Ok(Value::Int(n))
            }
            Tok::Decimal(d) => {
                self.advance();
                Ok(Value::Decimal(d))
            }
            Tok::UnitLit(mantissa, suffix) => {
                self.advance();
                Ok(Value::Unit { mantissa, suffix })
            }
            Tok::Str(s) => {
                self.advance();
                Ok(Value::Str(s))
            }
            Tok::InterpStr(parts) => {
                self.advance();
                Ok(Value::Interp(parts))
            }
            Tok::Bool(b) => {
                self.advance();
                Ok(Value::Bool(b))
            }
            // `unset` (§5.4): the composition marker; legal anywhere a value is.
            Tok::Bareword(b) if b == "unset" => {
                self.advance();
                Ok(Value::Unset)
            }
            // `match scrutinee { pat: val, … }` (§6.1), reduced by eval (M4c).
            Tok::Bareword(b) if b == "match" => self.parse_match(depth),
            // Any other bare identifier in value position is an L3 reference to a
            // param or sibling binding (§6.1), reduced by the eval stage (M4a).
            Tok::Bareword(name) => {
                self.advance();
                Ok(Value::Ref(name))
            }
            Tok::Bytes(b) => {
                self.advance();
                Ok(Value::Bytes(b))
            }
            Tok::LBrace => {
                if depth >= MAX_DEPTH {
                    return Err(self.error("nesting too deep".into()));
                }
                self.advance();
                Ok(Value::Map(self.parse_bindings(false, depth + 1)?))
            }
            Tok::LBracket => {
                if depth >= MAX_DEPTH {
                    return Err(self.error("nesting too deep".into()));
                }
                self.advance();
                self.parse_list(depth + 1)
            }
            other => Err(self.error(format!("expected a value, found {other:?}"))),
        }
    }

    fn parse_list(&mut self, depth: usize) -> Result<Value, ParseError> {
        let mut items = Vec::new();
        self.skip_seps();
        loop {
            if self.check(&Tok::RBracket) {
                self.advance();
                break;
            }
            if self.at_eof() {
                return Err(self.error("unexpected end of input, expected ']'".into()));
            }
            items.push(self.parse_value(depth)?);

            let had_sep = self.at_sep();
            self.skip_seps();
            if self.check(&Tok::RBracket) {
                self.advance();
                break;
            }
            if !had_sep {
                return Err(self.error("expected ',' or newline in list".into()));
            }
        }
        Ok(Value::List(items))
    }

    // ---- type grammar (L1) ----

    /// `union = intersection { "|" intersection }`. `depth` is the container
    /// nesting level — guards the type parser against stack-overflow on inputs
    /// like `[[[[…` exactly as the value parser does (see [`MAX_DEPTH`]).
    fn parse_type_expr(&mut self, depth: usize) -> Result<Type, ParseError> {
        let mut variants = vec![self.parse_intersection(depth)?];
        // `| *value` is a field default, not a union variant — stop before it.
        while self.check(&Tok::Pipe) && !self.next_is(&Tok::Star) {
            self.advance();
            variants.push(self.parse_intersection(depth)?);
        }
        if variants.len() == 1 {
            Ok(variants.pop().unwrap())
        } else {
            Ok(Type::Union(variants))
        }
    }

    /// `intersection = atom { "&" refinement }`
    fn parse_intersection(&mut self, depth: usize) -> Result<Type, ParseError> {
        let mut ty = self.parse_atom(depth)?;
        while self.check(&Tok::Amp) {
            self.advance();
            ty = self.apply_refinement(ty)?;
        }
        Ok(ty)
    }

    fn parse_atom(&mut self, depth: usize) -> Result<Type, ParseError> {
        // `brand <type>` — a nominal newtype; the name is filled by the typedef.
        if matches!(&self.peek().tok, Tok::Bareword(b) if b == "brand") {
            self.advance();
            let inner = self.parse_intersection(depth)?;
            return Ok(Type::Brand {
                name: String::new(),
                inner: Box::new(inner),
            });
        }
        match self.peek().tok.clone() {
            Tok::Bareword(name) => {
                self.advance();
                Ok(match name.as_str() {
                    "int" => Type::Int,
                    "decimal" => Type::Decimal,
                    "str" => Type::Str,
                    "bool" => Type::Bool,
                    "bytes" => Type::Bytes,
                    _ => Type::Named(name),
                })
            }
            Tok::Str(s) => {
                self.advance();
                Ok(Type::LitStr(s))
            }
            Tok::Int(n) => {
                self.advance();
                Ok(Type::LitInt(n))
            }
            Tok::Bool(b) => {
                self.advance();
                Ok(Type::LitBool(b))
            }
            Tok::LBracket => {
                if depth >= MAX_DEPTH {
                    return Err(self.error("type nesting too deep".into()));
                }
                self.advance();
                let inner = self.parse_type_expr(depth + 1)?;
                self.skip_seps();
                self.expect(&Tok::RBracket)?;
                Ok(Type::List(Box::new(inner)))
            }
            Tok::LBrace => {
                if depth >= MAX_DEPTH {
                    return Err(self.error("type nesting too deep".into()));
                }
                self.parse_record_or_map(depth + 1)
            }
            other => Err(self.error(format!("expected a type, found {other:?}"))),
        }
    }

    /// `{ [str]: V }` (map) or `{ name [?] : type, … }` (record). `depth` is
    /// already the nesting level of this `{` (incremented by the caller).
    fn parse_record_or_map(&mut self, depth: usize) -> Result<Type, ParseError> {
        self.expect(&Tok::LBrace)?;
        if self.check(&Tok::LBracket) {
            // map: { [str]: V }
            self.advance();
            match self.peek().tok.clone() {
                Tok::Bareword(k) if k == "str" => self.advance(),
                other => {
                    return Err(self.error(format!("map key type must be `str`, found {other:?}")));
                }
            }
            self.expect(&Tok::RBracket)?;
            self.expect(&Tok::Colon)?;
            let v = self.parse_type_expr(depth)?;
            self.skip_seps();
            self.expect(&Tok::RBrace)?;
            return Ok(Type::Map(Box::new(v)));
        }
        let mut fields = Vec::new();
        let mut requires = Vec::new();
        self.skip_seps();
        loop {
            if self.check(&Tok::RBrace) {
                self.advance();
                break;
            }
            if self.at_eof() {
                return Err(self.error("unterminated record type".into()));
            }
            // `require: <predicate> [@message(...)]` — a cross-field constraint,
            // not a field. `require` is reserved as a field name inside records.
            if matches!(&self.peek().tok, Tok::Bareword(b) if b == "require")
                && self.next_is(&Tok::Colon)
            {
                self.advance(); // 'require'
                self.expect(&Tok::Colon)?;
                let pred = self.parse_pred(0)?;
                let anns = self.parse_annotations()?;
                let message = Annotation::find(&anns, "message").map(str::to_string);
                requires.push(crate::ty::Require { pred, message });
                let had_sep = self.at_sep();
                self.skip_seps();
                if self.check(&Tok::RBrace) {
                    self.advance();
                    break;
                }
                if !had_sep {
                    return Err(self.error("expected ',' or newline in record type".into()));
                }
                continue;
            }
            let name = match self.peek().tok.clone() {
                Tok::Bareword(n) => {
                    self.advance();
                    n
                }
                Tok::Str(n) => {
                    self.advance();
                    n
                }
                other => return Err(self.error(format!("expected a field name, found {other:?}"))),
            };
            let optional = if self.check(&Tok::Question) {
                self.advance();
                true
            } else {
                false
            };
            self.expect(&Tok::Colon)?;
            let ty = self.parse_type_expr(depth)?;
            // optional default: `| *value`
            let default = if self.check(&Tok::Pipe) && self.next_is(&Tok::Star) {
                self.advance(); // |
                self.advance(); // *
                Some(self.parse_value(depth)?)
            } else {
                None
            };
            let annotations = self.parse_annotations()?;
            fields.push(FieldDef {
                name,
                optional,
                ty,
                default,
                annotations,
            });

            let had_sep = self.at_sep();
            self.skip_seps();
            if self.check(&Tok::RBrace) {
                self.advance();
                break;
            }
            if !had_sep {
                return Err(self.error("expected ',' or newline in record type".into()));
            }
        }
        Ok(Type::Record { fields, requires })
    }

    // ---- require predicate sublanguage (§4.7) ----

    fn parse_pred(&mut self, depth: usize) -> Result<crate::ty::Pred, ParseError> {
        // `depth` tracks the resulting tree's depth, so it bounds not just parse
        // recursion but also the evaluator's recursion and the AST's recursive
        // `Drop` — a flat `a && a && …` chain builds a left-deep tree that would
        // otherwise overflow eval/Drop even though the parse loop is iterative.
        if depth >= MAX_DEPTH {
            return Err(self.error("predicate nesting too deep".into()));
        }
        self.parse_pred_or(depth)
    }

    fn parse_pred_or(&mut self, depth: usize) -> Result<crate::ty::Pred, ParseError> {
        let mut lhs = self.parse_pred_and(depth)?;
        let mut d = depth;
        while self.check(&Tok::PipePipe) {
            self.advance();
            d += 1; // each chain link deepens the left-deep tree
            if d >= MAX_DEPTH {
                return Err(self.error("predicate nesting too deep".into()));
            }
            let rhs = self.parse_pred_and(d)?;
            lhs = crate::ty::Pred::Or(Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_pred_and(&mut self, depth: usize) -> Result<crate::ty::Pred, ParseError> {
        let mut lhs = self.parse_pred_not(depth)?;
        let mut d = depth;
        while self.check(&Tok::AmpAmp) {
            self.advance();
            d += 1;
            if d >= MAX_DEPTH {
                return Err(self.error("predicate nesting too deep".into()));
            }
            let rhs = self.parse_pred_not(d)?;
            lhs = crate::ty::Pred::And(Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_pred_not(&mut self, depth: usize) -> Result<crate::ty::Pred, ParseError> {
        if self.check(&Tok::Bang) {
            if depth >= MAX_DEPTH {
                return Err(self.error("predicate nesting too deep".into()));
            }
            self.advance();
            Ok(crate::ty::Pred::Not(Box::new(
                self.parse_pred_not(depth + 1)?,
            )))
        } else {
            self.parse_pred_cmp(depth)
        }
    }

    fn parse_pred_cmp(&mut self, depth: usize) -> Result<crate::ty::Pred, ParseError> {
        use crate::ty::{CmpOp, Pred};
        let lhs = self.parse_operand(depth)?;
        let op = match self.peek().tok {
            Tok::EqEq => Some(CmpOp::Eq),
            Tok::Ne => Some(CmpOp::Ne),
            Tok::Lt => Some(CmpOp::Lt),
            Tok::Le => Some(CmpOp::Le),
            Tok::Gt => Some(CmpOp::Gt),
            Tok::Ge => Some(CmpOp::Ge),
            _ => None,
        };
        match op {
            Some(op) => {
                self.advance();
                let rhs = self.parse_operand(depth)?;
                Ok(Pred::Compare { op, lhs, rhs })
            }
            None => Ok(Pred::Truthy(lhs)),
        }
    }

    fn parse_operand(&mut self, depth: usize) -> Result<crate::ty::Operand, ParseError> {
        use crate::ty::Operand;
        match self.peek().tok.clone() {
            Tok::LParen => {
                self.advance();
                let p = self.parse_pred(depth + 1)?;
                self.expect(&Tok::RParen)?;
                Ok(Operand::Pred(Box::new(p)))
            }
            Tok::Bareword(b) if b == "len" && self.next_is(&Tok::LParen) => {
                self.advance(); // len
                self.expect(&Tok::LParen)?;
                let path = self.parse_path()?;
                self.expect(&Tok::RParen)?;
                Ok(Operand::Len(path))
            }
            Tok::Bareword(_) => Ok(Operand::Path(self.parse_path()?)),
            Tok::Int(n) => {
                self.advance();
                Ok(Operand::Int(n))
            }
            Tok::Decimal(d) => {
                self.advance();
                Ok(Operand::Decimal(d))
            }
            Tok::Str(s) => {
                self.advance();
                Ok(Operand::Str(s))
            }
            Tok::Bool(b) => {
                self.advance();
                Ok(Operand::Bool(b))
            }
            other => Err(self.error(format!("expected a predicate operand, found {other:?}"))),
        }
    }

    fn parse_path(&mut self) -> Result<Vec<String>, ParseError> {
        let mut segs = Vec::new();
        loop {
            match self.peek().tok.clone() {
                Tok::Bareword(n) => {
                    self.advance();
                    segs.push(n);
                }
                other => return Err(self.error(format!("expected a field name, found {other:?}"))),
            }
            if self.check(&Tok::Dot) {
                self.advance();
            } else {
                break;
            }
        }
        Ok(segs)
    }

    /// Fold a refinement (after `&`) into `ty`. Enforces refinement-atom
    /// compatibility (D10/F2): bounds only on int/decimal, regex only on str.
    /// Dispatch is by the *atom's* kind, so a decimal atom accepts an int bound
    /// (`decimal & >= 0`), but an int atom rejects a decimal bound.
    fn apply_refinement(&mut self, ty: Type) -> Result<Type, ParseError> {
        match self.peek().tok.clone() {
            op @ (Tok::Ge | Tok::Le | Tok::Gt | Tok::Lt) => {
                self.advance();
                match &ty {
                    Type::Int | Type::IntRange { .. } => {
                        let n = match self.peek().tok.clone() {
                            Tok::Int(n) => {
                                self.advance();
                                n
                            }
                            other => {
                                return Err(self.error(format!(
                                    "int bound must be an integer, found {other:?}"
                                )));
                            }
                        };
                        Ok(refine_int(ty, &op, n))
                    }
                    Type::Decimal | Type::DecRange { .. } => {
                        let d = match self.peek().tok.clone() {
                            Tok::Int(n) => {
                                self.advance();
                                BigDecimal::from(n)
                            }
                            Tok::Decimal(d) => {
                                self.advance();
                                d
                            }
                            other => {
                                return Err(self.error(format!(
                                    "decimal bound must be a number, found {other:?}"
                                )));
                            }
                        };
                        if matches!(op, Tok::Gt | Tok::Lt) {
                            return Err(
                                self.error("strict < / > on decimal is unsupported in M2a".into())
                            );
                        }
                        Ok(refine_dec(ty, &op, d))
                    }
                    other => Err(self.error(format!(
                        "interval bound applies only to int/decimal, not {other:?}"
                    ))),
                }
            }
            Tok::Match => {
                self.advance();
                match self.peek().tok.clone() {
                    Tok::Str(re) => {
                        self.advance();
                        match ty {
                            Type::Str => Ok(Type::StrRegex(re)),
                            other => {
                                Err(self.error(format!("=~ applies only to str, not {other:?}")))
                            }
                        }
                    }
                    other => {
                        Err(self.error(format!("expected a string after =~, found {other:?}")))
                    }
                }
            }
            other => Err(self.error(format!("expected a refinement after &, found {other:?}"))),
        }
    }
}

fn refine_int(ty: Type, op: &Tok, n: BigInt) -> Type {
    let (mut min, mut max) = match ty {
        Type::IntRange { min, max } => (min, max),
        _ => (None, None),
    };
    match op {
        Tok::Ge => min = Some(n),
        Tok::Gt => min = Some(n + BigInt::from(1)),
        Tok::Le => max = Some(n),
        Tok::Lt => max = Some(n - BigInt::from(1)),
        _ => unreachable!(),
    }
    Type::IntRange { min, max }
}

fn refine_dec(ty: Type, op: &Tok, d: BigDecimal) -> Type {
    let (mut min, mut max) = match ty {
        Type::DecRange { min, max } => (min, max),
        _ => (None, None),
    };
    match op {
        Tok::Ge => min = Some(d),
        Tok::Le => max = Some(d),
        _ => unreachable!(),
    }
    Type::DecRange { min, max }
}

#[cfg(test)]
mod tests {
    use super::{parse, parse_document};
    use mangrove_core::Value;
    use num_bigint::BigInt;

    #[test]
    fn parses_typedefs_schema_and_body() {
        let d = parse_document(
            "type Port = int & >= 1 & <= 65535\nschema Server\nhost: \"x\"\nport: 8443",
        )
        .unwrap();
        assert_eq!(d.typedefs.len(), 1);
        assert_eq!(d.typedefs[0].name, "Port");
        assert_eq!(d.schema.as_deref(), Some("Server"));
        let Value::Map(m) = &d.body else { panic!() };
        assert!(m.contains_key("host") && m.contains_key("port"));
    }

    #[test]
    fn field_named_type_or_schema_is_a_binding_not_a_statement() {
        let d = parse_document("type: \"lib\"\nschema: \"x\"").unwrap();
        assert!(d.typedefs.is_empty() && d.schema.is_none());
        let Value::Map(m) = &d.body else { panic!() };
        assert_eq!(m.get("type"), Some(&Value::Str("lib".into())));
        assert_eq!(m.get("schema"), Some(&Value::Str("x".into())));
    }

    #[test]
    fn pure_l0_document_has_no_typedefs_or_schema() {
        let d = parse_document("a: 1\nb: 2").unwrap();
        assert!(d.typedefs.is_empty() && d.schema.is_none());
        assert!(d.uses.is_empty());
    }

    #[test]
    fn spread_and_unset_and_use_parse() {
        use super::{Stmt, Use};
        let d = parse_document("use \"./base.mang\" as base\n...base\nport: 9090\ndebug: unset\n")
            .unwrap();
        assert_eq!(
            d.uses,
            vec![Use {
                path: "./base.mang".into(),
                alias: "base".into()
            }]
        );
        // stmts in order: Spread(base), Bind(port,9090), Bind(debug, Unset)
        assert_eq!(d.stmts.len(), 3);
        assert!(matches!(&d.stmts[0], Stmt::Spread(a) if a == "base"));
        assert!(matches!(&d.stmts[1], Stmt::Bind(k, _) if k == "port"));
        assert!(matches!(&d.stmts[2], Stmt::Bind(k, v) if k == "debug" && *v == Value::Unset));
        // folded body (spread-free view) has the plain bind, not the unset
        let Value::Map(m) = &d.body else { panic!() };
        assert_eq!(m.get("port"), Some(&Value::Int(9090.into())));
        assert!(!m.contains_key("debug"));
    }

    #[test]
    fn two_schema_statements_is_error() {
        assert!(parse_document("schema A\nschema B").is_err());
    }

    #[test]
    fn parses_unit_declaration() {
        let d = parse_document(
            "unit Bytes : int { B = 1, Ki = 1024B, Mi = 1024Ki }\nschema Bytes\nx: 1\n",
        )
        .unwrap();
        assert_eq!(d.unitdefs.len(), 1);
        let b = &d.unitdefs[0];
        assert_eq!(b.name, "Bytes");
        assert_eq!(
            b.members.iter().find(|(n, _)| n == "Mi").unwrap().1,
            1_048_576.into()
        );
    }

    #[test]
    fn brand_type_parses_and_takes_typedef_name() {
        use crate::ty::Type;
        let d =
            parse_document("type Satoshis = brand int & >= 0\nschema Satoshis\nx: 1\n").unwrap();
        let td = &d.typedefs[0];
        assert_eq!(td.name, "Satoshis");
        let Type::Brand { name: bn, inner } = &td.ty else {
            panic!()
        };
        assert_eq!(bn, "Satoshis");
        assert!(matches!(**inner, Type::IntRange { .. }));
    }

    #[test]
    fn deeply_nested_require_errors_instead_of_overflowing() {
        // Was: SIGABRT via parser/evaluator/Drop recursion. Now a clean error.
        let parens = format!(
            "type C = {{ a: int, require: {}a == a{} }}\nschema C\n",
            "(".repeat(5000),
            ")".repeat(5000)
        );
        assert!(parse_document(&parens).is_err());
        let bangs = format!(
            "type C = {{ a: bool, require: {}a }}\nschema C\n",
            "!".repeat(5000)
        );
        assert!(parse_document(&bangs).is_err());
        // a long flat && chain (left-deep tree) is also bounded
        let chain = format!(
            "type C = {{ a: bool, require: {} }}\nschema C\n",
            vec!["a"; 5000].join(" && ")
        );
        assert!(parse_document(&chain).is_err());
        // a reasonable predicate still parses
        assert!(
            parse_document(
                "type C = { a: int, b: int, require: (a <= b) && (b >= 0) }\nschema C\n"
            )
            .is_ok()
        );
    }

    #[test]
    fn require_clause_parses() {
        use crate::ty::Type;
        let d = parse_document(
            "type C = { a: int, b: int, require: a <= b @message(\"m\") }\nschema C\n",
        )
        .unwrap();
        let Type::Record { requires, fields } = &d.typedefs[0].ty else {
            panic!()
        };
        assert_eq!(fields.len(), 2);
        assert_eq!(requires.len(), 1);
        assert_eq!(requires[0].message.as_deref(), Some("m"));
    }

    #[test]
    fn annotations_parse_on_typedef_and_field() {
        use crate::ty::Type;
        let d = parse_document(
            "type Port = int @doc(\"p\") @message(\"m\")\ntype R = { image: str @deprecated(\"use x\") }\nschema R\n",
        )
        .unwrap();
        let port = d.typedefs.iter().find(|t| t.name == "Port").unwrap();
        assert_eq!(port.annotations.len(), 2);
        assert_eq!(
            crate::ty::Annotation::find(&port.annotations, "message"),
            Some("m")
        );
        let r = d.typedefs.iter().find(|t| t.name == "R").unwrap();
        let Type::Record { fields, .. } = &r.ty else {
            panic!()
        };
        assert_eq!(
            crate::ty::Annotation::find(&fields[0].annotations, "deprecated"),
            Some("use x")
        );
    }

    #[test]
    fn unit_member_unknown_ref_errors() {
        assert!(parse_document("unit U : int { a = 1b }\nschema U\n").is_err());
    }

    #[test]
    fn unit_field_named_is_a_binding() {
        // `unit:` (colon) is a field, not a unit declaration.
        let d = parse_document("unit: \"x\"").unwrap();
        assert!(d.unitdefs.is_empty());
    }

    #[test]
    fn top_level_bindings_make_a_map() {
        let v = parse("name: \"x\"\nn: 1").unwrap();
        match v {
            Value::Map(m) => {
                assert_eq!(m.get("name"), Some(&Value::Str("x".into())));
                assert_eq!(m.get("n"), Some(&Value::Int(BigInt::from(1))));
            }
            _ => panic!("expected map"),
        }
    }

    #[test]
    fn nested_map_and_list() {
        let v = parse("a: { b: [1, 2] }").unwrap();
        let Value::Map(m) = v else { panic!() };
        let Some(Value::Map(inner)) = m.get("a") else {
            panic!()
        };
        assert_eq!(
            inner.get("b"),
            Some(&Value::List(vec![
                Value::Int(BigInt::from(1)),
                Value::Int(BigInt::from(2))
            ]))
        );
    }

    #[test]
    fn comma_or_newline_separators_and_trailing() {
        assert!(parse("a: 1, b: 2,").is_ok());
        assert!(parse("a: 1\nb: 2\n").is_ok());
    }

    #[test]
    fn missing_separator_is_error() {
        assert!(parse("a: 1 b: 2").is_err());
    }

    #[test]
    fn duplicate_key_is_error() {
        assert!(parse("a: 1\na: 2").is_err());
    }

    #[test]
    fn parse_error_reports_position() {
        let e = parse("a: ").unwrap_err();
        assert!(format!("{e}").contains(':'), "{e}");
    }

    #[test]
    fn deep_nesting_errors_instead_of_overflowing() {
        // Was: SIGABRT stack overflow. Now a clean error well before the limit.
        let src = format!("a: {}", "[".repeat(5000));
        assert!(parse(&src).is_err());
        // A reasonable nesting depth still parses fine.
        let ok = format!("a: {}{}", "[".repeat(50), "]".repeat(50));
        assert!(parse(&ok).is_ok());
    }
}
