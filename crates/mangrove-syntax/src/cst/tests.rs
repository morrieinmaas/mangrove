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
