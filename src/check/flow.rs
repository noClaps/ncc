//! Control-flow helpers over the typed IR: reference collection and completion analysis.

use std::collections::HashSet;

use crate::tir::*;
use crate::types::NEVER;

/// Visit every expression (pre-order) in a statement, including nested blocks.
pub fn visit_stmt<'a>(s: &'a Stmt, f: &mut dyn FnMut(&'a Expr)) {
    for e in s.own_exprs() {
        visit_expr(e, f);
    }
    if let Some(b) = s.body() {
        visit_block(b, f);
    }
}

pub fn visit_block<'a>(b: &'a Block, f: &mut dyn FnMut(&'a Expr)) {
    for s in &b.stmts {
        visit_stmt(s, f);
    }
}

/// Visit every expression (pre-order) in every function of the program.
pub fn visit_program<'a>(p: &'a Program, f: &mut dyn FnMut(&'a Expr)) {
    for func in &p.funcs {
        visit_block(&func.body, f);
    }
}

pub fn visit_expr<'a>(e: &'a Expr, f: &mut dyn FnMut(&'a Expr)) {
    f(e);
    e.for_each_child(&mut |c| visit_expr(c, f));
    match &e.kind {
        EK::If(i) => {
            for arm in &i.arms {
                for p in &arm.patterns {
                    p.for_each_expr(&mut |e| visit_expr(e, f));
                }
                visit_block(&arm.body, f);
            }
        }
        EK::Else(_, Fallback::Block(b, _)) | EK::Catch(_, _, b, _) => visit_block(b, f),
        _ => {}
    }
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

/// Like `nested_blocks`, with mutable access.
pub fn nested_blocks_mut(e: &mut Expr) -> Vec<&mut Block> {
    match &mut e.kind {
        EK::If(i) => i.arms.iter_mut().map(|a| &mut a.body).collect(),
        EK::Catch(_, _, b, _) | EK::Else(_, Fallback::Block(b, _)) => vec![b],
        _ => vec![],
    }
}

/// Whether a statement's expressions contain blocks (`if` arms, `catch` and `else` blocks).
pub fn has_nested_blocks(s: &Stmt) -> bool {
    let mut found = false;
    visit_stmt(s, &mut |e| found |= !nested_blocks(e).is_empty());
    found
}

/// Visit a statement and every statement nested in it, in the blocks of statements and of
/// expressions.
pub fn visit_stmts(s: &Stmt, f: &mut dyn FnMut(&Stmt)) {
    // through statement bodies only: blocks inside expressions are found by `visit_stmt`
    fn bodies(s: &Stmt, f: &mut dyn FnMut(&Stmt)) {
        f(s);
        for s in s.body().iter().flat_map(|b| &b.stmts) {
            bodies(s, f);
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

/// `visit_stmts` for each statement of a block.
pub fn visit_block_stmts(b: &Block, f: &mut dyn FnMut(&Stmt)) {
    for s in &b.stmts {
        visit_stmts(s, f);
    }
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
    completes(b, &broken(b))
}

/// The constructs some `break` in the block leaves, with or without a value.
pub fn broken(b: &Block) -> HashSet<JumpId> {
    let mut out = HashSet::new();
    visit_block_stmts(b, &mut |s| {
        if let SK::Break(id, _) = s.kind {
            out.insert(id);
        }
    });
    out
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
