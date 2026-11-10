//! Code actions, and the warnings the language server adds to the compiler's diagnostics
//! (unused variables and imports).
//!
//! Quick fixes: make a variable mutable, discard an unused value, rename an unused variable to
//! `_`, remove an unused import, fix a misspelt name, make a function throwing, add missing
//! struct fields, add missing `if` branches.
//! Refactors: extract an expression into a variable, inline a variable.
//! Source actions: remove all unused imports.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use lsp_types::*;

use super::nav::{Node, walk};
use super::{Analysis, Server, name_span, offset_to_pos, pos_to_offset, span_range, uri_to_path};
use crate::ast::{self, ExprKind as E, StmtKind as S};
use crate::check::index::DeclKind;
use crate::diag::Span;
use crate::lexer::Tok;

const UNUSED_VARIABLE: &str = "unused-variable";
const UNUSED_IMPORT: &str = "unused-import";
const UNREACHABLE_BRANCH: &str = "unreachable-branch";

fn line_start(text: &str, off: usize) -> usize {
    text[..off.min(text.len())]
        .rfind('\n')
        .map(|i| i + 1)
        .unwrap_or(0)
}

fn line_end(text: &str, off: usize) -> usize {
    text[off.min(text.len())..]
        .find('\n')
        .map(|i| off + i + 1)
        .unwrap_or(text.len())
}

fn indent_at(text: &str, off: usize) -> &str {
    let s = line_start(text, off);
    let l = &text[s..];
    &l[..l.len() - l.trim_start_matches([' ', '\t']).len()]
}

// ------------------------------------------------------------------ warnings

/// Unused local variables of the entry file (with the range of their name).
fn unused_variables(a: &Analysis, fid: u32) -> Vec<(Span, String)> {
    let text = &a.sm.file(fid).text;
    let mut used: HashSet<Span> = HashSet::new();
    // spans that are also a reference to something else (a `lock` alias shadows its mutex)
    let mut other: HashSet<Span> = HashSet::new();
    for r in &a.index.refs {
        if let Some(def) = r.def {
            if r.span != def {
                used.insert(def);
            } else {
                continue;
            }
            other.insert(r.span);
        }
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for d in &a.index.decls {
        if d.kind != DeclKind::Variable
            || d.scope.is_none()
            || d.span.file != fid
            || d.name.starts_with('_')
            || used.contains(&d.span)
            || other.contains(&d.span)
            || !seen.insert(d.span)
        {
            continue;
        }
        // the declaration must end with the name (`int x`, `x` in a pattern)
        let Some(seg) = text.get(d.span.start as usize..d.span.end as usize) else {
            continue;
        };
        if !seg.ends_with(&d.name) {
            continue;
        }
        let ns = Span {
            file: fid,
            start: d.span.end - d.name.len() as u32,
            end: d.span.end,
        };
        let what = if d.param { "parameter" } else { "variable" };
        out.push((ns, format!("unused {what} `{}`", d.name)));
    }
    out
}

/// Imports of a module whose alias is never used: (import span, alias name span, alias).
fn unused_imports(text: &str) -> Vec<(Span, Span, String)> {
    let (module, _) = crate::parser::parse_module_recover(text, 0);
    let (toks, _) = crate::lexer::lex_all(text, 0, 0);
    let mut used: HashSet<&str> = HashSet::new();
    let note = |toks: &[crate::lexer::Token], used: &mut HashSet<String>| {
        for (i, t) in toks.iter().enumerate() {
            if let Tok::Ident(n) = &t.tok
                && matches!(toks.get(i + 1).map(|t| &t.tok), Some(Tok::Dot))
            {
                used.insert(n.clone());
            }
        }
    };
    let mut used_owned = HashSet::new();
    note(&toks, &mut used_owned);
    // names used inside string interpolations
    for t in &toks {
        if let Tok::Str(parts) = &t.tok {
            for p in parts {
                if let crate::lexer::StrPart::Expr(src, off) = p {
                    let (inner, _) = crate::lexer::lex_all(src, 0, *off);
                    note(&inner, &mut used_owned);
                }
            }
        }
    }
    used.extend(used_owned.iter().map(|s| s.as_str()));
    let mut out = Vec::new();
    for it in &module.items {
        if let ast::Item::Import(im) = it
            && !used.contains(im.alias.as_str())
        {
            let ns = name_span(text, im.span, &im.alias, false).unwrap_or(im.span);
            out.push((im.span, ns, im.alias.clone()));
        }
    }
    out
}

/// `if` branches that can never run, because earlier branches match everything they match:
/// branches after a catch-all (`_`, a binding, or the condition `true`), and branches whose
/// patterns all appeared in earlier branches. Returns the spans of the branches.
fn unreachable_arms(a: &Analysis, text: &str) -> Vec<Span> {
    let (module, _) = crate::parser::parse_module_recover(text, 0);
    let mut out = Vec::new();
    walk(&module, &mut |n| {
        let Node::Expr(e) = n else { return };
        let E::If(ife) = &e.kind else { return };
        let mut seen: HashSet<String> = HashSet::new();
        let mut caught = false;
        for arm in &ife.arms {
            let simple = |p: &ast::Expr| -> Option<String> {
                let ok = match &p.kind {
                    E::Int(..) | E::Char(_) | E::Bool(_) | E::None | E::Float(_) => true,
                    E::Str(segs) => segs.iter().all(|s| matches!(s, ast::StrSeg::Lit(_))),
                    E::Field(b, _) => matches!(b.kind, E::Ident(_)),
                    E::Unary(ast::UnOp::Neg, x) => matches!(x.kind, E::Int(..) | E::Float(_)),
                    _ => false,
                };
                ok.then(|| {
                    text[p.span.start as usize..p.span.end as usize]
                        .split_whitespace()
                        .collect()
                })
            };
            if caught
                || (!arm.patterns.is_empty()
                    && arm
                        .patterns
                        .iter()
                        .all(|p| simple(p).is_some_and(|k| seen.contains(&k))))
            {
                out.push(arm.span);
                continue;
            }
            for p in &arm.patterns {
                let catch_all = match &p.kind {
                    E::Wildcard => true,
                    // a name that declares a variable binds every value
                    E::Ident(_) if ife.scrutinee.is_some() => a.decl(p.span).is_some(),
                    E::Bool(true) => ife.scrutinee.is_none(),
                    _ => false,
                };
                caught |= catch_all;
                if let Some(k) = simple(p) {
                    seen.insert(k);
                }
            }
        }
    });
    out
}

/// The language server's own warnings for an analysed entry file.
pub(super) fn warnings(a: &Analysis, entry: &Path) -> Vec<(PathBuf, Diagnostic)> {
    let Some(fid) = a.file_id(entry) else {
        return vec![];
    };
    let text = &a.sm.file(fid).text;
    let warn = |span: Span, msg: String, code: &str| {
        (
            entry.to_path_buf(),
            Diagnostic {
                range: span_range(text, span),
                severity: Some(DiagnosticSeverity::WARNING),
                code: Some(NumberOrString::String(code.into())),
                source: Some("ncc".into()),
                message: msg,
                tags: Some(vec![DiagnosticTag::UNNECESSARY]),
                ..Default::default()
            },
        )
    };
    let mut out = Vec::new();
    for (s, msg) in unused_variables(a, fid) {
        out.push(warn(s, msg, UNUSED_VARIABLE));
    }
    for (_, ns, alias) in unused_imports(text) {
        out.push(warn(ns, format!("unused import `{alias}`"), UNUSED_IMPORT));
    }
    for arm in unreachable_arms(a, text) {
        out.push(warn(
            arm,
            "unreachable branch: earlier branches match every value it matches".into(),
            UNREACHABLE_BRANCH,
        ));
    }
    out
}

// ------------------------------------------------------------------ helpers

/// Edits removing an import entry: the whole `import { ... }` when it is the only entry, its line
/// when it is alone on it, or the entry and its separating comma.
fn remove_import(text: &str, span: Span) -> TextEdit {
    let (s, e) = (span.start as usize, span.end as usize);
    let open = text[..s].rfind("import").unwrap_or(s);
    let close = text[e..].find('}').map(|i| e + i + 1).unwrap_or(e);
    let inner = &text[open..close];
    let entries = inner.matches(" as ").count();
    let range = |a: usize, b: usize| Range::new(offset_to_pos(text, a), offset_to_pos(text, b));
    if entries <= 1 {
        return TextEdit::new(
            range(line_start(text, open), line_end(text, close)),
            String::new(),
        );
    }
    let ls = line_start(text, s);
    let le = line_end(text, e);
    let line = text[ls..le].trim();
    let entry = text[s..e].trim();
    if line.trim_end_matches(',') == entry {
        return TextEdit::new(range(ls, le), String::new());
    }
    // `a, b` on one line: remove the following comma, or the preceding one for the last entry
    let after = &text[e..];
    let trail = after.len() - after.trim_start_matches(',').trim_start_matches(' ').len();
    if after.starts_with(',') {
        TextEdit::new(range(s, e + trail), String::new())
    } else {
        let before = text[..s].trim_end_matches(' ');
        let b = before.strip_suffix(',').map(|b| b.len()).unwrap_or(s);
        TextEdit::new(range(b, e), String::new())
    }
}

/// Modules `file` could import, as (name, import path): siblings (`./name`), other project files
/// (`.../dir/name`) and dependencies (`std/name`), best first.
pub(super) fn importable_modules(file: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(dir) = file.parent() else {
        return out;
    };
    let stem = |f: &Path| f.file_stem().map(|n| n.to_string_lossy().into_owned());
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let f = e.path();
            if f.extension().is_some_and(|x| x == "nc") && f != file && f.is_file() {
                let n = stem(&f).unwrap_or_default();
                out.push((n.clone(), format!("./{n}")));
            }
        }
    }
    let Some(root) = crate::modules::find_root(dir) else {
        out.sort();
        return out;
    };
    let mut files = Vec::new();
    super::collect_nc_files(&root, &mut files, 0);
    for f in files {
        if f.parent() != Some(dir)
            && let Ok(rel) = f.with_extension("").strip_prefix(&root)
        {
            out.push((
                stem(&f).unwrap_or_default(),
                format!(".../{}", rel.to_string_lossy()),
            ));
        }
    }
    let deps = root.join(".nc").join("deps");
    let mut files = Vec::new();
    collect_all_nc(&deps, &mut files, 0);
    for f in files {
        if let Ok(rel) = f.with_extension("").strip_prefix(&deps) {
            out.push((
                stem(&f).unwrap_or_default(),
                rel.to_string_lossy().into_owned(),
            ));
        }
    }
    out.sort_by_key(|(n, p)| {
        (
            n.clone(),
            !p.starts_with("./"),
            p.starts_with(".../"),
            p.clone(),
        )
    });
    out.retain(|(n, _)| {
        n.chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !crate::parser::is_keyword(n)
    });
    out
}

/// Import paths of modules named `name` that `file` could import.
fn find_modules(file: &Path, name: &str) -> Vec<String> {
    importable_modules(file)
        .into_iter()
        .filter(|(n, _)| n == name)
        .map(|(_, p)| p)
        .collect()
}

/// `.nc` files under a directory, including hidden ones (dependencies live under `.nc/`).
fn collect_all_nc(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 8 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => collect_all_nc(&p, out, depth + 1),
            Ok(t) if t.is_file() && p.extension().is_some_and(|x| x == "nc") => out.push(p),
            _ => {}
        }
    }
}

/// An edit adding `import { "path" as alias }` after the last import (or at the top).
pub(super) fn add_import(text: &str, module: &ast::Module, path: &str, alias: &str) -> TextEdit {
    let last = module
        .items
        .iter()
        .filter_map(|it| match it {
            ast::Item::Import(im) => Some(im.span.end as usize),
            _ => None,
        })
        .max();
    let at = match last {
        // after the `}` closing the block of the last import
        Some(e) => {
            let close = text[e..].find('}').map(|i| e + i).unwrap_or(e);
            line_end(text, close)
        }
        None => 0,
    };
    let mut ins = format!("import {{ \"{path}\" as {alias} }}\n");
    if last.is_none() && !text.starts_with("import") && !text.trim().is_empty() {
        ins.push('\n');
    }
    if at == text.len() && !text.is_empty() && !text.ends_with('\n') {
        ins.insert(0, '\n');
    }
    let p = offset_to_pos(text, at);
    TextEdit::new(Range::new(p, p), ins)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let sub = prev[j - 1] + usize::from(a[i - 1] != b[j - 1]);
            cur[j] = sub.min(prev[j] + 1).min(cur[j - 1] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

/// A default value for a field of the given type, for "add missing fields".
fn default_value(ty: &str) -> String {
    match ty {
        "int" | "uint" | "byte" => "0".into(),
        "float" => "0.0".into(),
        "bool" => "false".into(),
        "str" => "\"\"".into(),
        "char" => "' '".into(),
        t if t.ends_with('?') => "none".into(),
        t if t.ends_with("[]") => "[]".into(),
        t if t.starts_with('[') && !t.ends_with(']') => "[:]".into(),
        _ => "_".into(),
    }
}

/// Whether an expression can replace a name without parentheses.
fn atomic(e: &ast::Expr) -> bool {
    matches!(
        e.kind,
        E::Int(..)
            | E::Float(_)
            | E::Char(_)
            | E::Str(_)
            | E::Bool(_)
            | E::None
            | E::Ident(_)
            | E::Call(..)
            | E::Builtin(..)
            | E::Field(..)
            | E::Index(..)
            | E::Tuple(_)
            | E::Array(_)
            | E::Map(_)
            | E::Struct(..)
            | E::Generic(..)
    )
}

// ------------------------------------------------------------------ code actions

impl Server {
    // `WorkspaceEdit::changes` is keyed by `Uri`, as defined by lsp-types
    #[allow(clippy::mutable_key_type)]
    pub(super) fn code_actions(&self, p: CodeActionParams) -> Option<CodeActionResponse> {
        let uri = p.text_document.uri.clone();
        let path = uri_to_path(&uri)?;
        let text = self.text_of(&path)?;
        let mut out = Vec::new();
        let action = |title: String,
                      kind: CodeActionKind,
                      diag: Option<&Diagnostic>,
                      edits: Vec<TextEdit>,
                      preferred: bool| {
            let mut changes = HashMap::new();
            changes.insert(uri.clone(), edits);
            CodeActionOrCommand::CodeAction(CodeAction {
                title,
                kind: Some(kind),
                diagnostics: diag.map(|d| vec![d.clone()]),
                edit: Some(WorkspaceEdit {
                    changes: Some(changes),
                    ..Default::default()
                }),
                is_preferred: preferred.then_some(true),
                ..Default::default()
            })
        };
        let insert = |at: Position, s: &str| TextEdit::new(Range::new(at, at), s.to_string());
        let (module, _) = crate::parser::parse_module_recover(&text, 0);
        let fresh = self.fresh(&uri);

        for d in &p.context.diagnostics {
            let code = match &d.code {
                Some(NumberOrString::String(c)) => c.as_str(),
                _ => "",
            };
            let msg = d.message.as_str();
            if msg.contains("declare it with `mut`") && msg.contains("immutable variable") {
                if let Some((a, r)) = self.lookup(&uri, d.range.start)
                    && let Some(def) = r.def
                    && a.sm.file(def.file).path == path
                {
                    let at = offset_to_pos(&text, def.start as usize);
                    out.push(action(
                        "Make the variable mutable".into(),
                        CodeActionKind::QUICKFIX,
                        Some(d),
                        vec![insert(at, "mut ")],
                        true,
                    ));
                }
            } else if msg.contains("not used") || msg.starts_with("unused value") {
                out.push(action(
                    "Discard the value with `_ =`".into(),
                    CodeActionKind::QUICKFIX,
                    Some(d),
                    vec![insert(d.range.start, "_ = ")],
                    true,
                ));
            } else if code == UNUSED_VARIABLE {
                out.push(action(
                    "Rename to `_`".into(),
                    CodeActionKind::QUICKFIX,
                    Some(d),
                    vec![TextEdit::new(d.range, "_".into())],
                    true,
                ));
            } else if code == UNREACHABLE_BRANCH {
                let (s, e) = (
                    pos_to_offset(&text, d.range.start),
                    pos_to_offset(&text, d.range.end),
                );
                // the whole lines when the branch is alone on them
                let (ls, le) = (line_start(&text, s), line_end(&text, e));
                let alone = text[ls..s].trim().is_empty() && text[e..le].trim().is_empty();
                let (s, e) = if alone { (ls, le) } else { (s, e) };
                out.push(action(
                    "Remove the unreachable branch".into(),
                    CodeActionKind::QUICKFIX,
                    Some(d),
                    vec![TextEdit::new(
                        Range::new(offset_to_pos(&text, s), offset_to_pos(&text, e)),
                        String::new(),
                    )],
                    true,
                ));
            } else if code == UNUSED_IMPORT {
                let off = pos_to_offset(&text, d.range.start) as u32;
                if let Some((span, ..)) = unused_imports(&text)
                    .into_iter()
                    .find(|(s, ..)| s.start <= off && off <= s.end)
                {
                    out.push(action(
                        "Remove unused import".into(),
                        CodeActionKind::QUICKFIX,
                        Some(d),
                        vec![remove_import(&text, span)],
                        true,
                    ));
                }
            } else if let Some(name) = msg
                .strip_prefix("unknown name `")
                .or_else(|| msg.strip_prefix("unknown variable `"))
                .and_then(|r| r.strip_suffix('`'))
            {
                // names in scope (or imported) that are close to the misspelt one
                if let Some(a) = self.analyses.get(&path)
                    && let Some(fid) = a.file_id(&path)
                {
                    let off = pos_to_offset(&text, d.range.start);
                    let mut cands: Vec<(usize, String)> =
                        super::complete::visible_names(a, fid, off)
                            .into_iter()
                            .chain(a.imports.keys().cloned())
                            .filter(|n| n != name)
                            .map(|n| (levenshtein(name, &n), n))
                            .filter(|(dist, n)| *dist <= (name.len().max(n.len()) / 3).max(1))
                            .collect();
                    cands.sort();
                    cands.dedup();
                    let s = pos_to_offset(&text, d.range.start);
                    if let Some(i) = text[s..pos_to_offset(&text, d.range.end)].find(name) {
                        let r = Range::new(
                            offset_to_pos(&text, s + i),
                            offset_to_pos(&text, s + i + name.len()),
                        );
                        // a module used as `name.x` that is not imported yet
                        let used_as_module = text[s + i + name.len()..].starts_with('.');
                        if used_as_module {
                            for m in find_modules(&path, name).into_iter().take(3) {
                                out.push(action(
                                    format!("Import `{m}` as `{name}`"),
                                    CodeActionKind::QUICKFIX,
                                    Some(d),
                                    vec![add_import(&text, &module, &m, name)],
                                    true,
                                ));
                            }
                        }
                        // a call to a function that does not exist yet
                        if text[s + i + name.len()..].starts_with('(')
                            && let Some(x) =
                                self.generate_function(a, fid, &module, &text, s + i, name, &uri)
                        {
                            let mut x = x;
                            if let CodeActionOrCommand::CodeAction(ca) = &mut x {
                                ca.diagnostics = Some(vec![d.clone()]);
                            }
                            out.push(x);
                        }
                        for (k, (_, c)) in cands.into_iter().take(3).enumerate() {
                            out.push(action(
                                format!("Change to `{c}`"),
                                CodeActionKind::QUICKFIX,
                                Some(d),
                                vec![TextEdit::new(r, c)],
                                k == 0 && !used_as_module,
                            ));
                        }
                    }
                }
            } else if msg.contains("requires the enclosing function to be throwing") {
                // the innermost function around the error with a return type
                let off = pos_to_offset(&text, d.range.start) as u32;
                let mut best: Option<&ast::FnSig> = None;
                walk(&module, &mut |n| {
                    if let Node::Fn(sig) = n
                        && sig.span.start <= off
                        && best.is_none_or(|b| b.span.start < sig.span.start)
                    {
                        best = Some(sig);
                    }
                });
                if let Some(sig) = best {
                    let (at, ins) = match &sig.ret {
                        Some(r) => (r.span().end as usize, "!".to_string()),
                        None => {
                            // `fn f(...)` -> `fn f(...) void!` (the span covers the name)
                            let from = sig.span.end as usize;
                            let brace = text[from..].find('{').map(|i| from + i);
                            let close =
                                brace.and_then(|b| text[from..b].rfind(')').map(|i| from + i));
                            match close {
                                Some(c) => (c + 1, " void!".to_string()),
                                None => (sig.span.end as usize, String::new()),
                            }
                        }
                    };
                    if !ins.is_empty() {
                        out.push(action(
                            format!("Make `{}` throwing", sig.name),
                            CodeActionKind::QUICKFIX,
                            Some(d),
                            vec![insert(offset_to_pos(&text, at), &ins)],
                            true,
                        ));
                    }
                }
            } else if msg.starts_with("missing field `") && msg.contains("` in `") {
                if let Some(a) = fresh.map(|f| f.0)
                    && let Some(edit) = self.add_missing_fields(a, &module, &text, d)
                {
                    out.push(action(
                        "Add missing fields".into(),
                        CodeActionKind::QUICKFIX,
                        Some(d),
                        vec![edit],
                        true,
                    ));
                }
            } else if msg.starts_with("`if` branches are not exhaustive") {
                let off = pos_to_offset(&text, d.range.start) as u32;
                let mut target: Option<&ast::IfExpr> = None;
                walk(&module, &mut |n| {
                    if let Node::Expr(e) = n
                        && let E::If(i) = &e.kind
                        && i.span.start <= off
                        && off <= i.span.end
                        && target.is_none_or(|t| t.span.start <= i.span.start)
                    {
                        target = Some(i);
                    }
                });
                if let Some(ife) = target {
                    let arm_indent = ife
                        .arms
                        .first()
                        .map(|a| indent_at(&text, a.span.start as usize).to_string())
                        .unwrap_or_else(|| {
                            format!("{}  ", indent_at(&text, ife.span.start as usize))
                        });
                    let end = ife.span.end as usize;
                    let close = text[..end].rfind('}').unwrap_or(end);
                    let at = line_start(&text, close);
                    let at = if text[at..close].trim().is_empty() {
                        at
                    } else {
                        close
                    };
                    let lead = if at == close { "\n" } else { "" };
                    let missing = fresh.and_then(|(a, ..)| self.missing_variants(a, ife, &text));
                    if let Some(vs) = missing.filter(|v| !v.is_empty()) {
                        let arms: String = vs
                            .iter()
                            .map(|v| format!("{lead}{arm_indent}{v} -> {{}}\n"))
                            .collect();
                        out.push(action(
                            "Add missing branches".into(),
                            CodeActionKind::QUICKFIX,
                            Some(d),
                            vec![insert(offset_to_pos(&text, at), &arms)],
                            true,
                        ));
                    }
                    out.push(action(
                        "Add a fallback `_` branch".into(),
                        CodeActionKind::QUICKFIX,
                        Some(d),
                        vec![insert(
                            offset_to_pos(&text, at),
                            &format!("{lead}{arm_indent}_ -> {{}}\n"),
                        )],
                        false,
                    ));
                }
            }
        }

        // remove every unused import
        let unused = unused_imports(&text);
        if !unused.is_empty() {
            let mut edits: Vec<TextEdit> = Vec::new();
            for (span, ..) in &unused {
                let e = remove_import(&text, *span);
                if !edits.iter().any(|x| x.range == e.range) {
                    edits.push(e);
                }
            }
            // whole-block removals cover their entries
            let whole: Vec<Range> = edits
                .iter()
                .filter(|e| e.range.end.character == 0 && e.range.end.line > e.range.start.line)
                .map(|e| e.range)
                .collect();
            edits.retain(|e| {
                !whole.iter().any(|w| {
                    *w != e.range
                        && (w.start.line, w.start.character)
                            <= (e.range.start.line, e.range.start.character)
                        && (e.range.end.line, e.range.end.character)
                            <= (w.end.line, w.end.character)
                })
            });
            if edits.len() == 1 || unused.len() > 1 {
                out.push(action(
                    "Remove unused imports".into(),
                    CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
                    None,
                    edits,
                    false,
                ));
            }
        }

        // refactors at the selection or cursor
        if let Some((a, fid, ftext)) = fresh {
            let (s, e) = (
                pos_to_offset(ftext, p.range.start) as u32,
                pos_to_offset(ftext, p.range.end) as u32,
            );
            if let Some(x) = self.extract_variable(a, fid, &module, ftext, s, e, &uri) {
                out.push(x);
            }
            if let Some(x) = self.inline_variable(a, fid, &module, ftext, s, &uri) {
                out.push(x);
            }
            if let Some(x) = self.match_on_value(a, fid, &module, ftext, s, &uri) {
                out.push(x);
            }
        }
        out.extend(self.more_actions(&p, &module, &text));
        Some(out)
    }

    /// The `.field = value` entries missing from the struct literal of a diagnostic.
    fn add_missing_fields(
        &self,
        a: &Analysis,
        module: &ast::Module,
        text: &str,
        d: &Diagnostic,
    ) -> Option<TextEdit> {
        let off = pos_to_offset(text, d.range.start) as u32;
        let mut lit: Option<&ast::Expr> = None;
        walk(module, &mut |n| {
            if let Node::Expr(e) = n
                && let E::Struct(..) = &e.kind
                && e.span.start <= off
                && off < e.span.end
                && lit.is_none_or(|l| l.span.start <= e.span.start)
            {
                lit = Some(e);
            }
        });
        let lit = lit?;
        let E::Struct(tyx, fields) = &lit.kind else {
            return None;
        };
        // the literal failed to check, so it has no recorded type: find the struct by name
        let mut t = tyx.as_ref();
        if let E::Generic(inner, _) = &t.kind {
            t = inner;
        }
        let name = match &t.kind {
            E::Ident(n) | E::Field(_, n) => n,
            _ => return None,
        };
        let decl = a
            .index
            .decls
            .iter()
            .filter(|d| &d.name == name && d.kind == DeclKind::Struct)
            .max_by_key(|d| d.span.file == lit.span.file)?;
        let members = a.index.types.get(decl.ty.as_ref()?)?;
        let present: HashSet<&str> = fields.iter().map(|f| f.0.as_str()).collect();
        let missing: Vec<String> = members
            .iter()
            .filter(|m| !present.contains(m.name.as_str()))
            .map(|m| format!(".{} = {}", m.name, default_value(&m.ty)))
            .collect();
        if missing.is_empty() {
            return None;
        }
        let end = lit.span.end as usize;
        let close = text[..end].rfind('}')?;
        let inner = text[..close].trim_end();
        let multiline = text[lit.span.start as usize..close].contains('\n');
        if multiline {
            let indent = fields
                .first()
                .map(|f| indent_at(text, f.2.start as usize).to_string())
                .unwrap_or_else(|| format!("{}  ", indent_at(text, lit.span.start as usize)));
            let comma = if inner.ends_with(',') || inner.ends_with('{') {
                ""
            } else {
                ","
            };
            let body: Vec<String> = missing.iter().map(|m| format!("{indent}{m},")).collect();
            let at = inner.len();
            return Some(TextEdit::new(
                Range::new(offset_to_pos(text, at), offset_to_pos(text, at)),
                format!("{comma}\n{}", body.join("\n")),
            ));
        }
        let sep = if inner.ends_with('{') { "" } else { ", " };
        let at = inner.len();
        Some(TextEdit::new(
            Range::new(offset_to_pos(text, at), offset_to_pos(text, at)),
            format!("{sep}{}", missing.join(", ")),
        ))
    }

    /// Variants of the enum matched by an `if` that no branch names.
    fn missing_variants(&self, a: &Analysis, ife: &ast::IfExpr, text: &str) -> Option<Vec<String>> {
        let scrut = ife.scrutinee.as_ref()?;
        let ty = a
            .index
            .exprs
            .iter()
            .find(|x| x.span.start == scrut.span.start && x.span.end == scrut.span.end)?;
        let def = ty.type_def?;
        let decl = a.decl(def)?;
        if decl.kind != DeclKind::Enum || ty.ty.ends_with("[]") {
            return None;
        }
        // variant names a branch matches with `Enum.Variant` or `Enum.Variant(...)`
        let mut named = HashSet::new();
        for arm in &ife.arms {
            for p in &arm.patterns {
                let mut pe = p;
                if let E::Call(c, _) = &pe.kind {
                    pe = c;
                }
                if let E::Field(_, v) = &pe.kind {
                    named.insert(v.clone());
                }
                if matches!(p.kind, E::Wildcard) {
                    return Some(vec![]);
                }
            }
        }
        // the enum's name as written in the scrutinee's module (`Status`, `m.Status`)
        let prefix = ife
            .arms
            .iter()
            .flat_map(|a| &a.patterns)
            .find_map(|p| {
                let pe = if let E::Call(c, _) = &p.kind { c } else { p };
                if let E::Field(b, _) = &pe.kind {
                    Some(text[b.span.start as usize..b.span.end as usize].to_string())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| decl.name.clone());
        Some(
            decl.members
                .iter()
                .filter(|(m, _)| !named.contains(m))
                .map(|(m, detail)| {
                    let arity = detail
                        .split_once('(')
                        .map(|(_, ps)| ps.trim_end_matches(')').split(',').count())
                        .unwrap_or(0);
                    if arity == 0 {
                        format!("{prefix}.{m}")
                    } else {
                        format!("{prefix}.{m}({})", vec!["_"; arity].join(", "))
                    }
                })
                .collect(),
        )
    }

    /// "Extract into a variable" for a selection that is exactly a typed expression.
    #[allow(clippy::too_many_arguments, clippy::mutable_key_type)]
    fn extract_variable(
        &self,
        a: &Analysis,
        fid: u32,
        module: &ast::Module,
        text: &str,
        s: u32,
        e: u32,
        uri: &Uri,
    ) -> Option<CodeActionOrCommand> {
        if s >= e {
            return None;
        }
        let x = a
            .index
            .exprs
            .iter()
            .find(|x| x.span.file == fid && x.span.start == s && x.span.end == e)?;
        if matches!(x.ty.as_str(), "void" | "never") || x.ty.starts_with("fn") {
            return None;
        }
        // the innermost statement around the selection that sits directly in a block
        let mut stmt: Option<&ast::Stmt> = None;
        let mut in_block: HashSet<(u32, u32)> = HashSet::new();
        walk(module, &mut |n| {
            if let Node::Block(b) = n {
                for st in &b.stmts {
                    in_block.insert((st.span.start, st.span.end));
                }
            }
        });
        for it in &module.items {
            if let ast::Item::Stmt(st) = it {
                in_block.insert((st.span.start, st.span.end));
            }
        }
        walk(module, &mut |n| {
            if let Node::Stmt(st) = n
                && st.span.start <= s
                && e <= st.span.end
                && in_block.contains(&(st.span.start, st.span.end))
                && stmt.is_none_or(|o| o.span.start <= st.span.start)
            {
                stmt = Some(st);
            }
        });
        let st = stmt?;
        // a pattern, or a condition evaluated on every loop iteration, cannot be extracted
        if let S::While { cond, .. } = &st.kind
            && cond.span.start <= s
            && e <= cond.span.end
        {
            return None;
        }
        let taken: HashSet<String> = super::complete::visible_names(a, fid, s as usize)
            .into_iter()
            .collect();
        let mut name = "value".to_string();
        let mut k = 2;
        while taken.contains(&name) {
            name = format!("value{k}");
            k += 1;
        }
        let at = line_start(text, st.span.start as usize);
        let indent = indent_at(text, st.span.start as usize);
        let expr = &text[s as usize..e as usize];
        let decl = format!("{indent}{} {name} = {expr}\n", x.ty);
        let mut changes = HashMap::new();
        changes.insert(
            uri.clone(),
            vec![
                TextEdit::new(
                    Range::new(offset_to_pos(text, at), offset_to_pos(text, at)),
                    decl,
                ),
                TextEdit::new(
                    Range::new(
                        offset_to_pos(text, s as usize),
                        offset_to_pos(text, e as usize),
                    ),
                    name,
                ),
            ],
        );
        Some(CodeActionOrCommand::CodeAction(CodeAction {
            title: "Extract into a variable".into(),
            kind: Some(CodeActionKind::REFACTOR_EXTRACT),
            edit: Some(WorkspaceEdit {
                changes: Some(changes),
                ..Default::default()
            }),
            ..Default::default()
        }))
    }

    /// "Generate function" for a call to an unknown name: a function after the current item,
    /// with parameters typed from the arguments and the return type from the context.
    #[allow(clippy::too_many_arguments, clippy::mutable_key_type)]
    fn generate_function(
        &self,
        a: &Analysis,
        fid: u32,
        module: &ast::Module,
        text: &str,
        at: usize,
        name: &str,
        uri: &Uri,
    ) -> Option<CodeActionOrCommand> {
        let at = at as u32;
        let mut call: Option<&ast::Expr> = None;
        walk(module, &mut |n| {
            if let Node::Expr(e) = n
                && let E::Call(c, _) = &e.kind
                && matches!(&c.kind, E::Ident(n) if n == name)
                && c.span.start == at
            {
                call = Some(e);
            }
        });
        let call = call?;
        let E::Call(_, args) = &call.kind else {
            return None;
        };
        let visible = super::complete::visible_decls(a, fid, at as usize);
        let mut params = Vec::new();
        let mut used = HashSet::new();
        for (i, arg) in args.iter().enumerate() {
            let ty = match &arg.kind {
                E::Int(..) => Some("int".to_string()),
                E::Float(_) => Some("float".into()),
                E::Str(_) => Some("str".into()),
                E::Bool(_) => Some("bool".into()),
                E::Char(_) => Some("char".into()),
                E::Ident(n) => visible
                    .iter()
                    .find(|d| &d.name == n)
                    .and_then(|d| d.ty.clone()),
                _ => None,
            }
            .or_else(|| {
                a.index
                    .exprs
                    .iter()
                    .find(|x| x.span.start == arg.span.start && x.span.end == arg.span.end)
                    .map(|x| x.ty.clone())
            })
            .unwrap_or_else(|| "int".into());
            let mut pname = match &arg.kind {
                E::Ident(n) => n.clone(),
                E::Field(_, n) => n.clone(),
                _ => format!("arg{}", i + 1),
            };
            while !used.insert(pname.clone()) {
                pname.push('_');
            }
            params.push(format!("{ty} {pname}"));
        }
        // the return type: the declared type of `T x = name(...)`, or the enclosing function's
        // return type for `return name(...)`
        let mut ret: Option<String> = None;
        walk(module, &mut |n| {
            if let Node::Stmt(st) = n
                && let S::VarDecl { targets, value, .. } = &st.kind
                && targets.len() == 1
                && value.span == call.span
            {
                let t = targets[0].ty.span();
                ret = Some(text[t.start as usize..t.end as usize].to_string());
            }
        });
        if ret.is_none() {
            // `return name(...)` inside a function with a return type
            let mut fsig: Option<&ast::FnSig> = None;
            let mut is_return = false;
            walk(module, &mut |n| match n {
                Node::Fn(sig) if sig.span.start <= at => {
                    if fsig.is_none_or(|f| f.span.start < sig.span.start) {
                        fsig = Some(sig);
                    }
                }
                Node::Stmt(st) => {
                    if let S::Return(Some(v)) = &st.kind
                        && v.span == call.span
                    {
                        is_return = true;
                    }
                }
                _ => {}
            });
            if is_return && let Some(r) = fsig.and_then(|f| f.ret.as_ref()) {
                let sp = r.span();
                ret = Some(text[sp.start as usize..sp.end as usize].to_string());
            }
        }
        // after the top-level item containing the call
        let mut item_end = text.len();
        for it in &module.items {
            let mut lo = u32::MAX;
            let mut hi = 0;
            super::nav::item_spans(it, &mut |n| {
                if let Node::Span(sp) = n {
                    lo = lo.min(sp.start);
                    hi = hi.max(sp.end);
                }
            });
            if lo <= at && at <= hi {
                item_end = line_end(text, hi as usize);
            }
        }
        let ret = ret.map(|r| format!(" {r}")).unwrap_or_default();
        let lead = if text[..item_end].ends_with('\n') {
            ""
        } else {
            "\n"
        };
        let body = format!("{lead}\nfn {name}({}){ret} {{\n}}\n", params.join(", "));
        let p = offset_to_pos(text, item_end);
        let mut changes = HashMap::new();
        changes.insert(uri.clone(), vec![TextEdit::new(Range::new(p, p), body)]);
        Some(CodeActionOrCommand::CodeAction(CodeAction {
            title: format!("Generate function `{name}`"),
            kind: Some(CodeActionKind::QUICKFIX),
            edit: Some(WorkspaceEdit {
                changes: Some(changes),
                ..Default::default()
            }),
            ..Default::default()
        }))
    }

    /// "Match on `x`" for a variable of an enum type: an `if` with a branch per variant after the
    /// statement at the cursor.
    #[allow(clippy::mutable_key_type)]
    fn match_on_value(
        &self,
        a: &Analysis,
        fid: u32,
        module: &ast::Module,
        text: &str,
        off: u32,
        uri: &Uri,
    ) -> Option<CodeActionOrCommand> {
        let r = a
            .index
            .refs
            .iter()
            .filter(|r| r.span.file == fid && r.span.start <= off && off <= r.span.end)
            .min_by_key(|r| r.span.end - r.span.start)?;
        let d = a.decl(r.def?)?;
        if d.kind != DeclKind::Variable {
            return None;
        }
        let edecl = a.decl(r.type_def?)?;
        // only plain enum values (not arrays or optionals of them)
        if edecl.kind != DeclKind::Enum || d.ty.as_deref() != Some(edecl.name.as_str()) {
            return None;
        }
        // the enum as written from this file
        let prefix = if edecl.span.file == fid {
            edecl.name.clone()
        } else {
            let epath = &a.sm.file(edecl.span.file).path;
            let alias = a.imports.iter().find(|(_, p)| *p == epath)?.0;
            format!("{alias}.{}", edecl.name)
        };
        // the statement around the cursor that sits directly in a block or at the top level
        let mut in_block: HashSet<(u32, u32)> = HashSet::new();
        walk(module, &mut |n| {
            if let Node::Block(b) = n {
                for st in &b.stmts {
                    in_block.insert((st.span.start, st.span.end));
                }
            }
        });
        for it in &module.items {
            if let ast::Item::Stmt(st) = it {
                in_block.insert((st.span.start, st.span.end));
            }
        }
        let mut stmt: Option<&ast::Stmt> = None;
        walk(module, &mut |n| {
            if let Node::Stmt(st) = n
                && st.span.start <= off
                && off <= st.span.end
                && in_block.contains(&(st.span.start, st.span.end))
                && stmt.is_none_or(|o| o.span.start <= st.span.start)
            {
                stmt = Some(st);
            }
        });
        let st = stmt?;
        let indent = indent_at(text, st.span.start as usize).to_string();
        let mut body = format!("{indent}if {} {{\n", d.name);
        for (m, detail) in &edecl.members {
            let arity = detail
                .split_once('(')
                .map(|(_, ps)| ps.trim_end_matches(')').split(',').count())
                .unwrap_or(0);
            let pat = if arity == 0 {
                format!("{prefix}.{m}")
            } else {
                format!("{prefix}.{m}({})", vec!["_"; arity].join(", "))
            };
            body.push_str(&format!("{indent}  {pat} -> {{}}\n"));
        }
        body.push_str(&format!("{indent}}}\n"));
        let at = line_end(text, st.span.end as usize);
        let lead = if text[..at].ends_with('\n') { "" } else { "\n" };
        let p = offset_to_pos(text, at);
        let mut changes = HashMap::new();
        changes.insert(
            uri.clone(),
            vec![TextEdit::new(Range::new(p, p), format!("{lead}{body}"))],
        );
        Some(CodeActionOrCommand::CodeAction(CodeAction {
            title: format!("Match on `{}`", d.name),
            kind: Some(CodeActionKind::REFACTOR_REWRITE),
            edit: Some(WorkspaceEdit {
                changes: Some(changes),
                ..Default::default()
            }),
            ..Default::default()
        }))
    }

    /// "Inline variable" on an immutable variable declared with a single name.
    #[allow(clippy::mutable_key_type)]
    fn inline_variable(
        &self,
        a: &Analysis,
        fid: u32,
        module: &ast::Module,
        text: &str,
        off: u32,
        uri: &Uri,
    ) -> Option<CodeActionOrCommand> {
        let r = a
            .index
            .refs
            .iter()
            .filter(|r| r.span.file == fid && r.span.start <= off && off <= r.span.end)
            .min_by_key(|r| r.span.end - r.span.start)?;
        let def = r.def?;
        let d = a.decl(def)?;
        if d.kind != DeclKind::Variable || d.mutable || def.file != fid {
            return None;
        }
        // its declaration statement
        let mut decl: Option<(&ast::Stmt, &ast::Expr)> = None;
        walk(module, &mut |n| {
            if let Node::Stmt(st) = n
                && let S::VarDecl {
                    targets,
                    value,
                    mutex: false,
                    public: false,
                    ..
                } = &st.kind
                && targets.len() == 1
                && targets[0].span == def
            {
                decl = Some((st, value));
            }
        });
        if decl.is_none() {
            for it in &module.items {
                if let ast::Item::Stmt(st) = it
                    && let S::VarDecl {
                        targets,
                        value,
                        mutex: false,
                        public: false,
                        ..
                    } = &st.kind
                    && targets.len() == 1
                    && targets[0].span == def
                {
                    decl = Some((st, value));
                }
            }
        }
        let (st, value) = decl?;
        let uses: Vec<Span> = {
            let mut seen = HashSet::new();
            a.index
                .refs
                .iter()
                .filter(|r| r.def == Some(def) && r.span != def && seen.insert(r.span))
                .map(|r| r.span)
                .collect()
        };
        // used from another file (a module-level variable): leave it
        if uses.is_empty() || uses.iter().any(|u| u.file != fid) {
            return None;
        }
        let vtext = &text[value.span.start as usize..value.span.end as usize];
        let replacement = if atomic(value) {
            vtext.to_string()
        } else {
            format!("({vtext})")
        };
        let mut edits: Vec<TextEdit> = uses
            .iter()
            .map(|u| TextEdit::new(span_range(text, *u), replacement.clone()))
            .collect();
        let (ls, le) = (
            line_start(text, st.span.start as usize),
            line_end(text, st.span.end as usize),
        );
        edits.push(TextEdit::new(
            Range::new(offset_to_pos(text, ls), offset_to_pos(text, le)),
            String::new(),
        ));
        let mut changes = HashMap::new();
        changes.insert(uri.clone(), edits);
        Some(CodeActionOrCommand::CodeAction(CodeAction {
            title: format!("Inline `{}`", d.name),
            kind: Some(CodeActionKind::REFACTOR_INLINE),
            edit: Some(WorkspaceEdit {
                changes: Some(changes),
                ..Default::default()
            }),
            ..Default::default()
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_unused_imports() {
        let src = "import {\n  \"./a\" as a\n  \"./b\" as b\n}\nb.f()\n";
        let u = unused_imports(src);
        assert_eq!(u.len(), 1);
        assert_eq!(u[0].2, "a");
        let e = remove_import(src, u[0].0);
        assert_eq!(
            e.range,
            Range::new(Position::new(1, 0), Position::new(2, 0))
        );
        let src = "import { \"./a\" as a }\n@println(1)\n";
        let u = unused_imports(src);
        let e = remove_import(src, u[0].0);
        assert_eq!(
            e.range,
            Range::new(Position::new(0, 0), Position::new(1, 0))
        );
        // used in a string interpolation
        assert!(unused_imports("import { \"./a\" as a }\n@println(\"{a.x}\")\n").is_empty());
    }

    #[test]
    fn edit_distance() {
        assert_eq!(levenshtein("lenght", "length"), 2);
        assert_eq!(levenshtein("a", "a"), 0);
    }
}
