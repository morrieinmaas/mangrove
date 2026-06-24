//! L0 recursive-descent parser: token stream → `mangrove_core::Value`.
//!
//! A document is a sequence of `key: value` bindings forming the root map
//! (Decision D2). `{ … }` is a map, `[ … ]` a list. Entries are separated by a
//! newline or comma, trailing separators allowed. `##`/`#!` tokens are treated
//! as separators here (not folded into the value — Decision D5).

use crate::lexer::{Tok, Token, lex};
use mangrove_core::Value;
use std::collections::BTreeMap;
use std::fmt;

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

/// Parse a complete L0 document into its (canonical) value.
pub fn parse(src: &str) -> Result<Value, ParseError> {
    let tokens = lex(src).map_err(|e| ParseError {
        message: e.message,
        line: e.line,
        col: e.col,
    })?;
    Parser { tokens, pos: 0 }.parse_document()
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

    fn parse_document(&mut self) -> Result<Value, ParseError> {
        Ok(Value::Map(self.parse_bindings(true)?))
    }

    /// Parse a sequence of bindings. `top_level` documents end at EOF; nested
    /// maps end at `}` (which this consumes).
    fn parse_bindings(&mut self, top_level: bool) -> Result<BTreeMap<String, Value>, ParseError> {
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
            let (key, value) = self.parse_binding()?;
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

    fn parse_binding(&mut self) -> Result<(String, Value), ParseError> {
        let key = match &self.peek().tok {
            Tok::Bareword(s) => s.clone(),
            Tok::Str(s) => s.clone(),
            other => return Err(self.error(format!("expected a key, found {other:?}"))),
        };
        self.advance();
        self.expect(&Tok::Colon)?;
        let value = self.parse_value()?;
        Ok((key, value))
    }

    fn parse_value(&mut self) -> Result<Value, ParseError> {
        match self.peek().tok.clone() {
            Tok::Int(n) => {
                self.advance();
                Ok(Value::Int(n))
            }
            Tok::Decimal(d) => {
                self.advance();
                Ok(Value::Decimal(d))
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
                self.advance();
                Ok(Value::Map(self.parse_bindings(false)?))
            }
            Tok::LBracket => {
                self.advance();
                self.parse_list()
            }
            other => Err(self.error(format!("expected a value, found {other:?}"))),
        }
    }

    fn parse_list(&mut self) -> Result<Value, ParseError> {
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
            items.push(self.parse_value()?);

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
}

#[cfg(test)]
mod tests {
    use super::parse;
    use mangrove_core::Value;
    use num_bigint::BigInt;

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
}
