//! Shared read-only expression traversal for linting and reachability.
use crate::ast::*;

pub fn item(item: &Item, f: &mut impl FnMut(&Expr)) {
    match item {
        Item::Function(fun) => block(&fun.body, f),
        Item::Global(v) => expr(&v.value, f),
        Item::Statement(s) => stmt(s, f),
        Item::Test { body, .. } => block(body, f),
        _ => {}
    }
}
fn block(b: &Block, f: &mut impl FnMut(&Expr)) {
    for s in &b.statements {
        stmt(s, f);
    }
}
fn stmt(s: &Stmt, f: &mut impl FnMut(&Expr)) {
    match s {
        Stmt::Block(b) | Stmt::Lock { body: b, .. } => block(b, f),
        Stmt::Var(v) => expr(&v.value, f),
        Stmt::Assign { target, value } => {
            expr(target, f);
            expr(value, f);
        }
        Stmt::Expr(e) | Stmt::Throw(e) | Stmt::Assert(e) => expr(e, f),
        Stmt::Return(e) | Stmt::Break(e, _) => {
            if let Some(e) = e {
                expr(e, f);
            }
        }
        Stmt::For { iterable, body, .. } => {
            expr(iterable, f);
            block(body, f);
        }
        Stmt::While {
            condition, body, ..
        } => {
            expr(condition, f);
            block(body, f);
        }
        Stmt::Continue(_) => {}
    }
}
fn pattern(p: &Pattern, f: &mut impl FnMut(&Expr)) {
    match p {
        Pattern::Literal(e) => expr(e, f),
        Pattern::Array(ps) | Pattern::Tuple(ps) | Pattern::Variant { values: ps, .. } => {
            for p in ps {
                pattern(p, f);
            }
        }
        Pattern::Struct { fields, .. } => {
            for (_, p) in fields {
                pattern(p, f);
            }
        }
        _ => {}
    }
}
fn expr(e: &Expr, f: &mut impl FnMut(&Expr)) {
    f(e);
    match e {
        Expr::Lambda(fun) => block(&fun.body, f),
        Expr::Cast { value, .. }
        | Expr::Unary { value, .. }
        | Expr::Async(value)
        | Expr::Await(value)
        | Expr::Try(value) => expr(value, f),
        Expr::Binary { left, right, .. } => {
            expr(left, f);
            expr(right, f);
        }
        Expr::Call { callee, args, .. } => {
            expr(callee, f);
            for arg in args {
                expr(arg, f);
            }
        }
        Expr::Index { object, index } => {
            expr(object, f);
            expr(index, f);
        }
        Expr::Member { object, .. } => expr(object, f),
        Expr::Array(xs) | Expr::Tuple(xs) => {
            for x in xs {
                expr(x, f);
            }
        }
        Expr::Map(xs) => {
            for (k, v) in xs {
                expr(k, f);
                expr(v, f);
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, v) in fields {
                expr(v, f);
            }
        }
        Expr::If { subject, arms } => {
            if let Some(s) = subject {
                expr(s, f);
            }
            for (patterns, b) in arms {
                for p in patterns {
                    pattern(p, f);
                }
                block(b, f);
            }
        }
        Expr::Else { value, fallback } => {
            expr(value, f);
            block(fallback, f);
        }
        Expr::Catch { value, body, .. } => {
            expr(value, f);
            block(body, f);
        }
        _ => {}
    }
}
