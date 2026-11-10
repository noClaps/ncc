//! Control-flow helpers over the typed IR: reference collection and completion analysis.

use std::collections::HashSet;

use super::Checker;
use crate::tir::*;
use crate::types::NEVER;

/// Visit every expression (pre-order) in a statement, including nested blocks.
pub fn visit_stmt(s: &Stmt, f: &mut dyn FnMut(&Expr)) {
    match &s.kind {
        SK::Let(_, e) | SK::Destructure(_, e) | SK::Expr(e) | SK::Throw(e) | SK::Assert(e) => {
            visit_expr(e, f)
        }
        SK::Assign(p, e) => {
            visit_place(p, f);
            visit_expr(e, f);
        }
        SK::Return(e) | SK::Break(_, e) => {
            if let Some(e) = e {
                visit_expr(e, f)
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

pub fn visit_place(p: &Place, f: &mut dyn FnMut(&Expr)) {
    match p {
        Place::Var(_) => {}
        Place::Field(b, _) | Place::TupleIndex(b, _) => visit_place(b, f),
        Place::Index(b, i) => {
            visit_place(b, f);
            visit_expr(i, f);
        }
    }
}

pub fn visit_block(b: &Block, f: &mut dyn FnMut(&Expr)) {
    for s in &b.stmts {
        visit_stmt(s, f);
    }
}

fn visit_callee(c: &Callee, f: &mut dyn FnMut(&Expr)) {
    if let Callee::Value(e) = c {
        visit_expr(e, f)
    }
}

pub fn visit_pat(p: &Pat, f: &mut dyn FnMut(&Expr)) {
    match p {
        Pat::Wild | Pat::Bind(_) => {}
        Pat::Value(e) | Pat::Cond(e) => visit_expr(e, f),
        Pat::Tuple(ps) | Pat::Struct(ps) | Pat::Array(ps) | Pat::Variant(_, ps) => {
            for p in ps {
                visit_pat(p, f)
            }
        }
    }
}

pub fn visit_expr(e: &Expr, f: &mut dyn FnMut(&Expr)) {
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
        EK::Call(c, args) | EK::Async(c, args) => {
            visit_callee(c, f);
            for a in args {
                visit_expr(a, f)
            }
        }
        EK::Print(_, _, args)
        | EK::Format(args)
        | EK::Tuple(args)
        | EK::Array(args)
        | EK::Struct(args)
        | EK::Variant(_, args) => {
            for a in args {
                visit_expr(a, f)
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
                visit_expr(s, f)
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

/// Globals written (declared or assigned) by a statement, including nested blocks.
pub fn writes_stmt(s: &Stmt, out: &mut HashSet<GlobalId>) {
    match &s.kind {
        SK::Let(Some(VarRef::Global(g)), _) => {
            out.insert(*g);
        }
        SK::Destructure(ts, _) => {
            for (t, _, _) in ts {
                if let Some(VarRef::Global(g)) = t {
                    out.insert(*g);
                }
            }
        }
        SK::Assign(p, _) => {
            if let Some(VarRef::Global(g)) = place_root(p) {
                out.insert(g);
            }
        }
        SK::While { body, .. } | SK::For { body, .. } | SK::Lock { body, .. } | SK::Block(body) => {
            for s in &body.stmts {
                writes_stmt(s, out);
            }
        }
        _ => {}
    }
    // assignments nested in if-arms / catch blocks
    let mut nested = |e: &Expr| {
        let blocks: Vec<&Block> = match &e.kind {
            EK::If(i) => i.arms.iter().map(|a| &a.body).collect(),
            EK::Catch(_, _, b, _) => vec![b],
            EK::Else(_, Fallback::Block(b, _)) => vec![b],
            _ => vec![],
        };
        for b in blocks {
            for s in &b.stmts {
                if matches!(s.kind, SK::Assign(..))
                    && let SK::Assign(p, _) = &s.kind
                    && let Some(VarRef::Global(g)) = place_root(p)
                {
                    out.insert(g);
                }
            }
        }
    };
    if !matches!(
        s.kind,
        SK::While { .. } | SK::For { .. } | SK::Lock { .. } | SK::Block(_)
    ) {
        visit_stmt(s, &mut nested);
    }
}

impl<'a> Checker<'a> {
    /// Whether execution can fall off the end of the block.
    pub fn block_completes(&self, b: &Block) -> bool {
        for s in &b.stmts {
            if !self.stmt_completes(s) {
                return false;
            }
        }
        true
    }

    fn stmt_completes(&self, s: &Stmt) -> bool {
        match &s.kind {
            SK::Return(_) | SK::Throw(_) | SK::Break(..) | SK::Continue(_) => false,
            SK::Expr(e) | SK::Let(_, e) | SK::Destructure(_, e) => self.expr_completes(e),
            SK::Assign(_, e) => self.expr_completes(e),
            SK::Block(b) => self.block_completes(b),
            SK::While { id, cond, .. } => {
                let infinite = matches!(cond.kind, EK::Lit(Lit::Bool(true)));
                !infinite || self.broken.contains(id)
            }
            SK::For { .. } => true,
            SK::Lock { id, body, .. } => self.block_completes(body) || self.broken.contains(id),
            SK::Assert(_) => true,
        }
    }

    fn expr_completes(&self, e: &Expr) -> bool {
        if e.ty == NEVER {
            return false;
        }
        match &e.kind {
            EK::If(i) if !i.is_value => {
                i.arms.iter().any(|a| self.block_completes(&a.body)) || self.broken.contains(&i.id)
            }
            EK::If(i) => {
                // a value if completes when some arm breaks with a value
                self.broken.contains(&i.id) || i.arms.iter().any(|a| self.block_completes(&a.body))
            }
            _ => true,
        }
    }
}
