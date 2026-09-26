//! Editor source index. Reuses parser declarations, never the lowered C names.
use crate::{
    lexer::{self, Keyword, Token, TokenKind},
    parser::{self, SourceSymbol},
};
use serde_json::{Value, json};

pub(super) struct Index<'a> {
    text: &'a str,
    tokens: Vec<Token>,
    symbols: Vec<SourceSymbol>,
}

impl<'a> Index<'a> {
    fn imported_path(&self, alias: &str) -> Option<String> {
        let mut importing = false;
        for (index, token) in self.tokens.iter().enumerate() {
            match &token.kind {
                TokenKind::Keyword(Keyword::Import) => importing = true,
                TokenKind::RBrace => importing = false,
                TokenKind::String(path)
                    if importing
                        && matches!(
                            self.tokens.get(index + 1).map(|token| &token.kind),
                            Some(TokenKind::Keyword(Keyword::As))
                        )
                        && matches!(self.tokens.get(index + 2).map(|token| &token.kind), Some(TokenKind::Ident(name)) if name == alias) =>
                {
                    return Some(path.clone());
                }
                _ => {}
            }
        }
        None
    }

    /// Recognize an unshadowed import namespace, not arbitrary member spelling.
    pub fn imported_member(&self, at: usize) -> Option<(String, String)> {
        let index = self
            .tokens
            .iter()
            .position(|token| token.span.contains(&at))
            .or_else(|| {
                self.tokens.iter().position(|token| {
                    token.span.end == at && matches!(token.kind, TokenKind::Ident(_))
                })
            })?;
        if index < 2 || self.tokens[index - 1].kind != TokenKind::Dot {
            return None;
        }
        let (TokenKind::Ident(alias), TokenKind::Ident(name)) =
            (&self.tokens[index - 2].kind, &self.tokens[index].kind)
        else {
            return None;
        };
        if self.resolve(alias, at).is_some() {
            return None;
        }
        Some((self.imported_path(alias)?, name.clone()))
    }

    pub fn exported_position(&self, name: &str) -> Option<usize> {
        self.symbols
            .iter()
            .find(|symbol| {
                symbol.name == name
                    && symbol.scope.is_none()
                    && self.text[..symbol.declaration.start]
                        .trim_end()
                        .ends_with("pub")
            })
            .map(|symbol| symbol.selection.start)
    }
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
                        || (symbol.owner.is_none()
                            && symbol.visible_from <= at
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
        if index > 0 && self.tokens[index - 1].kind == TokenKind::At {
            return None;
        }
        if index >= 2 && self.tokens[index - 1].kind == TokenKind::Dot {
            let object = self.selected(self.tokens[index - 2].span.start)?;
            let owner = self.type_name(object)?;
            let TokenKind::Ident(name) = &self.tokens[index].kind else {
                return None;
            };
            return self
                .symbols
                .iter()
                .find(|s| s.owner.as_deref() == Some(owner.as_str()) && s.name == *name);
        }
        let TokenKind::Ident(name) = &self.tokens[index].kind else {
            return None;
        };
        self.resolve(name, self.tokens[index].span.start)
    }
    fn type_name(&self, symbol: &SourceSymbol) -> Option<String> {
        self.tokens
            .iter()
            .filter(|t| {
                t.span.start >= symbol.declaration.start && t.span.end <= symbol.selection.start
            })
            .find_map(|t| match &t.kind {
                TokenKind::Ident(name) => Some(name.clone()),
                _ => None,
            })
    }
    pub fn type_definition(&self, uri: &str, at: usize) -> Value {
        self.selected(at)
            .and_then(|s| self.type_name(s))
            .and_then(|name| self.resolve(&name, at))
            .map_or(
                Value::Null,
                |s| json!({"uri":uri,"range":range(self.text,&s.selection)}),
            )
    }
    pub fn references(&self, uri: &str, at: usize, declaration: bool) -> Value {
        let Some(symbol) = self.selected(at) else {
            return json!([]);
        };
        let positions = self
            .tokens
            .iter()
            .filter(|t| matches!(&t.kind, TokenKind::Ident(name) if *name == symbol.name))
            .filter(|t| declaration || t.span != symbol.selection)
            .filter(|t| {
                self.selected(t.span.start)
                    .is_some_and(|s| s.selection == symbol.selection)
            })
            .map(|t| json!({"uri":uri,"range":range(self.text,&t.span)}))
            .collect::<Vec<_>>();
        json!(positions)
    }
    pub fn prepare_rename(&self, at: usize) -> Value {
        self.selected(at)
            .filter(|s| s.owner.is_none() && s.scope.is_some())
            .map_or(
                Value::Null,
                |s| {
                    let token = self.tokens.iter().find(|t| t.span.contains(&at))
                        .or_else(|| self.tokens.iter().find(|t| t.span.end == at && matches!(t.kind, TokenKind::Ident(_))));
                    json!({"range":range(self.text, &token.map_or_else(|| s.selection.clone(), |t| t.span.clone())),"placeholder":s.name})
                },
            )
    }
    pub fn rename(&self, uri: &str, at: usize, name: &str) -> Value {
        let Ok(tokens) = lexer::lex(name) else {
            return Value::Null;
        };
        if tokens.len() != 2
            || !matches!(&tokens[0].kind, TokenKind::Ident(n) if n == name && n != "_")
            || matches!(
                name,
                "bool" | "byte" | "int" | "uint" | "float" | "str" | "char" | "void"
            )
        {
            return Value::Null;
        }
        let Some(symbol) = self
            .selected(at)
            .filter(|s| s.owner.is_none() && s.scope.is_some())
        else {
            return Value::Null;
        };
        let references = self.references(uri, at, true);
        // Refuse a rename that would capture another binding in any affected scope.
        if self.symbols.iter().any(|s| {
            s.name == name
                && s.selection != symbol.selection
                && s.owner.is_none()
                && s.scope == symbol.scope
        }) || self.tokens.iter().any(|t| {
            self.selected(t.span.start)
                .is_some_and(|s| s.selection == symbol.selection)
                && self
                    .resolve(name, t.span.start)
                    .is_some_and(|s| s.selection != symbol.selection)
        }) {
            return Value::Null;
        }
        let edits = references
            .as_array()
            .unwrap()
            .iter()
            .map(|r| json!({"range":r["range"],"newText":name}))
            .collect::<Vec<_>>();
        json!({"changes":{uri:edits}})
    }
    pub fn folding_ranges(&self) -> Value {
        let mut stack = Vec::new();
        let mut ranges = Vec::new();
        for token in &self.tokens {
            match token.kind {
                TokenKind::LBrace | TokenKind::LBracket | TokenKind::LParen => stack.push(token),
                TokenKind::RBrace | TokenKind::RBracket | TokenKind::RParen => {
                    if let Some(start) = stack.pop() {
                        let first = super::position(self.text, start.span.start)["line"]
                            .as_u64()
                            .unwrap();
                        let last = super::position(self.text, token.span.start)["line"]
                            .as_u64()
                            .unwrap();
                        if last > first {
                            ranges.push(json!({"startLine":first,"endLine":last,"kind":"region"}));
                        }
                    }
                }
                _ => {}
            }
        }
        json!(ranges)
    }
    pub fn signature_help(&self, at: usize) -> Value {
        let mut stack: Vec<(usize, usize)> = Vec::new();
        for (i, token) in self
            .tokens
            .iter()
            .enumerate()
            .take_while(|(_, t)| t.span.start < at)
        {
            match token.kind {
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => stack.push((i, 0)),
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                    stack.pop();
                }
                TokenKind::Comma => {
                    if let Some((_, count)) = stack.last_mut() {
                        *count += 1;
                    }
                }
                _ => {}
            }
        }
        for (i, active) in stack.into_iter().rev() {
            if i == 0 || self.tokens[i].kind != TokenKind::LParen {
                continue;
            }
            let Some(symbol) = self
                .selected(self.tokens[i - 1].span.start)
                .filter(|s| s.kind == 12)
            else {
                continue;
            };
            let params = self
                .symbols
                .iter()
                .filter(|s| {
                    s.kind == 13
                        && s.scope
                            .as_ref()
                            .is_some_and(|scope| scope.start == symbol.selection.start)
                })
                .map(|s| json!({"label":self.detail(s)}))
                .collect::<Vec<_>>();
            return json!({"signatures":[{"label":self.detail(symbol),"documentation":{"kind":"markdown","value":self.documentation(symbol)},"parameters":params}],"activeSignature":0,"activeParameter":active});
        }
        Value::Null
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
        let before = self
            .tokens
            .iter()
            .enumerate()
            .rfind(|(_, t)| t.span.end <= at && !matches!(t.kind, TokenKind::Eof));
        let dot = before.and_then(|(i, t)| {
            if t.kind == TokenKind::Dot {
                Some(i)
            } else if matches!(t.kind, TokenKind::Ident(_))
                && i > 0
                && self.tokens[i - 1].kind == TokenKind::Dot
            {
                Some(i - 1)
            } else {
                None
            }
        });
        if let Some(dot) = dot {
            let owner = dot
                .checked_sub(1)
                .and_then(|i| self.selected(self.tokens[i].span.start))
                .and_then(|s| self.type_name(s));
            let items = self
                .symbols
                .iter()
                .filter(|s| owner.is_some() && s.owner == owner)
                .map(|s| json!({"label":s.name,"kind":5,"detail":self.detail(s)}))
                .collect::<Vec<_>>();
            return json!({"isIncomplete":false,"items":items});
        }
        let mut items = std::collections::BTreeMap::new();
        for builtin in [
            "@print",
            "@println",
            "@eprint",
            "@eprintln",
            "@as",
            "@args",
            "@env",
            "@target",
            "@embed",
        ] {
            items.insert(builtin.into(), json!({"label":builtin,"kind":3}));
        }
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
    fn references_and_rename_preserve_shadowing_and_reject_collisions() {
        let source =
            "fn f(int x) int {\n int y = x\n { int x = 3 @println(x) }\n return x + y\n}\n";
        let index = Index::new(source);
        let at = source.find("= x").unwrap() + 2;
        assert_eq!(
            index
                .references("file:///a.nc", at, true)
                .as_array()
                .unwrap()
                .len(),
            3
        );
        assert_eq!(
            index
                .references("file:///a.nc", at, false)
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            index.prepare_rename(at)["range"]["start"],
            super::super::position(source, at)
        );
        let edits = index.rename("file:///a.nc", at, "input");
        assert_eq!(
            edits["changes"]["file:///a.nc"].as_array().unwrap().len(),
            3
        );
        for name in ["y", "int", "a b", "_", "", "x.y"] {
            assert!(index.rename("file:///a.nc", at, name).is_null(), "{name}");
        }
        assert!(index.prepare_rename(source.find("f(").unwrap()).is_null());
    }

    #[test]
    fn fields_type_navigation_signatures_and_folds() {
        let source = "struct Point {\n int x\n int y\n}\nfn add(Point p, int n) int {\n return p.x + n\n}\nPoint p = Point { .x = 1, .y = 2 }\n@println(add(p, 3))\n";
        let index = Index::new(source);
        let field = source.find("p.x").unwrap() + 2;
        assert_eq!(
            index.selected(field).unwrap().selection.start,
            source.find("x\n").unwrap()
        );
        assert_eq!(
            index
                .references("file:///a.nc", field, false)
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            index.type_definition("file:///a.nc", field - 2)["range"]["start"],
            super::super::position(source, source.find("Point").unwrap())
        );
        let items = index.completion(field);
        assert_eq!(items["items"].as_array().unwrap().len(), 2);
        let signature = index.signature_help(source.find("p, 3").unwrap() + 3);
        assert_eq!(signature["activeParameter"], 1);
        assert_eq!(
            signature["signatures"][0]["parameters"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            signature["signatures"][0]["parameters"][0]["label"],
            "Point p"
        );
        assert_eq!(index.folding_ranges().as_array().unwrap().len(), 2);
    }

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
