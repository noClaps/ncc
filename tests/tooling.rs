#[cfg(feature = "lsp")]
use serde_json::{Value, json};

#[test]
fn formatter_preserves_comments_literals_and_is_idempotent() {
    let source = "test \"format\" {\n     // keep this\n   str s = \"{literal}\"\n if true {\ntrue -> { @print(s) }\nfalse -> {}\n}\n}\n";
    let formatted = ncc::formatter::format(source).unwrap();
    assert!(formatted.contains("\n  // keep this\n"));
    assert!(formatted.contains("\n    true ->"));
    assert_eq!(ncc::formatter::format(&formatted).unwrap(), formatted);
    let tokens = |s| {
        ncc::lexer::lex(s)
            .unwrap()
            .into_iter()
            .map(|t| t.kind)
            .collect::<Vec<_>>()
    };
    assert_eq!(tokens(source), tokens(&formatted));
}

#[test]
#[cfg(feature = "lsp")]
fn lsp_lifecycle_diagnostics_and_formatting() {
    let messages = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///test.nc","version":1,"text":"int x = true"}}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":"file:///test.nc","version":2},"contentChanges":[{"text":"test \"ok\" {\nassert true\n}\n"}]}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/formatting","params":{"textDocument":{"uri":"file:///test.nc"}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ];
    let input = messages
        .iter()
        .map(|m| {
            let s = m.to_string();
            format!("Content-Length: {}\r\n\r\n{s}", s.len())
        })
        .collect::<String>();
    let mut output = vec![];
    ncc::lsp::serve(std::io::Cursor::new(input), &mut output).unwrap();
    let text = String::from_utf8(output).unwrap();
    let messages: Vec<Value> = text
        .split("Content-Length: ")
        .filter(|s| !s.is_empty())
        .map(|s| serde_json::from_str(s.split_once("\r\n\r\n").unwrap().1).unwrap())
        .collect();
    assert_eq!(
        messages[0]["result"]["capabilities"]["textDocumentSync"]["change"],
        2
    );
    assert_eq!(
        messages[1]["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        messages[2]["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        messages[3]["result"][0]["newText"]
            .as_str()
            .unwrap()
            .contains("  assert true")
    );
    assert_eq!(messages[4]["id"], 3);
}

#[test]
fn char_literals_are_extended_graphemes() {
    for character in ["o\u{308}", "👩‍👩‍👧‍👦", "🇮🇳"] {
        assert!(ncc::lexer::lex(&format!("'{character}'")).is_ok());
    }
    assert!(ncc::lexer::lex("'ab'").is_err());
}

#[test]
#[cfg(feature = "lsp")]
fn lsp_navigation_docs_and_completion() {
    let source = "/// Increment an integer.\nfn increment(int n) int { return n + 1 }\nint answer = increment(41)\n";
    let messages = [
        json!({"method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///navigation.nc","version":1,"text":source}}}),
        json!({"id":1,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///navigation.nc"},"position":{"line":2,"character":15}}}),
        json!({"id":2,"method":"textDocument/definition","params":{"textDocument":{"uri":"file:///navigation.nc"},"position":{"line":2,"character":15}}}),
        json!({"id":3,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///navigation.nc"},"position":{"line":3,"character":0}}}),
        json!({"id":4,"method":"textDocument/documentSymbol","params":{"textDocument":{"uri":"file:///navigation.nc"}}}),
        json!({"id":5,"method":"shutdown"}),
        json!({"method":"exit"}),
    ];
    let input: String = messages
        .iter()
        .map(|message| {
            let body = message.to_string();
            format!("Content-Length: {}\r\n\r\n{body}", body.len())
        })
        .collect();
    let mut output = Vec::new();
    ncc::lsp::serve(std::io::Cursor::new(input), &mut output).unwrap();
    let output = String::from_utf8(output).unwrap();
    let responses: Vec<Value> = output
        .split("Content-Length: ")
        .skip(1)
        .map(|part| serde_json::from_str(part.split_once("\r\n\r\n").unwrap().1).unwrap())
        .collect();
    assert!(
        responses[1]["result"]["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("Increment an integer.")
    );
    assert_eq!(
        responses[2]["result"]["range"]["start"],
        json!({"line":1,"character":3})
    );
    let completions = responses[3]["result"]["items"].as_array().unwrap();
    assert!(completions.iter().any(|item| item["label"] == "increment"));
    assert!(!completions.iter().any(|item| item["label"] == "n"));
    assert_eq!(responses[4]["result"].as_array().unwrap().len(), 3);
}

#[test]
#[cfg(feature = "lsp")]
fn lsp_rechecks_importers_against_unsaved_buffers() {
    let directory = ncc::temp::Directory::new().unwrap();
    std::fs::write(
        directory.path().join("dependency.nc"),
        "pub fn value() str { return \"disk\" }",
    )
    .unwrap();
    let root = format!("file://{}/main.nc", directory.path().display());
    let dependency = format!("file://{}/dependency.nc", directory.path().display());
    let messages = [
        json!({"method":"textDocument/didOpen","params":{"textDocument":{"uri":root,"version":1,"text":"import { \"dependency\" as dep } int n = dep.value()"}}}),
        json!({"method":"textDocument/didOpen","params":{"textDocument":{"uri":dependency,"version":1,"text":"/// Unsaved documentation.\npub fn value() int { return 42 }"}}}),
        json!({"id":2,"method":"textDocument/definition","params":{"textDocument":{"uri":root},"position":{"line":0,"character":44}}}),
        json!({"id":3,"method":"textDocument/hover","params":{"textDocument":{"uri":root},"position":{"line":0,"character":44}}}),
        json!({"method":"textDocument/didChange","params":{"textDocument":{"uri":dependency,"version":2},"contentChanges":[{"text":"pub fn value() bool { return true }"}]}}),
        json!({"method":"textDocument/didClose","params":{"textDocument":{"uri":dependency}}}),
        json!({"id":1,"method":"shutdown"}),
        json!({"method":"exit"}),
    ];
    let input: String = messages
        .iter()
        .map(|message| {
            let text = message.to_string();
            format!("Content-Length: {}\r\n\r\n{text}", text.len())
        })
        .collect();
    let mut output = Vec::new();
    ncc::lsp::serve(std::io::Cursor::new(input), &mut output).unwrap();
    let text = String::from_utf8(output).unwrap();
    let notifications: Vec<Value> = text
        .split("Content-Length: ")
        .skip(1)
        .map(|part| serde_json::from_str(part.split_once("\r\n\r\n").unwrap().1).unwrap())
        .collect();
    let root_errors: Vec<bool> = notifications
        .iter()
        .filter(|message| message["params"]["uri"] == root)
        .map(|message| {
            !message["params"]["diagnostics"]
                .as_array()
                .unwrap()
                .is_empty()
        })
        .collect();
    assert_eq!(root_errors, [true, false, true, true]);
    let definition = notifications
        .iter()
        .find(|message| message["id"] == 2)
        .unwrap();
    assert_eq!(definition["result"]["uri"], dependency);
    assert_eq!(
        definition["result"]["range"]["start"],
        json!({"line":1,"character":7})
    );
    let hover = notifications
        .iter()
        .find(|message| message["id"] == 3)
        .unwrap();
    assert!(
        hover["result"]["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("Unsaved documentation.")
    );
    assert_eq!(
        std::fs::read_to_string(directory.path().join("dependency.nc")).unwrap(),
        "pub fn value() str { return \"disk\" }"
    );
}

#[test]
fn checks_can_import_new_unsaved_files() {
    let directory = ncc::temp::Directory::new().unwrap();
    let root = directory.path().join("main.nc");
    let sources = std::collections::HashMap::from([(
        ncc::modules::source_key(&directory.path().join("new.nc")),
        "pub fn value() int { return 42 }".into(),
    )]);
    ncc::lint::check_with_sources(
        "import { \"./new\" as fresh } int n = fresh.value()",
        &root,
        &sources,
    )
    .unwrap();
    assert!(!directory.path().join("new.nc").exists());
}

#[test]
fn imported_errors_retain_source_paths_and_declaration_locations() {
    let directory = ncc::temp::Directory::new().unwrap();
    let root = directory.path().join("main.nc");
    let imported = directory.path().join("library.nc");
    std::fs::write(
        &imported,
        "// header\n\npub fn broken() int {\n  int n = false\n  return n\n}\n",
    )
    .unwrap();
    let source = "import { \"library\" as lib }";
    let error = ncc::check_source(source, &root).unwrap_err();
    assert_eq!(error.0[0].path.as_deref(), Some(imported.as_path()));
    let rendered = error.render(source, &root);
    assert!(
        rendered.contains(&format!("{}:4:3:", imported.display())),
        "{rendered}"
    );
    assert!(rendered.contains("int n = false"));
    std::fs::write(&imported, "pub fn broken( { }").unwrap();
    assert_eq!(
        ncc::check_source(source, &root).unwrap_err().0[0]
            .path
            .as_deref(),
        Some(imported.as_path())
    );
}
