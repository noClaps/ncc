//! Completion: scope-aware names, members of any typed expression (`a.b[0].c.`), struct literal
//! fields, enum variants, module members, builtins, import and `@embed` paths, and snippets.

use std::collections::HashSet;
use std::path::Path;

use lsp_types::*;

use super::{Analysis, BUILTINS, KEYWORDS, Server, TYPES, docs, pos_to_offset, uri_to_path};
use crate::ast;
use crate::check::index::{DeclKind, SymDecl};

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
}

/// Parse the receiver chain at the end of `head` (`a.b(x)[0].c`), outermost last.
fn receiver_chain(head: &str) -> Option<Vec<Seg>> {
    receiver_chain_at(head).map(|(s, _)| s)
}

/// Like `receiver_chain`, with the offset where the receiver starts.
fn receiver_chain_at(head: &str) -> Option<(Vec<Seg>, usize)> {
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
            segs.push(if close == b')' { Seg::Call } else { Seg::Index });
            i = j;
        }
        let end = i;
        while i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_') {
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
fn visible(a: &Analysis, fid: u32, off: usize) -> Vec<&SymDecl> {
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

/// Declarations visible at `off`.
pub(super) fn visible_decls(a: &Analysis, fid: u32, off: usize) -> Vec<&SymDecl> {
    visible(a, fid, off)
}

/// Names visible at `off` (for suggestions).
pub(super) fn visible_names(a: &Analysis, fid: u32, off: usize) -> Vec<String> {
    visible(a, fid, off)
        .into_iter()
        .map(|d| d.name.clone())
        .collect()
}

/// Parameter names from a function detail (`fn add(int a, int b) (int, int)` -> `a`, `b`).
fn param_names(detail: &str) -> Vec<String> {
    super::calls::params(detail)
        .into_iter()
        .map(|(_, n)| n)
        .collect()
}

impl Server {
    fn doc_of(&self, a: &Analysis, d: &SymDecl) -> Option<Documentation> {
        let text = &a.sm.file(d.span.file).text;
        docs::doc_comment(text, d.span).map(|value| {
            Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            })
        })
    }

    /// A completion for a declaration; with a qualifier (`alias`), the item inserts
    /// `alias.name` and is found by typing the name.
    fn decl_item(
        &self,
        a: &Analysis,
        d: &SymDecl,
        call_parens: bool,
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
            documentation: self.doc_of(a, d),
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
        if d.kind == DeclKind::Function && call_parens && self.snippets {
            let ps = param_names(&d.detail);
            // a generic function takes its type arguments first: `map<${1:I}, ${2:O}>(...)`
            let tps = super::calls::type_params(&d.detail);
            let mut k = 0;
            let mut slot = |name: &str| {
                k += 1;
                format!("${{{k}:{name}}}")
            };
            let targs: Vec<String> = tps.iter().map(|t| slot(t)).collect();
            let args: Vec<String> = ps.iter().map(|p| slot(p)).collect();
            let generics = if targs.is_empty() {
                String::new()
            } else {
                format!("<{}>", targs.join(", "))
            };
            item.insert_text = Some(format!("{full}{generics}({})$0", args.join(", ")));
            item.insert_text_format = Some(InsertTextFormat::SNIPPET);
            if !ps.is_empty() {
                item.command = Some(Command::new(
                    "Trigger parameter hints".into(),
                    "editor.action.triggerParameterHints".into(),
                    None,
                ));
            }
        }
        item
    }

    pub(super) fn completion(&self, p: CompletionParams) -> Option<CompletionResponse> {
        let pp = p.text_document_position;
        let path = uri_to_path(&pp.text_document.uri)?;
        let text = self.text_of(&path)?;
        let off = pos_to_offset(&text, pp.position);
        let line_start = text[..off].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let before = &text[line_start..off];
        let analysis = self.analyses.get(&path);
        let fid = analysis.and_then(|a| a.file_id(&path));
        let call_parens = !text[off..].starts_with('(');
        let item = |label: &str, kind: CompletionItemKind, detail: Option<String>| CompletionItem {
            label: label.to_string(),
            kind: Some(kind),
            detail,
            ..Default::default()
        };
        let mut items = Vec::new();

        // paths inside `import { "..." }` and `@embed("...")`
        if before.matches('"').count() % 2 == 1 {
            let q = before.rfind('"').unwrap();
            let typed = &before[q + 1..];
            let pre = &text[..line_start + q];
            let in_embed = pre.trim_end().ends_with("@embed(");
            let in_import = match (pre.rfind("import"), pre.rfind('}')) {
                (Some(i), Some(c)) => i > c,
                (Some(_), None) => true,
                _ => false,
            };
            if in_embed || in_import {
                return Some(CompletionResponse::Array(path_items(
                    &path, typed, in_import,
                )));
            }
            return Some(CompletionResponse::Array(vec![]));
        }

        let word_start = before
            .char_indices()
            .rev()
            .find(|(_, c)| !(c.is_alphanumeric() || *c == '_'))
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        let prefix_char = before[..word_start].chars().last();

        // `@builtin`
        if prefix_char == Some('@') {
            for (n, sig, doc) in BUILTINS {
                let mut it = item(n, CompletionItemKind::FUNCTION, Some(sig.to_string()));
                it.documentation = Some(Documentation::String(doc.to_string()));
                items.push(it);
            }
            return Some(CompletionResponse::Array(items));
        }

        // after `.`
        if prefix_char == Some('.') {
            let Some(a) = analysis else {
                return Some(CompletionResponse::Array(items));
            };
            let dot = line_start + word_start - 1;
            if let Some(fields) = self.struct_literal_fields(a, &text, dot) {
                return Some(CompletionResponse::Array(fields));
            }
            let head = &text[..dot];
            let Some(chain) = receiver_chain(head) else {
                return Some(CompletionResponse::Array(items));
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
                let (_, start) = receiver_chain_at(head)?;
                postfix_items(&text, start, dot, off)
            } else {
                vec![]
            };
            if let Some(mut items) = self.member_items(a, fid, dot, &chain, call_parens) {
                items.extend(postfix);
                return Some(CompletionResponse::Array(items));
            }
            if !postfix.is_empty() {
                return Some(CompletionResponse::Array(postfix));
            }
            // `name.` where `name` is a module that is not imported: its public items, adding
            // the import
            if let [Seg::Name(name)] = chain.as_slice()
                && !a.imports.contains_key(name)
                && let Some((_, ipath)) = super::actions::importable_modules(&path)
                    .into_iter()
                    .find(|(n, _)| n == name)
            {
                let dir = path.parent()?;
                let file = if let Some(rel) = ipath.strip_prefix(".../") {
                    crate::modules::find_root(dir)?.join(format!("{rel}.nc"))
                } else if let Some(rel) = ipath.strip_prefix("./") {
                    dir.join(format!("{rel}.nc"))
                } else {
                    crate::modules::find_root(dir)?
                        .join(".nc")
                        .join("deps")
                        .join(format!("{ipath}.nc"))
                };
                let (module, _) = crate::parser::parse_module_recover(&text, 0);
                let edit = super::actions::add_import(&text, &module, &ipath, name);
                let mut names: Vec<String> = public_names(&std::fs::read_to_string(file).ok()?)
                    .into_iter()
                    .collect();
                names.sort();
                return Some(CompletionResponse::Array(
                    names
                        .into_iter()
                        .map(|n| CompletionItem {
                            label: n,
                            detail: Some(format!("import \"{ipath}\"")),
                            additional_text_edits: Some(vec![edit.clone()]),
                            ..Default::default()
                        })
                        .collect(),
                ));
            }
            return Some(CompletionResponse::Array(vec![]));
        }

        // names, keywords and types
        if let (Some(a), Some(fid)) = (analysis, fid) {
            for d in visible(a, fid, off) {
                items.push(self.decl_item(a, d, call_parens, None));
            }
            for alias in a.imports.keys() {
                items.push(item(
                    alias,
                    CompletionItemKind::MODULE,
                    Some("module".into()),
                ));
            }
            // extern blocks
            let (module, _) = crate::parser::parse_module_recover(&text, 0);
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
                    items.push(self.decl_item(a, d, call_parens, Some(q)));
                }
            }
            for (alias, mpath) in &a.imports {
                let Some(mfid) = a.file_id(mpath) else {
                    continue;
                };
                let Some(facts) = a.index.modules.iter().find(|m| m.file == mfid) else {
                    continue;
                };
                let mut seen = HashSet::new();
                for d in &a.index.decls {
                    if d.span.file == mfid
                        && d.scope.is_none()
                        && d.extern_alias.is_none()
                        && !matches!(d.kind, DeclKind::Field | DeclKind::Variant)
                        && facts.public.contains(&d.name)
                        && seen.insert(d.name.clone())
                    {
                        items.push(self.decl_item(a, d, call_parens, Some(alias)));
                    }
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
            for (name, ipath) in super::actions::importable_modules(&path) {
                if taken.contains(&name) || !offered.insert(name.clone()) {
                    continue;
                }
                let edit = super::actions::add_import(&text, &module, &ipath, &name);
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
        for (n, sig, doc) in BUILTINS {
            let snippet = self.snippets && call_parens;
            items.push(CompletionItem {
                label: format!("@{n}"),
                kind: Some(CompletionItemKind::FUNCTION),
                detail: Some(sig.to_string()),
                documentation: Some(Documentation::String(doc.to_string())),
                filter_text: Some(n.to_string()),
                insert_text: Some(if snippet {
                    format!("@{n}($0)")
                } else {
                    format!("@{n}")
                }),
                insert_text_format: snippet.then_some(InsertTextFormat::SNIPPET),
                sort_text: Some(format!("7{n}")),
                ..Default::default()
            });
        }
        // a function literal for an argument that takes a function
        if self.snippets {
            items.extend(self.fn_literal_items(&pp.text_document.uri, off));
        }
        for k in KEYWORDS {
            let mut it = item(k, CompletionItemKind::KEYWORD, None);
            it.sort_text = Some(format!("3{k}"));
            items.push(it);
        }
        for t in TYPES {
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
        Some(CompletionResponse::Array(items))
    }

    /// Fields of the struct literal or pattern around a `.` that starts a field (`Point{.x = 1, .`).
    fn struct_literal_fields(
        &self,
        a: &Analysis,
        text: &str,
        dot: usize,
    ) -> Option<Vec<CompletionItem>> {
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
                b'(' | b'[' if depth > 0 => depth -= 1,
                b'{' if depth > 0 => depth -= 1,
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
            .map(|i| i + 1)
            .unwrap_or(0);
        let name = &t[name_start..];
        if !name.chars().next()?.is_uppercase() {
            return None;
        }
        let d = a
            .index
            .decls
            .iter()
            .find(|d| d.name == name && d.kind == DeclKind::Struct)?;
        let members = a.index.types.get(d.ty.as_ref()?)?;
        let present: HashSet<&str> = text[open..dot]
            .split('.')
            .skip(1)
            .filter_map(|s| s.split(|c: char| c == '=' || c.is_whitespace()).next())
            .collect();
        Some(
            members
                .iter()
                .filter(|m| !present.contains(m.name.as_str()))
                .map(|m| CompletionItem {
                    label: m.name.clone(),
                    kind: Some(CompletionItemKind::FIELD),
                    detail: Some(m.detail.clone()),
                    insert_text: Some(format!("{} = ", m.name)),
                    documentation: m.def.and_then(|def| {
                        docs::doc_comment(&a.sm.file(def.file).text, def).map(|value| {
                            Documentation::MarkupContent(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value,
                            })
                        })
                    }),
                    ..Default::default()
                })
                .collect(),
        )
    }

    /// Members after a receiver chain: module members, enum variants or the members of the
    /// receiver's type.
    fn member_items(
        &self,
        a: &Analysis,
        fid: u32,
        off: usize,
        chain: &[Seg],
        call_parens: bool,
    ) -> Option<Vec<CompletionItem>> {
        let Seg::Name(first) = &chain[0] else {
            return None;
        };
        let mut rest = &chain[1..];
        let mut ty: String;
        // the functions of an extern block
        let externs: Vec<&SymDecl> = a
            .index
            .decls
            .iter()
            .filter(|d| d.span.file == fid && d.extern_alias.as_ref() == Some(first))
            .collect();
        if !externs.is_empty() {
            let Some(Seg::Name(n)) = rest.first() else {
                return Some(
                    externs
                        .into_iter()
                        .map(|d| self.decl_item(a, d, call_parens, None))
                        .collect(),
                );
            };
            let d = externs.into_iter().find(|d| &d.name == n)?;
            if rest.get(1) != Some(&Seg::Call) {
                return None;
            }
            ty = d.ty.clone()?;
            rest = &rest[2..];
        } else if let Some(mpath) = a.imports.get(first) {
            let mfid = a.file_id(mpath)?;
            let public = public_names(&self.text_of(mpath)?);
            if rest.is_empty() {
                let mut seen = HashSet::new();
                let mut out = Vec::new();
                for d in &a.index.decls {
                    if d.span.file == mfid
                        && d.scope.is_none()
                        && !matches!(d.kind, DeclKind::Field | DeclKind::Variant)
                        && public.contains(&d.name)
                        && seen.insert(d.name.clone())
                    {
                        out.push(self.decl_item(a, d, call_parens, None));
                    }
                }
                return Some(out);
            }
            let Seg::Name(n) = &rest[0] else { return None };
            let d = a
                .index
                .decls
                .iter()
                .find(|d| d.span.file == mfid && d.scope.is_none() && &d.name == n)?;
            if matches!(d.kind, DeclKind::Enum) && rest.len() == 1 {
                return Some(self.variant_items(a, d));
            }
            ty = d.ty.clone()?;
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
                DeclKind::Enum if rest.is_empty() => return Some(self.variant_items(a, d)),
                DeclKind::Function => {
                    if rest.first() != Some(&Seg::Call) {
                        return None;
                    }
                    rest = &rest[1..];
                }
                _ => {}
            }
            ty = d.ty.clone()?;
        }
        for s in rest {
            ty = match s {
                Seg::Name(n) => a
                    .index
                    .types
                    .get(&ty)?
                    .iter()
                    .find(|m| &m.name == n)?
                    .ty
                    .clone(),
                Seg::Index => a.index.elems.get(&ty)?.clone(),
                Seg::Call => a.index.returns.get(&ty)?.clone(),
            };
        }
        let members = a.index.types.get(&ty)?;
        Some(
            members
                .iter()
                .map(|m| CompletionItem {
                    label: m.name.clone(),
                    kind: Some(CompletionItemKind::FIELD),
                    detail: Some(m.detail.clone()),
                    documentation: m.def.and_then(|def| {
                        docs::doc_comment(&a.sm.file(def.file).text, def).map(|value| {
                            Documentation::MarkupContent(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value,
                            })
                        })
                    }),
                    ..Default::default()
                })
                .collect(),
        )
    }

    fn variant_items(&self, a: &Analysis, d: &SymDecl) -> Vec<CompletionItem> {
        d.members
            .iter()
            .map(|(m, detail)| {
                let variant = a.index.decls.iter().find(|v| {
                    v.kind == DeclKind::Variant
                        && v.detail.starts_with(&format!("{}.{m}", d.name))
                        && v.span.file == d.span.file
                });
                let payload = detail.ends_with(')');
                let mut it = CompletionItem {
                    label: m.clone(),
                    kind: Some(CompletionItemKind::ENUM_MEMBER),
                    detail: Some(detail.clone()),
                    documentation: variant.and_then(|v| self.doc_of(a, v)),
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

/// Postfix completions replacing `receiver.` with a snippet around the receiver (`x.if`).
fn postfix_items(text: &str, start: usize, dot: usize, off: usize) -> Vec<CompletionItem> {
    let receiver = &text[start..dot];
    if receiver.contains('\n') {
        return vec![];
    }
    let escaped = receiver
        .replace('\\', "\\\\")
        .replace('$', "\\$")
        .replace('}', "\\}");
    let range = Range::new(
        super::offset_to_pos(text, start),
        super::offset_to_pos(text, off),
    );
    POSTFIX
        .iter()
        .map(|(label, tmpl, desc)| CompletionItem {
            label: label.to_string(),
            kind: Some(CompletionItemKind::SNIPPET),
            detail: Some(
                tmpl.replace("{r}", receiver)
                    .replace("\t", "  ")
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
fn public_names(text: &str) -> HashSet<String> {
    let (m, _) = crate::parser::parse_module_recover(text, 0);
    let mut out = HashSet::new();
    for it in &m.items {
        match it {
            ast::Item::Fn(f) if f.public => {
                out.insert(f.sig.name.clone());
            }
            ast::Item::Struct(s) if s.public => {
                out.insert(s.name.clone());
            }
            ast::Item::Enum(e) if e.public => {
                out.insert(e.name.clone());
            }
            ast::Item::Type(t) if t.public => {
                out.insert(t.name.clone());
            }
            ast::Item::Stmt(ast::Stmt {
                kind:
                    ast::StmtKind::VarDecl {
                        public: true,
                        targets,
                        ..
                    },
                ..
            }) => {
                for t in targets {
                    out.insert(t.name.clone());
                }
            }
            _ => {}
        }
    }
    out
}

/// Completions for a path being typed in an import (modules, without `.nc`) or `@embed` (files).
fn path_items(file: &Path, typed: &str, modules: bool) -> Vec<CompletionItem> {
    let dir = file.parent().unwrap_or(Path::new("."));
    let (base, shown) = if let Some(rel) = typed.strip_prefix(".../") {
        match crate::modules::find_root(dir) {
            Some(r) => (r, rel),
            None => return vec![],
        }
    } else if typed.starts_with("./") || typed.starts_with("../") {
        (dir.to_path_buf(), typed)
    } else if modules {
        match crate::modules::find_root(dir) {
            Some(r) => (r.join(".nc").join("deps"), typed),
            None => return vec![],
        }
    } else {
        return vec![];
    };
    // the directory part already typed
    let sub = match shown.rfind('/') {
        Some(i) => &shown[..i + 1],
        None => "",
    };
    let Ok(rd) = std::fs::read_dir(base.join(sub)) else {
        return vec![];
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
        if is_dir {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_receiver_chains() {
        assert_eq!(
            receiver_chain("x = a.b(1, f(2))[i].c"),
            Some(vec![
                Seg::Name("a".into()),
                Seg::Name("b".into()),
                Seg::Call,
                Seg::Index,
                Seg::Name("c".into()),
            ])
        );
        assert_eq!(receiver_chain("1 + "), None);
        assert_eq!(
            param_names("fn add(int a, (int, int) b) int"),
            vec!["a".to_string(), "b".to_string()]
        );
    }
}
