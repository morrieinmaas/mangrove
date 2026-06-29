//! The `lsp-server` stdio event loop. Read-only and network-free: it never
//! resolves imports, fetches, or writes files. Document state is in-memory and
//! re-analyzed in full on every change.

use crate::analysis::{self, CompletionKind, SemKind, SymbolKind};
use crate::line_index::LineIndex;
use lsp_server::{Connection, ErrorCode, ExtractError, Message, Request, RequestId, Response};
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionList, CompletionOptions, CompletionResponse,
    Diagnostic, DiagnosticSeverity, DocumentSymbol, GotoDefinitionResponse, Hover, HoverContents,
    HoverProviderCapability, Location, MarkupContent, MarkupKind, OneOf, Position,
    PublishDiagnosticsParams, Range, RenameOptions, SemanticToken, SemanticTokenType,
    SemanticTokens, SemanticTokensFullOptions, SemanticTokensLegend, SemanticTokensOptions,
    SemanticTokensResult, SemanticTokensServerCapabilities, ServerCapabilities,
    SymbolKind as LspSymbolKind, TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit, Uri,
    WorkDoneProgressOptions, WorkspaceEdit,
    notification::{
        DidChangeTextDocument, DidOpenTextDocument, Notification as _, PublishDiagnostics,
    },
    request::{
        Completion, DocumentSymbolRequest, Formatting, GotoDefinition, HoverRequest, References,
        Rename, Request as _, SemanticTokensFullRequest,
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
        definition_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        rename_provider: Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(false),
            work_done_progress_options: WorkDoneProgressOptions::default(),
        })),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec![":".to_string(), " ".to_string()]),
            ..CompletionOptions::default()
        }),
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
    /// Session-scoped cache for imported-file reads (mtime/len-keyed).
    /// Lives for the lifetime of the server; avoids re-reading unchanged files
    /// on every completion/goto request.
    import_cache: analysis::ImportCache,
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
    // Capture the id before dispatching so we can reply on panic.
    let req_id = req.id.clone();
    let result =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match req.method.as_str() {
            HoverRequest::METHOD => on_hover(state, req),
            DocumentSymbolRequest::METHOD => on_document_symbol(state, req),
            SemanticTokensFullRequest::METHOD => on_semantic_tokens(state, req),
            Formatting::METHOD => on_formatting(state, req),
            GotoDefinition::METHOD => on_goto_definition(state, req),
            Completion::METHOD => on_completion(state, req),
            References::METHOD => on_references(state, req),
            Rename::METHOD => on_rename(state, req),
            _ => Response::new_err(
                req.id,
                ErrorCode::MethodNotFound as i32,
                format!("method not found: {}", req.method),
            ),
        }));
    let resp = result.unwrap_or_else(|_| {
        Response::new_err(
            req_id,
            ErrorCode::InternalError as i32,
            "internal server error".to_string(),
        )
    });
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
        let end = idx.position(t.range.1);
        // A single token is always on one line, so the UTF-16 length is the
        // column delta between end and start — not the byte delta.
        let len = end.character - start.character;
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

fn on_goto_definition(state: &State, req: Request) -> Response {
    let (id, params) = match cast::<GotoDefinition>(req) {
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

    // First try local (same-file) resolution.
    let result: Option<GotoDefinitionResponse> = analysis::goto_definition(text, offset)
        .and_then(|(start, end)| {
            let idx = LineIndex::new(text);
            let parsed: Uri = uri.parse().ok()?;
            Some(GotoDefinitionResponse::Scalar(Location {
                uri: parsed,
                range: to_range(&idx, (start, end)),
            }))
        })
        // Then try cross-file resolution for qualified references (alias.Type).
        .or_else(|| {
            let doc_path = uri_to_path(&uri)?;
            let (file_path, byte_range, file_text) = analysis::goto_definition_cross_file_cached(
                text,
                offset,
                &doc_path,
                &state.import_cache,
            )?;
            let target_uri = path_to_uri(&file_path)?;
            let idx = LineIndex::new(&file_text);
            Some(GotoDefinitionResponse::Scalar(Location {
                uri: target_uri,
                range: to_range(&idx, byte_range),
            }))
        });

    Response::new_ok(
        id,
        serde_json::to_value(result).unwrap_or(serde_json::Value::Null),
    )
}

/// Decode `%XX` percent-encoded sequences in a URI path component.
/// Decodes byte-by-byte; if the resulting bytes are not valid UTF-8, returns None.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16)?;
            let lo = (bytes[i + 2] as char).to_digit(16)?;
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Convert a `file://` URI string to a filesystem path.
///
/// Percent-decodes `%XX` sequences in the path component. Also handles the
/// Windows triple-slash form `file:///C:/...` by stripping the leading `/`
/// before a drive letter. Falls back to `None` if decoding fails.
pub(crate) fn uri_to_path(uri: &str) -> Option<std::path::PathBuf> {
    let path_str = uri.strip_prefix("file://")?;
    let decoded = percent_decode(path_str)?;
    // Windows: `file:///C:/...` → strip the leading `/` before the drive letter.
    let final_path = if let Some(rest) = decoded.strip_prefix('/') {
        // Drive letter pattern: single ASCII letter followed by `:`
        if rest.len() >= 2 && rest.as_bytes()[0].is_ascii_alphabetic() && rest.as_bytes()[1] == b':'
        {
            rest.to_string()
        } else {
            decoded
        }
    } else {
        decoded
    };
    Some(std::path::PathBuf::from(final_path))
}

/// Convert a filesystem path to a `file://` URI.
fn path_to_uri(path: &std::path::Path) -> Option<Uri> {
    let s = format!("file://{}", path.to_str()?);
    s.parse().ok()
}

fn on_completion(state: &State, req: Request) -> Response {
    let (id, params) = match cast::<Completion>(req) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let uri = params.text_document_position.text_document.uri.to_string();
    let Some(text) = state.docs.get(&uri) else {
        return Response::new_ok(id, serde_json::Value::Null);
    };
    let offset = offset_of(text, params.text_document_position.position);
    let doc_path = uri_to_path(&uri);
    let items: Vec<CompletionItem> =
        analysis::completions_cached(text, offset, doc_path.as_deref(), &state.import_cache)
            .into_iter()
            .map(|c| CompletionItem {
                label: c.label,
                kind: Some(completion_item_kind(c.kind)),
                ..CompletionItem::default()
            })
            .collect();
    let result = CompletionResponse::List(CompletionList {
        is_incomplete: false,
        items,
    });
    Response::new_ok(
        id,
        serde_json::to_value(result).unwrap_or(serde_json::Value::Null),
    )
}

fn on_references(state: &State, req: Request) -> Response {
    let (id, params) = match cast::<References>(req) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let uri = params.text_document_position.text_document.uri.to_string();
    let Some(text) = state.docs.get(&uri) else {
        return Response::new_ok(id, serde_json::Value::Null);
    };
    let offset = offset_of(text, params.text_document_position.position);
    let include_decl = params.context.include_declaration;
    let locs: Vec<Location> = analysis::references(text, offset, include_decl)
        .into_iter()
        .filter_map(|(start, end)| {
            let idx = LineIndex::new(text);
            let parsed: Uri = uri.parse().ok()?;
            Some(Location {
                uri: parsed,
                range: to_range(&idx, (start, end)),
            })
        })
        .collect();
    if locs.is_empty() {
        return Response::new_ok(id, serde_json::Value::Null);
    }
    Response::new_ok(
        id,
        serde_json::to_value(locs).unwrap_or(serde_json::Value::Null),
    )
}

#[allow(clippy::mutable_key_type)] // Uri has interior mutability; standard lsp_types pattern
fn on_rename(state: &State, req: Request) -> Response {
    let (id, params) = match cast::<Rename>(req) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let uri = params.text_document_position.text_document.uri.to_string();
    let Some(text) = state.docs.get(&uri) else {
        return Response::new_ok(id, serde_json::Value::Null);
    };
    let offset = offset_of(text, params.text_document_position.position);
    let Some(ranges) = analysis::rename(text, offset, &params.new_name) else {
        return Response::new_ok(id, serde_json::Value::Null);
    };
    let idx = LineIndex::new(text);
    let edits: Vec<TextEdit> = ranges
        .into_iter()
        .map(|(start, end)| TextEdit {
            range: to_range(&idx, (start, end)),
            new_text: params.new_name.clone(),
        })
        .collect();
    let parsed: Uri = match uri.parse() {
        Ok(u) => u,
        Err(_) => return Response::new_ok(id, serde_json::Value::Null),
    };
    let mut changes = std::collections::HashMap::new();
    changes.insert(parsed, edits);
    let workspace_edit = WorkspaceEdit {
        changes: Some(changes),
        ..WorkspaceEdit::default()
    };
    Response::new_ok(
        id,
        serde_json::to_value(workspace_edit).unwrap_or(serde_json::Value::Null),
    )
}

fn completion_item_kind(k: CompletionKind) -> CompletionItemKind {
    match k {
        CompletionKind::TypeName => CompletionItemKind::STRUCT,
        CompletionKind::Keyword => CompletionItemKind::KEYWORD,
        CompletionKind::Field => CompletionItemKind::FIELD,
        CompletionKind::EnumValue => CompletionItemKind::ENUM_MEMBER,
    }
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
    // Capture id before consuming req — JsonError loses it otherwise.
    let id = req.id.clone();
    match req.extract::<R::Params>(R::METHOD) {
        Ok((id, params)) => Ok((id, params)),
        Err(ExtractError::JsonError { error, .. }) => Err(Response::new_err(
            id,
            ErrorCode::InvalidParams as i32,
            error.to_string(),
        )),
        Err(ExtractError::MethodMismatch(req)) => {
            Err(Response::new_ok(req.id, serde_json::Value::Null))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- uri_to_path unit tests (Item 1) ----

    #[test]
    fn uri_to_path_decodes_space() {
        let p = uri_to_path("file:///Users/me/My%20Configs/main.mang").unwrap();
        assert_eq!(
            p,
            std::path::PathBuf::from("/Users/me/My Configs/main.mang")
        );
    }

    #[test]
    fn uri_to_path_decodes_unicode_sequence() {
        // %C3%A9 = U+00E9 é (UTF-8 two-byte sequence)
        let p = uri_to_path("file:///home/user/caf%C3%A9/doc.mang").unwrap();
        assert_eq!(p, std::path::PathBuf::from("/home/user/café/doc.mang"));
    }

    #[test]
    fn uri_to_path_plain_path_unchanged() {
        let p = uri_to_path("file:///simple/path.mang").unwrap();
        assert_eq!(p, std::path::PathBuf::from("/simple/path.mang"));
    }

    #[test]
    fn uri_to_path_windows_drive_strips_leading_slash() {
        let p = uri_to_path("file:///C:/Users/me/file.mang").unwrap();
        assert_eq!(p, std::path::PathBuf::from("C:/Users/me/file.mang"));
    }

    #[test]
    fn uri_to_path_non_file_scheme_returns_none() {
        assert!(uri_to_path("https://example.com/file.mang").is_none());
    }

    // ---- offset helper ----

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

    /// C1: semantic-token `length` must be UTF-16 code units, not bytes.
    ///
    /// "café" has 5 chars, 6 bytes (é = 2 bytes), but 5 UTF-16 code units.
    /// With surrounding quotes the string token `"café"` is 6 UTF-16 units (7 bytes).
    /// An emoji like 🎉 (U+1F389) is 4 bytes UTF-8 but 2 UTF-16 code units.
    #[test]
    fn semantic_token_length_is_utf16_not_bytes() {
        // "café" → 6 bytes for the quoted string but 6 UTF-16 code units (correct)
        // because é is 2 UTF-8 bytes but 1 UTF-16 code unit.
        // "🎉" → 6 bytes (2 for quotes + 4 for emoji) but 4 UTF-16 units (2 quotes + 2 surrogates).
        let src = "host: \"café\"\n";
        let data = encode_semantic_tokens(src);
        // find the string token
        let string_tok = data
            .iter()
            .find(|t| t.token_type == 2) // STRING = index 2
            .expect("expected a string token");
        // "café" with quotes: 6 chars = 6 UTF-16 code units (é is BMP), 7 bytes
        assert_eq!(
            string_tok.length, 6,
            "UTF-16 length of \"café\" should be 6, not byte-length 7"
        );

        // Now test with an emoji (surrogate pair in UTF-16)
        let src2 = "host: \"🎉\"\n";
        let data2 = encode_semantic_tokens(src2);
        let string_tok2 = data2
            .iter()
            .find(|t| t.token_type == 2)
            .expect("expected a string token");
        // "🎉" with quotes: 2 quotes (2 UTF-16) + 🎉 (2 UTF-16 surrogates) = 4 UTF-16 units
        // but in bytes: 2 + 4 = 6 bytes
        assert_eq!(
            string_tok2.length, 4,
            "UTF-16 length of \"🎉\" should be 4 (2 surrogates + 2 quotes), not byte-length 6"
        );
    }
}
