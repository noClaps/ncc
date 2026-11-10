//! The call around the cursor, shared by signature help and completion, and helpers for
//! function signature labels (`fn name<type T>(T a, (fn(T) int) f) (int, str)`).

use lsp_types::*;

use super::{BUILTINS, DeclKind, Server, docs, is_ident_byte, uri_to_path};

/// The byte range inside the parentheses of a signature label's parameter list: from the first
/// `(` to its matching `)` (a tuple return type has parentheses too).
pub fn params_range(label: &str) -> Option<(usize, usize)> {
    let lp = label.find('(')?;
    let mut depth = 0;
    for (i, c) in label[lp..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some((lp + 1, lp + i));
                }
            }
            _ => {}
        }
    }
    None
}

/// Split a comma-separated list at the top level (not inside brackets).
pub fn split_top(s: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' | '<' => depth += 1,
            ')' | ']' | '>' => depth -= 1,
            ',' if depth == 0 => {
                out.push((start, i));
                start = i + 1;
            }
            _ => {}
        }
    }
    if !s[start..].trim().is_empty() || !out.is_empty() {
        out.push((start, s.len()));
    }
    out.into_iter()
        .map(|(a, b)| {
            let t = &s[a..b];
            let a = a + (t.len() - t.trim_start().len());
            (a, a + t.trim().len())
        })
        .filter(|(a, b)| b > a)
        .collect()
}

/// The parameters of a signature label: (type, name) pairs.
pub fn params(label: &str) -> Vec<(String, String)> {
    let Some((l, r)) = params_range(label) else {
        return vec![];
    };
    split_top(&label[l..r])
        .into_iter()
        .map(|(a, b)| {
            let p = &label[l + a..l + b];
            match p.rsplit_once(' ') {
                Some((t, n)) => (t.trim().to_string(), n.to_string()),
                None => (p.to_string(), String::new()),
            }
        })
        .collect()
}

/// The type parameters of a generic signature label (`fn map<type I, type O>(` -> `I`, `O`).
pub fn type_params(label: &str) -> Vec<String> {
    let Some(lp) = label.find('(') else {
        return vec![];
    };
    let head = &label[..lp];
    let (Some(a), Some(b)) = (head.find('<'), head.rfind('>')) else {
        return vec![];
    };
    head[a + 1..b]
        .split(',')
        .map(|t| t.trim().trim_start_matches("type ").trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// Replace whole-word type names (`I` -> `int`) in a label or type.
pub fn substitute(s: &str, names: &[String], with: &[String]) -> String {
    let mut out = String::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < s.len() {
        if is_ident_byte(b[i]) {
            let start = i;
            while i < s.len() && is_ident_byte(b[i]) {
                i += 1;
            }
            let w = &s[start..i];
            match names.iter().position(|n| n == w).and_then(|k| with.get(k)) {
                Some(t) => out.push_str(t),
                None => out.push_str(w),
            }
        } else {
            let c = s[i..].chars().next().unwrap();
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

/// The call around a position.
pub struct CallCtx {
    /// the callee's signature, with explicit generic arguments substituted
    pub label: String,
    /// index of the argument the cursor is in
    pub arg: u32,
    pub documentation: Option<String>,
    pub param_docs: Vec<String>,
}

impl Server {
    /// The unclosed call `callee(` (or `callee<T>(`) around `off`.
    pub(super) fn call_at(&self, uri: &Uri, off: usize) -> Option<CallCtx> {
        let path = uri_to_path(uri)?;
        let text = self.text_of(&path)?;
        // find the unclosed `(` before the cursor and count the commas at its level, skipping
        // string and character literals and comments
        let b = text.as_bytes();
        let mut skip = vec![false; b.len()];
        let (toks, _) = crate::lexer::lex_all(&text, 0, 0);
        for t in &toks {
            if matches!(
                t.tok,
                crate::lexer::Tok::Str(_) | crate::lexer::Tok::Char(_)
            ) {
                let (s, e) = (t.span.start as usize, (t.span.end as usize).min(b.len()));
                skip[s..e].iter_mut().for_each(|x| *x = true);
            }
        }
        for (s, e) in crate::lexer::comments(&text) {
            skip[s..e.min(b.len())].iter_mut().for_each(|x| *x = true);
        }
        let (mut depth, mut commas, mut i) = (0i32, 0u32, off.min(b.len()));
        let open = loop {
            if i == 0 {
                return None;
            }
            i -= 1;
            if skip[i] {
                continue;
            }
            match b[i] {
                b')' | b']' | b'}' => depth += 1,
                b'(' | b'[' | b'{' if depth > 0 => depth -= 1,
                b'(' => break i,
                b'[' | b'{' => return None,
                b',' if depth == 0 => commas += 1,
                _ => {}
            }
        };
        let mut e = open;
        while e > 0 && b[e - 1] == b' ' {
            e -= 1;
        }
        // explicit generic arguments: `map<int, str>(`
        let mut generic_args: Vec<String> = Vec::new();
        if e > 0 && b[e - 1] == b'>' {
            let close = e - 1;
            let mut d = 0;
            while e > 0 {
                e -= 1;
                match b[e] {
                    b'>' => d += 1,
                    b'<' => {
                        d -= 1;
                        if d == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let inner = &text[e + 1..close];
            generic_args = split_top(inner)
                .into_iter()
                .map(|(a, b)| inner[a..b].to_string())
                .collect();
        }
        let mut s = e;
        while s > 0 && (is_ident_byte(b[s - 1]) || b[s - 1] == b'@' || b[s - 1] == b'.') {
            s -= 1;
        }
        let is_builtin = text[s..e].starts_with('@');
        let name = text[s..e].trim_start_matches('@');
        // a module or extern function: `math.sqrt(`, `c.read(`
        let (module, name) = match name.rsplit_once('.') {
            Some((m, n)) => (Some(m), n),
            None => (None, name),
        };
        let mut documentation = None;
        let mut param_docs: Vec<String> = Vec::new();
        let label = if is_builtin {
            let b = BUILTINS.iter().find(|(n, ..)| *n == name)?;
            documentation = Some(b.2.to_string());
            b.1.to_string()
        } else {
            let a = self.analyses.get(&path)?;
            let fid = a.file_id(&path);
            let target_file = module
                .and_then(|m| a.imports.get(m))
                .and_then(|mp| a.file_id(mp));
            let d = a
                .index
                .decls
                .iter()
                .filter(|d| d.name == name && d.kind == DeclKind::Function)
                .filter(|d| match (module, &d.extern_alias) {
                    (Some(m), Some(x)) => m == x,
                    (Some(_), None) => target_file == Some(d.span.file),
                    (None, Some(_)) => false,
                    (None, None) => true,
                })
                .filter(|d| {
                    d.scope.is_none_or(|sc| {
                        Some(sc.file) != fid || (sc.start as usize <= off && off <= sc.end as usize)
                    })
                })
                .max_by_key(|d| (Some(d.span.file) == fid, d.span.start as usize <= off))?;
            let ftext = &a.sm.file(d.span.file).text;
            if let Some(doc) = docs::doc_comment(ftext, d.span) {
                param_docs = docs::param_docs(&doc);
                documentation = Some(doc);
            }
            d.detail.clone()
        };
        // `fn map<type I, type O>(I[] arr, ...)` called as `map<int, str>(` shows
        // `fn map<int, str>(int[] arr, ...)`
        let tps = type_params(&label);
        let label = if !tps.is_empty() && tps.len() == generic_args.len() {
            let lp = label.find('<').unwrap_or(0);
            let head = &label[..lp];
            let rest = &label[label.find('(').unwrap_or(label.len())..];
            format!(
                "{head}<{}>{}",
                generic_args.join(", "),
                substitute(rest, &tps, &generic_args)
            )
        } else {
            label
        };
        Some(CallCtx {
            label,
            arg: commas,
            documentation,
            param_docs,
        })
    }

    pub(super) fn signature_help(&self, p: SignatureHelpParams) -> Option<SignatureHelp> {
        let pp = p.text_document_position_params;
        let path = uri_to_path(&pp.text_document.uri)?;
        let text = self.text_of(&path)?;
        let off = super::pos_to_offset(&text, pp.position);
        let c = self.call_at(&pp.text_document.uri, off)?;
        let (l, r) = params_range(&c.label)?;
        let label = &c.label;
        let parameters = split_top(&label[l..r])
            .into_iter()
            .enumerate()
            .map(|(i, (s, e))| ParameterInformation {
                label: ParameterLabel::LabelOffsets([
                    label[..l + s].encode_utf16().count() as u32,
                    label[..l + e].encode_utf16().count() as u32,
                ]),
                documentation: c.param_docs.get(i).map(|d| {
                    Documentation::MarkupContent(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: d.clone(),
                    })
                }),
            })
            .collect();
        Some(SignatureHelp {
            signatures: vec![SignatureInformation {
                label: c.label.clone(),
                documentation: c.documentation.map(|d| {
                    Documentation::MarkupContent(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: d,
                    })
                }),
                parameters: Some(parameters),
                active_parameter: Some(c.arg),
            }],
            active_signature: Some(0),
            active_parameter: Some(c.arg),
        })
    }

    /// A function literal for an argument whose parameter is a function: in
    /// `map<int, str>(xs, |`, `fn(int i) str { ... }`.
    pub(super) fn fn_literal_items(&self, uri: &Uri, off: usize) -> Vec<CompletionItem> {
        let Some(c) = self.call_at(uri, off) else {
            return vec![];
        };
        let ps = params(&c.label);
        let Some((ty, _)) = ps.get(c.arg as usize) else {
            return vec![];
        };
        let Some(inner) = ty.strip_prefix("(fn").and_then(|t| t.strip_suffix(')')) else {
            return vec![];
        };
        // `(int, str) bool` -> parameters and return type
        let Some((l, r)) = params_range(inner) else {
            return vec![];
        };
        let ret = inner[r + 1..].trim();
        let names = ["a", "b", "c", "d", "e", "f", "g", "h"];
        let args: Vec<(String, String)> = split_top(&inner[l..r])
            .into_iter()
            .enumerate()
            .map(|(i, (a, b))| {
                let t = inner[l + a..l + b].to_string();
                let n = names
                    .get(i)
                    .map(|n| n.to_string())
                    .unwrap_or(format!("a{i}"));
                (t, n)
            })
            .collect();
        let plain: Vec<String> = args.iter().map(|(t, n)| format!("{t} {n}")).collect();
        let snip: Vec<String> = args
            .iter()
            .enumerate()
            .map(|(i, (t, n))| format!("{t} ${{{}:{n}}}", i + 1))
            .collect();
        let ret_part = if ret.is_empty() {
            String::new()
        } else {
            format!(" {ret}")
        };
        let label = format!("fn({}){ret_part} {{}}", plain.join(", "));
        vec![CompletionItem {
            label: label.clone(),
            kind: Some(CompletionItemKind::SNIPPET),
            detail: Some(format!("function literal for `{ty}`")),
            filter_text: Some("fn".into()),
            sort_text: Some("0fn".into()),
            insert_text: Some(format!("fn({}){ret_part} {{\n\t$0\n}}", snip.join(", "))),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            preselect: Some(true),
            ..Default::default()
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_labels() {
        let l = "fn pair(int a, (fn(int) str) f) (int, str)";
        assert_eq!(
            params_range(l).map(|(a, b)| &l[a..b]),
            Some("int a, (fn(int) str) f")
        );
        assert_eq!(
            params(l),
            [
                ("int".to_string(), "a".to_string()),
                ("(fn(int) str)".to_string(), "f".to_string())
            ]
        );
        let g = "fn map<type I, type O>(I[] arr, (fn(I) O) f) O[]";
        assert_eq!(type_params(g), ["I", "O"]);
        assert_eq!(
            substitute(
                "(I[] arr, (fn(I) O) f) O[]",
                &type_params(g),
                &["int".into(), "str".into()]
            ),
            "(int[] arr, (fn(int) str) f) str[]"
        );
        assert_eq!(params("fn f()"), vec![]);
    }
}
