//! Editor source index. Reuses parser declarations, never the lowered C names.
use crate::{
    lexer::{self, Token, TokenKind},
    parser::{self, SourceSymbol},
};
use serde_json::{Value, json};

pub(super) struct Index<'a> {
    text: &'a str,
    tokens: Vec<Token>,
    symbols: Vec<SourceSymbol>,
}

impl<'a> Index<'a> {
    pub fn new(text: &'a str) -> Self {
        let tokens = lexer::lex(text).unwrap_or_default();
        let symbols = parser::source_symbols(tokens.clone());
        Self {
            text,
            tokens,
            symbols,
        }
    }

    fn resolve(&self, name: &str, at: usize) -> Option<&SourceSymbol> {
        self.symbols
            .iter()
            .filter(|symbol| {
                symbol.name == name
                    && (symbol.selection.contains(&at)
                        || (symbol.visible_from <= at
                            && symbol
                                .scope
                                .as_ref()
                                .is_none_or(|scope| scope.contains(&at))))
            })
            .max_by_key(|symbol| {
                (
                    symbol.scope.as_ref().map_or(0, |scope| scope.start + 1),
                    symbol.selection.start,
                )
            })
    }

    fn selected(&self, at: usize) -> Option<&SourceSymbol> {
        let index = self
            .tokens
            .iter()
            .position(|token| token.span.contains(&at))
            .or_else(|| {
                self.tokens.iter().position(|token| {
                    token.span.end == at && matches!(token.kind, TokenKind::Ident(_))
                })
            })?;
        // A member's spelling alone does not identify its declaration.
        if index > 0 && matches!(self.tokens[index - 1].kind, TokenKind::Dot | TokenKind::At) {
            return None;
        }
        let TokenKind::Ident(name) = &self.tokens[index].kind else {
            return None;
        };
        self.resolve(name, self.tokens[index].span.start)
    }

    pub fn definition(&self, uri: &str, at: usize) -> Value {
        self.selected(at).map_or(
            Value::Null,
            |symbol| json!({"uri":uri,"range":range(self.text, &symbol.selection)}),
        )
    }

    fn detail(&self, symbol: &SourceSymbol) -> &str {
        let declaration = self.text.get(symbol.declaration.clone()).unwrap_or("");
        if matches!(symbol.kind, 12 | 23 | 10) {
            declaration.split('{').next().unwrap_or(declaration).trim()
        } else {
            declaration.split('=').next().unwrap_or(declaration).trim()
        }
    }

    fn documentation(&self, symbol: &SourceSymbol) -> String {
        let line = self.text[..symbol.declaration.start]
            .rfind('\n')
            .map_or(0, |i| i + 1);
        let mut lines = Vec::new();
        for line in self.text[..line].lines().rev() {
            if let Some(comment) = line.trim().strip_prefix("///") {
                lines.push(comment.trim_start());
            } else {
                break;
            }
        }
        lines.reverse();
        lines.join("\n")
    }

    pub fn hover(&self, at: usize) -> Value {
        self.selected(at).map_or(Value::Null, |symbol| {
            let documentation = self.documentation(symbol);
            json!({"contents":{"kind":"markdown","value":format!("```nc\n{}\n```\n\n{}", self.detail(symbol), documentation)}})
        })
    }

    pub fn symbols(&self, uri: &str, query: &str) -> Vec<Value> {
        self.symbols.iter().filter(|symbol| symbol.name.to_lowercase().contains(&query.to_lowercase())).map(|symbol| {
            json!({"name":symbol.name,"kind":symbol.kind,"location":{"uri":uri,"range":range(self.text, &symbol.selection)}})
        }).collect()
    }

    pub fn completion(&self, at: usize) -> Value {
        let mut items = std::collections::BTreeMap::new();
        for symbol in &self.symbols {
            if let Some(resolved) = self.resolve(&symbol.name, at) {
                items.insert(symbol.name.clone(), json!({"label":symbol.name,"kind":match resolved.kind { 12 => 3, 23 => 22, 10 => 13, 14 => 21, 26 => 25, _ => 6 }, "detail":self.detail(resolved),"documentation":{"kind":"markdown","value":self.documentation(resolved)}}));
            }
        }
        for keyword in [
            "fn", "struct", "enum", "type", "pub", "mut", "mutex", "fut", "import", "extern",
            "test", "if", "else", "catch", "for", "while", "in", "lock", "async", "await", "try",
            "return", "throw", "break", "continue", "assert", "true", "false", "none", "and", "or",
            "not", "bool", "byte", "int", "uint", "float", "str", "char", "void",
        ] {
            items
                .entry(keyword.into())
                .or_insert_with(|| json!({"label":keyword,"kind":14}));
        }
        json!({"isIncomplete":false,"items":items.into_values().collect::<Vec<_>>()})
    }
}

fn range(text: &str, span: &std::ops::Range<usize>) -> Value {
    json!({"start":super::position(text, span.start),"end":super::position(text, span.end)})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_uses_scopes_shadowing_parameters_and_doc_comments() {
        let source = "/// Add one.\nfn add(int n) int {\n  int x = n\n  { str x = \"inner\" @println(x) }\n  return x + 1\n}\nint x = add(2)\n";
        let index = Index::new(source);
        assert_eq!(
            index
                .selected(source.find("x +").unwrap())
                .unwrap()
                .selection
                .start,
            source.find("x = n").unwrap()
        );
        assert_eq!(
            index
                .selected(source.find("println(x)").unwrap() + 8)
                .unwrap()
                .selection
                .start,
            source.find("x = \"").unwrap()
        );
        assert_eq!(
            index
                .selected(source.find("= n").unwrap() + 2)
                .unwrap()
                .selection
                .start,
            source.find("n)").unwrap()
        );
        let hover = index.hover(source.rfind("add(2)").unwrap());
        assert!(
            hover["contents"]["value"]
                .as_str()
                .unwrap()
                .contains("Add one.")
        );
        assert!(
            index
                .selected(source.find("int x = add").unwrap() + 6)
                .is_none()
        );
        let labels = index.completion(source.len());
        assert!(
            !labels["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["label"] == "n")
        );
    }
}
