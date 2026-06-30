//! Event-emitting recursive-descent parser that builds a lossless `rowan` green
//! tree. Mirrors the grammar of `super::super::parser` but emits nodes/tokens
//! instead of constructing AST values. Lowering (`super::lower`) turns the tree
//! back into the existing `Document`.

use super::super::parser::ParseError;
use super::kind::{MangroveLang, SyntaxKind, SyntaxNode};
use super::lex::{LosslessTok, lex_lossless};
use rowan::GreenNodeBuilder;

/// Maximum value-container nesting depth. Matches the legacy parser's `MAX_DEPTH`
/// (parser.rs:55) so both parsers agree on the depth limit for deep inputs.
const CST_MAX_DEPTH: usize = 128;

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

    /// Record a parse error at the current position (line/col 0 — LSP recomputes
    /// from byte offsets; exact position is non-critical for v0.3.0).
    fn push_error(&mut self, msg: &str) {
        self.errors.push(ParseError {
            message: msg.to_string(),
            line: 0,
            col: 0,
        });
    }

    /// Push an error, wrap the offending tokens in an ERROR node, and resync.
    ///
    /// Resync strategy:
    /// - If `stop_at_closer` is true (inside a record/list): consume until
    ///   NEWLINE, EOF, R_BRACE, or R_BRACKET — but do NOT consume the closer
    ///   itself; the caller's loop will handle it.
    /// - Otherwise (top-level / atom context): consume until NEWLINE or EOF,
    ///   including the NEWLINE.
    ///
    /// Recovery always consumes ≥1 token when there is a non-closer, non-EOF
    /// token to consume. At an immediate sync point the ERROR node may be
    /// empty, but `parse_atom`'s caller (e.g. `parse_binding`) has already
    /// advanced past the key/colon, so the outer loop still makes progress.
    fn error_and_recover(&mut self, msg: &str, stop_at_closer: bool) {
        self.push_error(msg);
        self.start(SyntaxKind::ERROR);
        loop {
            match self.current() {
                SyntaxKind::EOF => break,
                SyntaxKind::NEWLINE => {
                    if stop_at_closer {
                        // Don't consume the newline — leave it as separator for
                        // the enclosing container loop.
                        break;
                    } else {
                        self.bump(); // consume the newline, then stop
                        break;
                    }
                }
                SyntaxKind::R_BRACE | SyntaxKind::R_BRACKET if stop_at_closer => {
                    // Don't consume the closer — the container loop owns it.
                    break;
                }
                _ => {
                    self.bump();
                }
            }
        }
        self.finish(); // ERROR node
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
        } else if p.current() == SyntaxKind::DOT_DOT_DOT {
            parse_spread(p);
        } else if is_bare_value_start_cst(p) {
            parse_bare_value_body(p);
            // After the bare-value body the document is fully consumed — any
            // remaining tokens are errors. Continue the loop so they are
            // captured into the tree (preserving losslessness) via the normal
            // binding error-recovery path. The loop exits naturally at EOF.
        } else {
            let second = nth_sig(p, 1);
            if (p.current() == SyntaxKind::BAREWORD || p.current() == SyntaxKind::STR)
                && (second == SyntaxKind::PLUS_EQ || second == SyntaxKind::L_BRACE)
            {
                parse_list_op_item(p);
            } else {
                parse_binding(p);
            }
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

/// Returns the kind of the Nth significant token, skipping trivia AND ERROR tokens.
/// Used for disambiguation where malformed input may insert ERROR tokens between
/// a key and its colon (e.g. `café: 1` → BAREWORD("caf") + ERROR("é") + COLON).
fn nth_sig_skip_errors(p: &Parser, n: usize) -> SyntaxKind {
    let mut count = 0;
    let mut i = p.pos;
    while i < p.toks.len() {
        let kind = p.toks[i].kind;
        if !kind.is_trivia() && kind != SyntaxKind::ERROR {
            if count == n {
                return kind;
            }
            count += 1;
        }
        i += 1;
    }
    SyntaxKind::EOF
}

/// True if the current significant token starts a bare-value body.
///
/// Mirrors `Parser::is_bare_value_start` in parser.rs — same rule applied to
/// `SyntaxKind` instead of `Tok`. `{`-leading bodies are NOT bare-value.
fn is_bare_value_start_cst(p: &Parser) -> bool {
    match p.current() {
        // List literal — unambiguously a bare value
        SyntaxKind::L_BRACKET => true,
        // All scalar tokens — never a key
        SyntaxKind::INT
        | SyntaxKind::DECIMAL
        | SyntaxKind::UNIT_LIT
        | SyntaxKind::BOOL
        | SyntaxKind::BYTES
        | SyntaxKind::INTERP_STR => true,
        // String: bare value only if NOT followed by COLON (skip errors too)
        SyntaxKind::STR => nth_sig_skip_errors(p, 1) != SyntaxKind::COLON,
        // Bareword: check the text for `unset`/`match`, or check that the next
        // non-error significant token is not `:` / `+=` / `{`.
        // Skip ERROR tokens in the lookahead so that inputs like `café: 1`
        // (BAREWORD + ERROR + COLON) are still treated as bindings, not bare values.
        SyntaxKind::BAREWORD => {
            let text = current_bareword_text(p);
            match text.as_deref() {
                Some("unset") | Some("match") => true,
                _ => {
                    let next = nth_sig_skip_errors(p, 1);
                    next != SyntaxKind::COLON
                        && next != SyntaxKind::PLUS_EQ
                        && next != SyntaxKind::L_BRACE
                }
            }
        }
        // Everything else (including L_BRACE) — not a bare-value start
        _ => false,
    }
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
                // When we return to depth 0, do NOT break immediately — fall
                // through to the outer loop so it can consume any trailing
                // tokens on this logical line (e.g. `@doc("…")` or `@key(name)`
                // annotations after a closing brace) before stopping at NEWLINE.
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
    parse_atom(p, false, 0);
    p.finish();
}

/// Parse a value expression.
///
/// `stop_at_closer`: when `true` (inside a record or list), the error-recovery
/// sync boundary also includes `R_BRACE` and `R_BRACKET` — the container's
/// closer must NOT be consumed by recovery.
/// `depth`: current container nesting level; incremented on each descent into a
/// record or list.
fn parse_atom(p: &mut Parser, stop_at_closer: bool, depth: usize) {
    match p.current() {
        SyntaxKind::L_BRACE => parse_record(p, depth + 1),
        SyntaxKind::L_BRACKET => parse_list(p, depth + 1),
        SyntaxKind::INT
        | SyntaxKind::STR
        | SyntaxKind::BOOL
        | SyntaxKind::DECIMAL
        | SyntaxKind::UNIT_LIT
        | SyntaxKind::INTERP_STR
        | SyntaxKind::BYTES => p.bump(),
        SyntaxKind::BAREWORD => {
            let bw_text = current_bareword_text(p);
            match bw_text.as_deref() {
                Some("unset") => {
                    p.start(SyntaxKind::UNSET);
                    p.bump();
                    p.finish();
                }
                Some("match") => {
                    p.start(SyntaxKind::MATCH_EXPR);
                    consume_through_newline_at_depth_0(p);
                    p.finish();
                }
                _ if nth_sig(p, 1) == SyntaxKind::L_PAREN => {
                    p.start(SyntaxKind::CALL);
                    p.bump(); // name
                    consume_paren_block(p);
                    p.finish();
                }
                _ => {
                    p.start(SyntaxKind::REF);
                    p.bump();
                    p.finish();
                }
            }
        }
        _ => {
            p.error_and_recover("unexpected token in value position", stop_at_closer);
        }
    }
}

fn parse_record(p: &mut Parser, depth: usize) {
    if depth >= CST_MAX_DEPTH {
        p.push_error("nesting too deep");
        p.start(SyntaxKind::ERROR);
        consume_nested_remainder(p);
        p.finish();
        return;
    }
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
                parse_atom(p, true, depth); // value — stop_at_closer=true: don't eat }
                p.finish(); // FIELD
            }
            // Foreign closers (R_BRACKET, R_PAREN) inside a record: error_and_recover
            // with stop_at_closer=true would break without consuming them, causing an
            // infinite loop. Consume the token explicitly into an ERROR node so the
            // loop always makes progress.
            SyntaxKind::R_BRACKET | SyntaxKind::R_PAREN => {
                p.push_error("unexpected closer in record");
                p.start(SyntaxKind::ERROR);
                p.bump(); // consume the foreign closer
                p.finish();
            }
            _ => {
                p.error_and_recover("unexpected token in record", true);
            }
        }
    }
    p.bump(); // R_BRACE (or no-op at EOF)
    p.finish(); // RECORD
}

fn parse_list(p: &mut Parser, depth: usize) {
    if depth >= CST_MAX_DEPTH {
        p.push_error("nesting too deep");
        p.start(SyntaxKind::ERROR);
        consume_nested_remainder(p);
        p.finish();
        return;
    }
    p.start(SyntaxKind::LIST);
    p.bump(); // L_BRACKET
    loop {
        match p.current() {
            SyntaxKind::R_BRACKET | SyntaxKind::EOF => break,
            SyntaxKind::COMMA | SyntaxKind::NEWLINE => {
                p.bump(); // separator — stays in tree for losslessness
            }
            // Foreign closers (R_BRACE, R_PAREN) inside a list: parse_atom calls
            // error_and_recover with stop_at_closer=true, which breaks without
            // consuming them, causing an infinite loop. Consume the token explicitly
            // into an ERROR node so the loop always makes progress.
            SyntaxKind::R_BRACE | SyntaxKind::R_PAREN => {
                p.push_error("unexpected closer in list");
                p.start(SyntaxKind::ERROR);
                p.bump(); // consume the foreign closer
                p.finish();
            }
            SyntaxKind::DOT_DOT_DOT => {
                p.start(SyntaxKind::LIST_SPREAD);
                p.bump(); // DOT_DOT_DOT
                parse_atom(p, true, depth); // inner expression — stop_at_closer=true
                p.finish(); // LIST_SPREAD
            }
            _ => {
                // `item if cond` — conditional element. Only possible for single-token
                // items (scalars/refs): peek if the 2nd significant token is bareword `if`.
                let is_single_atom = matches!(
                    p.current(),
                    SyntaxKind::INT
                        | SyntaxKind::STR
                        | SyntaxKind::BOOL
                        | SyntaxKind::DECIMAL
                        | SyntaxKind::UNIT_LIT
                        | SyntaxKind::INTERP_STR
                        | SyntaxKind::BYTES
                        | SyntaxKind::BAREWORD
                );
                let has_if_suffix = is_single_atom
                    && nth_sig(p, 1) == SyntaxKind::BAREWORD
                    && nth_bareword_text(p, 1).as_deref() == Some("if");

                if has_if_suffix {
                    p.start(SyntaxKind::COND_ELEM);
                    parse_atom(p, true, depth); // the item
                    p.bump(); // the `if` bareword
                    parse_atom(p, true, depth); // the cond
                    p.finish(); // COND_ELEM
                } else {
                    parse_atom(p, true, depth); // plain element — stop_at_closer=true: don't eat ]
                }
            }
        }
    }
    p.bump(); // R_BRACKET (or no-op at EOF)
    p.finish(); // LIST
}

/// Consume all remaining tokens of the current deeply-nested construct as raw
/// tokens without calling parse_atom/parse_record/parse_list. Tracks
/// brace/bracket/paren depth so the balanced remainder is consumed up to the
/// matching closers or EOF. Every token is bumped into the tree, preserving
/// losslessness.
fn consume_nested_remainder(p: &mut Parser) {
    let mut depth = 0usize;
    loop {
        match p.current() {
            SyntaxKind::EOF => break,
            SyntaxKind::L_BRACE | SyntaxKind::L_BRACKET | SyntaxKind::L_PAREN => {
                depth += 1;
                p.bump();
            }
            SyntaxKind::R_BRACE | SyntaxKind::R_BRACKET | SyntaxKind::R_PAREN => {
                if depth == 0 {
                    // Don't consume the closer that belongs to the parent container.
                    break;
                }
                depth -= 1;
                p.bump();
            }
            SyntaxKind::NEWLINE if depth == 0 => {
                // At top-level (outside any opener we ate), stop before the newline
                // so the enclosing binding loop can use it as a separator.
                break;
            }
            _ => {
                p.bump();
            }
        }
    }
}

fn parse_spread(p: &mut Parser) {
    p.start(SyntaxKind::SPREAD);
    p.bump(); // DOT_DOT_DOT
    if p.current() == SyntaxKind::BAREWORD {
        p.bump(); // alias
    }
    p.finish();
}

/// A bare-value document body: a single value expression at the top level.
/// Wraps the value in a BARE_VALUE node so `lower.rs` can identify it.
fn parse_bare_value_body(p: &mut Parser) {
    p.start(SyntaxKind::BARE_VALUE);
    parse_atom(p, false, 0);
    // Consume trailing newline if present (keeps the tree lossless)
    if p.current() == SyntaxKind::NEWLINE {
        p.bump();
    }
    p.finish();
}

fn parse_list_op_item(p: &mut Parser) {
    p.start(SyntaxKind::LIST_OP_ITEM);
    p.bump(); // key (BAREWORD or STR)
    match p.current() {
        SyntaxKind::PLUS_EQ => {
            p.bump(); // +=
            parse_atom(p, false, 0); // value (typically a list)
        }
        SyntaxKind::L_BRACE => {
            // key { patch/append/remove ops } — consume the whole brace block + newline
            consume_through_newline_at_depth_0(p);
        }
        _ => {}
    }
    p.finish();
}

/// Get the text of the current significant BAREWORD token, without consuming it.
fn current_bareword_text(p: &Parser) -> Option<String> {
    let mut i = p.pos;
    while i < p.toks.len() && p.toks[i].kind.is_trivia() {
        i += 1;
    }
    if i < p.toks.len() && p.toks[i].kind == SyntaxKind::BAREWORD {
        Some(p.src[p.toks[i].start..p.toks[i].end].to_string())
    } else {
        None
    }
}

/// Get the text of the Nth significant BAREWORD token (0 = current), without consuming it.
fn nth_bareword_text(p: &Parser, n: usize) -> Option<String> {
    let mut count = 0;
    let mut i = p.pos;
    while i < p.toks.len() {
        if !p.toks[i].kind.is_trivia() {
            if count == n {
                return if p.toks[i].kind == SyntaxKind::BAREWORD {
                    Some(p.src[p.toks[i].start..p.toks[i].end].to_string())
                } else {
                    None
                };
            }
            count += 1;
        }
        i += 1;
    }
    None
}

/// Consume a parenthesized block `(...)`, already positioned at the opening `(`.
fn consume_paren_block(p: &mut Parser) {
    p.bump(); // `(`
    let mut depth = 1usize;
    loop {
        match p.current() {
            SyntaxKind::EOF => break,
            SyntaxKind::L_PAREN => {
                depth += 1;
                p.bump();
            }
            SyntaxKind::R_PAREN => {
                depth -= 1;
                p.bump();
                if depth == 0 {
                    break;
                }
            }
            _ => {
                p.bump();
            }
        }
    }
}
