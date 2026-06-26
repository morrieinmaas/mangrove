//! The `SyntaxKind` tag space (tokens + nodes) and the `rowan::Language` impl.

/// Every token and node kind. `#[repr(u16)]` so it round-trips through
/// `rowan::SyntaxKind(u16)`. Order is irrelevant except `__LAST` must be last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
#[allow(non_camel_case_types, clippy::upper_case_acronyms)]
pub enum SyntaxKind {
    // ---- trivia (NEW — the eval lexer discards these) ----
    WHITESPACE, // spaces, tabs, CR
    COMMENT,    // `# ...` ordinary comment
    // ---- significant tokens (mirror lexer::Tok) ----
    NEWLINE,
    L_BRACE,
    R_BRACE,
    L_BRACKET,
    R_BRACKET,
    L_PAREN,
    R_PAREN,
    COLON,
    COMMA,
    AMP,
    PIPE,
    EQ,
    MATCH,
    QUESTION,
    GE,
    LE,
    GT,
    LT,
    STAR,
    AT,
    DOT,
    DOT_DOT_DOT,
    EQ_EQ,
    NE,
    BANG,
    AMP_AMP,
    PIPE_PIPE,
    PLUS_EQ,
    INT,
    DECIMAL,
    UNIT_LIT,
    STR,
    INTERP_STR,
    BOOL,
    BYTES,
    BAREWORD,
    DOC,
    DIRECTIVE,
    ERROR, // an unrecognized/!invalid token (used by recovery, Task 11)
    EOF,
    // ---- nodes ----
    DOCUMENT,
    UNIT_DEF,
    TYPE_DEF,
    SCHEMA_DECL,
    USE_DECL,
    PARAM_DECL,
    FN_DEF,
    // values / body
    RECORD,
    FIELD,
    LIST,
    LIST_OP_ITEM,
    MATCH_EXPR,
    MATCH_ARM,
    CALL,
    SPREAD,
    UNSET,
    REF,
    INTERP,
    ANNOTATION,
    DEFAULT,
    BINDING,
    // types
    TYPE_PRIMITIVE,
    TYPE_RANGE,
    TYPE_REGEX,
    TYPE_LITERAL,
    TYPE_RECORD,
    TYPE_FIELD,
    TYPE_MAP,
    TYPE_LIST,
    TYPE_UNION,
    TYPE_NAMED,
    TYPE_BRAND,
    REQUIRE,
    PRED,
    __LAST,
}

impl SyntaxKind {
    /// Safe inverse of `as u16`. Returns `None` if out of range.
    pub fn from_u16(n: u16) -> Option<SyntaxKind> {
        (n < SyntaxKind::__LAST as u16).then(|| {
            // repr(u16), contiguous from 0 — index into the variant list.
            SyntaxKind::ALL[n as usize]
        })
    }

    /// All variants in discriminant order, for `from_u16`.
    pub const ALL: &'static [SyntaxKind] = &[
        SyntaxKind::WHITESPACE,
        SyntaxKind::COMMENT,
        SyntaxKind::NEWLINE,
        SyntaxKind::L_BRACE,
        SyntaxKind::R_BRACE,
        SyntaxKind::L_BRACKET,
        SyntaxKind::R_BRACKET,
        SyntaxKind::L_PAREN,
        SyntaxKind::R_PAREN,
        SyntaxKind::COLON,
        SyntaxKind::COMMA,
        SyntaxKind::AMP,
        SyntaxKind::PIPE,
        SyntaxKind::EQ,
        SyntaxKind::MATCH,
        SyntaxKind::QUESTION,
        SyntaxKind::GE,
        SyntaxKind::LE,
        SyntaxKind::GT,
        SyntaxKind::LT,
        SyntaxKind::STAR,
        SyntaxKind::AT,
        SyntaxKind::DOT,
        SyntaxKind::DOT_DOT_DOT,
        SyntaxKind::EQ_EQ,
        SyntaxKind::NE,
        SyntaxKind::BANG,
        SyntaxKind::AMP_AMP,
        SyntaxKind::PIPE_PIPE,
        SyntaxKind::PLUS_EQ,
        SyntaxKind::INT,
        SyntaxKind::DECIMAL,
        SyntaxKind::UNIT_LIT,
        SyntaxKind::STR,
        SyntaxKind::INTERP_STR,
        SyntaxKind::BOOL,
        SyntaxKind::BYTES,
        SyntaxKind::BAREWORD,
        SyntaxKind::DOC,
        SyntaxKind::DIRECTIVE,
        SyntaxKind::ERROR,
        SyntaxKind::EOF,
        SyntaxKind::DOCUMENT,
        SyntaxKind::UNIT_DEF,
        SyntaxKind::TYPE_DEF,
        SyntaxKind::SCHEMA_DECL,
        SyntaxKind::USE_DECL,
        SyntaxKind::PARAM_DECL,
        SyntaxKind::FN_DEF,
        SyntaxKind::RECORD,
        SyntaxKind::FIELD,
        SyntaxKind::LIST,
        SyntaxKind::LIST_OP_ITEM,
        SyntaxKind::MATCH_EXPR,
        SyntaxKind::MATCH_ARM,
        SyntaxKind::CALL,
        SyntaxKind::SPREAD,
        SyntaxKind::UNSET,
        SyntaxKind::REF,
        SyntaxKind::INTERP,
        SyntaxKind::ANNOTATION,
        SyntaxKind::DEFAULT,
        SyntaxKind::BINDING,
        SyntaxKind::TYPE_PRIMITIVE,
        SyntaxKind::TYPE_RANGE,
        SyntaxKind::TYPE_REGEX,
        SyntaxKind::TYPE_LITERAL,
        SyntaxKind::TYPE_RECORD,
        SyntaxKind::TYPE_FIELD,
        SyntaxKind::TYPE_MAP,
        SyntaxKind::TYPE_LIST,
        SyntaxKind::TYPE_UNION,
        SyntaxKind::TYPE_NAMED,
        SyntaxKind::TYPE_BRAND,
        SyntaxKind::REQUIRE,
        SyntaxKind::PRED,
    ];

    pub fn is_trivia(self) -> bool {
        matches!(self, SyntaxKind::WHITESPACE | SyntaxKind::COMMENT)
    }
}

/// The rowan `Language` marker for Mangrove.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MangroveLang {}

impl rowan::Language for MangroveLang {
    type Kind = SyntaxKind;
    fn kind_from_raw(raw: rowan::SyntaxKind) -> Self::Kind {
        Self::Kind::from_u16(raw.0).expect("SyntaxKind out of range")
    }
    fn kind_to_raw(kind: Self::Kind) -> rowan::SyntaxKind {
        rowan::SyntaxKind(kind as u16)
    }
}

pub type SyntaxNode = rowan::SyntaxNode<MangroveLang>;
pub type SyntaxToken = rowan::SyntaxToken<MangroveLang>;
pub type SyntaxElement = rowan::SyntaxElement<MangroveLang>;
