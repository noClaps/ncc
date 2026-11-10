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
                VarRef::Global(g) if p.globals[*g].mutable => out.push((p.globals[*g].name.clone(), e.span)),
                VarRef::Local(l) if matches!(func.locals[*l].kind, LocalKind::CapRef(_)) => {
                    out.push((func.locals[*l].name.clone(), e.span))
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
                    Some(VarRef::Global(g)) if p.globals[g].mutable => out.push((p.globals[g].name.clone(), s.span)),
                    Some(VarRef::Local(l)) if matches!(func.locals[l].kind, LocalKind::CapRef(_)) => {
                        out.push((func.locals[l].name.clone(), s.span))
                    }
                    _ => {}
                }
            }
            match &s.kind {
                SK::While { body, .. } | SK::For { body, .. } | SK::Lock { body, .. } | SK::Block(body) => {
                    places(body, func, p, out)
                }
                _ => {}
            }
            let mut nested: Vec<Block> = Vec::new();
            flow::visit_stmt(s, &mut |e: &Expr| match &e.kind {
                EK::If(i) => nested.extend(i.arms.iter().map(|a| a.body.clone())),
                EK::Catch(_, _, b, _) | EK::Else(_, Fallback::Block(b, _)) => nested.push(b.clone()),
                _ => {}
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
                && seen.insert(span) {
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

fn collect(s: &Stmt, closures: &mut HashMap<LocalId, FuncId>, asyncs: &mut Vec<(Option<FuncId>, Span)>) {
    if let SK::Let(Some(VarRef::Local(l)), Expr { kind: EK::Closure(f), .. }) = &s.kind {
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
