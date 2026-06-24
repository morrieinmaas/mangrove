//! L0 lexer: turns source text into a flat token stream.
//!
//! Whitespace (spaces/tabs/CR) is insignificant; a newline is its own token
//! (`Newline`) because the parser uses it as a binding separator. Ordinary `#`
//! comments are discarded; `##` (doc) and `#!` (directive) are kept on the
//! stream (their canonical-form treatment is decided in M2 — see the M1 spec).

use bigdecimal::BigDecimal;
use num_bigint::BigInt;
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Colon,
    Comma,
    Newline,
    // L1 type-grammar tokens
    Amp,      // &
    Pipe,     // |
    Eq,       // =
    Match,    // =~
    Question, // ?
    Ge,       // >=
    Le,       // <=
    Gt,       // >
    Lt,       // <
    Int(BigInt),
    Decimal(BigDecimal),
    /// A unit literal: `(mantissa, suffix)`, e.g. `512Mi` → `(512, "Mi")`.
    /// Resolved against a unit type during validation (M2b).
    UnitLit(BigDecimal, String),
    Str(String),
    Bool(bool),
    Bytes(Vec<u8>),
    Bareword(String),
    Doc(String),
    Directive(String),
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub tok: Tok,
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LexError {
    pub message: String,
    pub line: usize,
    pub col: usize,
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.col, self.message)
    }
}

impl std::error::Error for LexError {}

/// Tokenize `src`, or return the first lexical error.
pub fn lex(src: &str) -> Result<Vec<Token>, LexError> {
    Lexer {
        chars: src.chars().collect(),
        pos: 0,
        line: 1,
        col: 1,
    }
    .run()
}

struct Lexer {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident_continue(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

impl Lexer {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.chars.get(self.pos + n).copied()
    }

    fn starts_with(&self, s: &str) -> bool {
        s.chars()
            .enumerate()
            .all(|(i, c)| self.peek_at(i) == Some(c))
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.chars.get(self.pos).copied();
        match c {
            Some('\n') => {
                self.pos += 1;
                self.line += 1;
                self.col = 1;
            }
            Some(_) => {
                self.pos += 1;
                self.col += 1;
            }
            None => {}
        }
        c
    }

    fn skip_inline_ws(&mut self) {
        while matches!(self.peek(), Some(' ') | Some('\t') | Some('\r')) {
            self.bump();
        }
    }

    fn take_to_eol(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c == '\n' {
                break;
            }
            s.push(c);
            self.bump();
        }
        s
    }

    fn run(&mut self) -> Result<Vec<Token>, LexError> {
        let mut out = Vec::new();
        loop {
            self.skip_inline_ws();
            let (line, col) = (self.line, self.col);
            let Some(c) = self.peek() else {
                out.push(Token {
                    tok: Tok::Eof,
                    line,
                    col,
                });
                return Ok(out);
            };
            let tok = match c {
                '\n' => {
                    self.bump();
                    Tok::Newline
                }
                '{' => {
                    self.bump();
                    Tok::LBrace
                }
                '}' => {
                    self.bump();
                    Tok::RBrace
                }
                '[' => {
                    self.bump();
                    Tok::LBracket
                }
                ']' => {
                    self.bump();
                    Tok::RBracket
                }
                ':' => {
                    self.bump();
                    Tok::Colon
                }
                ',' => {
                    self.bump();
                    Tok::Comma
                }
                '&' => {
                    self.bump();
                    Tok::Amp
                }
                '|' => {
                    self.bump();
                    Tok::Pipe
                }
                '?' => {
                    self.bump();
                    Tok::Question
                }
                '=' => {
                    self.bump();
                    if self.peek() == Some('~') {
                        self.bump();
                        Tok::Match
                    } else {
                        Tok::Eq
                    }
                }
                '>' => {
                    self.bump();
                    if self.peek() == Some('=') {
                        self.bump();
                        Tok::Ge
                    } else {
                        Tok::Gt
                    }
                }
                '<' => {
                    self.bump();
                    if self.peek() == Some('=') {
                        self.bump();
                        Tok::Le
                    } else {
                        Tok::Lt
                    }
                }
                '#' => {
                    self.bump();
                    if self.peek() == Some('!') {
                        self.bump();
                        Tok::Directive(self.take_to_eol())
                    } else if self.peek() == Some('#') {
                        self.bump();
                        Tok::Doc(self.take_to_eol())
                    } else {
                        self.take_to_eol();
                        continue;
                    }
                }
                '"' => self.lex_string_or_block(line, col)?,
                'r' if self.peek_at(1) == Some('"') => {
                    self.bump(); // consume 'r'
                    // `r"""…"""` is a raw text block (spec §3.4); `r"…"` a raw string.
                    // Text blocks carry no escapes either way, so both route to the
                    // same literal text-block lexer.
                    if self.starts_with("\"\"\"") {
                        self.lex_text_block(line, col)?
                    } else {
                        self.lex_raw_string(line, col)?
                    }
                }
                'b' if self.starts_with("b64\"") => self.lex_bytes(line, col)?,
                '-' | '0'..='9' => self.lex_number(line, col)?,
                c if is_ident_start(c) => self.lex_bareword(),
                _ => {
                    return Err(LexError {
                        message: format!("unexpected character {c:?}"),
                        line,
                        col,
                    });
                }
            };
            out.push(Token { tok, line, col });
        }
    }

    fn lex_bareword(&mut self) -> Tok {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if is_ident_continue(c) {
                s.push(c);
                self.bump();
            } else {
                break;
            }
        }
        match s.as_str() {
            "true" => Tok::Bool(true),
            "false" => Tok::Bool(false),
            _ => Tok::Bareword(s),
        }
    }

    fn take_digits(&mut self, s: &mut String) {
        while matches!(self.peek(), Some(c) if c.is_ascii_digit() || c == '_') {
            s.push(self.peek().unwrap());
            self.bump();
        }
    }

    fn lex_number(&mut self, line: usize, col: usize) -> Result<Tok, LexError> {
        let mut s = String::new();
        let mut is_decimal = false;
        if self.peek() == Some('-') {
            s.push('-');
            self.bump();
        }
        // After an optional sign there must be at least one digit; otherwise this
        // is not a number (e.g. a stray `-`), and a clear error beats the
        // misleading unit-literal message below.
        if !matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            return Err(LexError {
                message: "expected a number".into(),
                line,
                col,
            });
        }
        self.take_digits(&mut s);
        if self.peek() == Some('.') {
            is_decimal = true;
            s.push('.');
            self.bump();
            self.take_digits(&mut s);
        }
        if matches!(self.peek(), Some('e') | Some('E')) {
            is_decimal = true;
            s.push('e');
            self.bump();
            if matches!(self.peek(), Some('+') | Some('-')) {
                s.push(self.peek().unwrap());
                self.bump();
            }
            self.take_digits(&mut s);
        }
        // A number immediately followed by an identifier is a unit literal
        // (`512Mi`); it is resolved against a unit type later (D14).
        if matches!(self.peek(), Some(c) if is_ident_start(c)) {
            let mut suffix = String::new();
            while matches!(self.peek(), Some(c) if c.is_ascii_alphanumeric() || c == '_') {
                suffix.push(self.peek().unwrap());
                self.bump();
            }
            let cleaned: String = s.chars().filter(|c| *c != '_').collect();
            let mantissa = BigDecimal::from_str(&cleaned).map_err(|e| LexError {
                message: format!("invalid unit-literal number {cleaned:?}: {e}"),
                line,
                col,
            })?;
            return Ok(Tok::UnitLit(mantissa, suffix));
        }
        let cleaned: String = s.chars().filter(|c| *c != '_').collect();
        if is_decimal {
            BigDecimal::from_str(&cleaned)
                .map(Tok::Decimal)
                .map_err(|e| LexError {
                    message: format!("invalid decimal {cleaned:?}: {e}"),
                    line,
                    col,
                })
        } else {
            BigInt::from_str(&cleaned)
                .map(Tok::Int)
                .map_err(|e| LexError {
                    message: format!("invalid integer {cleaned:?}: {e}"),
                    line,
                    col,
                })
        }
    }

    fn lex_string_or_block(&mut self, line: usize, col: usize) -> Result<Tok, LexError> {
        if self.peek_at(1) == Some('"') && self.peek_at(2) == Some('"') {
            self.lex_text_block(line, col)
        } else {
            self.lex_plain_string(line, col)
        }
    }

    fn lex_plain_string(&mut self, line: usize, col: usize) -> Result<Tok, LexError> {
        self.bump(); // opening "
        let mut s = String::new();
        loop {
            match self.peek() {
                None => return Err(self.err("unterminated string", line, col)),
                Some('"') => {
                    self.bump();
                    return Ok(Tok::Str(s));
                }
                Some('\\') => {
                    self.bump();
                    self.read_escape(&mut s, line, col)?;
                }
                Some(c) => {
                    s.push(c);
                    self.bump();
                }
            }
        }
    }

    fn read_escape(&mut self, s: &mut String, line: usize, col: usize) -> Result<(), LexError> {
        match self.peek() {
            Some('"') => s.push('"'),
            Some('\\') => s.push('\\'),
            Some('n') => s.push('\n'),
            Some('t') => s.push('\t'),
            Some('r') => s.push('\r'),
            Some('0') => s.push('\0'),
            Some('$') => s.push('$'),
            Some('x') => {
                self.bump(); // consume 'x'
                let hi = self.hex_digit(line, col)?;
                let lo = self.hex_digit(line, col)?;
                let byte = (hi << 4) | lo;
                // `\xNN` is restricted to ASCII (0x00–0x7F); use `\u{…}` for
                // higher code points, so the byte-vs-code-point ambiguity cannot arise.
                if byte > 0x7f {
                    return Err(self.err("\\x escape must be 0x00–0x7F; use \\u{…}", line, col));
                }
                s.push(byte as char);
                return Ok(());
            }
            Some('u') => {
                self.bump(); // consume 'u'
                if self.peek() != Some('{') {
                    return Err(self.err("expected '{' after \\u", line, col));
                }
                self.bump();
                let mut code = 0u32;
                while let Some(c) = self.peek() {
                    if c == '}' {
                        break;
                    }
                    let d = c
                        .to_digit(16)
                        .ok_or_else(|| self.err("invalid \\u hex digit", line, col))?;
                    code = code
                        .checked_mul(16)
                        .and_then(|c| c.checked_add(d))
                        .ok_or_else(|| self.err("\\u escape out of range", line, col))?;
                    self.bump();
                }
                if self.peek() != Some('}') {
                    return Err(self.err("unterminated \\u escape", line, col));
                }
                self.bump();
                let ch = char::from_u32(code)
                    .ok_or_else(|| self.err("invalid unicode scalar", line, col))?;
                s.push(ch);
                return Ok(());
            }
            _ => return Err(self.err("invalid escape", line, col)),
        }
        self.bump();
        Ok(())
    }

    fn hex_digit(&mut self, line: usize, col: usize) -> Result<u8, LexError> {
        let d = self
            .peek()
            .and_then(|c| c.to_digit(16))
            .ok_or_else(|| self.err("invalid \\x hex digit", line, col))?;
        self.bump();
        Ok(d as u8)
    }

    fn lex_raw_string(&mut self, line: usize, col: usize) -> Result<Tok, LexError> {
        self.bump(); // opening " (the 'r' was already consumed)
        let mut s = String::new();
        loop {
            match self.peek() {
                None => return Err(self.err("unterminated raw string", line, col)),
                Some('"') => {
                    self.bump();
                    return Ok(Tok::Str(s));
                }
                Some(c) => {
                    s.push(c);
                    self.bump();
                }
            }
        }
    }

    fn lex_text_block(&mut self, line: usize, col: usize) -> Result<Tok, LexError> {
        self.bump();
        self.bump();
        self.bump(); // consume opening """
        // The opening line must contain only whitespace then a newline (spec §3.4
        // grammar). Enforcing this prevents `"""hello"""` from silently dedenting
        // to the empty string.
        while matches!(self.peek(), Some(' ') | Some('\t') | Some('\r')) {
            self.bump();
        }
        if self.peek() != Some('\n') {
            return Err(self.err(
                "text block content must begin on the line after \"\"\"",
                line,
                col,
            ));
        }
        let mut raw = String::new();
        loop {
            if self.peek().is_none() {
                return Err(self.err("unterminated text block", line, col));
            }
            if self.peek() == Some('"')
                && self.peek_at(1) == Some('"')
                && self.peek_at(2) == Some('"')
            {
                let margin = self.col.saturating_sub(1); // spaces before the closing """
                self.bump();
                self.bump();
                self.bump();
                return Ok(Tok::Str(dedent(&raw, margin)));
            }
            let c = self.peek().unwrap();
            raw.push(c);
            self.bump();
        }
    }

    fn lex_bytes(&mut self, line: usize, col: usize) -> Result<Tok, LexError> {
        self.bump();
        self.bump();
        self.bump(); // b64
        self.bump(); // opening "
        let mut s = String::new();
        loop {
            match self.peek() {
                None => return Err(self.err("unterminated bytes literal", line, col)),
                Some('"') => {
                    self.bump();
                    break;
                }
                Some(c) => {
                    s.push(c);
                    self.bump();
                }
            }
        }
        decode_base64(&s)
            .map(Tok::Bytes)
            .map_err(|_| self.err("invalid base64", line, col))
    }

    fn err(&self, message: &str, line: usize, col: usize) -> LexError {
        LexError {
            message: message.to_string(),
            line,
            col,
        }
    }
}

/// Strip the closing-delimiter margin from each line of a text block (spec §3.4):
/// drop the newline that follows the opening `"""`, drop the final
/// closing-margin line, and remove up to `margin` leading spaces from each line.
fn dedent(raw: &str, margin: usize) -> String {
    let body = raw.strip_prefix('\n').unwrap_or(raw);
    let lines: Vec<&str> = body.split('\n').collect();
    let content = if lines.is_empty() {
        &lines[..]
    } else {
        &lines[..lines.len() - 1]
    };
    content
        .iter()
        .map(|line| {
            let mut taken = 0;
            let mut byte_idx = 0;
            for (i, ch) in line.char_indices() {
                if ch == ' ' && taken < margin {
                    taken += 1;
                    byte_idx = i + 1;
                } else {
                    break;
                }
            }
            &line[byte_idx..]
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Minimal standard-alphabet base64 decoder (ignores `=` padding).
fn decode_base64(input: &str) -> Result<Vec<u8>, ()> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0u32;
    for b in input.bytes() {
        if b == b'=' {
            continue;
        }
        let v = val(b).ok_or(())? as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Tok> {
        lex(s).unwrap().into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn scalars_and_punct() {
        assert_eq!(
            toks(r#"name: "x""#),
            vec![
                Tok::Bareword("name".into()),
                Tok::Colon,
                Tok::Str("x".into()),
                Tok::Eof
            ]
        );
    }

    #[test]
    fn ints_and_bools() {
        assert_eq!(
            toks("a: 42"),
            vec![
                Tok::Bareword("a".into()),
                Tok::Colon,
                Tok::Int(BigInt::from(42)),
                Tok::Eof
            ]
        );
        assert_eq!(toks("a: true").get(2), Some(&Tok::Bool(true)));
    }

    #[test]
    fn ordinary_comment_dropped_doc_kept() {
        assert_eq!(
            toks("# gone\n## kept"),
            vec![Tok::Newline, Tok::Doc(" kept".into()), Tok::Eof]
        );
    }

    #[test]
    fn directive_kept() {
        assert_eq!(
            toks("#!mangrove 0.1"),
            vec![Tok::Directive("mangrove 0.1".into()), Tok::Eof]
        );
    }

    #[test]
    fn underscores_stripped_in_int() {
        assert_eq!(toks("a: 1_000").get(2), Some(&Tok::Int(BigInt::from(1000))));
    }

    #[test]
    fn decimal_lexes() {
        assert_eq!(
            toks("a: 3.14").get(2),
            Some(&Tok::Decimal(BigDecimal::from_str("3.14").unwrap()))
        );
    }

    #[test]
    fn unit_literal_lexes() {
        use std::str::FromStr;
        assert_eq!(
            toks("a: 512Mi").get(2),
            Some(&Tok::UnitLit(BigDecimal::from(512), "Mi".into()))
        );
        assert_eq!(
            toks("x: 0.5btc").get(2),
            Some(&Tok::UnitLit(
                BigDecimal::from_str("0.5").unwrap(),
                "btc".into()
            ))
        );
        assert_eq!(
            toks("x: 100_000_000sat").get(2),
            Some(&Tok::UnitLit(BigDecimal::from(100_000_000), "sat".into()))
        );
    }

    #[test]
    fn raw_string_no_escapes() {
        assert_eq!(toks(r#"a: r"\n""#).get(2), Some(&Tok::Str("\\n".into())));
    }

    #[test]
    fn text_block_dedents_to_closing_margin() {
        let src = "a: \"\"\"\n  line one\n  line two\n  \"\"\"";
        assert_eq!(
            toks(src).get(2),
            Some(&Tok::Str("line one\nline two".into()))
        );
    }

    #[test]
    fn single_line_text_block_is_error_not_silent_drop() {
        // Was: silently produced Str(""). Now an explicit error (spec §3.4).
        assert!(lex("a: \"\"\"hello\"\"\"").is_err());
        assert!(lex("a: \"\"\"\"\"\"").is_err()); // empty """""" too
    }

    #[test]
    fn raw_text_block_is_recognized() {
        let src = "a: r\"\"\"\n  raw \\n stays\n  \"\"\"";
        assert_eq!(toks(src).get(2), Some(&Tok::Str("raw \\n stays".into())));
    }

    #[test]
    fn unicode_escape_overflow_is_error_not_panic() {
        assert!(lex("a: \"\\u{FFFFFFFFFFFFFFFF}\"").is_err());
        assert!(lex("a: \"\\u{110000}\"").is_err()); // above max scalar
        assert_eq!(toks("a: \"\\u{41}\"").get(2), Some(&Tok::Str("A".into())));
    }

    #[test]
    fn hex_escape_restricted_to_ascii() {
        assert!(lex("a: \"\\xFF\"").is_err());
        assert_eq!(toks("a: \"\\x41\"").get(2), Some(&Tok::Str("A".into())));
    }

    #[test]
    fn type_grammar_tokens() {
        use Tok::*;
        assert_eq!(
            toks("a & b | c =~ d ? >= <= > <"),
            vec![
                Bareword("a".into()),
                Amp,
                Bareword("b".into()),
                Pipe,
                Bareword("c".into()),
                Match,
                Bareword("d".into()),
                Question,
                Ge,
                Le,
                Gt,
                Lt,
                Eof
            ]
        );
    }

    #[test]
    fn two_char_operators_are_greedy() {
        assert_eq!(toks(">="), vec![Tok::Ge, Tok::Eof]);
        assert_eq!(toks("=~"), vec![Tok::Match, Tok::Eof]);
        assert_eq!(toks("<="), vec![Tok::Le, Tok::Eof]);
        assert_eq!(toks("="), vec![Tok::Eq, Tok::Eof]);
        assert_eq!(toks(">"), vec![Tok::Gt, Tok::Eof]);
    }
}
