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
