//! Event-emitting recursive-descent parser that builds a lossless `rowan` green
//! tree. Mirrors the grammar of `super::super::parser` but emits nodes/tokens
//! instead of constructing AST values. Lowering (`super::lower`) turns the tree
//! back into the existing `Document`.

use super::super::parser::ParseError;
use super::kind::{MangroveLang, SyntaxKind, SyntaxNode};
use super::lex::{LosslessTok, lex_lossless};
use rowan::GreenNodeBuilder;

pub struct Parse {
    pub green: rowan::GreenNode,
    pub errors: Vec<ParseError>,
}

impl Parse {
    pub fn syntax(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }
}

struct Parser<'a> {
    src: &'a str,
    toks: Vec<LosslessTok>,
    pos: usize,
    builder: GreenNodeBuilder<'static>,
    errors: Vec<ParseError>,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Parser {
            src,
            toks: lex_lossless(src),
            pos: 0,
            builder: GreenNodeBuilder::new(),
            errors: Vec::new(),
        }
    }

    /// The kind of the next *significant* token (trivia skipped, not consumed).
    fn current(&self) -> SyntaxKind {
        let mut i = self.pos;
        while i < self.toks.len() && self.toks[i].kind.is_trivia() {
            i += 1;
        }
        self.toks.get(i).map_or(SyntaxKind::EOF, |t| t.kind)
    }

    /// Push all pending trivia onto the tree, then return.
    fn eat_trivia(&mut self) {
        while self.pos < self.toks.len() && self.toks[self.pos].kind.is_trivia() {
            self.push_token();
        }
    }

    /// Emit the current raw token (any kind) into the builder and advance.
    fn push_token(&mut self) {
        let t = self.toks[self.pos];
        let text = &self.src[t.start..t.end];
        self.builder.token(rowan_kind(t.kind), text);
        self.pos += 1;
    }

    /// Consume one significant token (after attaching leading trivia).
    fn bump(&mut self) {
        self.eat_trivia();
        if self.toks[self.pos].kind != SyntaxKind::EOF {
            self.push_token();
        }
    }

    fn start(&mut self, kind: SyntaxKind) {
        self.eat_trivia();
        self.builder.start_node(rowan_kind(kind));
    }
    fn finish(&mut self) {
        self.builder.finish_node();
    }
}

fn rowan_kind(k: SyntaxKind) -> rowan::SyntaxKind {
    use rowan::Language;
    MangroveLang::kind_to_raw(k)
}

pub fn parse_cst(src: &str) -> Parse {
    let mut p = Parser::new(src);
    p.builder.start_node(rowan_kind(SyntaxKind::DOCUMENT));
    parse_document(&mut p);
    p.eat_trivia(); // trailing trivia before EOF stays in the tree
    p.builder.finish_node();
    Parse {
        green: p.builder.finish(),
        errors: p.errors,
    }
}

fn parse_document(p: &mut Parser) {
    while p.current() != SyntaxKind::EOF {
        if p.current() == SyntaxKind::NEWLINE {
            p.bump();
            continue;
        }
        // Determine what kind of top-level item this is by inspecting the
        // leading significant token (and the next one to distinguish declarations
        // from bindings named the same as a keyword).
        if is_decl_keyword(p, "use") && lookahead_is_str(p) {
            parse_use_decl(p);
        } else if is_decl_keyword(p, "type") && lookahead_is_bareword(p) {
            parse_type_def(p);
        } else if is_decl_keyword(p, "unit") && lookahead_is_bareword(p) {
            parse_unit_def(p);
        } else if is_decl_keyword(p, "params") && lookahead_is_lbrace(p) {
            parse_param_decl(p);
        } else if is_decl_keyword(p, "fn") && lookahead_is_bareword(p) {
            parse_fn_def(p);
        } else if is_decl_keyword(p, "schema") && lookahead_is_bareword(p) {
            parse_schema_decl(p);
        } else {
            parse_binding(p);
        }
    }
}

/// Returns true if the next significant token is a BAREWORD with the given text.
fn is_decl_keyword(p: &Parser, kw: &str) -> bool {
    if p.current() != SyntaxKind::BAREWORD {
        return false;
    }
    // Find the raw token position for this significant token.
    let mut i = p.pos;
    while i < p.toks.len() && p.toks[i].kind.is_trivia() {
        i += 1;
    }
    if i >= p.toks.len() {
        return false;
    }
    &p.src[p.toks[i].start..p.toks[i].end] == kw
}

/// Returns true if the first significant token AFTER the current one is a BAREWORD.
fn lookahead_is_bareword(p: &Parser) -> bool {
    nth_sig(p, 1) == SyntaxKind::BAREWORD
}

/// Returns true if the first significant token after the current one is a STR.
fn lookahead_is_str(p: &Parser) -> bool {
    nth_sig(p, 1) == SyntaxKind::STR
}

/// Returns true if the first significant token after the current one is L_BRACE.
fn lookahead_is_lbrace(p: &Parser) -> bool {
    nth_sig(p, 1) == SyntaxKind::L_BRACE
}

/// Returns the kind of the Nth significant token (0 = current, 1 = next, ...).
fn nth_sig(p: &Parser, n: usize) -> SyntaxKind {
    let mut count = 0;
    let mut i = p.pos;
    while i < p.toks.len() {
        if !p.toks[i].kind.is_trivia() {
            if count == n {
                return p.toks[i].kind;
            }
            count += 1;
        }
        i += 1;
    }
    SyntaxKind::EOF
}

// ---- Declaration parsers ----
// For each declaration, the strategy is: start the node, then consume all
// tokens belonging to this declaration losslessly, then finish. Brace/bracket/
// paren depth tracking ensures we consume the full block for block-bearing forms.

/// `use "path" as alias` — single logical line.
fn parse_use_decl(p: &mut Parser) {
    p.start(SyntaxKind::USE_DECL);
    consume_through_newline(p);
    p.finish();
}

/// `type Name = <type-expr> [@annotations...]` — single logical line (no brace in
/// simple cases; some types like record types span multiple lines but the
/// depth-tracking handles that).
fn parse_type_def(p: &mut Parser) {
    p.start(SyntaxKind::TYPE_DEF);
    consume_through_newline_at_depth_0(p);
    p.finish();
}

/// `unit Name : int { ... }` — has a brace-delimited block.
fn parse_unit_def(p: &mut Parser) {
    p.start(SyntaxKind::UNIT_DEF);
    consume_through_newline_at_depth_0(p);
    p.finish();
}

/// `params { ... }` — has a brace-delimited block.
fn parse_param_decl(p: &mut Parser) {
    p.start(SyntaxKind::PARAM_DECL);
    consume_through_newline_at_depth_0(p);
    p.finish();
}

/// `fn name(params): RetType = body` — the body may be a brace/bracket-delimited
/// value.
fn parse_fn_def(p: &mut Parser) {
    p.start(SyntaxKind::FN_DEF);
    consume_through_newline_at_depth_0(p);
    p.finish();
}

/// `schema Name` or `schema Name & { ... }` — the latter has a brace block.
fn parse_schema_decl(p: &mut Parser) {
    p.start(SyntaxKind::SCHEMA_DECL);
    consume_through_newline_at_depth_0(p);
    p.finish();
}

/// Consume all tokens up to and including the next NEWLINE at brace/bracket/paren
/// depth 0, or until EOF. This handles declarations that may contain nested braces
/// (type records, unit blocks, fn bodies, etc.).
fn consume_through_newline_at_depth_0(p: &mut Parser) {
    // First bump leading trivia.
    p.eat_trivia();
    let mut depth = 0usize;
    loop {
        match p.current() {
            SyntaxKind::EOF => break,
            SyntaxKind::NEWLINE if depth == 0 => {
                p.bump(); // include the newline in the node
                break;
            }
            SyntaxKind::L_BRACE | SyntaxKind::L_BRACKET | SyntaxKind::L_PAREN => {
                depth += 1;
                p.bump();
            }
            SyntaxKind::R_BRACE | SyntaxKind::R_BRACKET | SyntaxKind::R_PAREN => {
                depth = depth.saturating_sub(1);
                p.bump();
                // After closing the outermost block, eat the trailing newline.
                if depth == 0 {
                    if p.current() == SyntaxKind::NEWLINE {
                        p.bump();
                    }
                    break;
                }
            }
            _ => {
                p.bump();
            }
        }
    }
}

/// Consume all tokens up to and including the next NEWLINE (no depth tracking —
/// used for `use` which is guaranteed single-line).
fn consume_through_newline(p: &mut Parser) {
    p.eat_trivia();
    loop {
        match p.current() {
            SyntaxKind::EOF => break,
            SyntaxKind::NEWLINE => {
                p.bump();
                break;
            }
            _ => {
                p.bump();
            }
        }
    }
}

fn parse_binding(p: &mut Parser) {
    p.start(SyntaxKind::BINDING);
    p.bump(); // key (BAREWORD or STR)
    if p.current() == SyntaxKind::COLON {
        p.bump();
    }
    parse_atom(p);
    p.finish();
}

fn parse_atom(p: &mut Parser) {
    match p.current() {
        SyntaxKind::L_BRACE => parse_record(p),
        SyntaxKind::L_BRACKET => parse_list(p),
        SyntaxKind::INT
        | SyntaxKind::STR
        | SyntaxKind::BOOL
        | SyntaxKind::DECIMAL
        | SyntaxKind::UNIT_LIT
        | SyntaxKind::INTERP_STR
        | SyntaxKind::BYTES => p.bump(),
        _ => {
            // Task 11 turns this into recovery; for now consume one token as ERROR-ish.
            p.bump();
        }
    }
}

fn parse_record(p: &mut Parser) {
    p.start(SyntaxKind::RECORD);
    p.bump(); // L_BRACE
    loop {
        match p.current() {
            SyntaxKind::R_BRACE | SyntaxKind::EOF => break,
            SyntaxKind::COMMA | SyntaxKind::NEWLINE => {
                p.bump(); // separator — stays in tree for losslessness
            }
            SyntaxKind::BAREWORD | SyntaxKind::STR => {
                p.start(SyntaxKind::FIELD);
                p.bump(); // key
                if p.current() == SyntaxKind::COLON {
                    p.bump(); // COLON
                }
                parse_atom(p); // value
                p.finish(); // FIELD
            }
            _ => {
                // Unknown token — bump to avoid infinite loop; Task 11 adds recovery.
                p.bump();
            }
        }
    }
    p.bump(); // R_BRACE (or no-op at EOF)
    p.finish(); // RECORD
}

fn parse_list(p: &mut Parser) {
    p.start(SyntaxKind::LIST);
    p.bump(); // L_BRACKET
    loop {
        match p.current() {
            SyntaxKind::R_BRACKET | SyntaxKind::EOF => break,
            SyntaxKind::COMMA | SyntaxKind::NEWLINE => {
                p.bump(); // separator — stays in tree for losslessness
            }
            _ => {
                parse_atom(p); // element value (recurses for nested composites)
            }
        }
    }
    p.bump(); // R_BRACKET (or no-op at EOF)
    p.finish(); // LIST
}
