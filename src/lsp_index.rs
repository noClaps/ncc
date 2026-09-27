//! Editor source index. Reuses parser declarations, never the lowered C names.
use crate::{
    lexer::{self, Keyword, Token, TokenKind},
    parser::{self, SourceSymbol},
};
use serde_json::{Value, json};

const BUILTINS: &[(&str, &str, &str)] = &[
    (
        "@print",
        "fn @print(...values)",
        "Write values to standard output without a newline.",
    ),
    (
        "@println",
        "fn @println(...values)",
        "Write values to standard output followed by a newline.",
    ),
    (
        "@eprint",
        "fn @eprint(...values)",
        "Write values to standard error without a newline.",
    ),
    (
        "@eprintln",
        "fn @eprintln(...values)",
        "Write values to standard error followed by a newline.",
    ),
    (
        "@as",
        "fn @as(type T, value) T",
        "Convert a value to the requested type. Numeric conversions check the target range.",
    ),
    (
        "@args",
        "fn @args() str[]",
        "Read process arguments at runtime. The first entry is the executable name.",
    ),
    (
        "@env",
        "fn @env() [str]str",
        "Read the process environment at runtime.",
    ),
    (
        "@target",
        "fn @target() (str, str)",
        "Return the target operating system and architecture at compile time.",
    ),
    (
        "@embed",
        "fn @embed(str path) byte[]",
        "Embed file bytes at compile time. Relative paths are resolved beside this source file; symbolic links are rejected.",
    ),
];

pub(super) struct Index<'a> {
    text: &'a str,
    tokens: Vec<Token>,
    symbols: Vec<SourceSymbol>,
}

impl<'a> Index<'a> {
    fn builtin(&self, at: usize) -> Option<&'static (&'static str, &'static str, &'static str)> {
        self.tokens.windows(2).find_map(|pair| {
            if pair[0].kind != TokenKind::At
                || !(pair[0].span.start..=pair[1].span.end).contains(&at)
            {
                return None;
            }
            let name = match &pair[1].kind {
                TokenKind::Ident(name) => name.as_str(),
                TokenKind::Keyword(Keyword::As) => "as",
                _ => return None,
            };
            BUILTINS
                .iter()
                .find(|(builtin, _, _)| &builtin[1..] == name)
        })
    }
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
    pub fn exported_at(&self, at: usize) -> Option<String> {
        let symbol = self.selected(at)?;
        (self.exported_position(&symbol.name) == Some(symbol.selection.start))
            .then(|| symbol.name.clone())
    }
    pub fn imported_references(&self, uri: &str, name: &str) -> Vec<(String, Value)> {
        self.tokens
            .iter()
            .filter(|t| matches!(&t.kind, TokenKind::Ident(n) if n == name))
            .filter_map(|t| {
                let (path, _) = self.imported_member(t.span.start)?;
                Some((path, json!({"uri":uri,"range":range(self.text,&t.span)})))
            })
            .collect()
    }
    pub fn imported_completion(&self, at: usize) -> Option<String> {
        let (i, token) = self
            .tokens
            .iter()
            .enumerate()
            .rfind(|(_, t)| t.span.start < at && t.kind != TokenKind::Eof)?;
        let dot = if token.kind == TokenKind::Dot {
            i
        } else if matches!(token.kind, TokenKind::Ident(_)) {
            i.checked_sub(1)?
        } else {
            return None;
        };
        if self.tokens[dot].kind != TokenKind::Dot {
            return None;
        }
        let TokenKind::Ident(alias) = &self.tokens[dot.checked_sub(1)?].kind else {
            return None;
        };
        if self.resolve(alias, at).is_some() {
            return None;
        }
        self.imported_path(alias)
    }
    pub fn exported_completions(&self) -> Value {
        let items = self.symbols.iter().filter(|s| self.exported_position(&s.name) == Some(s.selection.start))
            .map(|s| json!({"label":s.name,"kind":match s.kind {12 => 3, 23 | 10 | 26 => 7, _ => 6},"detail":self.detail(s),"documentation":{"kind":"markdown","value":self.documentation(s)}})).collect::<Vec<_>>();
        json!({"isIncomplete":false,"items":items})
    }
    pub fn new(text: &'a str) -> Self {
        let tokens = lexer::lex(text).unwrap_or_else(|error| {
            let end = error.0.first().map_or(0, |d| d.span.start.min(text.len()));
            lexer::lex(&text[..end])
                .or_else(|_| {
                    let line = text[..end].rfind('\n').unwrap_or(0);
                    lexer::lex(&text[..line])
                })
                .unwrap_or_default()
        });
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
        if matches!(symbol.kind, 10 | 23 | 26) {
            return Some(symbol.name.clone());
        }
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
        if !valid_name(name) {
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
    pub fn can_rename_export(&self, at: usize, name: &str) -> bool {
        let Some(old) = self.exported_at(at) else {
            return false;
        };
        valid_name(name)
            && (old == name
                || (!self
                    .symbols
                    .iter()
                    .any(|s| s.owner.is_none() && s.name == name)
                    && self.imported_path(name).is_none()))
    }
    pub fn rename_range(&self, at: usize, name: &str) -> Value {
        self.tokens
            .iter()
            .find(|t| t.span.contains(&at) && matches!(&t.kind, TokenKind::Ident(n) if n == name))
            .map_or(
                Value::Null,
                |t| json!({"range":range(self.text, &t.span),"placeholder":name}),
            )
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
    fn call_sites(&self, at: usize) -> Vec<(usize, usize)> {
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
        stack
            .into_iter()
            .rev()
            .filter(|(i, _)| *i > 0 && self.tokens[*i].kind == TokenKind::LParen)
            .map(|(i, active)| (self.tokens[i - 1].span.start, active))
            .collect()
    }
    pub fn imported_signature(&self, at: usize) -> Option<(String, String, usize)> {
        for (callee, active) in self.call_sites(at) {
            if self.builtin(callee).is_some()
                || self
                    .selected(callee)
                    .is_some_and(|s| matches!(s.kind, 12 | 22))
            {
                return None;
            }
            if let Some((path, name)) = self.imported_member(callee) {
                return Some((path, name, active));
            }
        }
        None
    }
    pub fn signature_help(&self, at: usize) -> Value {
        for (callee, active) in self.call_sites(at) {
            if let Some((_, signature, documentation)) = self.builtin(callee) {
                let parameters = signature
                    .split_once('(')
                    .unwrap()
                    .1
                    .split_once(')')
                    .unwrap()
                    .0;
                let params = parameters
                    .split(',')
                    .filter(|p| !p.is_empty())
                    .map(|p| json!({"label":p.trim()}))
                    .collect::<Vec<_>>();
                let active = active.min(params.len().saturating_sub(1));
                return json!({"signatures":[{"label":signature,"documentation":documentation,"parameters":params}],"activeSignature":0,"activeParameter":active});
            }
            let signature = self.signature_at(callee, active);
            if !signature.is_null() {
                return signature;
            }
        }
        Value::Null
    }
    pub fn signature_at(&self, at: usize, active: usize) -> Value {
        let Some(symbol) = self.selected(at).filter(|s| matches!(s.kind, 12 | 22)) else {
            return Value::Null;
        };
        if symbol.kind == 22 {
            let mut depth = 0usize;
            let mut start = None;
            let mut params = Vec::new();
            for token in self.tokens.iter().filter(|t| {
                t.span.start >= symbol.selection.end && t.span.end <= symbol.declaration.end
            }) {
                match token.kind {
                    TokenKind::LParen | TokenKind::LBracket | TokenKind::Lt => {
                        depth += 1;
                        if start.is_none() {
                            start = Some(token.span.end);
                        }
                    }
                    TokenKind::RParen | TokenKind::RBracket | TokenKind::Gt => {
                        depth = depth.saturating_sub(1);
                        if depth == 0
                            && let Some(start) = start.take()
                        {
                            let label = self.text[start..token.span.start].trim();
                            if !label.is_empty() {
                                params.push(json!({"label":label}));
                            }
                        }
                    }
                    TokenKind::Comma if depth == 1 => {
                        if let Some(start) = start.replace(token.span.end) {
                            params.push(json!({"label":self.text[start..token.span.start].trim()}));
                        }
                    }
                    _ => {}
                }
            }
            return json!({"signatures":[{"label":format!("{}.{}",symbol.owner.as_deref().unwrap_or(""),self.detail(symbol)),"documentation":{"kind":"markdown","value":self.documentation(symbol)},"parameters":params}],"activeSignature":0,"activeParameter":active.min(params.len().saturating_sub(1))});
        }
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
        let active = active.min(params.len().saturating_sub(1));
        json!({"signatures":[{"label":self.detail(symbol),"documentation":{"kind":"markdown","value":self.documentation(symbol)},"parameters":params}],"activeSignature":0,"activeParameter":active})
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
        if let Some((_, signature, documentation)) = self.builtin(at) {
            return json!({"contents":{"kind":"markdown","value":format!("```nc\n{signature}\n```\n\n{documentation}")}});
        }
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
                .map(|s| json!({"label":s.name,"kind":if s.kind == 22 {20} else {5},"detail":self.detail(s),"documentation":{"kind":"markdown","value":self.documentation(s)}}))
                .collect::<Vec<_>>();
            return json!({"isIncomplete":false,"items":items});
        }
        let mut items = std::collections::BTreeMap::new();
        let mut start = at;
        while start > 0
            && (self.text.as_bytes()[start - 1].is_ascii_alphanumeric()
                || self.text.as_bytes()[start - 1] == b'_')
        {
            start -= 1;
        }
        let builtin_prefix = start > 0 && self.text.as_bytes()[start - 1] == b'@';
        if builtin_prefix {
            start -= 1;
        }
        for (builtin, signature, documentation) in BUILTINS {
            items.insert((*builtin).into(), json!({"label":builtin,"kind":3,"detail":signature,"documentation":{"kind":"markdown","value":documentation},"textEdit":{"range":range(self.text,&(start..at)),"newText":builtin}}));
        }
        if builtin_prefix {
            return json!({"isIncomplete":false,"items":items.into_values().collect::<Vec<_>>()});
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

pub(super) fn valid_name(name: &str) -> bool {
    lexer::lex(name).is_ok_and(|tokens| {
        tokens.len() == 2
            && matches!(&tokens[0].kind, TokenKind::Ident(n) if n == name && n != "_")
            && !matches!(
                name,
                "bool" | "byte" | "int" | "uint" | "float" | "str" | "char" | "void"
            )
    })
}
fn range(text: &str, span: &std::ops::Range<usize>) -> Value {
    json!({"start":super::position(text, span.start),"end":super::position(text, span.end)})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imported_signatures_follow_the_innermost_call_and_ignore_nested_commas() {
        let source = "import { \"dep\" as dep }\nfn local(int n) int { return n }\n@println(dep.apply([1, 2], local(3), ";
        let index = Index::new(source);
        assert_eq!(
            index.imported_signature(source.len()),
            Some(("dep".into(), "apply".into(), 2))
        );
        assert!(
            index
                .imported_signature(source.find("local(3)").unwrap() + 7)
                .is_none()
        );
        assert_eq!(
            index.signature_help(source.find("local(3)").unwrap() + 7)["signatures"][0]["label"],
            "fn local(int n) int"
        );
    }

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

    #[test]
    fn incomplete_statements_do_not_disable_navigation_or_leak_locals() {
        let source = "fn incomplete(int n) int {\n int broken = +\n int valid = n\n @println(\n return valid\n}\nfn later(int input) int { return input }\n@println(later(2))\n";
        assert!(crate::check_source(source, std::path::Path::new("broken.nc")).is_err());
        let index = Index::new(source);
        assert_eq!(
            index
                .selected(source.find("= n").unwrap() + 2)
                .unwrap()
                .name,
            "n"
        );
        assert_eq!(
            index
                .selected(source.find("return valid").unwrap() + 7)
                .unwrap()
                .name,
            "valid"
        );
        assert_eq!(
            index
                .selected(source.rfind("later(").unwrap())
                .unwrap()
                .selection
                .start,
            source.find("later(").unwrap()
        );
        assert!(index.resolve("n", source.len()).is_none());
        assert!(
            index
                .resolve("broken", source.find("return valid").unwrap())
                .is_none()
        );
    }

    #[test]
    fn index_survives_typing_and_tracks_pattern_and_catch_bindings() {
        let source = "fn value(int n) int {\n if (n, 2) {\n (a, b) -> { return a + b }\n }\n}\nfn fail() int! { throw \"oops\" }\nint result = fail() catch message { @println(message) break 0 }\n";
        let index = Index::new(source);
        assert_eq!(
            index
                .selected(source.find("a +").unwrap())
                .unwrap()
                .selection
                .start,
            source.find("a, b").unwrap()
        );
        assert_eq!(
            index
                .selected(source.find("println(message").unwrap() + 8)
                .unwrap()
                .selection
                .start,
            source.find("message {").unwrap()
        );
        assert!(index.resolve("a", source.len()).is_none());
        assert!(index.resolve("message", source.len()).is_none());
        for (end, _) in source.char_indices() {
            let partial = Index::new(&source[..end]);
            partial.completion(end);
            partial.signature_help(end);
            partial.folding_ranges();
        }
        let incomplete = "fn f(int parameter) int {\n @println(";
        assert!(
            Index::new(incomplete)
                .resolve("parameter", incomplete.len())
                .is_some()
        );
        let unfinished_string = "fn f(int n) int { return n }\nstr s = \"unfinished";
        assert!(
            Index::new(unfinished_string)
                .resolve("f", unfinished_string.len())
                .is_some()
        );
    }

    #[test]
    fn builtin_completion_replaces_the_sigil_and_provides_documentation() {
        for source in ["@", "@pri", "fn f() { @pri"] {
            let index = Index::new(source);
            let completion = index.completion(source.len());
            let item = completion["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["label"] == "@println")
                .unwrap();
            assert_eq!(
                item["textEdit"]["range"]["start"],
                super::super::position(source, source.find('@').unwrap())
            );
            assert!(item["detail"].as_str().unwrap().starts_with("fn @println"));
        }
        let source = "@as(int, 2.5)";
        let index = Index::new(source);
        assert!(
            index.hover(0)["contents"]["value"]
                .as_str()
                .unwrap()
                .contains("Convert a value")
        );
        assert_eq!(index.signature_help(9)["activeParameter"], 1);
    }

    #[test]
    fn enum_variants_have_navigation_documentation_completion_and_signatures() {
        let source = "enum Node {\n /// Text contents\n Text(str)\n Pair((int, int), str)\n Empty\n}\nNode node = Node.Text(\"hello\")\nif node { Node.Text(text) -> { @println(text) } _ -> {} }\n_ = Node.Pair((1, 2), \"x\")\n";
        let index = Index::new(source);
        let declaration = source.find("Text(str)").unwrap();
        let use_at = source.find("Node.Text").unwrap() + 5;
        assert_eq!(
            index.definition("file:///a.nc", use_at)["range"]["start"],
            super::super::position(source, declaration)
        );
        assert!(
            index.hover(use_at)["contents"]["value"]
                .as_str()
                .unwrap()
                .contains("Text contents")
        );
        assert_eq!(
            index
                .references("file:///a.nc", use_at, true)
                .as_array()
                .unwrap()
                .len(),
            3
        );
        let completion = index.completion(use_at);
        assert_eq!(completion["items"].as_array().unwrap().len(), 3);
        assert!(
            completion["items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item["kind"] == 20)
        );
        let at = source.find(", \"x\"").unwrap() + 2;
        let signature = index.signature_help(at);
        assert_eq!(signature["activeParameter"], 1);
        assert_eq!(
            signature["signatures"][0]["parameters"],
            json!([{"label":"(int, int)"},{"label":"str"}])
        );
        assert_eq!(
            signature["signatures"][0]["label"],
            "Node.Pair((int, int), str)"
        );
        assert!(
            index.completion(source.len())["items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item["label"] != "Text")
        );
    }
}
