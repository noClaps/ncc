//! Stdio LSP server: full document sync, diagnostics and formatting.
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{self, BufRead, Write},
    path::PathBuf,
};

pub fn serve(mut input: impl BufRead, mut output: impl Write) -> io::Result<()> {
    let mut documents = HashMap::<String, String>::new();
    let mut shutdown = false;
    loop {
        let mut length = None;
        loop {
            let mut header = String::new();
            if input.read_line(&mut header)? == 0 {
                return Ok(());
            }
            if header == "\r\n" || header == "\n" {
                break;
            }
            if let Some((key, value)) = header.split_once(':')
                && key.eq_ignore_ascii_case("content-length")
            {
                length = value.trim().parse::<usize>().ok();
            }
        }
        let length = length
            .filter(|n| *n <= 16 * 1024 * 1024)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid Content-Length"))?;
        let mut body = vec![0; length];
        input.read_exact(&mut body)?;
        let request: Value = match serde_json::from_slice(&body) {
            Ok(x) => x,
            Err(_) => {
                send(
                    &mut output,
                    json!({"jsonrpc":"2.0", "id":null, "error":{"code":-32700,"message":"Parse error"}}),
                )?;
                continue;
            }
        };
        let method = request["method"].as_str().unwrap_or("");
        let id = request.get("id").cloned();
        let p = &request["params"];
        if method == "exit" {
            return if shutdown {
                Ok(())
            } else {
                Err(io::Error::other("exit before shutdown"))
            };
        }
        let result = match method {
            "initialize" => Some(
                json!({"capabilities":{"positionEncoding":"utf-16","textDocumentSync":{"openClose":true,"change":1},"documentFormattingProvider":true},"serverInfo":{"name":"ncc","version":env!("CARGO_PKG_VERSION")}}),
            ),
            "shutdown" => {
                shutdown = true;
                Some(Value::Null)
            }
            _ if shutdown => {
                if let Some(id) = id {
                    send(
                        &mut output,
                        json!({"jsonrpc":"2.0","id":id,"error":{"code":-32600,"message":"Server is shut down"}}),
                    )?;
                }
                continue;
            }
            "initialized" | "$/cancelRequest" => None,
            "textDocument/didOpen" | "textDocument/didChange" => {
                if let Some(uri) = p["textDocument"]["uri"].as_str() {
                    let text = if method.ends_with("didOpen") {
                        p["textDocument"]["text"].as_str()
                    } else {
                        p["contentChanges"]
                            .as_array()
                            .and_then(|a| a.last())
                            .and_then(|v| v["text"].as_str())
                    };
                    if let Some(text) = text {
                        documents.insert(uri.into(), text.into());
                        let path = document_path(uri);
                        let diagnostics = match crate::lint::check(text, &path) {
                            Ok(warnings) => warnings.iter().filter(|w| w.path == path).map(|w| json!({"range":{"start":position(text,w.span.start),"end":position(text,w.span.end)},"severity":2,"source":"ncc","code":w.code,"message":w.message})).collect(),
                            Err(errors) => errors.0.iter().map(|e| json!({"range":{"start":position(text, e.span.start),"end":position(text,e.span.end)},"severity":1,"source":"ncc","message":e.message})).collect::<Vec<_>>()
                        };
                        send(
                            &mut output,
                            json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":uri,"version":p["textDocument"]["version"],"diagnostics":diagnostics}}),
                        )?;
                    }
                }
                None
            }
            "textDocument/didClose" => {
                if let Some(uri) = p["textDocument"]["uri"].as_str() {
                    documents.remove(uri);
                    send(
                        &mut output,
                        json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":uri,"diagnostics":[]}}),
                    )?;
                }
                None
            }
            "textDocument/formatting" => {
                let text = p["textDocument"]["uri"]
                    .as_str()
                    .and_then(|uri| documents.get(uri));
                Some(if let Some(text) = text {
                    match crate::formatter::format(text) {
                        Ok(formatted) => {
                            json!([{"range":{"start":{"line":0,"character":0},"end":position(text,text.len())},"newText":formatted}])
                        }
                        Err(_) => json!([]),
                    }
                } else {
                    json!([])
                })
            }
            _ => {
                if let Some(id) = id {
                    send(
                        &mut output,
                        json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not found"}}),
                    )?;
                }
                continue;
            }
        };
        if let (Some(id), Some(result)) = (id, result) {
            send(
                &mut output,
                json!({"jsonrpc":"2.0","id":id,"result":result}),
            )?;
        }
    }
}
fn document_path(uri: &str) -> PathBuf {
    let Some(path) = uri.strip_prefix("file://") else {
        return PathBuf::from(uri);
    };
    let path = path
        .strip_prefix("localhost/")
        .map_or_else(|| path.to_owned(), |p| format!("/{p}"));
    let mut bytes = Vec::new();
    let mut i = 0;
    while i < path.len() {
        if path.as_bytes()[i] == b'%'
            && i + 2 < path.len()
            && let Some(hex) = path.get(i + 1..i + 3)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            bytes.push(byte);
            i += 3;
            continue;
        }
        bytes.push(path.as_bytes()[i]);
        i += 1;
    }
    PathBuf::from(String::from_utf8_lossy(&bytes).into_owned())
}
fn send(output: &mut impl Write, message: Value) -> io::Result<()> {
    let body = serde_json::to_vec(&message)?;
    write!(output, "Content-Length: {}\r\n\r\n", body.len())?;
    output.write_all(&body)?;
    output.flush()
}
fn position(text: &str, offset: usize) -> Value {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let prefix = &text[..offset];
    let line = prefix.bytes().filter(|b| *b == b'\n').count();
    let start = prefix.rfind('\n').map_or(0, |i| i + 1);
    json!({"line":line,"character":prefix[start..].encode_utf16().count()})
}
