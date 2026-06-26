//! `mangrove fmt` — a deterministic, comment-preserving, meaning-preserving
//! formatter that walks the lossless CST and normalizes whitespace/indentation.

use mangrove_syntax::ParseError;
use mangrove_syntax::cst::parse_cst;
use rowan::NodeOrToken;

pub struct FormatResult {
    pub text: String,
    pub errors: Vec<ParseError>,
}

/// Format `src`. Always returns formatted text; `errors` is non-empty if the
/// source had parse errors (the formatter still produces best-effort output,
/// since the CST is lossless even on error).
pub fn format_str(src: &str) -> FormatResult {
    let parse = parse_cst(src);
    let mut out = String::new();
    // v1 identity: emit every token's text in document order.
    for elem in parse.syntax().descendants_with_tokens() {
        if let NodeOrToken::Token(t) = elem {
            out.push_str(t.text());
        }
    }
    FormatResult {
        text: out,
        errors: parse.errors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_formatter_round_trips() {
        for src in [
            "a: 1\n",
            "a: { b: 1, c: [ 2, 3 ] }\n",
            "# comment\ntype T = int\nschema T\nx: 1\n",
            "",
        ] {
            assert_eq!(
                format_str(src).text,
                src,
                "identity walk must reproduce source"
            );
        }
    }
}
