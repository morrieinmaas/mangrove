#[allow(unused_imports)]
use super::{kind::*, lex::*, lower::*, parse::*};

use super::kind::SyntaxKind;

#[test]
fn syntaxkind_all_matches_discriminants() {
    // ALL must list every variant in discriminant order, and have length __LAST.
    assert_eq!(SyntaxKind::ALL.len(), SyntaxKind::__LAST as usize);
    for (i, k) in SyntaxKind::ALL.iter().enumerate() {
        assert_eq!(
            *k as usize, i,
            "ALL[{i}] = {k:?} is out of discriminant order"
        );
        assert_eq!(SyntaxKind::from_u16(i as u16), Some(*k));
    }
}

fn relex_roundtrips(src: &str) {
    let toks = super::lex::lex_lossless(src);
    let joined: String = toks.iter().map(|t| &src[t.start..t.end]).collect();
    assert_eq!(
        joined, src,
        "lossless lexer must reproduce the source byte-for-byte"
    );
    assert_eq!(toks.last().unwrap().kind, SyntaxKind::EOF);
}

#[test]
fn lexer_is_lossless_on_examples() {
    for src in [
        "host: \"api.eu\"\nport: 8443\n",
        "# a comment\ntype T = int  # trailing\nschema T\nx: 1\n",
        "a: { b: [ 1, 2 ], c: 512Mi }\n",
        "s: \"\"\"text ${v}\nblock\"\"\"\n",
        "",
        "   \n\t# only trivia\n",
    ] {
        relex_roundtrips(src);
    }
}
