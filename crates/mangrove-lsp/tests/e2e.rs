//! End-to-end stdio-protocol test: drive the server over an in-memory
//! connection through a real initialize → didOpen → request → shutdown cycle.

use lsp_server::{Connection, Message, Notification, Request, RequestId};
use lsp_types::{
    DidOpenTextDocumentParams, GotoDefinitionParams, HoverParams, InitializeParams,
    InitializedParams, Position, TextDocumentIdentifier, TextDocumentItem,
    TextDocumentPositionParams, Uri, WorkDoneProgressParams,
    notification::{DidOpenTextDocument, Exit, Initialized, Notification as _},
    request::{GotoDefinition, HoverRequest, Initialize, Request as _, Shutdown},
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

/// C2: a request with malformed params must be answered to the CORRECT request id.
/// If the server replies to id 0 instead of the real id, the client hangs.
#[test]
fn malformed_params_reply_uses_correct_request_id() {
    let (server, client) = Connection::memory();
    let server_thread = thread::spawn(move || {
        mangrove_lsp::server::run_on(&server).unwrap();
    });

    // handshake
    let init_id = RequestId::from(1);
    client
        .sender
        .send(Message::Request(Request::new(
            init_id.clone(),
            Initialize::METHOD.to_string(),
            serde_json::to_value(InitializeParams::default()).unwrap(),
        )))
        .unwrap();
    recv_response(&client, &init_id);
    client
        .sender
        .send(Message::Notification(Notification::new(
            Initialized::METHOD.to_string(),
            serde_json::to_value(InitializedParams {}).unwrap(),
        )))
        .unwrap();

    // Send hover request with intentionally malformed params (empty object instead of HoverParams)
    let bad_id = RequestId::from(42);
    client
        .sender
        .send(Message::Request(Request::new(
            bad_id.clone(),
            HoverRequest::METHOD.to_string(),
            serde_json::json!({}), // missing all required fields → JsonError on deserialization
        )))
        .unwrap();

    // The server must reply to id 42, NOT id 0.
    // recv_response waits for id 42; if the server sends id 0 we'd wait forever (timeout via join).
    let resp = recv_response(&client, &bad_id);
    // It should be an error response (not Ok(null))
    assert!(
        resp.error.is_some(),
        "expected an error response for malformed params, got: {:?}",
        resp.result
    );

    // shutdown
    let shutdown_id = RequestId::from(99);
    client
        .sender
        .send(Message::Request(Request::new(
            shutdown_id.clone(),
            Shutdown::METHOD.to_string(),
            serde_json::Value::Null,
        )))
        .unwrap();
    recv_response(&client, &shutdown_id);
    client
        .sender
        .send(Message::Notification(Notification::new(
            Exit::METHOD.to_string(),
            serde_json::Value::Null,
        )))
        .unwrap();
    server_thread.join().unwrap();
}

/// Security invariant: a document containing a namespaced import (`use "ns/pkg@v1" as p`)
/// must not panic and must produce zero type-check diagnostics (the read-only skip guard holds).
#[test]
fn namespaced_import_skips_typecheck_no_panic() {
    let (server, client) = Connection::memory();
    let server_thread = thread::spawn(move || {
        mangrove_lsp::server::run_on(&server).unwrap();
    });

    // handshake
    let init_id = RequestId::from(1);
    client
        .sender
        .send(Message::Request(Request::new(
            init_id.clone(),
            Initialize::METHOD.to_string(),
            serde_json::to_value(InitializeParams::default()).unwrap(),
        )))
        .unwrap();
    recv_response(&client, &init_id);
    client
        .sender
        .send(Message::Notification(Notification::new(
            Initialized::METHOD.to_string(),
            serde_json::to_value(InitializedParams {}).unwrap(),
        )))
        .unwrap();

    // Document with a namespaced import + valid content
    let src = "use \"ns/pkg@v1\" as p\ntype Server = { host: str }\nschema Server\nhost: \"x\"\n";
    client
        .sender
        .send(Message::Notification(Notification::new(
            DidOpenTextDocument::METHOD.to_string(),
            serde_json::to_value(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: uri(),
                    language_id: "mangrove".to_string(),
                    version: 1,
                    text: src.to_string(),
                },
            })
            .unwrap(),
        )))
        .unwrap();

    // (a) server must not panic — if it did, the thread would exit and recv would fail
    // (b) diagnostics must be empty — no type-check errors from the import
    let diag_note = recv_notification(&client, "textDocument/publishDiagnostics");
    let params: lsp_types::PublishDiagnosticsParams =
        serde_json::from_value(diag_note.params).unwrap();
    assert!(
        params.diagnostics.is_empty(),
        "namespaced import doc must yield no diagnostics, got: {:?}",
        params.diagnostics
    );

    // shutdown
    let shutdown_id = RequestId::from(99);
    client
        .sender
        .send(Message::Request(Request::new(
            shutdown_id.clone(),
            Shutdown::METHOD.to_string(),
            serde_json::Value::Null,
        )))
        .unwrap();
    recv_response(&client, &shutdown_id);
    client
        .sender
        .send(Message::Notification(Notification::new(
            Exit::METHOD.to_string(),
            serde_json::Value::Null,
        )))
        .unwrap();
    server_thread.join().unwrap();
}

/// Item 2: LSP-level contract — go-to-definition on a qualified ref whose namespace
/// uses a git backend returns null (no fetch, no panic).
///
/// The read-only / no-network invariant is asserted at the handler boundary: even
/// if `resolve_local_path` returns Err for a git backend, the server must respond
/// with null rather than panicking or blocking on a network call.
#[test]
fn goto_definition_git_backend_namespace_returns_null() {
    // Build a temp project with a git-backend namespace.
    let dir = {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let id = N.fetch_add(1, Ordering::Relaxed);
        let d =
            std::env::temp_dir().join(format!("mangrove_lsp_e2e_git_{}_{id}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    };

    // Write .mangrove/resolvers.toml with a git backend.
    let resolvers_dir = dir.join(".mangrove");
    std::fs::create_dir_all(&resolvers_dir).unwrap();
    std::fs::write(
        resolvers_dir.join("resolvers.toml"),
        "[namespace.myns]\ngit = \"https://example.com/repo.git\"\n",
    )
    .unwrap();

    // The document — a qualified ref `g.SomeType` whose namespace is git-backed.
    let doc_src = "use \"myns/pkg@v1\" as g\ntype Local = g.SomeType\n";
    // URI points into our temp dir so the server can find resolvers.toml via upward search.
    let doc_path = dir.join("main.mang");
    let doc_uri: Uri = format!("file://{}", doc_path.to_str().unwrap())
        .parse()
        .unwrap();

    let (server, client) = Connection::memory();
    let server_thread = thread::spawn(move || {
        mangrove_lsp::server::run_on(&server).unwrap();
    });

    // handshake
    let init_id = RequestId::from(1);
    client
        .sender
        .send(Message::Request(Request::new(
            init_id.clone(),
            Initialize::METHOD.to_string(),
            serde_json::to_value(InitializeParams::default()).unwrap(),
        )))
        .unwrap();
    recv_response(&client, &init_id);
    client
        .sender
        .send(Message::Notification(Notification::new(
            Initialized::METHOD.to_string(),
            serde_json::to_value(InitializedParams {}).unwrap(),
        )))
        .unwrap();

    // didOpen the document (diagnostics may follow — drain them)
    client
        .sender
        .send(Message::Notification(Notification::new(
            DidOpenTextDocument::METHOD.to_string(),
            serde_json::to_value(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: doc_uri.clone(),
                    language_id: "mangrove".to_string(),
                    version: 1,
                    text: doc_src.to_string(),
                },
            })
            .unwrap(),
        )))
        .unwrap();

    // Drain the publishDiagnostics notification.
    recv_notification(&client, "textDocument/publishDiagnostics");

    // Send go-to-definition on the `g.SomeType` reference (cursor on `SomeType`).
    let goto_id = RequestId::from(2);
    // "SomeType" starts at offset 33 in the source — use line 1, char 13 (0-indexed).
    client
        .sender
        .send(Message::Request(Request::new(
            goto_id.clone(),
            GotoDefinition::METHOD.to_string(),
            serde_json::to_value(GotoDefinitionParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier { uri: doc_uri },
                    position: Position::new(1, 13), // "SomeType" in "type Local = g.SomeType"
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: lsp_types::PartialResultParams::default(),
            })
            .unwrap(),
        )))
        .unwrap();

    let resp = recv_response(&client, &goto_id);
    // Must respond (no panic / no hang) and result must be null (no fetch performed).
    assert!(
        resp.error.is_none(),
        "server must not return an error for git-backend goto, got: {:?}",
        resp.error
    );
    assert!(
        resp.result.as_ref().is_none_or(|v| v.is_null()),
        "git-backend goto must return null (no fetch), got: {:?}",
        resp.result
    );

    // shutdown
    let shutdown_id = RequestId::from(99);
    client
        .sender
        .send(Message::Request(Request::new(
            shutdown_id.clone(),
            Shutdown::METHOD.to_string(),
            serde_json::Value::Null,
        )))
        .unwrap();
    recv_response(&client, &shutdown_id);
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
