//! End-to-end stdio-protocol test: drive the server over an in-memory
//! connection through a real initialize → didOpen → request → shutdown cycle.

use lsp_server::{Connection, Message, Notification, Request, RequestId};
use lsp_types::{
    DidOpenTextDocumentParams, HoverParams, InitializeParams, InitializedParams, Position,
    TextDocumentIdentifier, TextDocumentItem, TextDocumentPositionParams, Uri,
    WorkDoneProgressParams,
    notification::{DidOpenTextDocument, Exit, Initialized, Notification as _},
    request::{HoverRequest, Initialize, Request as _, Shutdown},
};
use std::thread;

fn uri() -> Uri {
    "file:///test.mang".parse().unwrap()
}

#[test]
fn initialize_open_diagnostics_hover_shutdown() {
    let (server, client) = Connection::memory();

    // Run the server in a background thread.
    let server_thread = thread::spawn(move || {
        mangrove_lsp::server::run_on(&server).unwrap();
    });

    // --- handshake ---
    let init_id = RequestId::from(1);
    client
        .sender
        .send(Message::Request(Request::new(
            init_id.clone(),
            Initialize::METHOD.to_string(),
            serde_json::to_value(InitializeParams::default()).unwrap(),
        )))
        .unwrap();
    // server replies with InitializeResult
    let resp = recv_response(&client, &init_id);
    assert!(resp.result.is_some(), "expected InitializeResult");
    client
        .sender
        .send(Message::Notification(Notification::new(
            Initialized::METHOD.to_string(),
            serde_json::to_value(InitializedParams {}).unwrap(),
        )))
        .unwrap();

    // --- didOpen a document with a schema error (duplicate type) ---
    let bad = "type T = int\ntype T = str\nschema T\nx: 1\n";
    client
        .sender
        .send(Message::Notification(Notification::new(
            DidOpenTextDocument::METHOD.to_string(),
            serde_json::to_value(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: uri(),
                    language_id: "mangrove".to_string(),
                    version: 1,
                    text: bad.to_string(),
                },
            })
            .unwrap(),
        )))
        .unwrap();

    // server should publish diagnostics
    let diag_note = recv_notification(&client, "textDocument/publishDiagnostics");
    let params: lsp_types::PublishDiagnosticsParams =
        serde_json::from_value(diag_note.params).unwrap();
    assert!(
        !params.diagnostics.is_empty(),
        "expected a diagnostic for the bad schema"
    );
    assert!(
        params.diagnostics[0].message.contains("schema error"),
        "diagnostic: {:?}",
        params.diagnostics[0].message
    );

    // --- hover on the `T` type name ---
    let hover_id = RequestId::from(2);
    let off = Position::new(0, 5); // the `T` in `type T`
    client
        .sender
        .send(Message::Request(Request::new(
            hover_id.clone(),
            HoverRequest::METHOD.to_string(),
            serde_json::to_value(HoverParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier { uri: uri() },
                    position: off,
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
            })
            .unwrap(),
        )))
        .unwrap();
    let hover_resp = recv_response(&client, &hover_id);
    assert!(
        hover_resp.result.is_some_and(|v| !v.is_null()),
        "expected a hover result on the type name"
    );

    // --- shutdown / exit ---
    let shutdown_id = RequestId::from(3);
    client
        .sender
        .send(Message::Request(Request::new(
            shutdown_id.clone(),
            Shutdown::METHOD.to_string(),
            serde_json::Value::Null,
        )))
        .unwrap();
    let _ = recv_response(&client, &shutdown_id);
    client
        .sender
        .send(Message::Notification(Notification::new(
            Exit::METHOD.to_string(),
            serde_json::Value::Null,
        )))
        .unwrap();

    server_thread.join().unwrap();
}

// ---- receive helpers: drain until the expected message arrives ----

fn recv_response(client: &Connection, id: &RequestId) -> lsp_server::Response {
    loop {
        match client.receiver.recv().expect("server closed") {
            Message::Response(r) if &r.id == id => return r,
            _ => continue,
        }
    }
}

fn recv_notification(client: &Connection, method: &str) -> lsp_server::Notification {
    loop {
        match client.receiver.recv().expect("server closed") {
            Message::Notification(n) if n.method == method => return n,
            _ => continue,
        }
    }
}
