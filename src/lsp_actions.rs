//! Source-preserving editor actions. All changes are proposals, never disk writes.
use super::{Document, document_path, offset, position};
use crate::lexer::{self, TokenKind};
use serde_json::{Value, json};
use std::{collections::HashMap, ops::Range};

pub(super) fn actions(params: &Value, documents: &HashMap<String, Document>) -> Value {
    let Some(uri) = params["textDocument"]["uri"].as_str() else {
        return json!([]);
    };
    let Some(document) = documents.get(uri) else {
        return json!([]);
    };
    let text = &document.text;
    let Some(at) = offset(text, &params["range"]["start"]) else {
        return json!([]);
    };
    let Ok(tokens) = lexer::lex(text) else {
        return json!([]);
    };
    let mut actions = Vec::new();
    let mut propose = |title: String, kind: &str, span: Range<usize>, replacement: String| {
        if let Some(only) = params["context"]["only"].as_array()
            && !only
                .iter()
                .filter_map(Value::as_str)
                .any(|filter| kind == filter || kind.starts_with(&format!("{filter}.")))
        {
            return;
        }
        actions.push(json!({"title":title,"kind":kind,"edit":{"changes":{uri:[{
            "range":{"start":position(text,span.start),"end":position(text,span.end)},"newText":replacement
        }]}}}));
    };
    for token in &tokens {
        if !token.span.contains(&at) && token.span.end != at {
            continue;
        }
        if let TokenKind::Int(literal) = &token.kind
            && let Ok(value) = crate::lexer::integer(literal)
        {
            let suffix = if literal.ends_with('u') { "u" } else { "" };
            for (base, value) in [
                ("decimal", value.to_string()),
                ("hexadecimal", format!("0x{value:x}")),
                ("octal", format!("0o{value:o}")),
                ("binary", format!("0b{value:b}")),
            ] {
                let replacement = format!("{value}{suffix}");
                if text[token.span.clone()] != replacement {
                    propose(
                        format!("Convert integer to {base}"),
                        "refactor.rewrite",
                        token.span.clone(),
                        replacement,
                    );
                }
            }
        }
    }
    // Comments live in the gaps between lexer tokens. Never interpret slashes
    // inside a quoted or multiline literal as a comment marker.
    let mut previous = 0;
    for token in &tokens {
        if previous <= at && at <= token.span.start {
            let line_start = text[..at]
                .rfind('\n')
                .map_or(previous, |n| n + 1)
                .max(previous);
            let line_end = text[at..token.span.start]
                .find('\n')
                .map_or(token.span.start, |n| at + n);
            if let Some(relative) = text[line_start..line_end].find("//") {
                let start = line_start + relative;
                if at >= start {
                    let doc = text[start..].starts_with("///");
                    propose(
                        if doc {
                            "Convert to regular comment"
                        } else {
                            "Convert to documentation comment"
                        }
                        .into(),
                        "refactor.rewrite",
                        start..start + if doc { 3 } else { 2 },
                        if doc { "// " } else { "///" }.into(),
                    );
                }
            }
        }
        previous = token.span.end;
    }
    let path = document_path(uri);
    let sources = documents
        .iter()
        .map(|(uri, document)| {
            (
                crate::modules::source_key(&document_path(uri)),
                document.text.clone(),
            )
        })
        .collect();
    if crate::lint::check_with_sources(text, &path, &sources).is_err_and(|errors| {
        errors
            .0
            .iter()
            .any(|error| error.message.contains("return value of function not used"))
    }) {
        let line = text[..at].rfind('\n').map_or(0, |n| n + 1);
        let start = line + text[line..].len() - text[line..].trim_start_matches([' ', '\t']).len();
        let mut edited = text.clone();
        edited.insert_str(start, "_ = ");
        if crate::lint::check_with_sources(&edited, &path, &sources).is_ok() {
            propose(
                "Discard unused result explicitly".into(),
                "quickfix",
                start..start,
                "_ = ".into(),
            );
        }
    }
    json!(actions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(text: &str, at: usize, only: Option<&str>) -> Value {
        let uri = "file:///actions.nc";
        let mut params = json!({"textDocument":{"uri":uri},"range":{"start":position(text,at),"end":position(text,at)},"context":{}});
        if let Some(kind) = only {
            params["context"]["only"] = json!([kind]);
        }
        actions(
            &params,
            &HashMap::from([(
                uri.into(),
                Document {
                    text: text.into(),
                    version: 1,
                },
            )]),
        )
    }
    fn apply(text: &str, action: &Value) -> String {
        let edit = &action["edit"]["changes"]["file:///actions.nc"][0];
        let start = offset(text, &edit["range"]["start"]).unwrap();
        let end = offset(text, &edit["range"]["end"]).unwrap();
        let mut result = text.to_string();
        result.replace_range(start..end, edit["newText"].as_str().unwrap());
        result
    }
    #[test]
    fn integer_actions_preserve_values_suffixes_and_utf16_positions() {
        for literal in [
            "255",
            "0xFFu",
            "0b1010",
            "0o77",
            "18446744073709551615u",
            "9223372036854775808",
        ] {
            let text = format!("str s = \"🍪\"\n_ = -{literal}\n");
            let result = request(&text, text.find(literal).unwrap(), None);
            assert!(!result.as_array().unwrap().is_empty());
            for action in result.as_array().unwrap() {
                let edited = apply(&text, action);
                let replacement = edited.split("_ = -").nth(1).unwrap().trim();
                assert_eq!(
                    crate::lexer::integer(literal).unwrap(),
                    crate::lexer::integer(replacement)
                        .unwrap_or_else(|_| panic!("{literal} -> {replacement}: {action}"))
                );
                assert_eq!(literal.ends_with('u'), replacement.ends_with('u'));
            }
        }
        assert!(
            request("_ = 42", 5, Some("quickfix"))
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn comment_actions_do_not_touch_literals_and_discard_is_checked() {
        for text in [
            "// regular\n",
            "/// documentation\n",
            "//// extra slash\r\n",
            "_ = 1 // trailing\n",
        ] {
            let action = request(text, text.find("//").unwrap() + 2, None);
            let edited = apply(text, &action[0]);
            assert_eq!(
                lexer::lex(text)
                    .unwrap()
                    .into_iter()
                    .map(|t| t.kind)
                    .collect::<Vec<_>>(),
                lexer::lex(&edited)
                    .unwrap()
                    .into_iter()
                    .map(|t| t.kind)
                    .collect::<Vec<_>>()
            );
        }
        for text in [
            "str s = \"// not comment\"",
            "str s = \"\"\"\n// not comment\n\"\"\"",
        ] {
            assert!(
                request(text, text.find("//").unwrap(), None)
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
        }
        let text = "fn value() int { return 1 }\nvalue()\n";
        let result = request(text, text.rfind("value()").unwrap(), Some("quickfix"));
        assert_eq!(result.as_array().unwrap().len(), 1);
        let edited = apply(text, &result[0]);
        assert!(edited.contains("\n_ = value()\n"));
        crate::check_source(&edited, std::path::Path::new("actions.nc")).unwrap();
    }
}
