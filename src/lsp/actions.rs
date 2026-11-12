//! Code actions, and the warnings the language server adds to the compiler's diagnostics
//! (unused variables and imports).
//!
//! Quick fixes: make a variable mutable, discard an unused value, rename an unused variable to
//! `_`, remove an unused import, fix a misspelt name, make a function throwing, add missing
//! struct fields, add missing `if` branches.
//! Refactors: extract an expression into a variable, inline a variable.
//! Source actions: remove all unused imports.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use lsp_types::*;

use super::{diag_code, diag_data};
use crate::diag::codes;
use crate::types::TyKind;

use super::ssr::atomic;
use super::{
    Analysis, Server, edit_action, indent_at, insert_at, last_node, line_end, line_start,
    offset_to_pos, pos_to_offset, range, span_range,
};
use crate::ast::visit::{Node, walk};
use crate::ast::{self, ExprKind as E, StmtKind as S};
use crate::check::index::DeclKind;
use crate::diag::Span;

const UNUSED_IMPORT: &str = "unused-import";
const UNREACHABLE_BRANCH: &str = "unreachable-branch";

// ------------------------------------------------------------------ warnings

/// Imports of a module whose alias is never used: (import span, alias name span, alias).
fn unused_imports(text: &str, module: &ast::Module) -> Vec<(Span, Span, String)> {
    let (toks, _) = crate::lexer::lex_all(text, 0, 0);
    let mut used = HashSet::new();
    super::nav::qualifier_uses(&toks, &mut |n, _| {
        used.insert(n.to_string());
    });
    let mut out = Vec::new();
    for it in &module.items {
        if let ast::Item::Import(im) = it
            && !used.contains(&im.alias)
        {
            out.push((im.span, im.alias_span, im.alias.clone()));
        }
    }
    out
}

/// `if` branches that can never run, because earlier branches match everything they match:
/// branches after a catch-all (`_`, a binding, or the condition `true`), and branches whose
/// patterns all appeared in earlier branches. Returns the spans of the branches.
fn unreachable_arms(a: &Analysis, text: &str, module: &ast::Module) -> Vec<Span> {
    let mut out = Vec::new();
    walk(module, &mut |n| {
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
pub(super) fn warnings(
    a: &Analysis,
    entry: &Path,
    module: &ast::Module,
) -> Vec<(PathBuf, Diagnostic)> {
    let Some(fid) = a.file_id(entry) else {
        return vec![];
    };
    let warn = |d: crate::diag::Diag, code| {
        let tags = Some(vec![DiagnosticTag::UNNECESSARY]);
        super::to_diagnostic(&a.sm, d, DiagnosticSeverity::WARNING, Some(code), tags)
    };
    let mut out = Vec::new();
    // unused code, as the compiler reports it (a warning in debug builds, an error in release)
    if a.clean {
        for u in crate::lint::unused(&a.index, &a.types) {
            if u.diag.span.file == fid {
                out.push(warn(u.diag, u.kind.code()));
            }
        }
    }
    let msg = "unreachable branch: earlier branches match every value it matches";
    for arm in unreachable_arms(a, &a.sm.file(fid).text, module) {
        out.push(warn(crate::diag::Diag::new(arm, msg), UNREACHABLE_BRANCH));
    }
    out
}

// ------------------------------------------------------------------ helpers

/// Edits removing an import entry: the whole `import { ... }` when it is the only entry, its line
/// when it is alone on it, or the entry and its separating comma.
fn remove_import(text: &str, span: Span) -> TextEdit {
    let (s, e) = (span.start as usize, span.end as usize);
    let open = text[..s].rfind("import").unwrap_or(s);
    let close = text[e..].find('}').map_or(e, |i| e + i + 1);
    let inner = &text[open..close];
    let entries = inner.matches(" as ").count();
    if entries <= 1 {
        return TextEdit::new(
            range(text, line_start(text, open), line_end(text, close)),
            String::new(),
        );
    }
    let ls = line_start(text, s);
    let le = line_end(text, e);
    let line = text[ls..le].trim();
    let entry = text[s..e].trim();
    if line.trim_end_matches(',') == entry {
        return TextEdit::new(range(text, ls, le), String::new());
    }
    // `a, b` on one line: remove the following comma, or the preceding one for the last entry
    let after = &text[e..];
    let trail = after.len() - after.trim_start_matches(',').trim_start_matches(' ').len();
    if after.starts_with(',') {
        TextEdit::new(range(text, s, e + trail), String::new())
    } else {
        let before = text[..s].trim_end_matches(' ');
        let b = before.strip_suffix(',').map_or(s, str::len);
        TextEdit::new(range(text, b, e), String::new())
    }
}

/// The innermost statement containing `s..e` that sits directly in a block or at the top level.
fn block_stmt_around(module: &ast::Module, s: u32, e: u32) -> Option<&ast::Stmt> {
    let mut in_block: HashSet<(u32, u32)> = (module.items.iter())
        .filter_map(|it| match it {
            ast::Item::Stmt(st) => Some((st.span.start, st.span.end)),
            _ => None,
        })
        .collect();
    walk(module, &mut |n| {
        if let Node::Block(b) = n {
            in_block.extend(b.stmts.iter().map(|st| (st.span.start, st.span.end)));
        }
    });
    let (start, end) = (s, e);
    last_node(module, |n| match n {
        Node::Stmt(st)
            if st.span.start <= start
                && end <= st.span.end
                && in_block.contains(&(st.span.start, st.span.end)) =>
        {
            Some(st)
        }
        _ => None,
    })
}

/// Modules the files of `dir` could import, as (name, import path): files of `dir` (`./name`),
/// other project files (`.../dir/name`) and dependencies (`std/name`), best first. Also returns
/// the directories scanned.
pub(super) fn importable_modules(dir: &Path) -> (Vec<(String, String)>, Vec<PathBuf>) {
    let mut out = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    let stem = |f: &Path| f.file_stem().map(|n| n.to_string_lossy().into_owned());
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let f = e.path();
            if f.extension().is_some_and(|x| x == "nc") && f.is_file() {
                let n = stem(&f).unwrap_or_default();
                out.push((n.clone(), format!("./{n}")));
            }
        }
    }
    // dependency modules in `scan`, imported by their path from `base`
    fn add_deps(
        scan: &Path,
        base: &Path,
        out: &mut Vec<(String, String)>,
        dirs: &mut Vec<PathBuf>,
    ) {
        let mut files = Vec::new();
        super::collect_nc_files(scan, true, &mut files, dirs, 0);
        for f in files {
            if let Ok(rel) = f.with_extension("").strip_prefix(base) {
                let name = f.file_stem().unwrap_or_default().to_string_lossy();
                out.push((name.into_owned(), rel.to_string_lossy().into_owned()));
            }
        }
    }
    if let Some(home) = crate::modules::nc_home() {
        for lib in crate::modules::SYSTEM_LIBS {
            add_deps(&home.join(lib), &home, &mut out, &mut dirs);
        }
    }
    if let Some(root) = crate::modules::find_root(dir) {
        let mut files = Vec::new();
        super::collect_nc_files(&root, false, &mut files, &mut dirs, 0);
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
        add_deps(&deps, &deps, &mut out, &mut dirs);
    }
    out.sort_by_key(|(n, p)| {
        (
            n.clone(),
            !p.starts_with("./"),
            p.starts_with(".../"),
            p.clone(),
        )
    });
    out.retain(|(n, _)| super::valid_name(n));
    (out, dirs)
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
            let close = text[e..].find('}').map_or(e, |i| e + i);
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
fn default_value(types: &crate::types::Types, ty: crate::types::TyId) -> String {
    match types.kind(types.base(ty)) {
        TyKind::Int | TyKind::UInt | TyKind::Byte => "0".into(),
        TyKind::Float => "0.0".into(),
        TyKind::Bool => "false".into(),
        TyKind::Str => "\"\"".into(),
        TyKind::Char => "' '".into(),
        TyKind::Opt(_) => "none".into(),
        TyKind::Array(_) => "[]".into(),
        TyKind::Map(..) => "[:]".into(),
        _ => "_".into(),
    }
}

// ------------------------------------------------------------------ code actions

impl Server {
    pub(super) fn code_actions(&self, p: CodeActionParams) -> Option<CodeActionResponse> {
        let uri = p.text_document.uri.clone();
        let (path, text) = self.source(&uri)?;
        let mut out = Vec::new();
        let insert = |at: Position, s: &str| TextEdit::new(Range::new(at, at), s.to_string());
        let module = self.module_of(&path, &text);
        let fresh = self.fresh(&uri);

        for d in &p.context.diagnostics {
            let code = diag_code(d);
            // a quick fix for the diagnostic
            let fix = |title: &str, edit, preferred: bool| {
                let (kind, edits) = (CodeActionKind::QUICKFIX, vec![edit]);
                let mut a = edit_action(title.into(), kind, &uri, edits, Some(d));
                if let CodeActionOrCommand::CodeAction(c) = &mut a {
                    c.is_preferred = preferred.then_some(true);
                }
                a
            };
            let (s, e) = (
                pos_to_offset(&text, d.range.start),
                pos_to_offset(&text, d.range.end),
            );
            match code {
                codes::IMMUTABLE => {
                    if let Some((a, r)) = self.lookup(&uri, d.range.start)
                        && let Some(def) = r.def
                        && a.sm.file(def.file).path == path
                    {
                        let edit = insert_at(&text, def.start as usize, "mut ");
                        out.push(fix("Make the variable mutable", edit, true));
                    }
                }
                codes::UNUSED_VALUE => {
                    let edit = insert(d.range.start, "_ = ");
                    out.push(fix("Discard the value with `_ =`", edit, true));
                }
                "unused-variable" | "unused-parameter" | "unused-function" | "unused-type" => {
                    let edit = insert(d.range.start, "_");
                    out.push(fix("Prefix the name with `_`", edit, true));
                    if matches!(code, "unused-variable" | "unused-parameter") {
                        let edit = TextEdit::new(d.range, "_".into());
                        out.push(fix("Rename to `_`", edit, false));
                    }
                }
                UNREACHABLE_BRANCH => {
                    // the whole lines when the branch is alone on them
                    let (ls, le) = (line_start(&text, s), line_end(&text, e));
                    let alone = text[ls..s].trim().is_empty() && text[e..le].trim().is_empty();
                    let (s, e) = if alone { (ls, le) } else { (s, e) };
                    let edit = TextEdit::new(range(&text, s, e), String::new());
                    out.push(fix("Remove the unreachable branch", edit, true));
                }
                UNUSED_IMPORT => {
                    let off = s as u32;
                    if let Some((span, ..)) = unused_imports(&text, &module)
                        .into_iter()
                        .find(|(sp, ..)| sp.start <= off && off <= sp.end)
                    {
                        let edit = remove_import(&text, span);
                        out.push(fix("Remove unused import", edit, true));
                    }
                }
                codes::UNKNOWN_NAME => {
                    // names in scope (or imported) that are close to the misspelt one
                    if let Some(name) = diag_data(d, 0)
                        && let Some(a) = self.analyses.get(&path)
                        && let Some(fid) = a.file_id(&path)
                    {
                        let mut cands: Vec<(usize, String)> =
                            super::complete::visible_names(a, fid, s)
                                .into_iter()
                                .chain(a.imports.keys().cloned())
                                .filter(|n| n != name)
                                .map(|n| (levenshtein(name, &n), n))
                                .filter(|(dist, n)| *dist <= (name.len().max(n.len()) / 3).max(1))
                                .collect();
                        cands.sort();
                        cands.dedup();
                        if let Some(i) = text[s..e].find(name) {
                            let r = range(&text, s + i, s + i + name.len());
                            // a module used as `name.x` that is not imported yet
                            let used_as_module = text[s + i + name.len()..].starts_with('.');
                            if used_as_module {
                                for m in self.find_modules(&path, name).into_iter().take(3) {
                                    let edit = add_import(&text, &module, &m, name);
                                    out.push(fix(&format!("Import `{m}` as `{name}`"), edit, true));
                                }
                            }
                            // a call to a function that does not exist yet
                            if text[s + i + name.len()..].starts_with('(')
                                && let Some(mut x) =
                                    generate_function(a, fid, &module, &text, s + i, name, &uri)
                            {
                                if let CodeActionOrCommand::CodeAction(ca) = &mut x {
                                    ca.diagnostics = Some(vec![d.clone()]);
                                }
                                out.push(x);
                            }
                            for (k, (_, c)) in cands.into_iter().take(3).enumerate() {
                                let title = format!("Change to `{c}`");
                                let preferred = k == 0 && !used_as_module;
                                out.push(fix(&title, TextEdit::new(r, c), preferred));
                            }
                        }
                    }
                }
                codes::NEEDS_THROWING => {
                    // the innermost function around the error with a return type
                    let best = last_node(&module, |n| match n {
                        Node::Fn(sig) if sig.span.start as usize <= s => Some(sig),
                        _ => None,
                    });
                    if let Some(sig) = best {
                        let (at, ins) = if let Some(r) = &sig.ret {
                            (r.span().end as usize, "!".to_string())
                        } else {
                            // `fn f(...)` -> `fn f(...) void!` (the span covers the name)
                            let from = sig.span.end as usize;
                            let brace = text[from..].find('{').map(|i| from + i);
                            let close =
                                brace.and_then(|b| text[from..b].rfind(')').map(|i| from + i));
                            match close {
                                Some(c) => (c + 1, " void!".to_string()),
                                None => (sig.span.end as usize, String::new()),
                            }
                        };
                        if !ins.is_empty() {
                            let edit = insert_at(&text, at, ins);
                            out.push(fix(&format!("Make `{}` throwing", sig.name), edit, true));
                        }
                    }
                }
                codes::MISSING_FIELD => {
                    if let Some(a) = fresh.map(|f| f.0)
                        && let Some(edit) =
                            add_missing_fields(a, &module, &text, s as u32, default_value)
                    {
                        out.push(fix("Add missing fields", edit, true));
                    }
                }
                codes::NOT_EXHAUSTIVE => {
                    let off = s as u32;
                    let target = last_node(&module, |n| match n {
                        Node::Expr(e) => match &e.kind {
                            E::If(i) if i.span.start <= off && off <= i.span.end => Some(i),
                            _ => None,
                        },
                        _ => None,
                    });
                    if let Some(ife) = target {
                        let arm_indent = ife.arms.first().map_or_else(
                            || format!("{}  ", indent_at(&text, ife.span.start as usize)),
                            |a| indent_at(&text, a.span.start as usize).to_string(),
                        );
                        let end = ife.span.end as usize;
                        let close = text[..end].rfind('}').unwrap_or(end);
                        let at = line_start(&text, close);
                        let at = if text[at..close].trim().is_empty() {
                            at
                        } else {
                            close
                        };
                        let lead = if at == close { "\n" } else { "" };
                        let missing =
                            fresh.and_then(|(a, fid, _)| missing_variants(a, fid, ife, &text));
                        if let Some(vs) = missing.filter(|v| !v.is_empty()) {
                            let arms: String = vs
                                .iter()
                                .map(|v| format!("{lead}{arm_indent}{v} -> {{}}\n"))
                                .collect();
                            let edit = insert_at(&text, at, arms);
                            out.push(fix("Add missing branches", edit, true));
                        }
                        let edit = insert_at(&text, at, format!("{lead}{arm_indent}_ -> {{}}\n"));
                        out.push(fix("Add a fallback `_` branch", edit, false));
                    }
                }
                _ => {}
            }
        }

        // remove every unused import
        let unused = unused_imports(&text, &module);
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
                let r = e.range;
                !whole
                    .iter()
                    .any(|w| *w != r && w.start <= r.start && r.end <= w.end)
            });
            if edits.len() == 1 || unused.len() > 1 {
                out.push(edit_action(
                    "Remove unused imports".into(),
                    CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
                    &uri,
                    edits,
                    None,
                ));
            }
        }

        // refactors at the selection or cursor
        if let Some((a, fid, ftext)) = fresh {
            let (s, e) = (
                pos_to_offset(ftext, p.range.start) as u32,
                pos_to_offset(ftext, p.range.end) as u32,
            );
            out.extend(extract_variable(a, fid, &module, ftext, s, e, &uri));
            out.extend(inline_variable(a, fid, &module, ftext, s, &uri));
            out.extend(match_on_value(a, fid, &module, ftext, s, &uri));
        }
        out.extend(self.more_actions(&p, &module, &text));
        Some(out)
    }
}

/// The `.field = value` entries missing from the struct literal or pattern around `off`, with
/// `value(field type)` as each value.
pub(super) fn add_missing_fields(
    a: &Analysis,
    module: &ast::Module,
    text: &str,
    off: u32,
    value: impl Fn(&crate::types::Types, crate::types::TyId) -> String,
) -> Option<TextEdit> {
    let (lit, tyx, fields) = last_node(module, |n| match n {
        Node::Expr(e) => match &e.kind {
            E::Struct(tyx, fields) if e.span.start <= off && off < e.span.end => {
                Some((e, tyx, fields))
            }
            _ => None,
        },
        _ => None,
    })?;
    // the literal failed to check, so it has no recorded type: find the struct by name
    let mut t = tyx.as_ref();
    if let E::Generic(inner, _) = &t.kind {
        t = inner;
    }
    let (E::Ident(name) | E::Field(_, name)) = &t.kind else {
        return None;
    };
    let decl = a
        .index
        .decls
        .iter()
        .filter(|d| &d.name == name && d.kind == DeclKind::Struct)
        .max_by_key(|d| d.span.file == lit.span.file)?;
    let members = a.members(decl.ty?);
    let present: HashSet<&str> = fields.iter().map(|f| f.0.as_str()).collect();
    let missing: Vec<String> = members
        .iter()
        .filter(|m| !present.contains(m.name.as_str()))
        .map(|m| format!(".{} = {}", m.name, value(&a.types, m.ty)))
        .collect();
    if missing.is_empty() {
        return None;
    }
    let end = lit.span.end as usize;
    let close = text[..end].rfind('}')?;
    let inner = text[..close].trim_end();
    let multiline = text[lit.span.start as usize..close].contains('\n');
    if multiline {
        let indent = fields.first().map_or_else(
            || format!("{}  ", indent_at(text, lit.span.start as usize)),
            |f| indent_at(text, f.2.start as usize).to_string(),
        );
        let comma = if inner.ends_with(',') || inner.ends_with('{') {
            ""
        } else {
            ","
        };
        let body: Vec<String> = missing.iter().map(|m| format!("{indent}{m},")).collect();
        return Some(insert_at(
            text,
            inner.len(),
            format!("{comma}\n{}", body.join("\n")),
        ));
    }
    let sep = if inner.ends_with('{') { "" } else { ", " };
    Some(insert_at(
        text,
        inner.len(),
        format!("{sep}{}", missing.join(", ")),
    ))
}

/// Variants of the enum matched by an `if` that no branch names.
fn missing_variants(a: &Analysis, fid: u32, ife: &ast::IfExpr, text: &str) -> Option<Vec<String>> {
    let scrut = ife.scrutinee.as_ref()?;
    let ty = a.expr_ty(fid, scrut.span.start, scrut.span.end)?;
    let def = ty.type_def?;
    let decl = a.decl(def)?;
    if decl.kind != DeclKind::Enum || matches!(a.types.kind(ty.ty), TyKind::Array(_)) {
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
        (a.variants(decl).into_iter())
            .filter(|(m, _)| !named.contains(m))
            .map(|(m, arity)| super::refactor::variant_pattern(&prefix, &m, arity))
            .collect(),
    )
}

/// "Extract into a variable" for a selection that is exactly a typed expression.
fn extract_variable(
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
    let x = a.expr_ty(fid, s, e)?;
    if matches!(
        a.types.kind(x.ty),
        TyKind::Void | TyKind::Never | TyKind::Fn(..)
    ) {
        return None;
    }
    let st = block_stmt_around(module, s, e)?;
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
    let decl = format!("{indent}{} {name} = {expr}\n", a.show(x.ty));
    Some(edit_action(
        "Extract into a variable".into(),
        CodeActionKind::REFACTOR_EXTRACT,
        uri,
        vec![
            insert_at(text, at, decl),
            TextEdit::new(range(text, s as usize, e as usize), name),
        ],
        None,
    ))
}

/// "Generate function" for a call to an unknown name: a function after the current item,
/// with parameters typed from the arguments and the return type from the context.
fn generate_function(
    a: &Analysis,
    fid: u32,
    module: &ast::Module,
    text: &str,
    at: usize,
    name: &str,
    uri: &Uri,
) -> Option<CodeActionOrCommand> {
    let at = at as u32;
    let (call, args) = last_node(module, |n| match n {
        Node::Expr(e) => match &e.kind {
            E::Call(c, args)
                if c.span.start == at && matches!(&c.kind, E::Ident(n) if n == name) =>
            {
                Some((e, args))
            }
            _ => None,
        },
        _ => None,
    })?;
    let visible = super::complete::visible(a, fid, at as usize);
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
                .and_then(|d| d.ty)
                .map(|t| a.show(t)),
            _ => None,
        }
        .or_else(|| {
            a.expr_ty(fid, arg.span.start, arg.span.end)
                .map(|x| a.show(x.ty))
        })
        .unwrap_or_else(|| "int".into());
        let mut pname = match &arg.kind {
            E::Ident(n) | E::Field(_, n) => n.clone(),
            _ => format!("arg{}", i + 1),
        };
        while !used.insert(pname.clone()) {
            pname.push('_');
        }
        params.push(format!("{ty} {pname}"));
    }
    // the return type: the declared type of `T x = name(...)`, or the enclosing function's
    // return type for `return name(...)`
    fn stmt(n: Node<'_>) -> Option<&S> {
        match n {
            Node::Stmt(st) => Some(&st.kind),
            _ => None,
        }
    }
    let ret = last_node(module, |n| match stmt(n)? {
        S::VarDecl { targets, value, .. } if targets.len() == 1 && value.span == call.span => {
            Some(targets[0].ty.span())
        }
        _ => None,
    })
    .or_else(|| {
        // `return name(...)` inside a function with a return type
        last_node(module, |n| match stmt(n)? {
            S::Return(Some(v)) if v.span == call.span => Some(()),
            _ => None,
        })?;
        let fsig = last_node(module, |n| match n {
            Node::Fn(sig) if sig.span.start <= at => Some(sig),
            _ => None,
        });
        Some(fsig?.ret.as_ref()?.span())
    })
    .map(|sp| &text[sp.start as usize..sp.end as usize]);
    // after the top-level item containing the call
    let item_end = super::refactor::item_extent(module, at)
        .map_or(text.len(), |(_, hi)| line_end(text, hi as usize));
    let ret = ret.map(|r| format!(" {r}")).unwrap_or_default();
    let lead = if text[..item_end].ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let body = format!("{lead}\nfn {name}({}){ret} {{\n}}\n", params.join(", "));
    Some(edit_action(
        format!("Generate function `{name}`"),
        CodeActionKind::QUICKFIX,
        uri,
        vec![insert_at(text, item_end, body)],
        None,
    ))
}

/// "Match on `x`" for a variable of an enum type: an `if` with a branch per variant after the
/// statement at the cursor.
fn match_on_value(
    a: &Analysis,
    fid: u32,
    module: &ast::Module,
    text: &str,
    off: u32,
    uri: &Uri,
) -> Option<CodeActionOrCommand> {
    let r = a.ref_at(fid, off)?;
    let d = a.decl(r.def?)?;
    if d.kind != DeclKind::Variable {
        return None;
    }
    let edecl = a.decl(r.type_def?)?;
    // only plain enum values (not arrays or optionals of them)
    if edecl.kind != DeclKind::Enum
        || d.ty
            .is_none_or(|t| !matches!(a.types.kind(t), TyKind::Enum(_)))
    {
        return None;
    }
    let st = block_stmt_around(module, off, off)?;
    let indent = indent_at(text, st.span.start as usize);
    let body = super::refactor::enum_match(a, fid, edecl, &d.name, indent)?;
    let at = line_end(text, st.span.end as usize);
    let lead = if text[..at].ends_with('\n') { "" } else { "\n" };
    Some(edit_action(
        format!("Match on `{}`", d.name),
        CodeActionKind::REFACTOR_REWRITE,
        uri,
        vec![insert_at(text, at, format!("{lead}{body}"))],
        None,
    ))
}

/// "Inline variable" on an immutable variable declared with a single name.
fn inline_variable(
    a: &Analysis,
    fid: u32,
    module: &ast::Module,
    text: &str,
    off: u32,
    uri: &Uri,
) -> Option<CodeActionOrCommand> {
    let r = a.ref_at(fid, off)?;
    let def = r.def?;
    let d = a.decl(def)?;
    if d.kind != DeclKind::Variable || d.mutable || def.file != fid {
        return None;
    }
    // its declaration statement
    let (st, value) = last_node(module, |n| match n {
        Node::Stmt(st) => match &st.kind {
            S::VarDecl {
                targets,
                value,
                mutex: false,
                public: false,
                ..
            } if targets.len() == 1 && targets[0].span == def => Some((st, value)),
            _ => None,
        },
        _ => None,
    })?;
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
    edits.push(TextEdit::new(range(text, ls, le), String::new()));
    Some(edit_action(
        format!("Inline `{}`", d.name),
        CodeActionKind::REFACTOR_INLINE,
        uri,
        edits,
        None,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_unused_imports() {
        let unused_imports =
            |src: &str| super::unused_imports(src, &crate::parser::parse_module_recover(src, 0).0);
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
