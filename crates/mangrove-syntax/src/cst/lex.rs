//! Lossless lexer: every byte of the source lands in exactly one token, so the
//! token texts concatenate back to the original. Trivia (whitespace, comments)
//! are emitted, not discarded. Never errors — unknown bytes become `ERROR`.

use super::kind::SyntaxKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LosslessTok {
    pub kind: SyntaxKind,
    pub start: usize,
    pub end: usize,
}

pub fn lex_lossless(src: &str) -> Vec<LosslessTok> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let start = i;
        let kind = match bytes[i] {
            b' ' | b'\t' | b'\r' => {
                while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\r') {
                    i += 1;
                }
                SyntaxKind::WHITESPACE
            }
            b'\n' => {
                i += 1;
                SyntaxKind::NEWLINE
            }
            b'#' => {
                // `##` doc, `#!` directive, else ordinary comment — all run to EOL.
                let kind = match bytes.get(i + 1) {
                    Some(b'#') => SyntaxKind::DOC,
                    Some(b'!') => SyntaxKind::DIRECTIVE,
                    _ => SyntaxKind::COMMENT,
                };
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                kind
            }
            _ => {
                // Delegate one significant token to a helper that mirrors the eval
                // lexer's rules and returns (kind, new_index).
                let (k, next) = scan_significant(src, i);
                i = next;
                k
            }
        };
        out.push(LosslessTok {
            kind,
            start,
            end: i,
        });
    }
    out.push(LosslessTok {
        kind: SyntaxKind::EOF,
        start: bytes.len(),
        end: bytes.len(),
    });
    out
}

/// Recognize one significant (non-trivia) token starting at byte offset `at`.
/// Returns `(SyntaxKind, end_byte_offset)`. Never panics — unrecognized bytes
/// become a single-byte `ERROR` token.
pub fn scan_significant(src: &str, at: usize) -> (SyntaxKind, usize) {
    let bytes = src.as_bytes();
    let b = bytes[at];

    match b {
        b'{' => (SyntaxKind::L_BRACE, at + 1),
        b'}' => (SyntaxKind::R_BRACE, at + 1),
        b'[' => (SyntaxKind::L_BRACKET, at + 1),
        b']' => (SyntaxKind::R_BRACKET, at + 1),
        b'(' => (SyntaxKind::L_PAREN, at + 1),
        b')' => (SyntaxKind::R_PAREN, at + 1),
        b':' => (SyntaxKind::COLON, at + 1),
        b',' => (SyntaxKind::COMMA, at + 1),
        b'?' => (SyntaxKind::QUESTION, at + 1),
        b'*' => (SyntaxKind::STAR, at + 1),
        b'@' => (SyntaxKind::AT, at + 1),
        b'!' => {
            if bytes.get(at + 1) == Some(&b'=') {
                (SyntaxKind::NE, at + 2)
            } else {
                (SyntaxKind::BANG, at + 1)
            }
        }
        b'&' => {
            if bytes.get(at + 1) == Some(&b'&') {
                (SyntaxKind::AMP_AMP, at + 2)
            } else {
                (SyntaxKind::AMP, at + 1)
            }
        }
        b'|' => {
            if bytes.get(at + 1) == Some(&b'|') {
                (SyntaxKind::PIPE_PIPE, at + 2)
            } else {
                (SyntaxKind::PIPE, at + 1)
            }
        }
        b'+' => {
            if bytes.get(at + 1) == Some(&b'=') {
                (SyntaxKind::PLUS_EQ, at + 2)
            } else {
                (SyntaxKind::ERROR, at + 1)
            }
        }
        b'.' => {
            if bytes.get(at + 1) == Some(&b'.') && bytes.get(at + 2) == Some(&b'.') {
                (SyntaxKind::DOT_DOT_DOT, at + 3)
            } else {
                (SyntaxKind::DOT, at + 1)
            }
        }
        b'=' => {
            if bytes.get(at + 1) == Some(&b'=') {
                (SyntaxKind::EQ_EQ, at + 2)
            } else if bytes.get(at + 1) == Some(&b'~') {
                (SyntaxKind::MATCH, at + 2)
            } else {
                (SyntaxKind::EQ, at + 1)
            }
        }
        b'>' => {
            if bytes.get(at + 1) == Some(&b'=') {
                (SyntaxKind::GE, at + 2)
            } else {
                (SyntaxKind::GT, at + 1)
            }
        }
        b'<' => {
            if bytes.get(at + 1) == Some(&b'=') {
                (SyntaxKind::LE, at + 2)
            } else {
                (SyntaxKind::LT, at + 1)
            }
        }
        b'"' => scan_string(bytes, at),
        b'r' if bytes.get(at + 1) == Some(&b'"') => scan_raw_string(bytes, at),
        b'b' if bytes.get(at..at + 4) == Some(b"b64\"") => scan_bytes_literal(bytes, at),
        b'-' | b'0'..=b'9' => scan_number(bytes, at),
        c if is_ident_start(c) => scan_bareword(bytes, at),
        // Any unrecognized byte (incl. multi-byte UTF-8 lead bytes) → ERROR for that byte.
        // We step one byte at a time to stay lossless.
        _ => (SyntaxKind::ERROR, at + 1),
    }
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

fn is_ident_continue(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'-'
}

/// Scan a plain `"..."` string, raw `r"..."` or text block `"""..."""`.
/// The `at` index points at the opening `"`.
/// Returns `(STR, end)` — the token text includes delimiters and raw escape bytes.
fn scan_string(bytes: &[u8], at: usize) -> (SyntaxKind, usize) {
    // Check for text block `"""`
    if bytes.get(at + 1) == Some(&b'"') && bytes.get(at + 2) == Some(&b'"') {
        return scan_text_block(bytes, at, false);
    }
    // Plain string
    let mut i = at + 1; // skip opening "
    loop {
        match bytes.get(i) {
            None => return (SyntaxKind::STR, i), // unterminated → consume to EOF, lossless
            Some(&b'"') => return (SyntaxKind::STR, i + 1),
            Some(&b'\\') => {
                i += 1; // skip backslash
                match bytes.get(i) {
                    None => return (SyntaxKind::STR, i), // unterminated after backslash
                    Some(&b'x') => {
                        // \xNN: skip x + 2 hex digits (if present)
                        i += 1;
                        if bytes.get(i).is_some_and(|b| b.is_ascii_hexdigit()) {
                            i += 1;
                        }
                        if bytes.get(i).is_some_and(|b| b.is_ascii_hexdigit()) {
                            i += 1;
                        }
                    }
                    Some(&b'u') => {
                        // \u{HHHHH}: skip u + { + hex digits + }
                        i += 1;
                        if bytes.get(i) == Some(&b'{') {
                            i += 1;
                            while bytes.get(i).is_some_and(|b| b.is_ascii_hexdigit()) {
                                i += 1;
                            }
                            if bytes.get(i) == Some(&b'}') {
                                i += 1;
                            }
                        }
                    }
                    Some(_) => {
                        i += 1; // skip one escape character (n, t, r, 0, $, ", \\, etc.)
                    }
                }
            }
            Some(_) => {
                i += 1;
            }
        }
    }
}

/// Scan a raw string: `r"..."` (no escape processing). `at` points at `r`.
fn scan_raw_string(bytes: &[u8], at: usize) -> (SyntaxKind, usize) {
    // Check for raw text block `r"""`
    if bytes.get(at + 1) == Some(&b'"')
        && bytes.get(at + 2) == Some(&b'"')
        && bytes.get(at + 3) == Some(&b'"')
    {
        return scan_text_block(bytes, at, true);
    }
    // Raw string: `r"..."` — no escape processing, just scan to closing `"`
    let mut i = at + 2; // skip `r"`
    loop {
        match bytes.get(i) {
            None => return (SyntaxKind::STR, i), // unterminated
            Some(&b'"') => return (SyntaxKind::STR, i + 1),
            Some(_) => i += 1,
        }
    }
}

/// Scan a text block `"""..."""` or `r"""..."""`.
/// `at` points at `r` (if raw) or the first `"` (if not raw).
fn scan_text_block(bytes: &[u8], at: usize, is_raw: bool) -> (SyntaxKind, usize) {
    // The opening: either `r"""` (4 bytes) or `"""` (3 bytes)
    let mut i = if is_raw { at + 4 } else { at + 3 };
    // Consume any trailing whitespace on the opening line before the required newline
    while matches!(bytes.get(i), Some(&b' ') | Some(&b'\t') | Some(&b'\r')) {
        i += 1;
    }
    // The spec requires content to begin on the next line, but for lossless lexing
    // we just scan forward regardless — if there's no newline it's still lossless
    // (the text block will be an error in the parser, but we still emit the bytes).
    // Scan until closing `"""`, or EOF.
    loop {
        if i + 2 < bytes.len() && bytes[i] == b'"' && bytes[i + 1] == b'"' && bytes[i + 2] == b'"' {
            return (SyntaxKind::STR, i + 3);
        }
        match bytes.get(i) {
            None => return (SyntaxKind::STR, i), // unterminated
            Some(_) => i += 1,
        }
    }
}

/// Scan a `b64"..."` bytes literal. `at` points at `b`.
fn scan_bytes_literal(bytes: &[u8], at: usize) -> (SyntaxKind, usize) {
    let mut i = at + 4; // skip `b64"`
    loop {
        match bytes.get(i) {
            None => return (SyntaxKind::BYTES, i), // unterminated
            Some(&b'"') => return (SyntaxKind::BYTES, i + 1),
            Some(_) => i += 1,
        }
    }
}

/// Scan an integer, decimal, or unit-literal starting at `at`.
/// Mirrors `Lexer::lex_number` from the eval lexer.
fn scan_number(bytes: &[u8], at: usize) -> (SyntaxKind, usize) {
    let mut i = at;
    let mut is_decimal = false;

    // Optional leading minus
    if bytes.get(i) == Some(&b'-') {
        i += 1;
        // Must be followed by a digit; if not, return ERROR for the `-`
        if !bytes.get(i).is_some_and(|b| b.is_ascii_digit()) {
            return (SyntaxKind::ERROR, at + 1);
        }
    }

    // Integer digits (with optional `_` separators)
    while bytes
        .get(i)
        .is_some_and(|b| b.is_ascii_digit() || *b == b'_')
    {
        i += 1;
    }

    // Optional decimal part
    if bytes.get(i) == Some(&b'.') {
        is_decimal = true;
        i += 1;
        while bytes
            .get(i)
            .is_some_and(|b| b.is_ascii_digit() || *b == b'_')
        {
            i += 1;
        }
    }

    // Optional exponent
    if matches!(bytes.get(i), Some(&b'e') | Some(&b'E')) {
        is_decimal = true;
        i += 1;
        if matches!(bytes.get(i), Some(&b'+') | Some(&b'-')) {
            i += 1;
        }
        while bytes
            .get(i)
            .is_some_and(|b| b.is_ascii_digit() || *b == b'_')
        {
            i += 1;
        }
    }

    // If followed by an identifier-start char → unit literal
    if bytes.get(i).is_some_and(|&b| is_ident_start(b)) {
        while bytes
            .get(i)
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
        {
            i += 1;
        }
        return (SyntaxKind::UNIT_LIT, i);
    }

    if is_decimal {
        (SyntaxKind::DECIMAL, i)
    } else {
        (SyntaxKind::INT, i)
    }
}

/// Scan a bareword (identifier or keyword like `true`/`false`).
fn scan_bareword(bytes: &[u8], at: usize) -> (SyntaxKind, usize) {
    let mut i = at;
    while bytes.get(i).is_some_and(|&b| is_ident_continue(b)) {
        i += 1;
    }
    let word = &bytes[at..i];
    let kind = match word {
        b"true" | b"false" => SyntaxKind::BOOL,
        _ => SyntaxKind::BAREWORD,
    };
    (kind, i)
}

#[cfg(test)]
mod scan_tests {
    use super::*;

    fn first(src: &str) -> (SyntaxKind, &str) {
        let t = lex_lossless(src)[0];
        (t.kind, &src[t.start..t.end])
    }

    #[test]
    fn scans_int() {
        assert_eq!(first("123 "), (SyntaxKind::INT, "123"));
    }

    #[test]
    fn scans_decimal() {
        assert_eq!(first("1.5 "), (SyntaxKind::DECIMAL, "1.5"));
    }

    #[test]
    fn scans_unit() {
        assert_eq!(first("512Mi"), (SyntaxKind::UNIT_LIT, "512Mi"));
    }

    #[test]
    fn scans_string() {
        assert_eq!(first("\"hi\" "), (SyntaxKind::STR, "\"hi\""));
    }

    #[test]
    fn scans_dotdotdot() {
        assert_eq!(first("...a"), (SyntaxKind::DOT_DOT_DOT, "..."));
    }

    #[test]
    fn scans_ge() {
        assert_eq!(first(">= 1"), (SyntaxKind::GE, ">="));
    }

    #[test]
    fn scans_bareword() {
        assert_eq!(first("schema"), (SyntaxKind::BAREWORD, "schema"));
    }

    // Additional per-class tests
    #[test]
    fn scans_bool_true() {
        assert_eq!(first("true "), (SyntaxKind::BOOL, "true"));
    }

    #[test]
    fn scans_bool_false() {
        assert_eq!(first("false "), (SyntaxKind::BOOL, "false"));
    }

    #[test]
    fn scans_raw_string() {
        assert_eq!(first(r#"r"\n" "#), (SyntaxKind::STR, r#"r"\n""#));
    }

    #[test]
    fn scans_bytes_literal() {
        assert_eq!(
            first("b64\"aGVsbG8=\" "),
            (SyntaxKind::BYTES, "b64\"aGVsbG8=\"")
        );
    }

    #[test]
    fn scans_le() {
        assert_eq!(first("<= 1"), (SyntaxKind::LE, "<="));
    }

    #[test]
    fn scans_eq_eq() {
        assert_eq!(first("== x"), (SyntaxKind::EQ_EQ, "=="));
    }

    #[test]
    fn scans_ne() {
        assert_eq!(first("!= x"), (SyntaxKind::NE, "!="));
    }

    #[test]
    fn scans_amp_amp() {
        assert_eq!(first("&& x"), (SyntaxKind::AMP_AMP, "&&"));
    }

    #[test]
    fn scans_pipe_pipe() {
        assert_eq!(first("|| x"), (SyntaxKind::PIPE_PIPE, "||"));
    }

    #[test]
    fn scans_plus_eq() {
        assert_eq!(first("+= x"), (SyntaxKind::PLUS_EQ, "+="));
    }

    #[test]
    fn scans_match() {
        assert_eq!(first("=~ x"), (SyntaxKind::MATCH, "=~"));
    }

    #[test]
    fn scans_text_block() {
        let src = "\"\"\"\ntext\n\"\"\"";
        assert_eq!(first(src), (SyntaxKind::STR, src));
    }

    #[test]
    fn unterminated_string_lossless() {
        let src = "\"hello";
        let toks = lex_lossless(src);
        let joined: String = toks.iter().map(|t| &src[t.start..t.end]).collect();
        assert_eq!(joined, src);
        assert_eq!(toks.last().unwrap().kind, SyntaxKind::EOF);
    }

    #[test]
    fn error_token_for_unknown_byte() {
        // `$` alone (not before ident or `{`) — lossless but ERROR
        let src = "$";
        let toks = lex_lossless(src);
        let joined: String = toks.iter().map(|t| &src[t.start..t.end]).collect();
        assert_eq!(joined, src);
        assert_eq!(toks[0].kind, SyntaxKind::ERROR);
    }

    #[test]
    fn negative_int() {
        assert_eq!(first("-42 "), (SyntaxKind::INT, "-42"));
    }

    #[test]
    fn exponent_decimal() {
        assert_eq!(first("1e10 "), (SyntaxKind::DECIMAL, "1e10"));
    }
}
