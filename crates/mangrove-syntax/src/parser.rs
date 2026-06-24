//! L0 recursive-descent parser: token stream → `mangrove_core::Value`.
//!
//! A document is a sequence of `key: value` bindings forming the root map
//! (Decision D2). `{ … }` is a map, `[ … ]` a list. Entries are separated by a
//! newline or comma, trailing separators allowed. `##`/`#!` tokens are treated
//! as separators here (not folded into the value — Decision D5).

use crate::lexer::{Tok, Token, lex};
use crate::ty::{FieldDef, Type};
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

/// A parsed document: any local `type`/`unit` definitions, an optional `schema`
/// binding, and the data body. A pure L0 document has empty defs and
/// `schema == None`.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    pub typedefs: Vec<(String, Type)>,
    pub unitdefs: Vec<UnitDef>,
    pub schema: Option<String>,
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
        let mut typedefs = Vec::new();
        let mut unitdefs = Vec::new();
        let mut schema: Option<String> = None;
        let mut body = BTreeMap::new();
        self.skip_seps();
        loop {
            if self.at_eof() {
                break;
            }
            if self.is_keyword_stmt("type") {
                typedefs.push(self.parse_typedef()?);
            } else if self.is_keyword_stmt("unit") {
                unitdefs.push(self.parse_unitdef()?);
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
                schema = Some(name);
            } else {
                let (key, value) = self.parse_binding(0)?;
                if body.contains_key(&key) {
                    return Err(self.error(format!("duplicate key {key:?}")));
                }
                body.insert(key, value);
            }

            let had_sep = self.at_sep();
            self.skip_seps();
            if !had_sep && !self.at_eof() {
                return Err(self.error("expected ',' or newline between statements".into()));
            }
        }
        Ok(Document {
            typedefs,
            unitdefs,
            schema,
            body: Value::Map(body),
        })
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

    fn parse_typedef(&mut self) -> Result<(String, Type), ParseError> {
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
        Ok((name, ty))
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
            Tok::Bool(b) => {
                self.advance();
                Ok(Value::Bool(b))
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
        while self.check(&Tok::Pipe) {
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
        self.skip_seps();
        loop {
            if self.check(&Tok::RBrace) {
                self.advance();
                break;
            }
            if self.at_eof() {
                return Err(self.error("unterminated record type".into()));
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
            fields.push(FieldDef { name, optional, ty });

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
        Ok(Type::Record { fields })
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
        assert_eq!(d.typedefs[0].0, "Port");
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
        let (name, ty) = &d.typedefs[0];
        assert_eq!(name, "Satoshis");
        let Type::Brand { name: bn, inner } = ty else {
            panic!()
        };
        assert_eq!(bn, "Satoshis");
        assert!(matches!(**inner, Type::IntRange { .. }));
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
