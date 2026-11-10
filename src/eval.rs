//! Compile-time evaluation of pure code.
//!
//! Used in every mode to reject programs that are guaranteed to panic (e.g. a call with
//! constant arguments that overflows), and in release mode to fold constant expressions and
//! pure calls with constant arguments into literals.

use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::{BinOp, UnOp};
use crate::check::flow;
use crate::diag::{Diag, Results, SourceMap, Span};
use crate::tir::*;
use crate::types::*;

#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Void,
    Bool(bool),
    Byte(u8),
    Int(i64),
    UInt(u64),
    Float(f64),
    Char(u32),
    Str(Rc<str>),
    Arr(Rc<Vec<Val>>),
    /// insertion-ordered association list
    Map(Rc<Vec<(Val, Val)>>),
    Tuple(Rc<Vec<Val>>),
    Struct(Rc<Vec<Val>>),
    Variant(usize, Rc<Vec<Val>>),
    None,
    Some(Box<Val>),
    Ok(Box<Val>),
    Err(Rc<str>),
    /// function value: function id and captured values (by value only)
    Fn(FuncId, Rc<Vec<Val>>),
}

/// Why evaluation stopped.
enum Stop {
    /// The program would panic: message and location.
    Panic(String, Span),
    /// Evaluation is not possible at compile time (impure operation, budget exhausted, ...).
    Unknown,
}

enum Flow {
    Normal,
    Return(Val),
    Throw(Rc<str>),
    Break(JumpId, Option<Val>),
    Continue(JumpId),
}

type R<T> = Result<T, Stop>;

pub struct Evaluator<'p> {
    p: &'p Program,
    sm: &'p SourceMap,
    pure: Vec<bool>,
    fuel: u64,
    depth: usize,
    /// constants known for immutable globals
    gconst: HashMap<GlobalId, Val>,
    /// every always-panicking expression found
    errs: Vec<Diag>,
}

const CALL_FUEL: u64 = 2_000_000;
const MAX_DEPTH: usize = 200;

pub fn purity(p: &Program) -> Vec<bool> {
    let n = p.funcs.len();
    let mut pure = vec![true; n];
    let mut calls: Vec<Vec<FuncId>> = vec![vec![]; n];
    for (i, f) in p.funcs.iter().enumerate() {
        if f.locals
            .iter()
            .any(|l| matches!(l.kind, LocalKind::CapRef(_) | LocalKind::LockAlias))
        {
            pure[i] = false;
        }
        let mut ok = true;
        let mut cs = vec![];
        let mut visit = |e: &Expr| match &e.kind {
            EK::Print(..) | EK::Async(..) | EK::Await(_) | EK::Call(Callee::Extern(_), _) => {
                ok = false
            }
            EK::Call(Callee::Direct(g), _) => cs.push(*g),
            EK::Var(VarRef::Global(g)) if p.globals[*g].mutable => ok = false,
            EK::Var(VarRef::Global(g))
                if matches!(p.types.kind(p.globals[*g].ty), TyKind::Mutex(_)) =>
            {
                ok = false;
            }
            _ => {}
        };
        for s in &f.body.stmts {
            flow::visit_stmt(s, &mut visit);
        }
        for s in &f.body.stmts {
            if !stmt_pure(s) {
                ok = false;
            }
        }
        if !ok {
            pure[i] = false;
        }
        calls[i] = cs;
    }
    loop {
        let mut changed = false;
        for i in 0..n {
            if pure[i] && calls[i].iter().any(|g| !pure[*g]) {
                pure[i] = false;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    pure
}

/// Statement-level purity: no global writes, locks, mutexes or asserts.
fn stmt_pure(s: &Stmt) -> bool {
    let mut w = std::collections::HashSet::new();
    flow::writes_stmt(s, &mut w);
    if !w.is_empty() {
        return false;
    }
    match &s.kind {
        SK::Lock { .. } | SK::Assert(_) | SK::Let(Some(VarRef::Global(_)), _) => false,
        SK::While { body, .. } | SK::For { body, .. } | SK::Block(body) => {
            body.stmts.iter().all(stmt_pure)
        }
        _ => {
            let mut ok = true;
            let mut check = |e: &Expr| {
                if !flow::nested_blocks(e)
                    .iter()
                    .all(|b| b.stmts.iter().all(stmt_pure))
                {
                    ok = false;
                }
            };
            flow::visit_stmt(s, &mut check);
            ok
        }
    }
}

impl<'p> Evaluator<'p> {
    pub fn new(p: &'p Program, sm: &'p SourceMap) -> Evaluator<'p> {
        Evaluator {
            p,
            sm,
            pure: purity(p),
            fuel: 0,
            depth: 0,
            gconst: HashMap::new(),
            errs: vec![],
        }
    }

    fn loc(&self, span: Span) -> String {
        if self.sm.files.is_empty() {
            return "?".into();
        }
        let f = self.sm.file(span.file);
        let (l, _) = f.line_col(span.start as usize);
        let mut path = f.path.clone();
        if let Ok(cwd) = std::env::current_dir()
            && let Ok(rel) = path.strip_prefix(&cwd)
        {
            path = rel.to_path_buf();
        }
        format!("{}:{l}", path.display())
    }

    fn panic<T>(&self, msg: &str, span: Span) -> R<T> {
        Err(Stop::Panic(msg.to_string(), span))
    }

    fn tick(&mut self) -> R<()> {
        if self.fuel == 0 {
            return Err(Stop::Unknown);
        }
        self.fuel -= 1;
        Ok(())
    }

    // ------------------------------------------------------------ program pass

    /// Check (and in release mode fold) the whole program.
    pub fn run(p: &mut Program, sm: &SourceMap, fold: bool) -> Results<()> {
        let snapshot = Program {
            types: std::mem::take(&mut p.types),
            funcs: p.funcs.clone(),
            globals: p.globals.clone(),
            externs: p.externs.clone(),
            main: p.main,
            tests: p.tests.clone(),
            target: p.target.clone(),
        };
        let (funcs, mut errs) = {
            let mut ev = Evaluator::new(&snapshot, sm);
            // constants for immutable globals with constant initialisers (in main, in order)
            let main = &snapshot.funcs[snapshot.main];
            for s in &main.body.stmts {
                if let SK::Let(Some(VarRef::Global(g)), e) = &s.kind
                    && !snapshot.globals[*g].mutable
                {
                    // panics are reported when the statement itself is folded below
                    if let Ok(Some(v)) = ev.try_const(e) {
                        ev.gconst.insert(*g, v);
                    }
                }
                // destructured immutable globals: `str os, str cpu = @target()`
                if let SK::Destructure(targets, e) = &s.kind
                    && let Ok(Some(Val::Tuple(items))) = ev.try_const(e)
                {
                    for (t, start, count) in targets {
                        if let Some(VarRef::Global(g)) = t
                            && !snapshot.globals[*g].mutable
                        {
                            let v = if *count == 1 {
                                items[*start].clone()
                            } else {
                                Val::Tuple(Rc::new(items[*start..*start + *count].to_vec()))
                            };
                            ev.gconst.insert(*g, v);
                        }
                    }
                }
            }
            let mut funcs = snapshot.funcs.clone();
            for f in &mut funcs {
                ev.fold_block(&mut f.body, &f.locals, &mut HashMap::new());
            }
            (funcs, ev.errs)
        };
        p.types = snapshot.types;
        if !errs.is_empty() {
            errs.sort_by_key(|d| (d.span.file, d.span.start));
            errs.dedup_by(|a, b| a.span == b.span && a.msg == b.msg);
            return Err(errs);
        }
        if fold {
            p.funcs = funcs;
        }
        Ok(())
    }

    /// Evaluate an expression that is fully constant (no variables), if possible.
    fn try_const(&mut self, e: &Expr) -> Result<Option<Val>, Diag> {
        self.fuel = CALL_FUEL;
        let mut env = Env {
            locals: vec![],
            dollar: vec![],
        };
        match self.expr(e, &mut env, true) {
            Ok(v) => Ok(Some(v)),
            Err(Stop::Unknown) => Ok(None),
            Err(Stop::Panic(m, sp)) => Err(self.panic_diag(e.span, &m, sp)),
        }
    }

    fn panic_diag(&self, at: Span, msg: &str, inner: Span) -> Diag {
        let inside = inner.file == at.file && inner.start >= at.start && inner.end <= at.end;
        if inside {
            Diag::new(at, format!("this expression always panics: {msg}"))
        } else {
            // the panic happens elsewhere (e.g. inside a called function): say where
            Diag::new(
                at,
                format!("this expression always panics: {msg} ({})", self.loc(inner)),
            )
        }
    }

    fn fold_block(&mut self, b: &mut Block, locals: &[Local], consts: &mut HashMap<LocalId, Val>) {
        for s in &mut b.stmts {
            if let Err(d) = self.fold_stmt(s, locals, consts) {
                self.errs.push(d);
            }
        }
    }

    fn fold_stmt(
        &mut self,
        s: &mut Stmt,
        locals: &[Local],
        consts: &mut HashMap<LocalId, Val>,
    ) -> Result<(), Diag> {
        match &mut s.kind {
            SK::Let(v, e) => {
                self.fold_expr(e, locals, consts)?;
                if let (Some(VarRef::Local(l)), EK::Lit(_)) = (v, &e.kind)
                    && !locals[*l].mutable
                    && !locals[*l].boxed
                    && locals[*l].kind == LocalKind::Normal
                    && let Some(val) = lit_val(e)
                {
                    consts.insert(*l, val);
                }
            }
            SK::Destructure(_, e) | SK::Expr(e) | SK::Throw(e) | SK::Assert(e) => {
                self.fold_expr(e, locals, consts)?;
            }
            SK::Assign(p, e) => {
                self.fold_place(p, locals, consts)?;
                self.fold_expr(e, locals, consts)?;
            }
            SK::Return(Some(e)) | SK::Break(_, Some(e)) => self.fold_expr(e, locals, consts)?,
            SK::Return(None) | SK::Break(_, None) | SK::Continue(_) => {}
            SK::While { cond, body, .. } => {
                self.fold_expr(cond, locals, consts)?;
                self.fold_block(body, locals, consts);
            }
            SK::For { iter, body, .. } => {
                self.fold_expr(iter, locals, consts)?;
                self.fold_block(body, locals, consts);
            }
            SK::Lock { body, .. } => self.fold_block(body, locals, consts),
            SK::Block(b) => self.fold_block(b, locals, consts),
        }
        Ok(())
    }

    fn fold_place(
        &mut self,
        p: &mut Place,
        locals: &[Local],
        consts: &mut HashMap<LocalId, Val>,
    ) -> Result<(), Diag> {
        match p {
            Place::Var(_) => Ok(()),
            Place::Field(b, _) | Place::TupleIndex(b, _) => self.fold_place(b, locals, consts),
            Place::Index(b, i) => {
                self.fold_place(b, locals, consts)?;
                self.fold_expr(i, locals, consts)
            }
        }
    }

    /// Fold children first, then this node when all of its inputs are constant.
    fn fold_expr(
        &mut self,
        e: &mut Expr,
        locals: &[Local],
        consts: &mut HashMap<LocalId, Val>,
    ) -> Result<(), Diag> {
        // substitute known constants
        match &e.kind {
            EK::Var(VarRef::Local(l)) => {
                if let Some(v) = consts.get(l)
                    && let Some(x) = val_expr(v, e.ty, e.span, &self.p.types)
                {
                    *e = x;
                }
                return Ok(());
            }
            EK::Var(VarRef::Global(g)) => {
                if let Some(v) = self.gconst.get(g)
                    && let Some(x) = val_expr(v, e.ty, e.span, &self.p.types)
                {
                    *e = x;
                }
                return Ok(());
            }
            _ => {}
        }
        // children
        let mut res = Ok(());
        {
            let mut f = |c: &mut Expr| {
                if res.is_ok() {
                    res = self.fold_expr(c, locals, consts);
                }
            };
            for_children(e, &mut f);
        }
        res?;
        // nested blocks
        match &mut e.kind {
            EK::If(i) => {
                for arm in &mut i.arms {
                    self.fold_block(&mut arm.body, locals, consts);
                }
                self.select_arm(i);
            }
            EK::Else(_, Fallback::Block(b, _)) | EK::Catch(_, _, b, _) => {
                self.fold_block(b, locals, consts);
            }
            _ => {}
        }
        // this node
        let foldable = match &e.kind {
            EK::Unary(..)
            | EK::Binary(..)
            | EK::Cast(_)
            | EK::ToStr(_)
            | EK::Format(_)
            | EK::Len(_)
            | EK::Index(..)
            | EK::Slice(..) => all_children_lit(e),
            EK::Field(..) | EK::TupleIndex(..) => all_children_lit(e),
            EK::Call(Callee::Direct(f), args) => self.pure[*f] && args.iter().all(is_lit_tree),
            _ => false,
        };
        if !foldable {
            return Ok(());
        }
        self.fuel = CALL_FUEL;
        let mut env = Env {
            locals: vec![],
            dollar: vec![],
        };
        match self.expr(e, &mut env, true) {
            Ok(v) => {
                if let Some(x) = val_expr(&v, e.ty, e.span, &self.p.types) {
                    *e = x;
                }
                Ok(())
            }
            Err(Stop::Unknown) => Ok(()),
            Err(Stop::Panic(m, sp)) => Err(self.panic_diag(e.span, &m, sp)),
        }
    }

    /// When an `if` is decided at compile time, keep only the arm that runs.
    fn select_arm(&mut self, i: &mut If) {
        let scrut = match &i.scrutinee {
            Some(s) if is_lit_tree(s) => {
                let mut env = Env {
                    locals: vec![],
                    dollar: vec![],
                };
                self.fuel = CALL_FUEL;
                match self.expr(s, &mut env, true) {
                    Ok(v) => Some(v),
                    Err(_) => return,
                }
            }
            Some(_) => return,
            None => None,
        };
        for (k, arm) in i.arms.iter().enumerate() {
            // Some(true): definitely matches; Some(false): definitely not; None: unknown
            let mut decided = Some(false);
            for p in &arm.patterns {
                let m = match (p, &scrut) {
                    (Pat::Wild, _) => Some(true),
                    (
                        Pat::Cond(Expr {
                            kind: EK::Lit(Lit::Bool(b)),
                            ..
                        }),
                        None,
                    ) => Some(*b),
                    (Pat::Value(e), Some(v)) if is_lit_tree(e) => {
                        let mut env = Env {
                            locals: vec![],
                            dollar: vec![],
                        };
                        match self.expr(e, &mut env, true) {
                            Ok(x) => Some(val_eq(&x, v)),
                            Err(_) => None,
                        }
                    }
                    _ => None,
                };
                match m {
                    Some(true) => {
                        decided = Some(true);
                        break;
                    }
                    Some(false) => {}
                    None => decided = None,
                }
            }
            match decided {
                Some(true) => {
                    let mut arm = i.arms[k].clone();
                    arm.patterns = vec![Pat::Wild];
                    i.arms = vec![arm];
                    i.scrutinee = None;
                    return;
                }
                Some(false) => {}
                None => return,
            }
        }
    }

    // ------------------------------------------------------------ interpreter

    fn call_fn(&mut self, f: FuncId, args: Vec<Val>, caps: &[Val]) -> R<Flow> {
        if !self.pure[f] {
            return Err(Stop::Unknown);
        }
        if self.depth >= MAX_DEPTH {
            return Err(Stop::Unknown);
        }
        let func = &self.p.funcs[f];
        let mut env = Env {
            locals: vec![None; func.locals.len()],
            dollar: vec![],
        };
        for (i, a) in func.params.iter().zip(args) {
            env.locals[*i] = Some(a);
        }
        for (li, l) in func.locals.iter().enumerate() {
            if let LocalKind::CapVal(c) = l.kind {
                env.locals[li] = caps.get(c).cloned();
            }
        }
        if let Some(sl) = func.self_local {
            env.locals[sl] = Some(Val::Fn(f, Rc::new(caps.to_vec())));
        }
        self.depth += 1;
        let r = self.block(&func.body, &mut env);
        self.depth -= 1;
        let flow = r?;
        Ok(match flow {
            Flow::Normal => Flow::Return(Val::Void),
            other => other,
        })
    }

    /// Call a function, producing its result value (wrapping error unions).
    fn invoke(&mut self, f: FuncId, args: Vec<Val>, caps: &[Val], span: Span) -> R<Val> {
        let ret = self.p.funcs[f].ret;
        let throws = matches!(self.p.types.kind(ret), TyKind::ErrU(_));
        match self.call_fn(f, args, caps)? {
            Flow::Return(v) => Ok(if throws { Val::Ok(Box::new(v)) } else { v }),
            Flow::Throw(e) => {
                if throws {
                    Ok(Val::Err(e))
                } else {
                    let _ = span;
                    Err(Stop::Unknown)
                }
            }
            _ => Err(Stop::Unknown),
        }
    }

    fn block(&mut self, b: &Block, env: &mut Env) -> R<Flow> {
        for s in &b.stmts {
            match self.stmt(s, env)? {
                Flow::Normal => {}
                f => return Ok(f),
            }
        }
        Ok(Flow::Normal)
    }

    fn stmt(&mut self, s: &Stmt, env: &mut Env) -> R<Flow> {
        self.tick()?;
        match &s.kind {
            SK::Let(v, e) => {
                let x = self.expr_flow(e, env)?;
                let x = match x {
                    Ok(x) => x,
                    Err(f) => return Ok(f),
                };
                match v {
                    Some(VarRef::Local(l)) => env.set(*l, x),
                    Some(VarRef::Global(_)) => return Err(Stop::Unknown),
                    None => {}
                }
                Ok(Flow::Normal)
            }
            SK::Destructure(ts, e) => {
                let x = match self.expr_flow(e, env)? {
                    Ok(x) => x,
                    Err(f) => return Ok(f),
                };
                let Val::Tuple(items) = x else {
                    return Err(Stop::Unknown);
                };
                for (v, start, count) in ts {
                    let part = if *count == 1 {
                        items[*start].clone()
                    } else {
                        Val::Tuple(Rc::new(items[*start..*start + *count].to_vec()))
                    };
                    match v {
                        Some(VarRef::Local(l)) => env.set(*l, part),
                        Some(VarRef::Global(_)) => return Err(Stop::Unknown),
                        None => {}
                    }
                }
                Ok(Flow::Normal)
            }
            SK::Assign(p, e) => {
                let x = match self.expr_flow(e, env)? {
                    Ok(x) => x,
                    Err(f) => return Ok(f),
                };
                self.assign(p, x, env)?;
                Ok(Flow::Normal)
            }
            SK::Expr(e) => match self.expr_flow(e, env)? {
                Ok(_) => Ok(Flow::Normal),
                Err(f) => Ok(f),
            },
            SK::Return(e) => match e {
                Some(e) => match self.expr_flow(e, env)? {
                    Ok(v) => Ok(Flow::Return(v)),
                    Err(f) => Ok(f),
                },
                None => Ok(Flow::Return(Val::Void)),
            },
            SK::Throw(e) => match self.expr_flow(e, env)? {
                Ok(Val::Str(s)) => Ok(Flow::Throw(s)),
                Ok(_) => Err(Stop::Unknown),
                Err(f) => Ok(f),
            },
            SK::Break(id, v) => {
                let val = match v {
                    Some(e) => match self.expr_flow(e, env)? {
                        Ok(v) => Some(v),
                        Err(f) => return Ok(f),
                    },
                    None => None,
                };
                Ok(Flow::Break(*id, val))
            }
            SK::Continue(id) => Ok(Flow::Continue(*id)),
            SK::While { id, cond, body } => loop {
                self.tick()?;
                let c = match self.expr_flow(cond, env)? {
                    Ok(Val::Bool(b)) => b,
                    Ok(_) => return Err(Stop::Unknown),
                    Err(f) => return Ok(f),
                };
                if !c {
                    return Ok(Flow::Normal);
                }
                if let Some(f) = self.loop_body(*id, body, env)? {
                    return Ok(f);
                }
            },
            SK::For {
                id,
                var,
                kind,
                iter,
                body,
            } => {
                let it = match self.expr_flow(iter, env)? {
                    Ok(v) => v,
                    Err(f) => return Ok(f),
                };
                let keys: Vec<Val> = match (kind, &it) {
                    (ForKind::Index, Val::Arr(a)) => (0..a.len() as u64).map(Val::UInt).collect(),
                    (ForKind::Index, Val::Str(s)) => {
                        (0..s.chars().count() as u64).map(Val::UInt).collect()
                    }
                    (ForKind::Keys, Val::Map(m)) => m.iter().map(|(k, _)| k.clone()).collect(),
                    _ => return Err(Stop::Unknown),
                };
                for k in keys {
                    self.tick()?;
                    env.set(*var, k);
                    if let Some(f) = self.loop_body(*id, body, env)? {
                        return Ok(f);
                    }
                }
                Ok(Flow::Normal)
            }
            SK::Lock { .. } | SK::Assert(_) => Err(Stop::Unknown),
            SK::Block(b) => self.block(b, env),
        }
    }

    /// Run one iteration of loop `id`; `Some` is how the loop statement completes.
    fn loop_body(&mut self, id: JumpId, body: &Block, env: &mut Env) -> R<Option<Flow>> {
        Ok(match self.block(body, env)? {
            Flow::Normal => None,
            Flow::Break(j, None) if j == id => Some(Flow::Normal),
            Flow::Continue(j) if j == id => None,
            f => Some(f),
        })
    }

    fn assign(&mut self, p: &Place, v: Val, env: &mut Env) -> R<()> {
        // compute index values first (immutable borrow of env), then mutate
        let mut path = Vec::new();
        let root = self.place_path(p, env, &mut path)?;
        let VarRef::Local(l) = root else {
            return Err(Stop::Unknown);
        };
        let slot = env
            .locals
            .get_mut(l)
            .and_then(|x| x.as_mut())
            .ok_or(Stop::Unknown)?;
        set_path(slot, &path, v)
    }

    fn place_path(&mut self, p: &Place, env: &mut Env, path: &mut Vec<PathStep>) -> R<VarRef> {
        match p {
            Place::Var(v) => Ok(*v),
            Place::Field(b, i) | Place::TupleIndex(b, i) => {
                let r = self.place_path(b, env, path)?;
                path.push(PathStep::Elem(*i));
                Ok(r)
            }
            Place::Index(b, idx) => {
                let r = self.place_path(b, env, path)?;
                // `$` refers to the current base value
                let base = self.read_path(r, path, env)?;
                env.dollar.push(base);
                let i = self.expr(idx, env, false);
                env.dollar.pop();
                path.push(PathStep::Index(i?, idx.span));
                Ok(r)
            }
        }
    }

    fn read_path(&self, r: VarRef, path: &[PathStep], env: &Env) -> R<Val> {
        let VarRef::Local(l) = r else {
            return Err(Stop::Unknown);
        };
        let mut cur = env
            .locals
            .get(l)
            .and_then(Clone::clone)
            .ok_or(Stop::Unknown)?;
        for st in path {
            cur = match (st, &cur) {
                (PathStep::Elem(i), Val::Tuple(xs) | Val::Struct(xs)) => xs[*i].clone(),
                (PathStep::Index(k, _), Val::Map(m)) => m
                    .iter()
                    .find(|(kk, _)| kk == k)
                    .map(|x| x.1.clone())
                    .ok_or(Stop::Unknown)?,
                (PathStep::Index(i, _), Val::Arr(a)) => {
                    a.get(idx_of(i)? as usize).cloned().ok_or(Stop::Unknown)?
                }
                _ => return Err(Stop::Unknown),
            };
        }
        Ok(cur)
    }

    /// Evaluate an expression; `Err(flow)` carries a control transfer (return/throw/break).
    fn expr_flow(&mut self, e: &Expr, env: &mut Env) -> R<Result<Val, Flow>> {
        match &e.kind {
            EK::If(i) => self.if_expr(i, env),
            EK::Catch(x, local, b, id) => {
                let v = match self.expr_flow(x, env)? {
                    Ok(v) => v,
                    Err(f) => return Ok(Err(f)),
                };
                match v {
                    Val::Ok(v) => Ok(Ok(*v)),
                    Val::Err(msg) => {
                        if let Some(l) = local {
                            env.set(*l, Val::Str(msg));
                        }
                        match self.block(b, env)? {
                            Flow::Break(j, Some(v)) if j == *id => Ok(Ok(v)),
                            Flow::Normal => Ok(Ok(Val::Void)),
                            f => Ok(Err(f)),
                        }
                    }
                    _ => Err(Stop::Unknown),
                }
            }
            EK::Else(o, fb) => {
                let v = match self.expr_flow(o, env)? {
                    Ok(v) => v,
                    Err(f) => return Ok(Err(f)),
                };
                match v {
                    Val::Some(v) => Ok(Ok(*v)),
                    Val::None => match fb {
                        Fallback::Expr(x) => self.expr_flow(x, env),
                        Fallback::Block(b, id) => match self.block(b, env)? {
                            Flow::Break(j, Some(v)) if j == *id => Ok(Ok(v)),
                            Flow::Normal => Ok(Ok(Val::Void)),
                            f => Ok(Err(f)),
                        },
                    },
                    _ => Err(Stop::Unknown),
                }
            }
            EK::Try(x) => match self.expr_flow(x, env)? {
                Ok(Val::Ok(v)) => Ok(Ok(*v)),
                Ok(Val::Err(m)) => Ok(Err(Flow::Throw(m))),
                Ok(_) => Err(Stop::Unknown),
                Err(f) => Ok(Err(f)),
            },
            _ => {
                // expressions containing control flow in sub-expressions
                if contains_control(e) {
                    return Err(Stop::Unknown);
                }
                self.expr(e, env, false).map(Ok)
            }
        }
    }

    fn if_expr(&mut self, i: &If, env: &mut Env) -> R<Result<Val, Flow>> {
        let scrut = match &i.scrutinee {
            Some(s) => match self.expr_flow(s, env)? {
                Ok(v) => Some(v),
                Err(f) => return Ok(Err(f)),
            },
            None => None,
        };
        for arm in &i.arms {
            let mut matched = false;
            for p in &arm.patterns {
                let ok = match (&scrut, p) {
                    (_, Pat::Cond(c)) => match self.expr(c, env, false)? {
                        Val::Bool(b) => b,
                        _ => return Err(Stop::Unknown),
                    },
                    (Some(s), p) => self.pat_match(p, s, env)?,
                    (None, Pat::Wild) => true,
                    _ => return Err(Stop::Unknown),
                };
                if ok {
                    matched = true;
                    break;
                }
            }
            if matched {
                return match self.block(&arm.body, env)? {
                    Flow::Break(j, v) if j == i.id => Ok(Ok(v.unwrap_or(Val::Void))),
                    Flow::Normal => Ok(Ok(Val::Void)),
                    f => Ok(Err(f)),
                };
            }
        }
        Err(Stop::Unknown)
    }

    fn pat_match(&mut self, p: &Pat, v: &Val, env: &mut Env) -> R<bool> {
        Ok(match (p, v) {
            (Pat::Wild, _) => true,
            (Pat::Bind(l), v) => {
                env.set(*l, v.clone());
                true
            }
            (Pat::Value(e), v) => {
                let x = self.expr(e, env, false)?;
                val_eq(&x, v)
            }
            (Pat::Tuple(ps), Val::Tuple(xs)) | (Pat::Struct(ps), Val::Struct(xs)) => {
                for (p, x) in ps.iter().zip(xs.iter()) {
                    if !self.pat_match(p, x, env)? {
                        return Ok(false);
                    }
                }
                true
            }
            (Pat::Array(ps), Val::Arr(xs)) => {
                if ps.len() != xs.len() {
                    return Ok(false);
                }
                for (p, x) in ps.iter().zip(xs.iter()) {
                    if !self.pat_match(p, x, env)? {
                        return Ok(false);
                    }
                }
                true
            }
            (Pat::Variant(i, ps), Val::Variant(j, xs)) => {
                if i != j {
                    return Ok(false);
                }
                for (p, x) in ps.iter().zip(xs.iter()) {
                    if !self.pat_match(p, x, env)? {
                        return Ok(false);
                    }
                }
                true
            }
            _ => return Err(Stop::Unknown),
        })
    }

    /// Evaluate a value expression. With `const_only`, variables are not available.
    fn expr(&mut self, e: &Expr, env: &mut Env, const_only: bool) -> R<Val> {
        self.tick()?;
        let span = e.span;
        Ok(match &e.kind {
            EK::Lit(l) => lit_to_val(l),
            EK::None => Val::None,
            EK::Var(VarRef::Local(l)) => {
                if const_only {
                    return Err(Stop::Unknown);
                }
                env.locals
                    .get(*l)
                    .and_then(Clone::clone)
                    .ok_or(Stop::Unknown)?
            }
            EK::Var(VarRef::Global(g)) => self.gconst.get(g).cloned().ok_or(Stop::Unknown)?,
            EK::FnRef(f) => Val::Fn(*f, Rc::new(vec![])),
            EK::Closure(f) => {
                let func = &self.p.funcs[*f];
                if func.captures.iter().any(|c| c.by_ref) {
                    return Err(Stop::Unknown);
                }
                let mut caps = Vec::new();
                for c in &func.captures {
                    if const_only {
                        return Err(Stop::Unknown);
                    }
                    caps.push(
                        env.locals
                            .get(c.outer)
                            .and_then(Clone::clone)
                            .ok_or(Stop::Unknown)?,
                    );
                }
                Val::Fn(*f, Rc::new(caps))
            }
            EK::Unary(op, a) => {
                let v = self.expr(a, env, const_only)?;
                match (op, v) {
                    (UnOp::Neg, Val::Int(i)) => match i.checked_neg() {
                        Some(x) => Val::Int(x),
                        None => return self.panic("integer overflow", span),
                    },
                    (UnOp::Neg, Val::Float(f)) => Val::Float(-f),
                    (UnOp::Not, Val::Bool(b)) => Val::Bool(!b),
                    (UnOp::BitNot, Val::Int(i)) => Val::Int(!i),
                    (UnOp::BitNot, Val::UInt(i)) => Val::UInt(!i),
                    (UnOp::BitNot, Val::Byte(i)) => Val::Byte(!i),
                    _ => return Err(Stop::Unknown),
                }
            }
            EK::Binary(BinOp::And, a, b) => match self.expr(a, env, const_only)? {
                Val::Bool(false) => Val::Bool(false),
                Val::Bool(true) => self.expr(b, env, const_only)?,
                _ => return Err(Stop::Unknown),
            },
            EK::Binary(BinOp::Or, a, b) => match self.expr(a, env, const_only)? {
                Val::Bool(true) => Val::Bool(true),
                Val::Bool(false) => self.expr(b, env, const_only)?,
                _ => return Err(Stop::Unknown),
            },
            EK::Binary(op, a, b) => {
                let x = self.expr(a, env, const_only)?;
                let y = self.expr(b, env, const_only)?;
                self.binop(*op, x, y, span)?
            }
            EK::Call(c, args) => {
                if const_only && !args.iter().all(is_lit_tree) {
                    return Err(Stop::Unknown);
                }
                let mut vs = Vec::new();
                for a in args {
                    vs.push(self.expr(a, env, const_only)?);
                }
                match c {
                    Callee::Direct(f) => self.invoke(*f, vs, &[], span)?,
                    Callee::Value(fe) => match self.expr(fe, env, const_only)? {
                        Val::Fn(f, caps) => self.invoke(f, vs, &caps, span)?,
                        _ => return Err(Stop::Unknown),
                    },
                    Callee::Extern(_) => return Err(Stop::Unknown),
                }
            }
            EK::Cast(x) => {
                let v = self.expr(x, env, const_only)?;
                self.cast(v, x.ty, e.ty, span)?
            }
            EK::ToStr(x) => {
                let v = self.expr(x, env, const_only)?;
                Val::Str(self.to_str(&v, x.ty)?.into())
            }
            EK::Format(parts) => {
                let mut s = String::new();
                for p in parts {
                    match self.expr(p, env, const_only)? {
                        Val::Str(x) => s.push_str(&x),
                        _ => return Err(Stop::Unknown),
                    }
                }
                Val::Str(s.into())
            }
            EK::Field(b, i) | EK::TupleIndex(b, i) => match self.expr(b, env, const_only)? {
                Val::Struct(xs) | Val::Tuple(xs) => xs[*i].clone(),
                _ => return Err(Stop::Unknown),
            },
            EK::Index(b, idx) => {
                let base = self.expr(b, env, const_only)?;
                env.dollar.push(base.clone());
                let i = self.expr(idx, env, const_only);
                env.dollar.pop();
                let i = i?;
                match base {
                    Val::Map(m) => match m.iter().find(|(k, _)| val_eq(k, &i)) {
                        Some((_, v)) => v.clone(),
                        None => return self.panic("key not found in map", span),
                    },
                    Val::Arr(a) => {
                        let k = idx_checked(&i, idx.span)?;
                        match a.get(k as usize) {
                            Some(v) if (k as usize) < a.len() => v.clone(),
                            _ => return self.panic("index out of bounds", span),
                        }
                    }
                    Val::Str(s) => {
                        let k = idx_checked(&i, idx.span)?;
                        match s.chars().nth(k as usize) {
                            Some(c) => Val::Char(c as u32),
                            None => return self.panic("string index out of bounds", span),
                        }
                    }
                    _ => return Err(Stop::Unknown),
                }
            }
            EK::Slice(b, lo, hi) => {
                let base = self.expr(b, env, const_only)?;
                env.dollar.push(base.clone());
                let l = self.expr(lo, env, const_only);
                let h = self.expr(hi, env, const_only);
                env.dollar.pop();
                let l = idx_checked(&l?, lo.span)? as usize;
                let len = match &base {
                    Val::Arr(a) => a.len(),
                    Val::Str(s) => s.chars().count(),
                    _ => return Err(Stop::Unknown),
                };
                // inclusive bounds: `l <= h < len`
                let h = idx_checked(&h?, hi.span)? as usize;
                if l > h || h >= len {
                    return self.panic("slice out of bounds", span);
                }
                let end = h + 1;
                match base {
                    Val::Arr(a) => Val::Arr(Rc::new(a[l..end].to_vec())),
                    Val::Str(s) => {
                        Val::Str(s.chars().skip(l).take(end - l).collect::<String>().into())
                    }
                    _ => unreachable!(),
                }
            }
            EK::Dollar => {
                let len = match env.dollar.last() {
                    Some(Val::Arr(a)) => a.len(),
                    Some(Val::Str(s)) => s.chars().count(),
                    _ => return Err(Stop::Unknown),
                };
                if len == 0 {
                    return self.panic("`$` used on an empty sequence", span);
                }
                Val::UInt(len as u64 - 1)
            }
            EK::Len(b) => match self.expr(b, env, const_only)? {
                Val::Arr(a) => Val::UInt(a.len() as u64),
                Val::Str(s) => Val::UInt(s.chars().count() as u64),
                Val::Map(m) => Val::UInt(m.len() as u64),
                _ => return Err(Stop::Unknown),
            },
            EK::Tuple(xs) => Val::Tuple(Rc::new(self.exprs(xs, env, const_only)?)),
            EK::Struct(xs) => Val::Struct(Rc::new(self.exprs(xs, env, const_only)?)),
            EK::Array(xs) => Val::Arr(Rc::new(self.exprs(xs, env, const_only)?)),
            EK::Map(es) => {
                let mut m: Vec<(Val, Val)> = Vec::new();
                for (k, v) in es {
                    let k = self.expr(k, env, const_only)?;
                    let v = self.expr(v, env, const_only)?;
                    map_set(&mut m, k, v);
                }
                Val::Map(Rc::new(m))
            }
            EK::Variant(i, xs) => Val::Variant(*i, Rc::new(self.exprs(xs, env, const_only)?)),
            EK::Some(x) => Val::Some(Box::new(self.expr(x, env, const_only)?)),
            EK::Retype(x) => self.expr(x, env, const_only)?,
            EK::If(_) | EK::Else(..) | EK::Catch(..) | EK::Try(_) => {
                if const_only {
                    return Err(Stop::Unknown);
                }
                match self.expr_flow(e, env)? {
                    Ok(v) => v,
                    Err(_) => return Err(Stop::Unknown),
                }
            }
            EK::Print(..) | EK::Async(..) | EK::Await(_) | EK::Args | EK::Env => {
                return Err(Stop::Unknown);
            }
            EK::Target => match &self.p.target {
                Some((os, cpu)) => Val::Tuple(Rc::new(vec![
                    Val::Str(os.as_str().into()),
                    Val::Str(cpu.as_str().into()),
                ])),
                None => return Err(Stop::Unknown),
            },
            EK::Embed(bytes, _) => Val::Arr(Rc::new(bytes.iter().map(|b| Val::Byte(*b)).collect())),
        })
    }

    fn exprs(&mut self, xs: &[Expr], env: &mut Env, const_only: bool) -> R<Vec<Val>> {
        let mut out = Vec::new();
        for x in xs {
            out.push(self.expr(x, env, const_only)?);
        }
        Ok(out)
    }

    fn binop(&mut self, op: BinOp, x: Val, y: Val, span: Span) -> R<Val> {
        use BinOp::*;
        let ovf = |s: &Self| s.panic::<Val>("integer overflow", span);
        Ok(match (op, x, y) {
            (Eq, a, b) => Val::Bool(val_eq(&a, &b)),
            (Ne, a, b) => Val::Bool(!val_eq(&a, &b)),
            (Concat, Val::Str(a), Val::Str(b)) => Val::Str(format!("{a}{b}").into()),
            (Concat, Val::Arr(a), Val::Arr(b)) => {
                let mut v = (*a).clone();
                v.extend(b.iter().cloned());
                Val::Arr(Rc::new(v))
            }
            (Concat, Val::Map(a), Val::Map(b)) => {
                let mut v = (*a).clone();
                for (k, x) in b.iter() {
                    map_set(&mut v, k.clone(), x.clone());
                }
                Val::Map(Rc::new(v))
            }
            (In, a, Val::Arr(b)) => Val::Bool(b.iter().any(|x| val_eq(x, &a))),
            (In, a, Val::Map(b)) => Val::Bool(b.iter().any(|(k, _)| val_eq(k, &a))),
            (In, Val::Char(c), Val::Str(s)) => {
                Val::Bool(char::from_u32(c).is_some_and(|c| s.contains(c)))
            }
            (In, Val::Str(a), Val::Str(s)) => Val::Bool(s.contains(&*a)),
            (op, Val::Int(a), Val::Int(b)) => match op {
                Add => a
                    .checked_add(b)
                    .map(Val::Int)
                    .map_or_else(|| ovf(self), Ok)?,
                Sub => a
                    .checked_sub(b)
                    .map(Val::Int)
                    .map_or_else(|| ovf(self), Ok)?,
                Mul => a
                    .checked_mul(b)
                    .map(Val::Int)
                    .map_or_else(|| ovf(self), Ok)?,
                Div => {
                    if b == 0 {
                        return self.panic("division by zero", span);
                    }
                    a.checked_div(b)
                        .map(Val::Int)
                        .map_or_else(|| ovf(self), Ok)?
                }
                Mod => {
                    if b == 0 {
                        return self.panic("division by zero", span);
                    }
                    Val::Int(if b == -1 { 0 } else { a % b })
                }
                Pow => {
                    if b < 0 {
                        return self.panic("negative exponent", span);
                    }
                    let mut r: i64 = 1;
                    for _ in 0..b.min(64) {
                        r = match r.checked_mul(a) {
                            Some(r) => r,
                            None => return ovf(self),
                        };
                        if r == 0 || r == 1 && a == 1 {
                            break;
                        }
                    }
                    if b > 64 && !(a == 0 || a == 1 || a == -1) {
                        return ovf(self);
                    }
                    if b > 64 && a == -1 {
                        r = if b % 2 == 0 { 1 } else { -1 };
                    }
                    Val::Int(r)
                }
                Shl => {
                    if !(0..64).contains(&b) {
                        return self.panic("shift amount out of range", span);
                    }
                    if b == 63 {
                        match a {
                            0 => Val::Int(0),
                            -1 => Val::Int(i64::MIN),
                            _ => return ovf(self),
                        }
                    } else {
                        a.checked_mul(1i64 << b)
                            .map(Val::Int)
                            .map_or_else(|| ovf(self), Ok)?
                    }
                }
                Shr => {
                    if !(0..64).contains(&b) {
                        return self.panic("shift amount out of range", span);
                    }
                    Val::Int(a >> b)
                }
                BitAnd => Val::Int(a & b),
                BitOr => Val::Int(a | b),
                BitXor => Val::Int(a ^ b),
                Lt => Val::Bool(a < b),
                Gt => Val::Bool(a > b),
                Le => Val::Bool(a <= b),
                Ge => Val::Bool(a >= b),
                _ => return Err(Stop::Unknown),
            },
            (op, Val::UInt(a), Val::UInt(b)) => {
                self.unsigned(op, a as u128, b as u128, u64::MAX as u128, span, |v| {
                    Val::UInt(v as u64)
                })?
            }
            (op, Val::Byte(a), Val::Byte(b)) => {
                self.unsigned(op, a as u128, b as u128, 255, span, |v| Val::Byte(v as u8))?
            }
            (op, Val::Float(a), Val::Float(b)) => match op {
                Add => Val::Float(a + b),
                Sub => Val::Float(a - b),
                Mul => Val::Float(a * b),
                Div => Val::Float(a / b),
                Pow => Val::Float(a.powf(b)),
                Lt => Val::Bool(a < b),
                Gt => Val::Bool(a > b),
                Le => Val::Bool(a <= b),
                Ge => Val::Bool(a >= b),
                _ => return Err(Stop::Unknown),
            },
            (op, Val::Char(a), Val::Char(b)) => match op {
                Lt => Val::Bool(a < b),
                Gt => Val::Bool(a > b),
                Le => Val::Bool(a <= b),
                Ge => Val::Bool(a >= b),
                _ => return Err(Stop::Unknown),
            },
            _ => return Err(Stop::Unknown),
        })
    }

    fn unsigned(
        &self,
        op: BinOp,
        a: u128,
        b: u128,
        max: u128,
        span: Span,
        mk: impl Fn(u128) -> Val,
    ) -> R<Val> {
        use BinOp::*;
        let bits = if max == 255 { 8 } else { 64 };
        let chk = |v: Option<u128>| -> R<Val> {
            match v {
                Some(v) if v <= max => Ok(mk(v)),
                _ => Err(Stop::Panic("integer overflow".into(), span)),
            }
        };
        match op {
            Add => chk(a.checked_add(b)),
            Sub => chk(a.checked_sub(b)),
            Mul => chk(a.checked_mul(b)),
            Div | Mod => {
                if b == 0 {
                    return self.panic("division by zero", span);
                }
                Ok(mk(if op == Div { a / b } else { a % b }))
            }
            Pow => {
                let mut r: u128 = 1;
                let mut i = 0;
                while i < b {
                    r = match r.checked_mul(a) {
                        Some(r) if r <= max => r,
                        _ => return self.panic("integer overflow", span),
                    };
                    if r == 0 || r == 1 {
                        break;
                    }
                    i += 1;
                }
                Ok(mk(r))
            }
            Shl => {
                if b >= bits {
                    return self.panic("shift amount out of range", span);
                }
                chk(Some(a << b))
            }
            Shr => {
                if b >= bits {
                    return self.panic("shift amount out of range", span);
                }
                Ok(mk(a >> b))
            }
            BitAnd => Ok(mk(a & b)),
            BitOr => Ok(mk(a | b)),
            BitXor => Ok(mk(a ^ b)),
            Lt => Ok(Val::Bool(a < b)),
            Gt => Ok(Val::Bool(a > b)),
            Le => Ok(Val::Bool(a <= b)),
            Ge => Ok(Val::Bool(a >= b)),
            _ => Err(Stop::Unknown),
        }
    }

    fn cast(&mut self, v: Val, from: TyId, to: TyId, span: Span) -> R<Val> {
        let t = &self.p.types;
        if t.base(to) == STR {
            return Ok(Val::Str(self.to_str(&v, from)?.into()));
        }
        Ok(match (v, t.kind(to)) {
            (Val::Bool(b), TyKind::Int) => Val::Int(b as i64),
            (Val::Bool(b), TyKind::UInt) => Val::UInt(b as u64),
            (Val::Byte(b), TyKind::Char) => Val::Char(b as u32),
            (Val::Byte(b), TyKind::Int) => Val::Int(b as i64),
            (Val::Byte(b), TyKind::UInt) => Val::UInt(b as u64),
            (Val::Int(i), TyKind::UInt) => {
                if i < 0 {
                    return self.panic("negative value cannot be converted to uint", span);
                }
                Val::UInt(i as u64)
            }
            (Val::Int(i), TyKind::Float) => Val::Float(i as f64),
            (Val::UInt(u), TyKind::Int) => {
                if u > i64::MAX as u64 {
                    return self.panic("value out of range for int", span);
                }
                Val::Int(u as i64)
            }
            (Val::UInt(u), TyKind::Float) => Val::Float(u as f64),
            (Val::Float(f), TyKind::Int) => {
                if !(f > -9223372036854775809.0 && f < 9223372036854775808.0) {
                    return self.panic("float value out of range for int", span);
                }
                Val::Int(f as i64)
            }
            (Val::Float(f), TyKind::UInt) => {
                if !(f > -1.0 && f < 18446744073709551616.0) {
                    return self.panic("float value out of range for uint", span);
                }
                Val::UInt(f as u64)
            }
            (Val::Char(c), TyKind::Array(_)) => {
                let s = char::from_u32(c).ok_or(Stop::Unknown)?.to_string();
                Val::Arr(Rc::new(s.bytes().map(Val::Byte).collect()))
            }
            (Val::Int(i), TyKind::Array(_)) => Val::Arr(Rc::new(
                i.to_le_bytes().iter().map(|b| Val::Byte(*b)).collect(),
            )),
            (Val::UInt(i), TyKind::Array(_)) => Val::Arr(Rc::new(
                i.to_le_bytes().iter().map(|b| Val::Byte(*b)).collect(),
            )),
            (Val::Float(f), TyKind::Array(_)) => Val::Arr(Rc::new(
                f.to_bits()
                    .to_le_bytes()
                    .iter()
                    .map(|b| Val::Byte(*b))
                    .collect(),
            )),
            (Val::Str(s), TyKind::Array(e)) if *e == CHAR => {
                Val::Arr(Rc::new(s.chars().map(|c| Val::Char(c as u32)).collect()))
            }
            (Val::Str(s), TyKind::Array(_)) => {
                Val::Arr(Rc::new(s.bytes().map(Val::Byte).collect()))
            }
            (v, _) => v,
        })
    }

    /// String conversion matching the C runtime exactly.
    fn to_str(&self, v: &Val, ty: TyId) -> R<String> {
        let mut s = String::new();
        self.str_into(&mut s, v, ty)?;
        Ok(s)
    }

    fn str_into(&self, s: &mut String, v: &Val, ty: TyId) -> R<()> {
        let t = &self.p.types;
        let ty = t.base(ty);
        match (v, t.kind(ty)) {
            (Val::Void, _) => {}
            (Val::Bool(b), _) => s.push_str(if *b { "true" } else { "false" }),
            (Val::Byte(b), _) => s.push_str(&b.to_string()),
            (Val::Int(i), _) => s.push_str(&i.to_string()),
            (Val::UInt(u), _) => s.push_str(&u.to_string()),
            (Val::Float(f), _) => s.push_str(&fmt_float(*f)),
            (Val::Char(c), _) => s.push(char::from_u32(*c).unwrap_or('\u{FFFD}')),
            (Val::Str(x), _) => s.push_str(x),
            (Val::Arr(xs), TyKind::Array(e) | TyKind::Fixed(e, _)) => {
                s.push('[');
                for (i, x) in xs.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    self.str_into(s, x, *e)?;
                }
                s.push(']');
            }
            (Val::Arr(xs), TyKind::Str) => {
                for x in xs.iter() {
                    self.str_into(s, x, CHAR)?;
                }
            }
            // map iteration order differs from the runtime's hash order: not foldable
            (Val::Map(_), _) => return Err(Stop::Unknown),
            (Val::Tuple(xs), TyKind::Tuple(ts)) => {
                s.push('(');
                for (i, x) in xs.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    self.str_into(s, x, ts[i])?;
                }
                s.push(')');
            }
            (Val::Struct(xs), TyKind::Struct(si)) => {
                let def = &t.structs[*si as usize];
                s.push_str(&def.name);
                s.push('{');
                for (i, x) in xs.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    s.push('.');
                    s.push_str(&def.fields[i].0);
                    s.push_str(" = ");
                    self.str_into(s, x, def.fields[i].1)?;
                }
                s.push('}');
            }
            (Val::Variant(vi, xs), TyKind::Enum(ei)) => {
                let def = &t.enums[*ei as usize];
                s.push_str(&def.name);
                s.push('.');
                s.push_str(&def.variants[*vi].0);
                if !xs.is_empty() {
                    s.push('(');
                    for (i, x) in xs.iter().enumerate() {
                        if i > 0 {
                            s.push_str(", ");
                        }
                        self.str_into(s, x, def.variants[*vi].1[i])?;
                    }
                    s.push(')');
                }
            }
            (Val::None, _) => s.push_str("none"),
            (Val::Some(x), TyKind::Opt(i)) | (Val::Ok(x), TyKind::ErrU(i)) => {
                self.str_into(s, x, *i)?
            }
            (Val::Err(m), _) => {
                s.push_str("error: ");
                s.push_str(m);
            }
            _ => return Err(Stop::Unknown),
        }
        Ok(())
    }
}

struct Env {
    locals: Vec<Option<Val>>,
    dollar: Vec<Val>,
}

impl Env {
    fn set(&mut self, l: LocalId, v: Val) {
        if l >= self.locals.len() {
            self.locals.resize(l + 1, None);
        }
        self.locals[l] = Some(v);
    }
}

enum PathStep {
    Elem(usize),
    Index(Val, Span),
}

fn set_path(slot: &mut Val, path: &[PathStep], v: Val) -> R<()> {
    if path.is_empty() {
        *slot = v;
        return Ok(());
    }
    match (&path[0], slot) {
        (PathStep::Elem(i), Val::Tuple(xs) | Val::Struct(xs)) => {
            set_path(&mut Rc::make_mut(xs)[*i], &path[1..], v)
        }
        (PathStep::Index(i, isp), Val::Arr(xs)) => {
            let k = idx_checked(i, *isp)? as usize;
            let xs = Rc::make_mut(xs);
            if k >= xs.len() {
                return Err(Stop::Panic("index out of bounds".into(), *isp));
            }
            set_path(&mut xs[k], &path[1..], v)
        }
        (PathStep::Index(k, _), Val::Map(m)) => {
            let m = Rc::make_mut(m);
            if let Some(pos) = m.iter().position(|(kk, _)| val_eq(kk, k)) {
                set_path(&mut m[pos].1, &path[1..], v)
            } else if path.len() == 1 {
                m.push((k.clone(), v));
                Ok(())
            } else {
                Err(Stop::Unknown)
            }
        }
        (PathStep::Index(i, isp), Val::Str(s)) if path.len() == 1 => {
            let k = idx_checked(i, *isp)? as usize;
            let Val::Char(c) = v else {
                return Err(Stop::Unknown);
            };
            let mut chars: Vec<char> = s.chars().collect();
            if k >= chars.len() {
                return Err(Stop::Panic("string index out of bounds".into(), *isp));
            }
            chars[k] = char::from_u32(c).ok_or(Stop::Unknown)?;
            *s = chars.into_iter().collect::<String>().into();
            Ok(())
        }
        _ => Err(Stop::Unknown),
    }
}

fn idx_of(v: &Val) -> R<u64> {
    match v {
        Val::UInt(u) => Ok(*u),
        Val::Int(i) if *i >= 0 => Ok(*i as u64),
        _ => Err(Stop::Unknown),
    }
}

fn idx_checked(v: &Val, span: Span) -> R<u64> {
    match v {
        Val::UInt(u) => Ok(*u),
        Val::Int(i) if *i >= 0 => Ok(*i as u64),
        Val::Int(_) => Err(Stop::Panic("negative index".into(), span)),
        _ => Err(Stop::Unknown),
    }
}

fn map_set(m: &mut Vec<(Val, Val)>, k: Val, v: Val) {
    if let Some(e) = m.iter_mut().find(|(kk, _)| val_eq(kk, &k)) {
        e.1 = v;
    } else {
        m.push((k, v));
    }
}

/// Structural equality with IEEE semantics for floats.
fn val_eq(a: &Val, b: &Val) -> bool {
    match (a, b) {
        (Val::Float(x), Val::Float(y)) => x == y,
        (Val::Arr(x), Val::Arr(y))
        | (Val::Tuple(x), Val::Tuple(y))
        | (Val::Struct(x), Val::Struct(y)) => {
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(a, b)| val_eq(a, b))
        }
        (Val::Variant(i, x), Val::Variant(j, y)) => {
            i == j && x.iter().zip(y.iter()).all(|(a, b)| val_eq(a, b))
        }
        (Val::Map(x), Val::Map(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.iter().any(|(k2, v2)| val_eq(k, k2) && val_eq(v, v2)))
        }
        (Val::Some(x), Val::Some(y)) | (Val::Ok(x), Val::Ok(y)) => val_eq(x, y),
        (Val::Fn(..), _) => false,
        _ => a == b,
    }
}

fn lit_to_val(l: &Lit) -> Val {
    match l {
        Lit::Bool(b) => Val::Bool(*b),
        Lit::Byte(b) => Val::Byte(*b),
        Lit::Int(i) => Val::Int(*i),
        Lit::UInt(u) => Val::UInt(*u),
        Lit::Float(f) => Val::Float(*f),
        Lit::Char(c) => Val::Char(*c),
        Lit::Str(s) => Val::Str(s.as_str().into()),
    }
}

fn lit_val(e: &Expr) -> Option<Val> {
    match &e.kind {
        EK::Lit(l) => Some(lit_to_val(l)),
        _ => None,
    }
}

/// Is this expression a tree of literals (suitable as constant call arguments)?
fn is_lit_tree(e: &Expr) -> bool {
    match &e.kind {
        EK::Lit(_) | EK::None | EK::FnRef(_) => true,
        EK::Tuple(xs) | EK::Array(xs) | EK::Struct(xs) | EK::Variant(_, xs) => {
            xs.iter().all(is_lit_tree)
        }
        EK::Map(es) => es.iter().all(|(k, v)| is_lit_tree(k) && is_lit_tree(v)),
        EK::Some(x) | EK::Retype(x) => is_lit_tree(x),
        _ => false,
    }
}

fn all_children_lit(e: &Expr) -> bool {
    let mut ok = true;
    let mut f = |c: &Expr| {
        if !is_lit_tree(c) && !matches!(c.kind, EK::Dollar) {
            ok = false;
        }
    };
    match &e.kind {
        EK::Unary(_, a)
        | EK::Cast(a)
        | EK::ToStr(a)
        | EK::Len(a)
        | EK::Field(a, _)
        | EK::TupleIndex(a, _) => f(a),
        EK::Binary(_, a, b) | EK::Index(a, b) => {
            f(a);
            f(b);
        }
        EK::Slice(a, lo, hi) => {
            f(a);
            f(lo);
            f(hi);
        }
        EK::Format(ps) => ps.iter().for_each(&mut f),
        _ => ok = false,
    }
    ok
}

fn contains_control(e: &Expr) -> bool {
    let mut found = false;
    flow::visit_expr(e, &mut |x: &Expr| {
        if matches!(
            x.kind,
            EK::If(_) | EK::Catch(..) | EK::Else(..) | EK::Try(_)
        ) {
            found = true;
        }
    });
    found
}

/// Apply `f` to each direct sub-expression (not inside nested blocks).
fn for_children(e: &mut Expr, f: &mut dyn FnMut(&mut Expr)) {
    match &mut e.kind {
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
        | EK::Await(a) => f(a),
        EK::Binary(_, a, b) | EK::Index(a, b) => {
            f(a);
            f(b);
        }
        EK::Slice(a, lo, hi) => {
            f(a);
            f(lo);
            f(hi);
        }
        EK::Call(c, args) | EK::Async(c, args) => {
            if let Callee::Value(x) = c {
                f(x);
            }
            args.iter_mut().for_each(f);
        }
        EK::Print(_, _, xs)
        | EK::Format(xs)
        | EK::Tuple(xs)
        | EK::Array(xs)
        | EK::Struct(xs)
        | EK::Variant(_, xs) => xs.iter_mut().for_each(f),
        EK::Map(es) => {
            for (k, v) in es.iter_mut() {
                f(k);
                f(v);
            }
        }
        EK::If(i) => {
            if let Some(s) = &mut i.scrutinee {
                f(s);
            }
        }
        EK::Else(a, fb) => {
            f(a);
            if let Fallback::Expr(b) = fb {
                f(b);
            }
        }
        EK::Catch(a, _, _, _) => f(a),
    }
}

/// Convert a value back into an IR expression (literals only).
fn val_expr(v: &Val, ty: TyId, span: Span, t: &Types) -> Option<Expr> {
    let lit = |l| {
        Some(Expr {
            kind: EK::Lit(l),
            ty,
            span,
        })
    };
    let b = t.base(ty);
    match v {
        Val::Bool(x) => lit(Lit::Bool(*x)),
        Val::Byte(x) => lit(Lit::Byte(*x)),
        Val::Int(x) => lit(Lit::Int(*x)),
        Val::UInt(x) => lit(Lit::UInt(*x)),
        Val::Float(x) => lit(Lit::Float(*x)),
        Val::Char(x) => lit(Lit::Char(*x)),
        Val::Str(s) if b == STR || b == ERROR => lit(Lit::Str(s.to_string())),
        Val::None if matches!(t.kind(b), TyKind::Opt(_)) => Some(Expr {
            kind: EK::None,
            ty,
            span,
        }),
        Val::Some(x) => match t.kind(b) {
            TyKind::Opt(i) => {
                let inner = val_expr(x, *i, span, t)?;
                Some(Expr {
                    kind: EK::Some(Box::new(inner)),
                    ty,
                    span,
                })
            }
            _ => None,
        },
        Val::Tuple(xs) => match t.kind(b) {
            TyKind::Tuple(ts) => {
                let items: Option<Vec<Expr>> = xs
                    .iter()
                    .zip(ts)
                    .map(|(x, et)| val_expr(x, *et, span, t))
                    .collect();
                Some(Expr {
                    kind: EK::Tuple(items?),
                    ty,
                    span,
                })
            }
            _ => None,
        },
        Val::Struct(xs) => match t.kind(b) {
            TyKind::Struct(si) => {
                let fs = &t.structs[*si as usize].fields;
                let items: Option<Vec<Expr>> = xs
                    .iter()
                    .zip(fs)
                    .map(|(x, (_, ft))| val_expr(x, *ft, span, t))
                    .collect();
                Some(Expr {
                    kind: EK::Struct(items?),
                    ty,
                    span,
                })
            }
            _ => None,
        },
        Val::Arr(xs) => {
            let et = t.elem(b)?;
            if b == STR {
                return None;
            }
            let items: Option<Vec<Expr>> = xs.iter().map(|x| val_expr(x, et, span, t)).collect();
            Some(Expr {
                kind: EK::Array(items?),
                ty,
                span,
            })
        }
        Val::Variant(i, xs) => match t.kind(b) {
            TyKind::Enum(ei) => {
                let ps = &t.enums[*ei as usize].variants[*i].1;
                let items: Option<Vec<Expr>> = xs
                    .iter()
                    .zip(ps)
                    .map(|(x, pt)| val_expr(x, *pt, span, t))
                    .collect();
                Some(Expr {
                    kind: EK::Variant(*i, items?),
                    ty,
                    span,
                })
            }
            _ => None,
        },
        _ => None,
    }
}

/// Float formatting identical to the runtime's `nc_sb_f64`.
pub fn fmt_float(v: f64) -> String {
    if v.is_nan() {
        return "nan".into();
    }
    if v.is_infinite() {
        return if v < 0.0 { "-inf".into() } else { "inf".into() };
    }
    // shortest round-trip digits via Rust's formatting
    let sci = format!("{v:e}");
    let (mant, exp) = sci.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let neg = mant.starts_with('-');
    let digits: String = mant.chars().filter(char::is_ascii_digit).collect();
    let prec = digits.len() as i32;
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if (-5..17).contains(&exp) {
        let decimals = (prec - 1 - exp).max(0);
        // place the decimal point
        if exp >= 0 {
            let int_len = (exp + 1) as usize;
            let mut d = digits.clone();
            while d.len() < int_len + decimals as usize {
                d.push('0');
            }
            out.push_str(&d[..int_len]);
            if decimals > 0 {
                out.push('.');
                out.push_str(&d[int_len..int_len + decimals as usize]);
            } else {
                out.push_str(".0");
            }
        } else {
            out.push_str("0.");
            for _ in 0..(-exp - 1) {
                out.push('0');
            }
            out.push_str(&digits);
        }
    } else {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if exp < 0 { '-' } else { '+' });
        out.push_str(&format!("{:02}", exp.abs()));
    }
    out
}
