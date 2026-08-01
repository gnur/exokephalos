use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

fn send(writer: &mut impl Write, message: &Value) {
    let body = serde_json::to_vec(message).unwrap();
    write!(writer, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
    writer.write_all(&body).unwrap();
    writer.flush().unwrap();
}

fn receive(reader: &mut impl BufRead) -> Value {
    let mut length = None;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).unwrap();
        if header == "\r\n" {
            break;
        }
        if let Some(value) = header.strip_prefix("Content-Length:") {
            length = Some(value.trim().parse::<usize>().unwrap());
        }
    }
    let mut body = vec![0; length.expect("Content-Length header")];
    reader.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn receive_until(reader: &mut impl BufRead, matches: impl Fn(&Value) -> bool) -> Value {
    loop {
        let message = receive(reader);
        if matches(&message) {
            return message;
        }
    }
}

#[test]
fn stdio_lifecycle_loads_workspace_diagnoses_and_completes() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(
        workspace.path().join("note.md"),
        "---\nid: abcdefg\ntitle: Protocol fixture\ntype: note\ntags: [lsp]\n---\nbody",
    )
    .unwrap();
    let root_uri = url::Url::from_directory_path(workspace.path()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_xo-lsp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    send(
        &mut stdin,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": root_uri,
                "capabilities": {}
            }
        }),
    );
    let initialized = receive(&mut stdout);
    assert_eq!(initialized["id"], 1);
    assert_eq!(initialized["result"]["serverInfo"]["name"], "xo-lsp");
    assert_eq!(initialized["result"]["capabilities"]["textDocumentSync"], 1);

    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
    );
    let draft_uri = url::Url::from_file_path(workspace.path().join("draft.md")).unwrap();
    send(
        &mut stdin,
        &json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": draft_uri.as_str(),
                    "languageId": "markdown",
                    "version": 1,
                    "text": "😀 [["
                }
            }
        }),
    );
    let diagnostics = receive_until(&mut stdout, |message| {
        message["method"] == "textDocument/publishDiagnostics"
            && message["params"]["uri"] == draft_uri.as_str()
    });
    assert!(
        diagnostics["params"]["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("frontmatter")
    );
    send(
        &mut stdin,
        &json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "textDocument/completion",
            "params": {
                "textDocument": {"uri": draft_uri.as_str()},
                "position": {"line": 0, "character": 5}
            }
        }),
    );
    let completion = receive_until(&mut stdout, |message| message["id"] == 3);
    assert_eq!(completion["result"][0]["label"], "Protocol fixture");
    assert_eq!(completion["result"][0]["insertText"], "abcdefg");
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown", "params": null}),
    );
    let shutdown = receive_until(&mut stdout, |message| message["id"] == 2);
    assert!(shutdown["result"].is_null());
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "method": "exit", "params": null}),
    );
    drop(stdin);
    assert!(child.wait().unwrap().success());
}
