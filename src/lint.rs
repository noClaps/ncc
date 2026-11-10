//! Warnings that do not prevent compilation.

use std::collections::{HashMap, HashSet};

use crate::check::flow;
use crate::diag::{Diag, Span};
use crate::tir::*;

/// Shared mutable state touched by a function: (variable name, access location).
fn shared_accesses(p: &Program, f: FuncId) -> Vec<(String, Span)> {
    let func = &p.funcs[f];
    let mut out = Vec::new();
    let mut visit = |e: &Expr| {
        if let EK::Var(v) = &e.kind {
            match v {
                VarRef::Global(g) if p.globals[*g].mutable => {
                    out.push((p.globals[*g].name.clone(), e.span));
                }
                VarRef::Local(l) if matches!(func.locals[*l].kind, LocalKind::CapRef(_)) => {
                    out.push((func.locals[*l].name.clone(), e.span));
                }
                _ => {}
            }
        }
    };
    for s in &func.body.stmts {
        flow::visit_stmt(s, &mut visit);
    }
    // assignment targets
    fn places(b: &Block, func: &Func, p: &Program, out: &mut Vec<(String, Span)>) {
        for s in &b.stmts {
            if let SK::Assign(pl, _) = &s.kind {
                match flow::place_root(pl) {
                    Some(VarRef::Global(g)) if p.globals[g].mutable => {
                        out.push((p.globals[g].name.clone(), s.span));
                    }
                    Some(VarRef::Local(l))
                        if matches!(func.locals[l].kind, LocalKind::CapRef(_)) =>
                    {
                        out.push((func.locals[l].name.clone(), s.span));
                    }
                    _ => {}
                }
            }
            match &s.kind {
                SK::While { body, .. }
                | SK::For { body, .. }
                | SK::Lock { body, .. }
                | SK::Block(body) => places(body, func, p, out),
                _ => {}
            }
            let mut nested: Vec<Block> = Vec::new();
            flow::visit_stmt(s, &mut |e: &Expr| {
                nested.extend(flow::nested_blocks(e).into_iter().cloned());
            });
            for b in &nested {
                places(b, func, p, out);
            }
        }
    }
    places(&func.body, func, p, &mut out);
    out
}

fn callees(p: &Program, f: FuncId) -> Vec<FuncId> {
    let mut out = Vec::new();
    for s in &p.funcs[f].body.stmts {
        flow::visit_stmt(s, &mut |e: &Expr| match &e.kind {
            EK::Call(Callee::Direct(g), _) | EK::FnRef(g) | EK::Closure(g) => out.push(*g),
            _ => {}
        });
    }
    out
}

/// Warn about `async` calls whose code touches shared mutable state outside a mutex.
pub fn race_warnings(p: &Program) -> Vec<Diag> {
    let mut warnings = Vec::new();
    let mut seen: HashSet<Span> = HashSet::new();
    for f in &p.funcs {
        // locals initialised with a closure: lets us resolve `async local_fn()`
        let mut closures: HashMap<LocalId, FuncId> = HashMap::new();
        let mut asyncs: Vec<(Option<FuncId>, Span)> = Vec::new();
        for s in &f.body.stmts {
            collect(s, &mut closures, &mut asyncs);
        }
        for (target, span) in asyncs {
            let Some(target) = target else { continue };
            // transitive closure of reachable functions
            let mut stack = vec![target];
            let mut visited = HashSet::new();
            let mut hits = Vec::new();
            while let Some(g) = stack.pop() {
                if !visited.insert(g) {
                    continue;
                }
                hits.extend(shared_accesses(p, g));
                stack.extend(callees(p, g));
            }
            if let Some((name, _)) = hits.first()
                && seen.insert(span)
            {
                warnings.push(Diag::new(
                        span,
                        format!(
                            "possible data race: `{}` runs asynchronously and accesses the mutable variable `{name}` without a mutex",
                            p.funcs[target].name
                        ),
                    ));
            }
        }
    }
    warnings
}

fn collect(
    s: &Stmt,
    closures: &mut HashMap<LocalId, FuncId>,
    asyncs: &mut Vec<(Option<FuncId>, Span)>,
) {
    if let SK::Let(
        Some(VarRef::Local(l)),
        Expr {
            kind: EK::Closure(f),
            ..
        },
    ) = &s.kind
    {
        closures.insert(*l, *f);
    }
    let mut found = Vec::new();
    flow::visit_stmt(s, &mut |e: &Expr| {
        if let EK::Async(c, _) = &e.kind {
            let target = match c {
                Callee::Direct(f) => Some(*f),
                Callee::Value(v) => match &v.kind {
                    EK::Closure(f) | EK::FnRef(f) => Some(*f),
                    EK::Var(VarRef::Local(l)) => closures.get(l).copied(),
                    _ => None,
                },
                Callee::Extern(_) => None,
            };
            found.push((target, e.span));
        }
    });
    asyncs.extend(found);
}

// ------------------------------------------------------------------ unused code

/// What an unused-code diagnostic is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnusedKind {
    Variable,
    Parameter,
    Function,
    Type,
    Import,
}

impl UnusedKind {
    /// The diagnostic code the language server attaches (and its quick fixes look for).
    pub fn code(self) -> &'static str {
        match self {
            UnusedKind::Variable => "unused-variable",
            UnusedKind::Parameter => "unused-parameter",
            UnusedKind::Function => "unused-function",
            UnusedKind::Type => "unused-type",
            UnusedKind::Import => "unused-import",
        }
    }
}

pub struct Unused {
    pub diag: Diag,
    pub kind: UnusedKind,
}

/// The span of `name` as a whole word inside `span` (the first or the last occurrence).
fn name_in(text: &str, span: Span, name: &str, first: bool) -> Option<Span> {
    let (s, e) = (span.start as usize, (span.end as usize).min(text.len()));
    let seg = text.get(s..e)?;
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    let mut found = None;
    for (i, _) in seg.match_indices(name) {
        let before = seg[..i].chars().next_back();
        let after = seg[i + name.len()..].chars().next();
        if before.is_some_and(ident) || after.is_some_and(ident) {
            continue;
        }
        found = Some(i);
        if first {
            break;
        }
    }
    found.map(|i| Span::new(span.file, s + i, s + i + name.len()))
}

/// Unused local variables, parameters, private items and imports of the project's modules
/// (not dependencies). Names starting with `_` and `pub` items are never reported. Uses
/// anywhere, including tests, count.
pub fn unused(ix: &crate::check::index::Index, sm: &crate::diag::SourceMap) -> Vec<Unused> {
    use crate::check::index::DeclKind;
    let facts: HashMap<u32, &crate::check::index::ModFacts> =
        ix.modules.iter().map(|m| (m.file, m)).collect();
    let mut used: HashSet<Span> = HashSet::new();
    // declaration spans that are also a use of something else (a `lock` alias)
    let mut shadowing: HashSet<Span> = HashSet::new();
    for r in &ix.refs {
        if let Some(def) = r.def
            && r.span != def
        {
            used.insert(def);
            shadowing.insert(r.span);
        }
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for d in &ix.decls {
        let Some(f) = facts.get(&d.span.file) else {
            continue;
        };
        if !f.project
            || ix
                .editor_only
                .iter()
                .any(|b| b.file == d.span.file && b.start <= d.span.start && d.span.end <= b.end)
            || d.name.is_empty()
            || d.name.starts_with('_')
            || matches!(d.kind, DeclKind::Field | DeclKind::Variant)
            || used.contains(&d.span)
            || shadowing.contains(&d.span)
            || !seen.insert(d.span)
            || (d.scope.is_none() && d.extern_alias.is_none() && f.public.contains(&d.name))
        {
            continue;
        }
        let text = &sm.file(d.span.file).text;
        let (kind, what) = match d.kind {
            DeclKind::Variable if d.param => (UnusedKind::Parameter, "parameter"),
            // a function declared inside another one (its span is just the name)
            DeclKind::Variable
                if text.get(d.span.start as usize..d.span.end as usize)
                    == Some(d.name.as_str())
                    && d.detail.starts_with("(fn") =>
            {
                (UnusedKind::Function, "function")
            }
            DeclKind::Variable => (UnusedKind::Variable, "variable"),
            DeclKind::Function if d.extern_alias.is_some() => {
                (UnusedKind::Function, "extern function")
            }
            DeclKind::Function => (UnusedKind::Function, "function"),
            DeclKind::Struct => (UnusedKind::Type, "struct"),
            DeclKind::Enum => (UnusedKind::Type, "enum"),
            DeclKind::Type => (UnusedKind::Type, "type"),
            DeclKind::Field | DeclKind::Variant => continue,
        };
        let first = !matches!(d.kind, DeclKind::Variable);
        let Some(at) = name_in(text, d.span, &d.name, first) else {
            continue;
        };
        out.push(Unused {
            diag: Diag::new(
                at,
                format!(
                    "unused {what} `{}`; prefix the name with `_` if this is intended",
                    d.name
                ),
            ),
            kind,
        });
    }
    for f in ix.modules.iter().filter(|f| f.project) {
        let text = &sm.file(f.file).text;
        for (alias, span, is_used) in &f.imports {
            if *is_used || alias.starts_with('_') {
                continue;
            }
            let at = name_in(text, *span, alias, false).unwrap_or(*span);
            out.push(Unused {
                diag: Diag::new(at, format!("unused import `{alias}`")),
                kind: UnusedKind::Import,
            });
        }
    }
    out.sort_by_key(|u| (u.diag.span.file, u.diag.span.start));
    out
}
