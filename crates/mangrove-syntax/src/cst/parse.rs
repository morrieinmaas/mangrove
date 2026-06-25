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
    // Filled out construct-by-construct in Task 6+. For Task 4 we handle only the
    // simplest binding: `bareword : value` where value is an int/str/bool. This
    // proves the event→tree→lower loop end to end before porting the full grammar.
    while p.current() != SyntaxKind::EOF {
        if p.current() == SyntaxKind::NEWLINE {
            p.bump();
            continue;
        }
        parse_binding(p);
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
