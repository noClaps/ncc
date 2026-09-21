//! Stdio LSP transport, versioned incremental document sync and diagnostics.
use serde_json::{Value, json};
#[path = "lsp_index.rs"]
mod index;
use std::{
    collections::HashMap,
    io::{self, BufRead, Write},
    path::PathBuf,
};

pub fn serve(mut input: impl BufRead, mut output: impl Write) -> io::Result<()> {
    let mut documents = HashMap::<String, Document>::new();
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
                json!({"capabilities":{"positionEncoding":"utf-16","textDocumentSync":{"openClose":true,"change":2},"documentFormattingProvider":true,"documentSymbolProvider":true,"workspaceSymbolProvider":true,"definitionProvider":true,"hoverProvider":true,"completionProvider":{}},"serverInfo":{"name":"ncc","version":env!("CARGO_PKG_VERSION")}}),
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
                    let version = p["textDocument"]["version"].as_i64().unwrap_or(0);
                    let updated = if method.ends_with("didOpen") {
                        p["textDocument"]["text"].as_str().map(|text| Document {
                            text: text.into(),
                            version,
                        })
                    } else {
                        documents
                            .get(uri)
                            .and_then(|document| document.changed(version, &p["contentChanges"]))
                    };
                    if let Some(document) = updated {
                        documents.insert(uri.into(), document);
                        publish_diagnostics(&documents, &mut output)?;
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
                    publish_diagnostics(&documents, &mut output)?;
                }
                None
            }
            "textDocument/didSave" | "workspace/didChangeWatchedFiles" => {
                publish_diagnostics(&documents, &mut output)?;
                None
            }
            "textDocument/formatting" => {
                let text = p["textDocument"]["uri"]
                    .as_str()
                    .and_then(|uri| documents.get(uri))
                    .map(|document| &document.text);
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
            "workspace/symbol" => {
                let query = p["query"].as_str().unwrap_or("");
                let mut symbols = Vec::new();
                for (uri, document) in &documents {
                    symbols.extend(index::Index::new(&document.text).symbols(uri, query));
                }
                Some(json!(symbols))
            }
            "textDocument/documentSymbol"
            | "textDocument/definition"
            | "textDocument/hover"
            | "textDocument/completion" => {
                let uri = p["textDocument"]["uri"].as_str().unwrap_or("");
                Some(if let Some(document) = documents.get(uri) {
                    let index = index::Index::new(&document.text);
                    if method.ends_with("documentSymbol") {
                        json!(index.symbols(uri, ""))
                    } else if let Some(at) = offset(&document.text, &p["position"]) {
                        let imported =
                            if matches!(method, "textDocument/definition" | "textDocument/hover") {
                                imported_navigation(&index, at, uri, method, &documents)
                            } else {
                                None
                            };
                        imported.unwrap_or_else(|| match method {
                            "textDocument/definition" => index.definition(uri, at),
                            "textDocument/hover" => index.hover(at),
                            _ => index.completion(at),
                        })
                    } else {
                        Value::Null
                    }
                } else {
                    Value::Null
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

struct Document {
    text: String,
    version: i64,
}
fn imported_navigation(
    index: &index::Index<'_>,
    at: usize,
    uri: &str,
    method: &str,
    documents: &HashMap<String, Document>,
) -> Option<Value> {
    let (imported, name) = index.imported_member(at)?;
    let root = document_path(uri);
    let path = root.parent()?.join(imported).with_extension("nc");
    let key = crate::modules::source_key(&path);
    let open = documents
        .iter()
        .find(|(uri, _)| crate::modules::source_key(&document_path(uri)) == key);
    let source = open
        .map(|(_, document)| document.text.clone())
        .or_else(|| std::fs::read_to_string(&path).ok())?;
    let imported = index::Index::new(&source);
    let position = imported.exported_position(&name)?;
    Some(if method == "textDocument/definition" {
        imported.definition(
            &open.map_or_else(|| file_uri(&path), |(uri, _)| uri.clone()),
            position,
        )
    } else {
        imported.hover(position)
    })
}
fn publish_diagnostics(
    documents: &HashMap<String, Document>,
    output: &mut impl Write,
) -> io::Result<()> {
    let sources = documents
        .iter()
        .map(|(uri, document)| {
            (
                crate::modules::source_key(&document_path(uri)),
                document.text.clone(),
            )
        })
        .collect();
    let mut ordered = documents.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|(uri, _)| *uri);
    for (uri, document) in ordered {
        let text = &document.text;
        let path = document_path(uri);
        let diagnostics = match crate::lint::check_with_sources(text, &path, &sources) {
            Ok(warnings) => warnings.iter().filter(|w| crate::modules::source_key(&w.path) == crate::modules::source_key(&path)).map(|w| json!({"range":{"start":position(text,w.span.start),"end":position(text,w.span.end)},"severity":2,"source":"ncc","code":w.code,"message":w.message})).collect(),
            Err(errors) => errors.0.iter().map(|error| {
                let actual_path = error.path.as_deref().unwrap_or(&path);
                if crate::modules::source_key(actual_path) != crate::modules::source_key(&path) {
                    let imported = crate::modules::read_source(actual_path, &sources).unwrap_or_default();
                    json!({"range":{"start":position(text,0),"end":position(text,0)},"severity":1,"source":"ncc","message":format!("{}: {}",actual_path.display(),error.message),"relatedInformation":[{"location":{"uri":file_uri(actual_path),"range":{"start":position(&imported,error.span.start),"end":position(&imported,error.span.end)}},"message":error.message}]})
                } else {
                    json!({"range":{"start":position(text, error.span.start),"end":position(text,error.span.end)},"severity":1,"source":"ncc","message":error.message})
                }
            }).collect::<Vec<_>>()
        };
        send(
            output,
            json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":uri,"version":document.version,"diagnostics":diagnostics}}),
        )?;
    }
    Ok(())
}
impl Document {
    /// Apply a batch atomically: each range refers to the preceding edit's text.
    fn changed(&self, version: i64, changes: &Value) -> Option<Self> {
        if version <= self.version {
            return None;
        }
        let mut text = self.text.clone();
        for change in changes.as_array()? {
            let replacement = change["text"].as_str()?;
            if let Some(range) = change.get("range") {
                let start = offset(&text, &range["start"])?;
                let end = offset(&text, &range["end"])?;
                if start > end {
                    return None;
                }
                text.replace_range(start..end, replacement);
            } else {
                text = replacement.into();
            }
        }
        Some(Self { text, version })
    }
}

/// LSP columns count UTF-16 code units, not bytes or Unicode scalar values.
fn offset(text: &str, position: &Value) -> Option<usize> {
    let line = usize::try_from(position["line"].as_u64()?).ok()?;
    let column = usize::try_from(position["character"].as_u64()?).ok()?;
    let mut start = 0;
    for _ in 0..line {
        start += text.get(start..)?.find('\n')? + 1;
    }
    let content = text
        .get(start..)?
        .split('\n')
        .next()?
        .trim_end_matches('\r');
    let mut units = 0;
    for (byte, character) in content.char_indices() {
        if units == column {
            return Some(start + byte);
        }
        units += character.len_utf16();
        if units > column {
            return None;
        }
    }
    // The protocol clamps columns beyond the end of the line.
    Some(start + content.len())
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
fn file_uri(path: &std::path::Path) -> String {
    let path = crate::modules::source_key(path);
    let mut uri = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        if byte.is_ascii_alphanumeric() || b"/-_.~:".contains(&byte) {
            uri.push(byte as char);
        } else {
            use std::fmt::Write;
            let _ = write!(uri, "%{byte:02X}");
        }
    }
    uri
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incremental_edits_are_utf16_versioned_and_atomic() {
        let document = Document {
            text: "str s = \"😀\"\r\nint x = 1\n".into(),
            version: 1,
        };
        let changes = json!([
            {"range":{"start":{"line":0,"character":9},"end":{"line":0,"character":11}},"text":"hi"},
            {"range":{"start":{"line":1,"character":8},"end":{"line":1,"character":999}},"text":"42"}
        ]);
        let updated = document.changed(2, &changes).unwrap();
        assert_eq!(updated.text, "str s = \"hi\"\r\nint x = 42\n");
        assert!(updated.changed(2, &changes).is_none());
        let invalid = json!([{"range":{"start":{"line":0,"character":10},"end":{"line":0,"character":11}},"text":"x"}]);
        assert!(document.changed(3, &invalid).is_none());
        assert_eq!(document.text, "str s = \"😀\"\r\nint x = 1\n");
        assert_eq!(
            offset(&document.text, &json!({"line":2,"character":0})),
            Some(document.text.len())
        );
        assert_eq!(
            offset(&document.text, &json!({"line":3,"character":0})),
            None
        );
    }
}
