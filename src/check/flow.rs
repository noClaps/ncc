//! Control-flow helpers over the typed IR: reference collection and completion analysis.

use std::collections::HashSet;

use super::Checker;
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

fn visit_callee<'a>(c: &'a Callee, f: &mut dyn FnMut(&'a Expr)) {
    if let Callee::Value(e) = c {
        visit_expr(e, f);
    }
}

fn visit_pat<'a>(p: &'a Pat, f: &mut dyn FnMut(&'a Expr)) {
    match p {
        Pat::Wild | Pat::Bind(_) => {}
        Pat::Value(e) | Pat::Cond(e) => visit_expr(e, f),
        Pat::Tuple(ps) | Pat::Struct(ps) | Pat::Array(ps) | Pat::Variant(_, ps) => {
            for p in ps {
                visit_pat(p, f);
            }
        }
    }
}

pub fn visit_expr<'a>(e: &'a Expr, f: &mut dyn FnMut(&'a Expr)) {
    f(e);
    match &e.kind {
        EK::Lit(_)
        | EK::None
        | EK::Var(_)
        | EK::FnRef(_)
        | EK::Closure(_)
        | EK::Dollar
        | EK::Args
        | EK::Env
        | EK::Target
        | EK::Embed(..) => {}
        EK::Unary(_, a)
        | EK::Cast(a)
        | EK::ToStr(a)
        | EK::Field(a, _)
        | EK::TupleIndex(a, _)
        | EK::Len(a)
        | EK::Some(a)
        | EK::Retype(a)
        | EK::Try(a)
        | EK::Await(a) => visit_expr(a, f),
        EK::Binary(_, a, b) | EK::Index(a, b) => {
            visit_expr(a, f);
            visit_expr(b, f);
        }
        EK::Slice(a, lo, hi) => {
            visit_expr(a, f);
            visit_expr(lo, f);
            visit_expr(hi, f);
        }
        EK::Call(c, args) | EK::Async(c, args) => {
            visit_callee(c, f);
            for a in args {
                visit_expr(a, f);
            }
        }
        EK::Print(_, _, args)
        | EK::Format(args)
        | EK::Tuple(args)
        | EK::Array(args)
        | EK::Struct(args)
        | EK::Variant(_, args) => {
            for a in args {
                visit_expr(a, f);
            }
        }
        EK::Map(es) => {
            for (k, v) in es {
                visit_expr(k, f);
                visit_expr(v, f);
            }
        }
        EK::If(i) => {
            if let Some(s) = &i.scrutinee {
                visit_expr(s, f);
            }
            for arm in &i.arms {
                for p in &arm.patterns {
                    visit_pat(p, f);
                }
                visit_block(&arm.body, f);
            }
        }
        EK::Else(a, fb) => {
            visit_expr(a, f);
            match fb {
                Fallback::Expr(b) => visit_expr(b, f),
                Fallback::Block(b, _) => visit_block(b, f),
            }
        }
        EK::Catch(a, _, b, _) => {
            visit_expr(a, f);
            visit_block(b, f);
        }
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
fn nested_blocks(e: &Expr) -> Vec<&Block> {
    match &e.kind {
        EK::If(i) => i.arms.iter().map(|a| &a.body).collect(),
        EK::Catch(_, _, b, _) | EK::Else(_, Fallback::Block(b, _)) => vec![b],
        _ => vec![],
    }
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

impl Checker {
    /// Whether execution can fall off the end of the block.
    pub fn block_completes(&self, b: &Block) -> bool {
        b.stmts.iter().all(|s| self.stmt_completes(s))
    }

    fn stmt_completes(&self, s: &Stmt) -> bool {
        match &s.kind {
            SK::Return(_) | SK::Throw(_) | SK::Break(..) | SK::Continue(_) => false,
            SK::Expr(e) | SK::Let(_, e) | SK::Destructure(_, e) | SK::Assign(_, e) => {
                self.expr_completes(e)
            }
            SK::Block(b) => self.block_completes(b),
            SK::While { id, cond, .. } => {
                let infinite = matches!(cond.kind, EK::Lit(Lit::Bool(true)));
                !infinite || self.broken.contains(id)
            }
            SK::For { .. } | SK::Assert(_) => true,
            SK::Lock { id, body, .. } => self.block_completes(body) || self.broken.contains(id),
        }
    }

    fn expr_completes(&self, e: &Expr) -> bool {
        if e.ty == NEVER {
            return false;
        }
        match &e.kind {
            // an `if` completes when an arm does, or one breaks out of it (with its value)
            EK::If(i) => {
                self.broken.contains(&i.id) || i.arms.iter().any(|a| self.block_completes(&a.body))
            }
            _ => true,
        }
    }
}
