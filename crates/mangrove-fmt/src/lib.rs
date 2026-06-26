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

    fn assert_idempotent(src: &str) {
        let once = format_str(src).text;
        let twice = format_str(&once).text;
        assert_eq!(once, twice, "fmt must be idempotent for {src:?}");
    }

    /// Meaning preserved: for evaluable input the content hash is unchanged;
    /// for non-evaluable input the formatted source re-parses to the same
    /// non-trivia token sequence (structural equivalence).
    ///
    /// NOTE: trailing-comma cases (e.g. `[1,2,3,]`) must be EVALUABLE so they
    /// take the hash branch. Removing a trailing `,` would change the non-trivia
    /// token sequence, so such cases cannot use the structural branch.
    fn assert_meaning_preserved(src: &str) {
        let formatted = format_str(src).text;
        match (
            mangrove_syntax::parse(src),
            mangrove_syntax::parse(&formatted),
        ) {
            (Ok(a), Ok(b)) => assert_eq!(
                mangrove_canonical::hash(&a),
                mangrove_canonical::hash(&b),
                "hash changed by fmt for {src:?}"
            ),
            _ => {
                // non-evaluable: compare non-trivia token kinds+text of both CSTs
                let toks = |s: &str| {
                    mangrove_syntax::cst::parse_cst(s)
                        .syntax()
                        .descendants_with_tokens()
                        .filter_map(|e| e.into_token())
                        .filter(|t| {
                            !matches!(
                                t.kind(),
                                mangrove_syntax::cst::SyntaxKind::WHITESPACE
                                    | mangrove_syntax::cst::SyntaxKind::NEWLINE
                                    | mangrove_syntax::cst::SyntaxKind::COMMENT
                            )
                        })
                        .map(|t| (t.kind(), t.text().to_string()))
                        .collect::<Vec<_>>()
                };
                assert_eq!(
                    toks(src),
                    toks(&formatted),
                    "fmt changed non-trivia tokens for {src:?}"
                );
            }
        }
    }

    // Trailing-comma entry (`a: [1,2,3,]`) is kept EVALUABLE so it exercises
    // the hash branch — removing a `,` would change the non-trivia token set,
    // making the structural branch fail after Task 5.
    const CORPUS: &[&str] = &[
        "a: 1\n",
        "a: { b: 1, c: [ 2, 3 ] }\n",
        "type P = int & >= 1 & <= 65535\nschema P\nx: 5\n",
        "# lead\na: \"x\"  # trailing\n",
        "a: [1,2,3,]\n", // trailing comma — evaluable, so hash branch is used
    ];

    #[test]
    fn oracles_hold_on_corpus() {
        for src in CORPUS {
            assert_idempotent(src);
            assert_meaning_preserved(src);
        }
    }

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
