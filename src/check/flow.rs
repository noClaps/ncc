//! Control-flow helpers over the typed IR: reference collection and completion analysis.

use std::collections::HashSet;

use crate::tir::*;
use crate::types::NEVER;

/// Visit every expression (pre-order) in a statement, including nested blocks.
pub fn visit_stmt<'a>(s: &'a Stmt, f: &mut dyn FnMut(&'a Expr)) {
    match &s.kind {
        SK::Let(_, e) | SK::Destructure(_, e) | SK::Expr(e) | SK::Throw(e) | SK::Assert(e) => {
            visit_expr(e, f);
        }
        SK::Assign(p, e) => {
            visit_place(p, f);
            visit_expr(e, f);
        }
        SK::Return(e) | SK::Break(_, e) => {
            if let Some(e) = e {
                visit_expr(e, f);
            }
        }
        SK::Continue(_) => {}
        SK::While { cond, body, .. } => {
            visit_expr(cond, f);
            visit_block(body, f);
        }
        SK::For { iter, body, .. } => {
            visit_expr(iter, f);
            visit_block(body, f);
        }
        SK::Lock { mutex, body, .. } => {
            visit_expr(mutex, f);
            visit_block(body, f);
        }
        SK::Block(b) => visit_block(b, f),
    }
}

fn visit_place<'a>(p: &'a Place, f: &mut dyn FnMut(&'a Expr)) {
    match p {
        Place::Var(_) => {}
        Place::Field(b, _) | Place::TupleIndex(b, _) => visit_place(b, f),
        Place::Index(b, i) => {
            visit_place(b, f);
            visit_expr(i, f);
        }
    }
}

fn visit_block<'a>(b: &'a Block, f: &mut dyn FnMut(&'a Expr)) {
    for s in &b.stmts {
        visit_stmt(s, f);
    }
}

fn visit_pat<'a>(p: &'a Pat, f: &mut dyn FnMut(&'a Expr)) {
    p.for_each_expr(&mut |e| visit_expr(e, f));
}

pub fn visit_expr<'a>(e: &'a Expr, f: &mut dyn FnMut(&'a Expr)) {
    f(e);
    e.for_each_child(&mut |c| visit_expr(c, f));
    match &e.kind {
        EK::If(i) => {
            for arm in &i.arms {
                for p in &arm.patterns {
                    visit_pat(p, f);
                }
                visit_block(&arm.body, f);
            }
        }
        EK::Else(_, Fallback::Block(b, _)) | EK::Catch(_, _, b, _) => visit_block(b, f),
        _ => {}
    }
}

/// Globals and functions referenced by a statement.
pub fn refs_stmt(s: &Stmt, globals: &mut HashSet<GlobalId>, funcs: &mut HashSet<FuncId>) {
    if let SK::Assign(p, _) = &s.kind
        && let Some(VarRef::Global(g)) = place_root(p)
    {
        globals.insert(g);
    }
    let mut f = |e: &Expr| match &e.kind {
        EK::Var(VarRef::Global(g)) => {
            globals.insert(*g);
        }
        EK::FnRef(id)
        | EK::Closure(id)
        | EK::Call(Callee::Direct(id), _)
        | EK::Async(Callee::Direct(id), _) => {
            funcs.insert(*id);
        }
        _ => {}
    };
    visit_stmt(s, &mut f);
    // nested lets declare globals only at module level, covered by writes_stmt
}

pub fn place_root(p: &Place) -> Option<VarRef> {
    match p {
        Place::Var(v) => Some(*v),
        Place::Field(b, _) | Place::TupleIndex(b, _) | Place::Index(b, _) => place_root(b),
    }
}

/// The blocks directly inside an expression: `if` arms, `catch` and `else` blocks.
pub fn nested_blocks(e: &Expr) -> Vec<&Block> {
    match &e.kind {
        EK::If(i) => i.arms.iter().map(|a| &a.body).collect(),
        EK::Catch(_, _, b, _) | EK::Else(_, Fallback::Block(b, _)) => vec![b],
        _ => vec![],
    }
}

/// Whether a statement's expressions contain blocks (`if` arms, `catch` and `else` blocks).
pub fn has_nested_blocks(s: &Stmt) -> bool {
    let mut found = false;
    visit_stmt(s, &mut |e| {
        found |= matches!(
            e.kind,
            EK::If(_) | EK::Catch(..) | EK::Else(_, Fallback::Block(..))
        );
    });
    found
}

/// Visit a statement and every statement nested in it, in the blocks of statements and of
/// expressions.
pub fn visit_stmts(s: &Stmt, f: &mut dyn FnMut(&Stmt)) {
    // through statement bodies only: blocks inside expressions are found by `visit_stmt`
    fn bodies(s: &Stmt, f: &mut dyn FnMut(&Stmt)) {
        f(s);
        if let SK::While { body, .. }
        | SK::For { body, .. }
        | SK::Lock { body, .. }
        | SK::Block(body) = &s.kind
        {
            for s in &body.stmts {
                bodies(s, f);
            }
        }
    }
    bodies(s, f);
    visit_stmt(s, &mut |e| {
        for b in nested_blocks(e) {
            for s in &b.stmts {
                bodies(s, f);
            }
        }
    });
}

/// Globals written (declared or assigned) by a statement, including nested blocks.
pub fn writes_stmt(s: &Stmt, out: &mut HashSet<GlobalId>) {
    visit_stmts(s, &mut |s| match &s.kind {
        SK::Let(Some(VarRef::Global(g)), _) => {
            out.insert(*g);
        }
        SK::Destructure(ts, _) => {
            out.extend(ts.iter().filter_map(|(t, _, _)| match t {
                Some(VarRef::Global(g)) => Some(*g),
                _ => None,
            }));
        }
        SK::Assign(p, _) => {
            if let Some(VarRef::Global(g)) = place_root(p) {
                out.insert(g);
            }
        }
        _ => {}
    });
}

/// Whether execution can fall off the end of the block.
pub fn block_completes(b: &Block) -> bool {
    // the constructs some `break` leaves, with or without a value
    let mut broken = HashSet::new();
    for s in &b.stmts {
        visit_stmts(s, &mut |s| {
            if let SK::Break(id, _) = &s.kind {
                broken.insert(*id);
            }
        });
    }
    completes(b, &broken)
}

fn completes(b: &Block, broken: &HashSet<JumpId>) -> bool {
    b.stmts.iter().all(|s| stmt_completes(s, broken))
}

fn stmt_completes(s: &Stmt, broken: &HashSet<JumpId>) -> bool {
    match &s.kind {
        SK::Return(_) | SK::Throw(_) | SK::Break(..) | SK::Continue(_) => false,
        SK::Expr(e) | SK::Let(_, e) | SK::Destructure(_, e) | SK::Assign(_, e) => {
            expr_completes(e, broken)
        }
        SK::Block(b) => completes(b, broken),
        SK::While { id, cond, .. } => {
            let infinite = matches!(cond.kind, EK::Lit(Lit::Bool(true)));
            !infinite || broken.contains(id)
        }
        SK::For { .. } | SK::Assert(_) => true,
        SK::Lock { id, body, .. } => completes(body, broken) || broken.contains(id),
    }
}

fn expr_completes(e: &Expr, broken: &HashSet<JumpId>) -> bool {
    if e.ty == NEVER {
        return false;
    }
    match &e.kind {
        // an `if` completes when an arm does, or one breaks out of it (with its value)
        EK::If(i) => broken.contains(&i.id) || i.arms.iter().any(|a| completes(&a.body, broken)),
        _ => true,
    }
}
