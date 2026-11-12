//! Completion: scope-aware names, members of any typed expression (`a.b[0].c.`), struct literal
//! fields, enum variants, module members, builtins, import and `@embed` paths, and snippets.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use lsp_types::*;

use super::tyexpr;
use crate::ast::TypeExpr as T;
use crate::types::TyId;

use super::{Analysis, BUILTINS, KEYWORDS, Server, docs, pos_to_offset};
use crate::ast;
use crate::check::index::{DeclKind, SymDecl};
use crate::diag::Span;
use crate::lexer::{Tok, Token, is_ident_byte};

/// Keyword snippets: label, snippet body, description.
const SNIPPETS: &[(&str, &str, &str)] = &[
    ("fn", "fn ${1:name}($2) {\n\t$0\n}", "function"),
    (
        "if",
        "if ${1:value} {\n\t${2:pattern} -> { $0 }\n\t_ -> {}\n}",
        "match a value",
    ),
    (
        "if",
        "if {\n\t${1:condition} -> { $0 }\n\t_ -> {}\n}",
        "conditions",
    ),
    ("while", "while ${1:condition} {\n\t$0\n}", "loop"),
    (
        "for",
        "for ${1:i} in ${2:items} {\n\t$0\n}",
        "loop over a collection",
    ),
    ("struct", "struct ${1:Name} {\n\t$0\n}", "struct"),
    ("enum", "enum ${1:Name} {\n\t$0\n}", "enum"),
    ("test", "test \"${1:name}\" {\n\t$0\n}", "test"),
    (
        "import",
        "import { \"${1:path}\" as ${2:alias} }",
        "import a module",
    ),
    ("lock", "lock ${1:name} {\n\t$0\n}", "lock a mutex"),
];

/// A step of a receiver expression before `.`.
#[derive(Debug, PartialEq)]
enum Seg {
    Name(String),
    Call,
    Index,
    /// `[a:b]`: the same kind of sequence
    Slice,
}

/// The offset of the opening quote when `line` (a line up to the cursor) ends in a string's
/// literal text; `None` when it ends in code, including an interpolation `{...}`.
fn open_string(line: &str) -> Option<usize> {
    let b = line.as_bytes();
    // open strings, innermost last: the quote's offset, and the brace depth inside the
    // interpolation the scan is in (`None` in the string's literal text)
    let mut open: Vec<(usize, Option<usize>)> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match (open.last_mut(), b[i]) {
            (Some((_, None)), b'\\') => i += 1,
            (Some((_, None)), b'"') => _ = open.pop(),
            (Some((_, d @ None)), b'{') => *d = Some(0),
            (Some((_, None)), _) => {}
            (Some((_, Some(d))), b'{') => *d += 1,
            (Some((_, d @ Some(0))), b'}') => *d = None,
            (Some((_, Some(d))), b'}') => *d -= 1,
            // code: a comment, a char literal or a string
            (_, b'/') if b.get(i + 1) == Some(&b'/') => break,
            (_, b'\'') => {
                i += 1;
                while i < b.len() && b[i] != b'\'' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
            }
            (_, b'"') => open.push((i, None)),
            _ => {}
        }
        i += 1;
    }
    match open.last() {
        Some(&(q, None)) => Some(q),
        _ => None,
    }
}

/// Parse the receiver chain at the end of `head` (`a.b(x)[0].c`), outermost last, with the
/// offset where the receiver starts.
fn receiver_chain(head: &str) -> Option<(Vec<Seg>, usize)> {
    let b = head.as_bytes();
    let mut i = head.trim_end().len();
    let mut segs = Vec::new();
    loop {
        // calls and indexes
        while i > 0 && (b[i - 1] == b')' || b[i - 1] == b']') {
            let close = b[i - 1];
            let open = if close == b')' { b'(' } else { b'[' };
            let mut depth = 0;
            let mut j = i;
            loop {
                if j == 0 {
                    return None;
                }
                j -= 1;
                if b[j] == close {
                    depth += 1;
                } else if b[j] == open {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
            }
            segs.push(if close == b')' {
                Seg::Call
            } else if is_slice(&b[j + 1..i - 1]) {
                Seg::Slice
            } else {
                Seg::Index
            });
            i = j;
        }
        let end = i;
        while i > 0 && is_ident_byte(b[i - 1]) {
            i -= 1;
        }
        if i == end {
            return None;
        }
        segs.push(Seg::Name(head[i..end].to_string()));
        if i > 0 && b[i - 1] == b'.' {
            i -= 1;
        } else {
            break;
        }
    }
    segs.reverse();
    Some((segs, i))
}

/// Postfix snippets: label, template (`{r}` is the receiver), description.
const POSTFIX: &[(&str, &str, &str)] = &[
    ("if", "if {r} {\n\t$0\n}", "match on the value"),
    ("not", "not {r}", "negate"),
    ("return", "return {r}", "return the value"),
    ("try", "try {r}", "propagate the error"),
    ("await", "await {r}", "wait for the future"),
    ("println", "@println({r})", "print the value"),
    ("discard", "_ = {r}", "discard the value"),
];

/// Declarations visible at `off` in file `fid`, innermost (latest) first, one per name.
pub(super) fn visible(a: &Analysis, fid: u32, off: usize) -> Vec<&SymDecl> {
    let mut ds: Vec<&SymDecl> = a
        .index
        .decls
        .iter()
        .filter(|d| d.span.file == fid && !matches!(d.kind, DeclKind::Field | DeclKind::Variant))
        // extern functions are only reachable through their block's alias
        .filter(|d| d.extern_alias.is_none())
        .filter(|d| {
            d.scope
                .is_none_or(|s| s.start as usize <= off && off <= s.end as usize)
        })
        .filter(|d| {
            matches!(
                d.kind,
                DeclKind::Function | DeclKind::Struct | DeclKind::Enum | DeclKind::Type
            ) && d.scope.is_none()
                || (d.span.start as usize) < off
        })
        .collect();
    ds.sort_by_key(|d| std::cmp::Reverse(d.span.start));
    let mut seen = HashSet::new();
    ds.retain(|d| seen.insert(d.name.clone()));
    ds
}

/// Names visible at `off` (for suggestions).
pub(super) fn visible_names(a: &Analysis, fid: u32, off: usize) -> Vec<String> {
    visible(a, fid, off)
        .into_iter()
        .map(|d| d.name.clone())
        .collect()
}

impl Server {
    /// A completion for a declaration; with a qualifier (`alias`), the item inserts
    /// `alias.name` and is found by typing the name.
    fn decl_item(
        &self,
        a: &Analysis,
        d: &SymDecl,
        ins: Insert<'_>,
        qualifier: Option<&str>,
    ) -> CompletionItem {
        let full = match qualifier {
            Some(q) => format!("{q}.{}", d.name),
            None => d.name.clone(),
        };
        let kind = match d.kind {
            DeclKind::Function => CompletionItemKind::FUNCTION,
            DeclKind::Struct => CompletionItemKind::STRUCT,
            DeclKind::Enum => CompletionItemKind::ENUM,
            DeclKind::Type => CompletionItemKind::TYPE_PARAMETER,
            DeclKind::Field => CompletionItemKind::FIELD,
            DeclKind::Variant => CompletionItemKind::ENUM_MEMBER,
            DeclKind::Variable if d.mutable => CompletionItemKind::VARIABLE,
            DeclKind::Variable => CompletionItemKind::CONSTANT,
        };
        let mut item = CompletionItem {
            label: full.clone(),
            filter_text: qualifier.map(|_| d.name.clone()),
            insert_text: qualifier.map(|_| full.clone()),
            kind: Some(kind),
            detail: Some(d.detail.clone()),
            documentation: doc_at(a, d.span),
            sort_text: Some(format!(
                "{}{}",
                match (d.kind, d.scope.is_some(), qualifier.is_some()) {
                    (_, _, true) => 6,
                    (DeclKind::Variable, true, _) => 0,
                    (DeclKind::Variable, false, _) | (DeclKind::Function, _, _) => 1,
                    _ => 2,
                },
                d.name
            )),
            ..Default::default()
        };
        // a struct literal with a placeholder for each field: `P{.x = $1, .y = $2}`
        if d.kind == DeclKind::Struct
            && let Some(expected) = ins.literal
            && self.snippets
        {
            let names: Vec<String> = struct_fields(a, d).into_iter().map(|f| f.0).collect();
            // its type arguments come from the expected type (`Box<int> b = B`) or are
            // placeholders: `Box<${1:T}>{.value = $2}`
            let tps = &d.type_params;
            let mut slot = tabstops();
            let targs = match expected {
                Some(T::Named {
                    module, name, args, ..
                }) if !args.is_empty()
                    && !tps.is_empty()
                    && full
                        == format!(
                            "{}{name}",
                            module.iter().map(|m| format!("{m}.")).collect::<String>()
                        ) =>
                {
                    generic_args(&args.iter().map(T::show).collect::<Vec<_>>())
                }
                _ => generic_args(&tps.iter().map(|t| slot(Some(t))).collect::<Vec<_>>()),
            };
            let fields: Vec<String> = names
                .iter()
                .map(|n| format!(".{n} = {}", slot(None)))
                .collect();
            if !fields.is_empty() {
                item.insert_text = Some(format!("{full}{targs}{{{}}}$0", fields.join(", ")));
                item.insert_text_format = Some(InsertTextFormat::SNIPPET);
            }
        }
        if d.kind == DeclKind::Function && ins.call_parens && self.snippets {
            let ps: Vec<&str> = (d.sig.iter().flat_map(|s| &s.params))
                .map(|(_, n)| n.as_str())
                .collect();
            // a generic function takes its type arguments first: `map<${1:I}, ${2:O}>(...)`
            let mut slot = tabstops();
            let targs: Vec<String> = d.type_params.iter().map(|t| slot(Some(t))).collect();
            let args: Vec<String> = ps.iter().map(|p| slot(Some(p))).collect();
            item.insert_text = Some(format!(
                "{full}{}({})$0",
                generic_args(&targs),
                args.join(", ")
            ));
            item.insert_text_format = Some(InsertTextFormat::SNIPPET);
            item.command = (!ps.is_empty()).then(param_hints);
        }
        item
    }

    pub(super) fn completion(&self, p: CompletionParams) -> Option<Vec<CompletionItem>> {
        let pp = p.text_document_position;
        let (path, text) = self.source(&pp.text_document.uri)?;
        let off = pos_to_offset(&text, pp.position);
        let line_start = super::line_start(&text, off);
        let before = &text[line_start..off];
        let analysis = self.analyses.get(&path);
        let fid = analysis.and_then(|a| a.file_id(&path));
        let slot = self.slot_at(&pp.text_document.uri, &text, analysis, fid, off);
        let after = &text[off..];
        // no `()` where a function value is expected (`apply(f|`) or already follows
        let call_parens =
            !after.starts_with('(') && !matches!(&slot, Slot::Value(Some(t)) if tyexpr::is_fn(t));
        let ins = Insert {
            call_parens,
            literal: match &slot {
                Slot::Value(t) if !after.starts_with('{') => Some(t.as_ref()),
                _ => None,
            },
        };
        let item = |label: &str, kind: CompletionItemKind, detail: Option<String>| CompletionItem {
            label: label.to_string(),
            kind: Some(kind),
            detail,
            ..Default::default()
        };
        let mut items = Vec::new();

        // paths inside `import { "..." }` and `@embed("...")`
        if let Some(q) = open_string(before) {
            let typed = &before[q + 1..];
            let pre = &text[..line_start + q];
            let in_embed = pre.trim_end().ends_with("@embed(");
            let in_import = match (pre.rfind("import"), pre.rfind('}')) {
                (Some(i), Some(c)) => i > c,
                (Some(_), None) => true,
                _ => false,
            };
            let in_path = in_embed || in_import;
            return Some(if in_path {
                path_items(&path, typed, in_import)
            } else {
                vec![]
            });
        }

        let word_start = before
            .char_indices()
            .rev()
            .find(|(_, c)| !(c.is_alphanumeric() || *c == '_'))
            .map_or(0, |(i, c)| i + c.len_utf8());
        let prefix_char = before[..word_start].chars().last();

        // `@builtin`
        if prefix_char == Some('@') {
            return Some(self.builtin_items(false, call_parens));
        }

        // after `.`
        if prefix_char == Some('.') {
            let Some(a) = analysis else {
                return Some(vec![]);
            };
            let dot = line_start + word_start - 1;
            if let Some(fields) = struct_literal_fields(a, &text, dot) {
                return Some(fields);
            }
            let head = &text[..dot];
            let Some((chain, start)) = receiver_chain(head) else {
                return Some(vec![]);
            };
            let fid = fid?;
            // postfix snippets for values (not modules or types)
            let is_value = match &chain[0] {
                Seg::Name(n) => {
                    !a.imports.contains_key(n)
                        && visible(a, fid, dot).iter().any(|d| {
                            &d.name == n
                                && matches!(d.kind, DeclKind::Variable | DeclKind::Function)
                        })
                }
                _ => false,
            };
            let postfix = if is_value && self.snippets {
                postfix_items(&text, start, dot, off)
            } else {
                vec![]
            };
            if let Some((mut items, ty)) = self.member_items(a, fid, dot, &chain, ins) {
                // functions taking the value as their first argument: `v.f` -> `f(v, ...)`
                if let Some(ty) = ty {
                    items.extend(self.first_arg_items(a, fid, &text, start, dot, off, &ty));
                }
                items.extend(postfix);
                return Some(items);
            }
            if !postfix.is_empty() {
                return Some(postfix);
            }
            // `name.` where `name` is a module that is not imported: its public items, adding
            // the import
            if let [Seg::Name(name)] = chain.as_slice()
                && !a.imports.contains_key(name)
                && let Some(ipath) = self.find_modules(&path, name).into_iter().next()
            {
                let dir = path.parent()?;
                let file =
                    crate::modules::import_file(dir, &ipath, || crate::modules::find_root(dir))?;
                let module = self.module_of(&path, &text);
                let edit = super::actions::add_import(&text, &module, &ipath, name);
                let ftext = self.text_of(&file)?;
                let mut names: Vec<String> = public_names(&self.module_of(&file, &ftext))
                    .into_iter()
                    .collect();
                names.sort();
                let item = |label| CompletionItem {
                    label,
                    detail: Some(format!("import \"{ipath}\"")),
                    additional_text_edits: Some(vec![edit.clone()]),
                    ..Default::default()
                };
                return Some(names.into_iter().map(item).collect());
            }
            return Some(vec![]);
        }

        // names, keywords and types
        if let (Some(a), Some(fid)) = (analysis, fid) {
            for d in visible(a, fid, off) {
                items.push(self.decl_item(a, d, ins, None));
            }
            for alias in a.imports.keys() {
                items.push(item(
                    alias,
                    CompletionItemKind::MODULE,
                    Some("module".into()),
                ));
            }
            // extern blocks
            let module = self.module_of(&path, &text);
            let externs: Vec<(&str, &str)> = module
                .items
                .iter()
                .filter_map(|it| match it {
                    ast::Item::Extern(e) => Some((e.alias.as_str(), e.path.as_str())),
                    _ => None,
                })
                .collect();
            for (alias, file) in &externs {
                items.push(item(
                    alias,
                    CompletionItemKind::MODULE,
                    Some(format!("extern \"{file}\"")),
                ));
            }
            // items reached through a qualifier, found by their own name: `c_` offers
            // `libc.c_exit`, `Ratio` offers `rational.Rational`
            for d in &a.index.decls {
                if d.span.file == fid
                    && d.scope.is_none()
                    && let Some(q) = &d.extern_alias
                {
                    items.push(self.decl_item(a, d, ins, Some(q)));
                }
            }
            for (alias, mpath) in &a.imports {
                for d in public_decls(a, mpath) {
                    items.push(self.decl_item(a, d, ins, Some(alias)));
                }
            }
            // modules that are not imported yet: accepting one adds the import
            let taken: HashSet<String> = a
                .imports
                .keys()
                .cloned()
                .chain(externs.iter().map(|(n, _)| n.to_string()))
                .chain(visible(a, fid, off).into_iter().map(|d| d.name.clone()))
                .collect();
            let mut offered = HashSet::new();
            for (name, ipath) in self.importable_modules(&path).iter() {
                if taken.contains(name) || !offered.insert(name.clone()) {
                    continue;
                }
                let edit = super::actions::add_import(&text, &module, ipath, name);
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::MODULE),
                    detail: Some(format!("import \"{ipath}\"")),
                    sort_text: Some(format!("5{name}")),
                    additional_text_edits: Some(vec![edit]),
                    ..Default::default()
                });
            }
        }
        // builtins, found without typing `@`
        items.extend(self.builtin_items(true, call_parens));
        // a function literal for an argument that takes a function
        if self.snippets {
            items.extend(self.fn_literal_items(&pp.text_document.uri, off));
        }
        for k in KEYWORDS {
            let kind = match *k {
                "true" | "false" | "none" => CompletionItemKind::CONSTANT,
                _ => CompletionItemKind::KEYWORD,
            };
            let mut it = item(k, kind, None);
            it.sort_text = Some(format!("3{k}"));
            items.push(it);
        }
        for (t, _) in crate::types::BUILTIN_TYPES {
            let mut it = item(t, CompletionItemKind::KEYWORD, Some("builtin type".into()));
            it.sort_text = Some(format!("2{t}"));
            items.push(it);
        }
        // keyword snippets only at the start of a statement
        if self.snippets && before[..word_start].trim().is_empty() {
            for (label, body, desc) in SNIPPETS {
                items.push(CompletionItem {
                    label: label.to_string(),
                    kind: Some(CompletionItemKind::SNIPPET),
                    detail: Some(desc.to_string()),
                    insert_text: Some(body.to_string()),
                    insert_text_format: Some(InsertTextFormat::SNIPPET),
                    sort_text: Some(format!("4{label}")),
                    ..Default::default()
                });
            }
        }
        Some(items)
    }

    /// What the expression being typed at `off` is: a value (with the type it is expected to
    /// have, when known) or something else (a statement, a type, a field name, ...). Values are
    /// call arguments (`apply(f|`), declarations' and assignments' values (`(fn() int) g = f|`,
    /// `p.cb = f|`), struct literal fields (`S{.cb = f|`), returned values (`return f|`) and
    /// array elements.
    fn slot_at(
        &self,
        uri: &Uri,
        text: &str,
        a: Option<&Analysis>,
        fid: Option<u32>,
        off: usize,
    ) -> Slot {
        let b = text.as_bytes();
        // the start of the expression: the word, a qualifier (`util.f`) and a builtin's `@`
        let mut s = off;
        while s > 0 && is_ident_byte(b[s - 1]) {
            s -= 1;
        }
        while s > 1 && b[s - 1] == b'.' && is_ident_byte(b[s - 2]) {
            s -= 1;
            while s > 0 && is_ident_byte(b[s - 1]) {
                s -= 1;
            }
        }
        if s > 0 && b[s - 1] == b'@' {
            s -= 1;
        }
        let (toks, _) = crate::lexer::lex_all(text, 0, 0);
        let toks: Vec<&Token> = toks
            .iter()
            .take_while(|t| (t.span.end as usize) <= s)
            .collect();
        match toks.last().map(|t| &t.tok) {
            // a call's argument, not a parameter of a declaration (`fn f(int a, |`)
            Some(Tok::LParen | Tok::Comma | Tok::LBracket) => {
                let Some((open, commas)) = unclosed(&toks) else {
                    return Slot::Other;
                };
                let open_off = toks[open].span.start as usize;
                if toks[open].tok == Tok::LBracket {
                    return match open.checked_sub(1).map(|i| &toks[i].tok) {
                        // a map type: `[str]int m`
                        None | Some(Tok::Newline | Tok::LBrace | Tok::RBrace) => Slot::Other,
                        // an index or key: `xs[`
                        Some(Tok::Ident(k)) if k != "return" => Slot::Value(None),
                        Some(Tok::RParen | Tok::RBracket) => Slot::Value(None),
                        // an array element: of the array's element type
                        _ => match self.slot_at(uri, text, a, fid, open_off) {
                            Slot::Value(t) => {
                                Slot::Value(t.as_ref().and_then(tyexpr::elem).cloned())
                            }
                            Slot::Other => Slot::Other,
                        },
                    };
                }
                if toks[open].tok != Tok::LParen || in_fn_header(&toks) {
                    return Slot::Other;
                }
                if let Some(c) = self.call_at(uri, s) {
                    return Slot::Value(arg_ty(a.zip(fid), text, &toks, open, &c));
                }
                // a tuple's element or a parenthesized value: where the `(` is
                match self.slot_at(uri, text, a, fid, open_off) {
                    Slot::Value(Some(T::Tuple(elems, _))) => {
                        Slot::Value(elems.get(commas).cloned())
                    }
                    Slot::Value(t) if commas == 0 => Slot::Value(t),
                    Slot::Value(_) => Slot::Value(None),
                    Slot::Other => Slot::Other,
                }
            }
            // a declaration's, assignment's or struct literal field's value
            Some(Tok::Assign) => {
                let eq = toks.len() - 1;
                let mut depth = 0i32;
                let mut i = eq;
                while i > 0 {
                    match toks[i - 1].tok {
                        Tok::RParen | Tok::RBracket => depth += 1,
                        Tok::LParen | Tok::LBracket if depth > 0 => depth -= 1,
                        Tok::Newline
                        | Tok::LBrace
                        | Tok::RBrace
                        | Tok::LParen
                        | Tok::LBracket
                        | Tok::Comma
                            if depth == 0 =>
                        {
                            break;
                        }
                        _ => {}
                    }
                    i -= 1;
                }
                if i == eq {
                    return Slot::Other;
                }
                let lstart = toks[i].span.start as usize;
                let lhs = text[lstart..toks[eq].span.start as usize].trim();
                let lhs = ["pub ", "mut "]
                    .iter()
                    .fold(lhs, |l, p| l.strip_prefix(p).unwrap_or(l).trim_start());
                // `Type name = `
                if let Some((ty, name)) = lhs.rsplit_once(' ')
                    && name.bytes().all(is_ident_byte)
                {
                    return Slot::Value(crate::parser::parse_type_text(ty));
                }
                let (Some(a), Some(fid)) = (a, fid) else {
                    return Slot::Value(None);
                };
                // `S{.field = `
                if let Some(field) = lhs.strip_prefix('.') {
                    let ty = literal_struct(a, text, lstart).and_then(|(d, _)| {
                        let fields = struct_fields(a, d);
                        fields.into_iter().find(|(n, ..)| n == field)?.1
                    });
                    return Slot::Value(ty);
                }
                // `name = `, `p.field = `, `ps[0].field = `
                Slot::Value(
                    receiver_chain(lhs)
                        .filter(|(_, start)| *start == 0)
                        .and_then(|(chain, _)| chain_ty(a, fid, s, &chain))
                        .map(|t| a.types.to_type_expr(t)),
                )
            }
            // the enclosing function's return value
            Some(Tok::Ident(k)) if k == "return" => Slot::Value(enclosing_fn_return(text, &toks)),
            _ => Slot::Other,
        }
    }

    /// Members after a receiver chain: module members, enum variants or the members of the
    /// receiver's type.
    fn member_items(
        &self,
        a: &Analysis,
        fid: u32,
        off: usize,
        chain: &[Seg],
        ins: Insert<'_>,
    ) -> Option<(Vec<CompletionItem>, Option<T>)> {
        let Seg::Name(first) = &chain[0] else {
            return None;
        };
        let mut rest = &chain[1..];
        let ty: TyId;
        // the functions of an extern block
        let externs: Vec<&SymDecl> = a
            .index
            .decls
            .iter()
            .filter(|d| d.span.file == fid && d.extern_alias.as_ref() == Some(first))
            .collect();
        if !externs.is_empty() {
            let Some(Seg::Name(n)) = rest.first() else {
                return Some((
                    externs
                        .into_iter()
                        .map(|d| self.decl_item(a, d, ins, None))
                        .collect(),
                    None,
                ));
            };
            let d = externs.into_iter().find(|d| &d.name == n)?;
            if rest.get(1) != Some(&Seg::Call) {
                return None;
            }
            ty = d.ty?;
            rest = &rest[2..];
        } else if let Some(mpath) = a.imports.get(first) {
            let mfid = a.file_id(mpath)?;
            if rest.is_empty() {
                let items = public_decls(a, mpath)
                    .into_iter()
                    .map(|d| self.decl_item(a, d, ins, None))
                    .collect();
                return Some((items, None));
            }
            let Seg::Name(n) = &rest[0] else { return None };
            let d = a
                .index
                .decls
                .iter()
                .find(|d| d.span.file == mfid && d.scope.is_none() && &d.name == n)?;
            if matches!(d.kind, DeclKind::Enum) && rest.len() == 1 {
                return Some((self.variant_items(a, d), None));
            }
            ty = d.ty?;
            rest = &rest[1..];
            if d.kind == DeclKind::Function {
                // the decl's type is its return type: skip the call
                if rest.first() == Some(&Seg::Call) {
                    rest = &rest[1..];
                } else {
                    return None;
                }
            }
        } else {
            let d = visible(a, fid, off)
                .into_iter()
                .find(|d| &d.name == first)?;
            match d.kind {
                DeclKind::Enum if rest.is_empty() => {
                    return Some((self.variant_items(a, d), None));
                }
                DeclKind::Function => {
                    if rest.first() != Some(&Seg::Call) {
                        return None;
                    }
                    rest = &rest[1..];
                }
                _ => {}
            }
            ty = d.ty?;
        }
        let ty = follow(a, ty, rest)?;
        let fields = (a.members(ty).into_iter())
            .map(|m| CompletionItem {
                label: m.name.clone(),
                kind: Some(CompletionItemKind::FIELD),
                sort_text: Some(format!("0{}", m.name)),
                detail: Some(m.detail.clone()),
                documentation: m.def.and_then(|def| doc_at(a, def)),
                ..Default::default()
            })
            .collect();
        Some((fields, Some(a.types.to_type_expr(ty))))
    }

    /// Functions whose first parameter accepts a value of type `ty`, offered after `receiver.`
    /// and inserted as a call with the receiver as the first argument: `v.add` -> `add(v, ...)`.
    /// Covers functions in scope, public functions of imported modules and extern functions;
    /// a generic function's type arguments are inferred from the receiver where possible.
    #[allow(clippy::too_many_arguments)]
    fn first_arg_items(
        &self,
        a: &Analysis,
        fid: u32,
        text: &str,
        start: usize,
        dot: usize,
        off: usize,
        ty: &T,
    ) -> Vec<CompletionItem> {
        let receiver = &text[start..dot];
        if receiver.contains('\n') {
            return vec![];
        }
        // (declaration, qualifier)
        let mut cands: Vec<(&SymDecl, Option<String>)> = visible(a, fid, dot)
            .into_iter()
            .filter(|d| d.kind == DeclKind::Function)
            .map(|d| (d, None))
            .collect();
        for d in &a.index.decls {
            if d.span.file == fid
                && let Some(alias) = &d.extern_alias
            {
                cands.push((d, Some(alias.clone())));
            }
        }
        let mut aliases: Vec<(&String, &std::path::PathBuf)> = a.imports.iter().collect();
        aliases.sort();
        for (alias, mpath) in aliases {
            for d in public_decls(a, mpath) {
                if d.kind == DeclKind::Function {
                    cands.push((d, Some(alias.clone())));
                }
            }
        }
        let escaped = snippet_escape(receiver);
        let range = super::range(text, start, off);
        let mut out = Vec::new();
        for (d, qual) in cands {
            let Some(ps) = d.sig.as_ref().map(|s| &s.params) else {
                continue;
            };
            let Some((Some(first), _)) = ps.first() else {
                continue;
            };
            let tps = &d.type_params;
            let Some(binds) = tyexpr::accepts(first, ty, tps) else {
                continue;
            };
            let full = match &qual {
                Some(q) => format!("{q}.{}", d.name),
                None => d.name.clone(),
            };
            let mut slot = tabstops();
            let targs: Vec<String> = tps
                .iter()
                .map(|t| binds.get(t).map_or_else(|| slot(Some(t)), T::show))
                .collect();
            let mut args = vec![escaped.clone()];
            args.extend(ps[1..].iter().map(|(_, n)| slot(Some(n))));
            let (new_text, format) = if self.snippets {
                (
                    format!("{full}{}({})$0", generic_args(&targs), args.join(", ")),
                    InsertTextFormat::SNIPPET,
                )
            } else {
                let plain: Vec<String> = tps
                    .iter()
                    .map(|t| binds.get(t).map_or_else(|| t.clone(), T::show))
                    .collect();
                (
                    format!("{full}{}({receiver})", generic_args(&plain)),
                    InsertTextFormat::PLAIN_TEXT,
                )
            };
            out.push(CompletionItem {
                label: full,
                kind: Some(CompletionItemKind::FUNCTION),
                detail: Some(d.detail.clone()),
                documentation: doc_at(a, d.span),
                filter_text: Some(format!("{receiver}.{}", d.name)),
                sort_text: Some(format!("5{}", d.name)),
                insert_text_format: Some(format),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(range, new_text))),
                command: (ps.len() > 1 && self.snippets).then(param_hints),
                ..Default::default()
            });
        }
        out
    }

    /// The builtins, as completions after a typed `@`, or inserting the `@` when `at`.
    fn builtin_items(&self, at: bool, call_parens: bool) -> Vec<CompletionItem> {
        let snippet = self.snippets && call_parens;
        let prefix = if at { "@" } else { "" };
        (BUILTINS.iter())
            .map(|b| {
                let label = format!("{prefix}{}", b.name);
                let has_args = !b.params.is_empty();
                CompletionItem {
                    kind: Some(CompletionItemKind::FUNCTION),
                    detail: Some(b.label()),
                    documentation: Some(Documentation::String(b.doc.to_string())),
                    filter_text: at.then(|| b.name.to_string()),
                    insert_text: if snippet {
                        Some(format!("{label}{}", call_snippet(has_args)))
                    } else {
                        at.then(|| label.clone())
                    },
                    insert_text_format: snippet.then_some(InsertTextFormat::SNIPPET),
                    command: (snippet && has_args).then(param_hints),
                    sort_text: at.then(|| format!("7{}", b.name)),
                    label,
                    ..Default::default()
                }
            })
            .collect()
    }

    fn variant_items(&self, a: &Analysis, d: &SymDecl) -> Vec<CompletionItem> {
        let spans = d.ty.and_then(|t| a.index.variant_spans.get(&t));
        let arity = a.variants(d);
        (d.members.iter().enumerate())
            .map(|(i, (m, detail))| {
                let span = spans.and_then(|s| s.get(i)).copied();
                let payload = arity.get(i).is_some_and(|(_, n)| *n > 0);
                let mut it = CompletionItem {
                    label: m.clone(),
                    kind: Some(CompletionItemKind::ENUM_MEMBER),
                    detail: Some(detail.clone()),
                    documentation: span.and_then(|s| doc_at(a, s)),
                    ..Default::default()
                };
                if payload && self.snippets {
                    it.insert_text = Some(format!("{m}($1)$0"));
                    it.insert_text_format = Some(InsertTextFormat::SNIPPET);
                }
                it
            })
            .collect()
    }
}

/// Public top-level declarations of an imported module, one per name.
fn public_decls<'a>(a: &'a Analysis, mpath: &Path) -> Vec<&'a SymDecl> {
    let Some(mfid) = a.file_id(mpath) else {
        return vec![];
    };
    let Some(facts) = a.index.modules.iter().find(|m| m.file == mfid) else {
        return vec![];
    };
    let mut seen = HashSet::new();
    a.index
        .decls
        .iter()
        .filter(|d| {
            d.span.file == mfid
                && d.scope.is_none()
                && d.extern_alias.is_none()
                && !matches!(d.kind, DeclKind::Field | DeclKind::Variant)
                && facts.public.contains(&d.name)
                && seen.insert(d.name.clone())
        })
        .collect()
}

/// `<A, B>`, or nothing without generic arguments.
fn generic_args(args: &[String]) -> String {
    if args.is_empty() {
        String::new()
    } else {
        format!("<{}>", args.join(", "))
    }
}

/// How names are inserted where the completion is.
#[derive(Clone, Copy)]
struct Insert<'a> {
    /// functions as calls: `f(...)`
    call_parens: bool,
    /// structs as literals (`S{.x = ...}`), with the type expected there when known
    literal: Option<Option<&'a T>>,
}

/// Where the expression being completed goes.
#[derive(Debug)]
enum Slot {
    /// a value, with its expected type when known
    Value(Option<T>),
    /// a statement, a type, a field name, ...
    Other,
}

/// The type expected for argument `c.arg` of the call whose `(` is `toks[open]`. A generic
/// parameter (`T`, `I[]`) is resolved from the types of the arguments before it
/// (`pick(h, |` with `fn pick<type T>(T a, T b)`); one that stays unresolved only says
/// whether it is a function.
fn arg_ty(
    a: Option<(&Analysis, u32)>,
    text: &str,
    toks: &[&Token],
    open: usize,
    c: &super::calls::CallCtx,
) -> Option<T> {
    let ps = &c.sig.params;
    let ty = ps.get(c.arg as usize)?.0.clone()?;
    let tps = &c.open_tps;
    if !tyexpr::mentions(&ty, tps) {
        return Some(ty);
    }
    // the arguments' spans: (first token, last token)
    let mut args: Vec<(usize, usize)> = Vec::new();
    let (mut depth, mut first) = (0, None);
    for (i, t) in toks.iter().enumerate().skip(open + 1) {
        match t.tok {
            Tok::Newline => continue,
            Tok::Comma if depth == 0 => {
                args.extend(first.map(|f| (f, i - 1)));
                first = None;
                continue;
            }
            Tok::LParen | Tok::LBracket | Tok::LBrace => depth += 1,
            Tok::RParen | Tok::RBracket | Tok::RBrace => depth -= 1,
            _ => {}
        }
        first.get_or_insert(i);
    }
    let mut binds: HashMap<String, T> = HashMap::new();
    if let Some((a, fid)) = a {
        for (k, &(f, l)) in args.iter().enumerate().take(c.arg as usize) {
            let (start, end) = (toks[f].span.start, toks[l].span.end);
            // the checked type, or (on the line being typed, which does not parse yet) the
            // type of a simple expression
            let x = match a.expr_ty(fid, start, end) {
                Some(x) => Some(a.types.to_type_expr(x.ty)),
                None => text_ty(a, fid, text, &toks[f..=l]),
            };
            let (Some(x), Some((Some(p), _))) = (x, ps.get(k)) else {
                continue;
            };
            for (n, t) in tyexpr::accepts(p, &x, tps).unwrap_or_default() {
                binds.entry(n).or_insert(t);
            }
        }
    }
    let ty = tyexpr::substitute(&ty, &binds);
    (tyexpr::is_fn(&ty) || !tyexpr::mentions(&ty, tps)).then_some(ty)
}

/// The type of a simple expression from its tokens: a literal, a name, a chain (`p.x`,
/// `f().y`), an array literal or a struct literal.
fn text_ty(a: &Analysis, fid: u32, text: &str, toks: &[&Token]) -> Option<T> {
    let (first, last) = (toks.first()?, toks.last()?);
    let src = &text[first.span.start as usize..last.span.end as usize];
    if let [t] = toks {
        match &t.tok {
            Tok::Int(_, false) => return Some(tyexpr::named("int")),
            Tok::Int(_, true) => return Some(tyexpr::named("uint")),
            Tok::Float(_) => return Some(tyexpr::named("float")),
            Tok::Str(_) => return Some(tyexpr::named("str")),
            Tok::Char(_) => return Some(tyexpr::named("char")),
            Tok::Ident(b) if b == "true" || b == "false" => return Some(tyexpr::named("bool")),
            Tok::Ident(n) => {
                let d = visible(a, fid, first.span.start as usize)
                    .into_iter()
                    .find(|d| &d.name == n)?;
                return match d.kind {
                    DeclKind::Variable => d.ty.map(|t| a.types.to_type_expr(t)),
                    DeclKind::Function => d.sig.as_ref().and_then(|s| s.value_ty()),
                    _ => None,
                };
            }
            _ => {}
        }
    }
    // `[a, b]`: an array of the first element's type
    if first.tok == Tok::LBracket && last.tok == Tok::RBracket {
        let mut depth = 0;
        let end = toks[1..]
            .iter()
            .position(|t| {
                match t.tok {
                    Tok::LParen | Tok::LBracket | Tok::LBrace => depth += 1,
                    Tok::RParen | Tok::RBracket | Tok::RBrace => depth -= 1,
                    Tok::Comma if depth == 0 => return true,
                    _ => {}
                }
                depth < 0
            })
            .map_or(toks.len() - 1, |p| p + 1);
        return (end > 1)
            .then(|| text_ty(a, fid, text, &toks[1..end]))
            .flatten()
            .map(|t| T::Array(Box::new(t), None, Span::default()));
    }
    // `Name{...}`, `Name<int>{...}`
    if last.tok == Tok::RBrace
        && let Some(b) = src.find('{')
        && src[..b]
            .trim()
            .starts_with(|c: char| c.is_ascii_uppercase())
    {
        return crate::parser::parse_type_text(src[..b].trim());
    }
    receiver_chain(src)
        .filter(|(_, start)| *start == 0)
        .and_then(|(chain, _)| chain_ty(a, fid, first.span.start as usize, &chain))
        .map(|t| a.types.to_type_expr(t))
}

/// The innermost unclosed `(`, `[` or `{` at the end of `toks`, with the number of commas
/// after it at its level.
fn unclosed(toks: &[&Token]) -> Option<(usize, usize)> {
    let (mut depth, mut commas) = (0, 0);
    let mut i = toks.len();
    loop {
        if i == 0 {
            return None;
        }
        i -= 1;
        match toks[i].tok {
            Tok::RParen | Tok::RBracket | Tok::RBrace => depth += 1,
            Tok::LParen | Tok::LBracket | Tok::LBrace if depth > 0 => depth -= 1,
            Tok::LParen | Tok::LBracket | Tok::LBrace => return Some((i, commas)),
            Tok::Comma if depth == 0 => commas += 1,
            _ => {}
        }
    }
}

/// Whether the innermost unclosed `(` at the end of `toks` opens the parameters of a function
/// declaration (`fn name<T>(`) or function type (`(fn(`).
fn in_fn_header(toks: &[&Token]) -> bool {
    let Some((open, _)) = unclosed(toks).filter(|&(i, _)| toks[i].tok == Tok::LParen) else {
        return false;
    };
    let mut j = open;
    // generic parameters
    if j > 0 && toks[j - 1].tok == Tok::Gt {
        let mut d = 0;
        while j > 0 {
            j -= 1;
            match toks[j].tok {
                Tok::Gt => d += 1,
                Tok::Lt => {
                    d -= 1;
                    if d == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    let ident = |k: usize| match &toks[k].tok {
        Tok::Ident(n) => Some(n.as_str()),
        _ => None,
    };
    if j > 0 && ident(j - 1).is_some_and(|n| n != "fn") {
        j -= 1;
    }
    j > 0 && ident(j - 1) == Some("fn")
}

/// The type reached by following `segs` from a value of type `ty` (`.field`, `[i]`, `()`).
fn follow(a: &Analysis, mut ty: TyId, segs: &[Seg]) -> Option<TyId> {
    for s in segs {
        ty = match s {
            Seg::Name(n) => a.members(ty).into_iter().find(|m| &m.name == n)?.ty,
            Seg::Index => crate::check::index::elem_of(&a.types, ty)?,
            Seg::Slice => ty,
            Seg::Call => crate::check::index::return_of(&a.types, ty)?,
        };
    }
    Some(ty)
}

/// The type of a chain starting at a variable or function in scope (`p.cb`, `ps[0].x`, `f().y`).
fn chain_ty(a: &Analysis, fid: u32, off: usize, chain: &[Seg]) -> Option<TyId> {
    let Seg::Name(first) = chain.first()? else {
        return None;
    };
    let d = visible(a, fid, off)
        .into_iter()
        .find(|d| &d.name == first)?;
    let rest = match d.kind {
        DeclKind::Variable => &chain[1..],
        // the decl's type is its return type: skip the call
        DeclKind::Function if chain.get(1) == Some(&Seg::Call) => &chain[2..],
        _ => return None,
    };
    follow(a, d.ty?, rest)
}

/// The return type of the innermost function (declaration or literal) whose body is open at
/// the end of `toks`, when it returns something.
fn enclosing_fn_return(text: &str, toks: &[&Token]) -> Option<T> {
    // the unclosed `{`s, innermost last
    let mut open = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        match t.tok {
            Tok::LBrace => open.push(i),
            Tok::RBrace => _ = open.pop(),
            _ => {}
        }
    }
    for &brace in open.iter().rev() {
        // back from the `{` to a `fn` at the same nesting level: `fn name<...>(...) ret {`
        let mut depth = 0i32;
        let mut i = brace;
        while i > 0 {
            i -= 1;
            match &toks[i].tok {
                Tok::RParen | Tok::RBracket => depth += 1,
                Tok::LParen | Tok::LBracket => depth -= 1,
                Tok::Ident(k) if k == "fn" && depth == 0 => {
                    let header =
                        &text[toks[i].span.start as usize..toks[brace].span.start as usize];
                    return crate::parser::parse_fn_header_text(header)?.ret;
                }
                Tok::Newline | Tok::LBrace | Tok::RBrace | Tok::Comma | Tok::Assign
                    if depth == 0 =>
                {
                    break;
                }
                _ => {}
            }
            if depth < 0 {
                break;
            }
        }
    }
    None
}

/// Numbered snippet placeholders: `${1:name}`, or `$1` without a name.
fn tabstops() -> impl FnMut(Option<&str>) -> String {
    let mut k = 0;
    move |name| {
        k += 1;
        match name {
            Some(n) => format!("${{{k}:{n}}}"),
            None => format!("${k}"),
        }
    }
}

/// The parentheses of a call snippet: the cursor goes inside them when the function takes
/// arguments, after them otherwise.
fn call_snippet(has_args: bool) -> &'static str {
    if has_args { "($0)" } else { "()$0" }
}

fn param_hints() -> Command {
    Command::new(
        "Trigger parameter hints".into(),
        "editor.action.triggerParameterHints".into(),
        None,
    )
}

/// `s` as literal snippet text.
fn snippet_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('$', "\\$")
        .replace('}', "\\}")
}

/// Postfix completions replacing `receiver.` with a snippet around the receiver (`x.if`).
fn postfix_items(text: &str, start: usize, dot: usize, off: usize) -> Vec<CompletionItem> {
    let receiver = &text[start..dot];
    if receiver.contains('\n') {
        return vec![];
    }
    let escaped = snippet_escape(receiver);
    let range = super::range(text, start, off);
    POSTFIX
        .iter()
        .map(|(label, tmpl, desc)| CompletionItem {
            label: label.to_string(),
            kind: Some(CompletionItemKind::SNIPPET),
            detail: Some(
                tmpl.replace("{r}", receiver)
                    .replace('\t', "  ")
                    .replace("$0", ""),
            ),
            documentation: Some(Documentation::String(format!("postfix: {desc}"))),
            filter_text: Some(format!("{receiver}.{label}")),
            sort_text: Some(format!("9{label}")),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                range,
                tmpl.replace("{r}", &escaped),
            ))),
            ..Default::default()
        })
        .collect()
}

/// Names a module exports.
fn public_names(m: &ast::Module) -> HashSet<String> {
    m.items
        .iter()
        .flat_map(ast::Item::public_names)
        .cloned()
        .collect()
}

/// Completions for a path being typed in an import (modules, without `.nc`) or `@embed` (files).
fn path_items(file: &Path, typed: &str, modules: bool) -> Vec<CompletionItem> {
    let dir = file.parent().unwrap_or(Path::new("."));
    let mut system = Vec::new();
    let (base, shown) = if let Some(rel) = typed.strip_prefix(".../") {
        match crate::modules::find_root(dir) {
            Some(r) => (r, rel),
            None => return vec![],
        }
    } else if typed.starts_with("./") || typed.starts_with("../") {
        (dir.to_path_buf(), typed)
    } else if modules && crate::modules::is_system_import(typed) {
        match crate::modules::nc_home() {
            Some(h) => (h, typed),
            None => return vec![],
        }
    } else if modules {
        // nothing typed past the first segment yet: offer the system libraries too
        if !typed.contains('/')
            && let Some(home) = crate::modules::nc_home()
        {
            system = crate::modules::SYSTEM_LIBS
                .iter()
                .filter(|lib| home.join(lib).is_dir())
                .map(|lib| CompletionItem {
                    label: format!("{lib}/"),
                    kind: Some(CompletionItemKind::FOLDER),
                    ..Default::default()
                })
                .collect();
        }
        match crate::modules::find_root(dir) {
            Some(r) => (r.join(".nc").join("deps"), typed),
            None => return system,
        }
    } else {
        return vec![];
    };
    // the directory part already typed
    let sub = match shown.rfind('/') {
        Some(i) => &shown[..=i],
        None => "",
    };
    let Ok(rd) = std::fs::read_dir(base.join(sub)) else {
        return system;
    };
    let mut out = system;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
        if is_dir {
            if out.iter().any(|o| o.label == format!("{name}/")) {
                continue;
            }
            out.push(CompletionItem {
                label: format!("{name}/"),
                kind: Some(CompletionItemKind::FOLDER),
                ..Default::default()
            });
        } else if !modules {
            out.push(CompletionItem {
                label: name,
                kind: Some(CompletionItemKind::FILE),
                ..Default::default()
            });
        } else if let Some(stem) = name.strip_suffix(".nc") {
            if file
                .file_name()
                .is_some_and(|f| f.to_string_lossy() == name)
                && base.join(sub) == dir
            {
                continue;
            }
            out.push(CompletionItem {
                label: stem.to_string(),
                kind: Some(CompletionItemKind::MODULE),
                ..Default::default()
            });
        }
    }
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

/// Do the contents of `[...]` make a slice (a `:` outside nested brackets and strings)?
fn is_slice(inner: &[u8]) -> bool {
    let (mut depth, mut in_str) = (0i32, false);
    for (k, &c) in inner.iter().enumerate() {
        match c {
            b'"' if k == 0 || inner[k - 1] != b'\\' => in_str = !in_str,
            _ if in_str => {}
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b':' if depth == 0 => return true,
            _ => {}
        }
    }
    false
}

/// The doc comment of the declaration at `span`, as Markdown.
fn doc_at(a: &Analysis, span: Span) -> Option<Documentation> {
    docs::doc_comment(&a.sm.file(span.file).text, span)
        .map(|value| Documentation::MarkupContent(super::markdown(value)))
}

/// The struct of the literal whose field starts with the `.` at `dot` (`Name{.x = 1, .`), with
/// the offset of the literal's `{`.
fn literal_struct<'a>(a: &'a Analysis, text: &str, dot: usize) -> Option<(&'a SymDecl, usize)> {
    let prev = text[..dot].trim_end();
    if !(prev.ends_with('{') || prev.ends_with(',')) {
        return None;
    }
    // the unclosed `{` around the cursor
    let b = text.as_bytes();
    let mut depth = 0;
    let mut i = dot;
    let open = loop {
        if i == 0 {
            return None;
        }
        i -= 1;
        match b[i] {
            b')' | b']' | b'}' => depth += 1,
            b'(' | b'[' | b'{' if depth > 0 => depth -= 1,
            b'{' => break i,
            b'(' | b'[' => return None,
            _ => {}
        }
    };
    // the type before it: `Name`, `Name<int>` or `mod.Name`
    let mut t = text[..open].trim_end();
    if t.ends_with('>') {
        t = &t[..t.rfind('<')?];
    }
    let name_start = t
        .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
        .map_or(0, |i| i + 1);
    let name = &t[name_start..];
    if !name.chars().next()?.is_uppercase() {
        return None;
    }
    let d = a
        .index
        .decls
        .iter()
        .find(|d| d.name == name && d.kind == DeclKind::Struct)?;
    Some((d, open))
}

/// A struct's fields: (name, type, detail, definition). A generic struct's come from its
/// declaration, with its type parameters unresolved.
fn struct_fields(a: &Analysis, d: &SymDecl) -> Vec<(String, Option<T>, String, Option<Span>)> {
    match d.ty {
        Some(t) => (a.members(t).into_iter())
            .map(|m| (m.name, Some(a.types.to_type_expr(m.ty)), m.detail, m.def))
            .collect(),
        None => (d.members.iter().enumerate())
            .map(|(i, (n, detail))| (n.clone(), d.field_tys.get(i).cloned(), detail.clone(), None))
            .collect(),
    }
}

/// Fields not set yet in the struct literal around the `.` at `dot`.
fn struct_literal_fields(a: &Analysis, text: &str, dot: usize) -> Option<Vec<CompletionItem>> {
    let (d, open) = literal_struct(a, text, dot)?;
    let members = struct_fields(a, d);
    let present: HashSet<&str> = text[open..dot]
        .split('.')
        .skip(1)
        .filter_map(|s| s.split(|c: char| c == '=' || c.is_whitespace()).next())
        .collect();
    Some(
        members
            .into_iter()
            .filter(|(n, ..)| !present.contains(n.as_str()))
            .map(|(name, _, detail, def)| CompletionItem {
                insert_text: Some(format!("{name} = ")),
                label: name,
                kind: Some(CompletionItemKind::FIELD),
                detail: Some(detail),
                documentation: def.and_then(|def| doc_at(a, def)),
                ..Default::default()
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_open_strings() {
        assert_eq!(open_string("x = \"ab"), Some(4));
        assert_eq!(open_string("x = \"ab\" + y"), None);
        // interpolations are code, and may hold strings of their own
        assert_eq!(open_string("x = \"a {p."), None);
        assert_eq!(open_string("x = \"a {m[\"k"), Some(10));
        assert_eq!(open_string("x = \"a {m[\"k\"]."), None);
        assert_eq!(open_string("x = \"a {f({1})} b"), Some(4));
        assert_eq!(open_string("x = \"a \\{ b"), Some(4));
        assert_eq!(open_string("x = \"a\\\" {b"), None);
        // char literals and comments
        assert_eq!(open_string("c = '\"' + "), None);
        assert_eq!(open_string("x // \"a"), None);
    }

    #[test]
    fn parses_receiver_chains() {
        assert_eq!(
            receiver_chain("x = a.b(1, f(2))[i].c"),
            Some((
                vec![
                    Seg::Name("a".into()),
                    Seg::Name("b".into()),
                    Seg::Call,
                    Seg::Index,
                    Seg::Name("c".into()),
                ],
                4
            ))
        );
        assert_eq!(receiver_chain("1 + "), None);
        assert_eq!(
            receiver_chain("@println(\"{p"),
            Some((vec![Seg::Name("p".into())], 11))
        );
    }
}
