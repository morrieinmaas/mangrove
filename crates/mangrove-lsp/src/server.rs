//! The `lsp-server` stdio event loop. Read-only and network-free: it never
//! resolves imports, fetches, or writes files. Document state is in-memory and
//! re-analyzed in full on every change.

use crate::analysis::{self, SemKind, SymbolKind};
use crate::line_index::LineIndex;
use lsp_server::{Connection, ExtractError, Message, Request, RequestId, Response};
use lsp_types::{
    Diagnostic, DiagnosticSeverity, DocumentSymbol, Hover, HoverContents, HoverProviderCapability,
    MarkupContent, MarkupKind, OneOf, Position, PublishDiagnosticsParams, Range, SemanticToken,
    SemanticTokenType, SemanticTokens, SemanticTokensFullOptions, SemanticTokensLegend,
    SemanticTokensOptions, SemanticTokensResult, SemanticTokensServerCapabilities,
    ServerCapabilities, SymbolKind as LspSymbolKind, TextDocumentSyncCapability,
    TextDocumentSyncKind, Uri, WorkDoneProgressOptions,
    notification::{
        DidChangeTextDocument, DidOpenTextDocument, Notification as _, PublishDiagnostics,
    },
    request::{
        DocumentSymbolRequest, Formatting, HoverRequest, Request as _, SemanticTokensFullRequest,
    },
};
use std::collections::HashMap;

/// Semantic-token type legend, in the order the LSP indexes them.
const TOKEN_TYPES: &[SemanticTokenType] = &[
    SemanticTokenType::KEYWORD,  // 0
    SemanticTokenType::TYPE,     // 1
    SemanticTokenType::STRING,   // 2
    SemanticTokenType::NUMBER,   // 3
    SemanticTokenType::PROPERTY, // 4
    SemanticTokenType::OPERATOR, // 5
    SemanticTokenType::COMMENT,  // 6
];

fn sem_index(k: SemKind) -> u32 {
    match k {
        SemKind::Keyword => 0,
        SemKind::Type => 1,
        SemKind::String => 2,
        // Units highlight as numbers (numeric literal with a suffix).
        SemKind::Number | SemKind::Unit => 3,
        SemKind::Property => 4,
        SemKind::Operator => 5,
        SemKind::Comment => 6,
    }
}

/// Run the server over stdio until the client shuts it down.
pub fn run() -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    let (connection, io_threads) = Connection::stdio();
    run_on(&connection)?;
    // The connection holds the writer-thread's channel sender; the writer only
    // terminates once that sender is dropped. Drop it BEFORE join, or join hangs.
    drop(connection);
    io_threads.join()?;
    Ok(())
}

/// Drive the protocol on an arbitrary connection (stdio in production, an
/// in-memory pair in tests). Performs the LSP handshake, then the event loop.
pub fn run_on(connection: &Connection) -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    let capabilities = server_capabilities();
    let init_value = serde_json::to_value(&capabilities)?;
    let _params = connection.initialize(init_value)?;
    main_loop(connection)?;
    Ok(())
}

fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        document_formatting_provider: Some(OneOf::Left(true)),
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            SemanticTokensOptions {
                legend: SemanticTokensLegend {
                    token_types: TOKEN_TYPES.to_vec(),
                    token_modifiers: vec![],
                },
                full: Some(SemanticTokensFullOptions::Bool(true)),
                range: Some(false),
                work_done_progress_options: WorkDoneProgressOptions::default(),
            },
        )),
        ..ServerCapabilities::default()
    }
}

/// In-memory document store. URI → current full text.
#[derive(Default)]
struct State {
    docs: HashMap<String, String>,
}

fn main_loop(connection: &Connection) -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    let mut state = State::default();
    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req)? {
                    return Ok(());
                }
                handle_request(connection, &state, req)?;
            }
            Message::Notification(note) => {
                handle_notification(connection, &mut state, note)?;
            }
            Message::Response(_) => {}
        }
    }
    Ok(())
}

fn handle_request(
    connection: &Connection,
    state: &State,
    req: Request,
) -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    let resp = match req.method.as_str() {
        HoverRequest::METHOD => on_hover(state, req),
        DocumentSymbolRequest::METHOD => on_document_symbol(state, req),
        SemanticTokensFullRequest::METHOD => on_semantic_tokens(state, req),
        Formatting::METHOD => on_formatting(state, req),
        _ => Response::new_ok(req.id, serde_json::Value::Null),
    };
    connection.sender.send(Message::Response(resp))?;
    Ok(())
}

fn handle_notification(
    connection: &Connection,
    state: &mut State,
    note: lsp_server::Notification,
) -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    match note.method.as_str() {
        DidOpenTextDocument::METHOD => {
            if let Ok(p) =
                note.extract::<lsp_types::DidOpenTextDocumentParams>(DidOpenTextDocument::METHOD)
            {
                let uri = p.text_document.uri.to_string();
                state.docs.insert(uri.clone(), p.text_document.text.clone());
                publish(connection, &uri, &p.text_document.text)?;
            }
        }
        DidChangeTextDocument::METHOD => {
            if let Ok(p) = note
                .extract::<lsp_types::DidChangeTextDocumentParams>(DidChangeTextDocument::METHOD)
            {
                // FULL sync: the last change carries the entire new text.
                if let Some(change) = p.content_changes.into_iter().last() {
                    let uri = p.text_document.uri.to_string();
                    state.docs.insert(uri.clone(), change.text.clone());
                    publish(connection, &uri, &change.text)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Analyze a document and publish its diagnostics.
fn publish(
    connection: &Connection,
    uri: &str,
    text: &str,
) -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    let idx = LineIndex::new(text);
    let diags = analysis::diagnostics(text)
        .into_iter()
        .map(|d| Diagnostic {
            range: to_range(&idx, d.range),
            severity: Some(DiagnosticSeverity::ERROR),
            source: Some("mangrove".to_string()),
            message: d.message,
            ..Diagnostic::default()
        })
        .collect();
    let parsed: Uri = uri.parse().map_err(|_| "bad uri")?;
    let params = PublishDiagnosticsParams {
        uri: parsed,
        diagnostics: diags,
        version: None,
    };
    let note = lsp_server::Notification::new(
        PublishDiagnostics::METHOD.to_string(),
        serde_json::to_value(params)?,
    );
    connection.sender.send(Message::Notification(note))?;
    Ok(())
}

fn on_hover(state: &State, req: Request) -> Response {
    let (id, params) = match cast::<HoverRequest>(req) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let uri = params
        .text_document_position_params
        .text_document
        .uri
        .to_string();
    let Some(text) = state.docs.get(&uri) else {
        return Response::new_ok(id, serde_json::Value::Null);
    };
    let offset = offset_of(text, params.text_document_position_params.position);
    let result = analysis::hover(text, offset).map(|md| Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: md,
        }),
        range: None,
    });
    Response::new_ok(
        id,
        serde_json::to_value(result).unwrap_or(serde_json::Value::Null),
    )
}

fn on_document_symbol(state: &State, req: Request) -> Response {
    let (id, params) = match cast::<DocumentSymbolRequest>(req) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let uri = params.text_document.uri.to_string();
    let Some(text) = state.docs.get(&uri) else {
        return Response::new_ok(id, serde_json::Value::Null);
    };
    let idx = LineIndex::new(text);
    #[allow(deprecated)]
    let symbols: Vec<DocumentSymbol> = analysis::symbols(text)
        .into_iter()
        .map(|s| {
            let range = to_range(&idx, s.range);
            DocumentSymbol {
                name: s.name,
                detail: None,
                kind: lsp_symbol_kind(s.kind),
                tags: None,
                deprecated: None,
                range,
                selection_range: range,
                children: None,
            }
        })
        .collect();
    Response::new_ok(
        id,
        serde_json::to_value(lsp_types::DocumentSymbolResponse::Nested(symbols))
            .unwrap_or(serde_json::Value::Null),
    )
}

fn on_semantic_tokens(state: &State, req: Request) -> Response {
    let (id, params) = match cast::<SemanticTokensFullRequest>(req) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let uri = params.text_document.uri.to_string();
    let Some(text) = state.docs.get(&uri) else {
        return Response::new_ok(id, serde_json::Value::Null);
    };
    let result = SemanticTokensResult::Tokens(SemanticTokens {
        result_id: None,
        data: encode_semantic_tokens(text),
    });
    Response::new_ok(
        id,
        serde_json::to_value(result).unwrap_or(serde_json::Value::Null),
    )
}

/// Delta-encode classified tokens into the LSP semantic-tokens wire format.
fn encode_semantic_tokens(text: &str) -> Vec<SemanticToken> {
    let idx = LineIndex::new(text);
    let mut data = Vec::new();
    let (mut prev_line, mut prev_char) = (0u32, 0u32);
    for t in analysis::semantic_tokens(text) {
        let start = idx.position(t.range.0);
        let len = (t.range.1 - t.range.0) as u32;
        let delta_line = start.line - prev_line;
        let delta_start = if delta_line == 0 {
            start.character - prev_char
        } else {
            start.character
        };
        data.push(SemanticToken {
            delta_line,
            delta_start,
            length: len,
            token_type: sem_index(t.kind),
            token_modifiers_bitset: 0,
        });
        prev_line = start.line;
        prev_char = start.character;
    }
    data
}

fn on_formatting(state: &State, req: Request) -> Response {
    let (id, params) = match cast::<Formatting>(req) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let uri = params.text_document.uri.to_string();
    let Some(text) = state.docs.get(&uri) else {
        return Response::new_ok(id, serde_json::Value::Null);
    };
    let formatted = mangrove_fmt::format_str(text).text;
    if formatted == *text {
        return Response::new_ok(
            id,
            serde_json::to_value(Vec::<lsp_types::TextEdit>::new()).unwrap(),
        );
    }
    // Replace the whole document with one edit.
    let idx = LineIndex::new(text);
    let end = idx.position(text.len());
    let edit = lsp_types::TextEdit {
        range: Range {
            start: Position::new(0, 0),
            end: Position::new(end.line, end.character),
        },
        new_text: formatted,
    };
    Response::new_ok(
        id,
        serde_json::to_value(vec![edit]).unwrap_or(serde_json::Value::Null),
    )
}

// ---- helpers ----

fn to_range(idx: &LineIndex, (start, end): (usize, usize)) -> Range {
    let s = idx.position(start);
    let e = idx.position(end);
    Range {
        start: Position::new(s.line, s.character),
        end: Position::new(e.line, e.character),
    }
}

/// LSP `Position` (UTF-16) → byte offset in `text`.
fn offset_of(text: &str, pos: Position) -> usize {
    let mut line = 0u32;
    let mut byte = 0usize;
    // advance to the start of the target line
    for (i, b) in text.bytes().enumerate() {
        if line == pos.line {
            byte = i;
            break;
        }
        if b == b'\n' {
            line += 1;
            byte = i + 1;
        }
    }
    if line < pos.line {
        return text.len();
    }
    // walk UTF-16 units within the line
    let mut utf16 = 0u32;
    for ch in text[byte..].chars() {
        if utf16 >= pos.character || ch == '\n' {
            break;
        }
        utf16 += ch.len_utf16() as u32;
        byte += ch.len_utf8();
    }
    byte
}

fn lsp_symbol_kind(k: SymbolKind) -> LspSymbolKind {
    match k {
        SymbolKind::Type => LspSymbolKind::STRUCT,
        SymbolKind::Unit => LspSymbolKind::ENUM,
        SymbolKind::Schema => LspSymbolKind::INTERFACE,
        SymbolKind::Param => LspSymbolKind::NAMESPACE,
        SymbolKind::Fn => LspSymbolKind::FUNCTION,
        SymbolKind::Binding => LspSymbolKind::FIELD,
    }
}

fn cast<R>(req: Request) -> Result<(RequestId, R::Params), Response>
where
    R: lsp_types::request::Request,
{
    match req.extract::<R::Params>(R::METHOD) {
        Ok((id, params)) => Ok((id, params)),
        Err(ExtractError::JsonError { .. }) => Err(Response::new_ok(
            RequestId::from(0),
            serde_json::Value::Null,
        )),
        Err(ExtractError::MethodMismatch(req)) => {
            Err(Response::new_ok(req.id, serde_json::Value::Null))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_maps_line_and_utf16_char_to_byte() {
        let text = "abc\ndef\n";
        // line 1, char 2 → byte 6 ('f')
        assert_eq!(offset_of(text, Position::new(1, 2)), 6);
        assert_eq!(offset_of(text, Position::new(0, 0)), 0);
    }

    #[test]
    fn semantic_token_indices_are_within_legend() {
        for k in [
            SemKind::Keyword,
            SemKind::Type,
            SemKind::String,
            SemKind::Number,
            SemKind::Unit,
            SemKind::Property,
            SemKind::Operator,
            SemKind::Comment,
        ] {
            assert!((sem_index(k) as usize) < TOKEN_TYPES.len());
        }
    }

    #[test]
    fn semantic_tokens_delta_encode_first_token_at_absolute_position() {
        let data = encode_semantic_tokens("type Server = { host: str }\n");
        assert!(!data.is_empty());
        // first token (`type`) is at line 0, char 0
        assert_eq!(data[0].delta_line, 0);
        assert_eq!(data[0].delta_start, 0);
        assert_eq!(data[0].length, 4);
        assert_eq!(data[0].token_type, 0); // keyword
    }
}
