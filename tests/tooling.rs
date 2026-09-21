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
