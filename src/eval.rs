//! Compile-time evaluation of pure code.
//!
//! Used in every mode to reject programs that are guaranteed to panic (e.g. a call with
//! constant arguments that overflows), and in release mode to fold constant expressions and
//! pure calls with constant arguments into literals.

use std::collections::{HashMap, HashSet};
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
    Arr(Rc<Vec<Self>>),
    /// insertion-ordered association list
    Map(Rc<Vec<(Self, Self)>>),
    Tuple(Rc<Vec<Self>>),
    Struct(Rc<Vec<Self>>),
    Variant(usize, Rc<Vec<Self>>),
    None,
    Some(Box<Self>),
    Ok(Box<Self>),
    Err(Rc<str>),
    /// function value: function id and captured values (by value only)
    Fn(FuncId, Rc<Vec<Self>>),
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

/// The value of an `expr_flow` result, returning any control transfer it carries: as the
/// statement's `Flow`, or (with `expr`) as the enclosing expression's.
macro_rules! value {
    ($r:expr) => {
        match $r? {
            Ok(v) => v,
            Err(f) => return Ok(f),
        }
    };
    (expr $r:expr) => {
        match $r? {
            Ok(v) => v,
            Err(f) => return Ok(Err(f)),
        }
    };
}

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
    /// per function, the globals it may change, directly or through calls (`None`: any)
    gwrites: Vec<Option<HashSet<GlobalId>>>,
    /// interpreter steps left for the whole propagation pass
    budget: u64,
    /// `while` loops whose condition propagation found true at the start of every iteration
    endless: HashSet<(FuncId, JumpId)>,
}

const CALL_FUEL: u64 = 2_000_000;
/// Largest value (in nodes) written into the program as a literal by propagation.
const MAX_LITERAL: usize = 1024;
/// Interpreter steps for all of release propagation, so failed attempts stay cheap overall.
const PROP_FUEL: u64 = 50_000_000;
const MAX_DEPTH: usize = 200;

pub fn purity(p: &Program) -> Vec<bool> {
    let n = p.funcs.len();
    let mut pure = vec![true; n];
    let mut calls: Vec<Vec<FuncId>> = vec![vec![]; n];
    for (i, f) in p.funcs.iter().enumerate() {
        let mut ok = !f
            .locals
            .iter()
            .any(|l| matches!(l.kind, LocalKind::CapRef(_) | LocalKind::LockAlias));
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
        pure[i] = ok && f.body.stmts.iter().all(stmt_pure);
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
    let mut w = HashSet::new();
    flow::writes_stmt(s, &mut w);
    let mut ok = w.is_empty();
    flow::visit_stmts(s, &mut |s| {
        ok &= !matches!(s.kind, SK::Lock { .. } | SK::Assert(_));
    });
    ok
}

impl<'p> Evaluator<'p> {
    pub fn new(p: &'p Program, sm: &'p SourceMap) -> Self {
        Evaluator {
            p,
            sm,
            pure: purity(p),
            fuel: 0,
            depth: 0,
            gconst: HashMap::new(),
            errs: vec![],
            gwrites: vec![],
            budget: PROP_FUEL,
            endless: HashSet::new(),
        }
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
        let (funcs, mut errs) = {
            let mut ev = Evaluator::new(p, sm);
            ev.init_gconst();
            let mut funcs = p.funcs.clone();
            for f in &mut funcs {
                ev.fold_block(&mut f.body, &f.locals, &mut HashMap::new());
            }
            (funcs, ev.errs)
        };
        if !errs.is_empty() {
            crate::diag::sort_dedup(&mut errs);
            return Err(errs);
        }
        if fold {
            p.funcs = funcs;
            let funcs = {
                let mut ev = Evaluator::new(p, sm);
                ev.init_gconst();
                ev.propagate()
            };
            p.funcs = funcs;
        }
        Ok(())
    }

    /// Constants for immutable globals with constant initialisers (in main, in order).
    fn init_gconst(&mut self) {
        let p = self.p;
        for s in &p.funcs[p.main].body.stmts {
            if let SK::Let(Some(VarRef::Global(g)), e) = &s.kind
                && !p.globals[*g].mutable
                // panics are reported when the statement itself is folded
                && let Ok(Some(v)) = self.try_const(e)
            {
                self.gconst.insert(*g, v);
            }
            // destructured immutable globals: `str os, str cpu = @target()`
            if let SK::Destructure(targets, e) = &s.kind
                && let Ok(Some(Val::Tuple(items))) = self.try_const(e)
            {
                for (t, start, count) in targets {
                    if let Some(VarRef::Global(g)) = t
                        && !p.globals[*g].mutable
                    {
                        self.gconst
                            .insert(*g, destructure_part(&items, *start, *count));
                    }
                }
            }
        }
    }

    /// Evaluate an expression that is fully constant (no variables), if possible.
    fn try_const(&mut self, e: &Expr) -> Result<Option<Val>, Diag> {
        self.fuel = CALL_FUEL;
        let mut env = Env::new(0);
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
                format!(
                    "this expression always panics: {msg} ({})",
                    self.sm.location(inner)
                ),
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
                if let (Some(VarRef::Local(l)), EK::Lit(lit)) = (v, &e.kind)
                    && !locals[*l].mutable
                    && !locals[*l].boxed
                    && locals[*l].kind == LocalKind::Normal
                {
                    consts.insert(*l, lit_to_val(lit));
                }
            }
            SK::Destructure(_, e)
            | SK::Expr(e)
            | SK::Throw(e)
            | SK::Assert(e)
            | SK::Return(Some(e))
            | SK::Break(_, Some(e)) => self.fold_expr(e, locals, consts)?,
            SK::Assign(p, e) => {
                self.fold_each(|f| place_indices(p, f), locals, consts)?;
                self.fold_expr(e, locals, consts)?;
            }
            SK::Return(None) | SK::Break(_, None) | SK::Continue(_) => {}
            SK::While { cond: e, body, .. } | SK::For { iter: e, body, .. } => {
                self.fold_expr(e, locals, consts)?;
                self.fold_block(body, locals, consts);
            }
            SK::Lock { body: b, .. } | SK::Block(b) => self.fold_block(b, locals, consts),
        }
        Ok(())
    }

    /// `fold_expr` on each expression `each` passes on, up to the first error.
    fn fold_each(
        &mut self,
        each: impl FnOnce(&mut dyn FnMut(&mut Expr)),
        locals: &[Local],
        consts: &mut HashMap<LocalId, Val>,
    ) -> Result<(), Diag> {
        let mut res = Ok(());
        each(&mut |c| {
            if res.is_ok() {
                res = self.fold_expr(c, locals, consts);
            }
        });
        res
    }

    /// Fold children first, then this node when all of its inputs are constant.
    fn fold_expr(
        &mut self,
        e: &mut Expr,
        locals: &[Local],
        consts: &mut HashMap<LocalId, Val>,
    ) -> Result<(), Diag> {
        // substitute known constants
        if let EK::Var(v) = &e.kind {
            let known = match v {
                VarRef::Local(l) => consts.get(l),
                VarRef::Global(g) => self.gconst.get(g),
            };
            if let Some(x) = known.and_then(|v| val_expr(v, e.ty, e.span, &self.p.types)) {
                *e = x;
            }
            return Ok(());
        }
        self.fold_each(|f| e.for_each_child_mut(f), locals, consts)?;
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
            | EK::Slice(..)
            | EK::Field(..)
            | EK::TupleIndex(..) => all_children_lit(e),
            EK::Call(Callee::Direct(f), args) => self.pure[*f] && args.iter().all(is_lit_tree),
            _ => false,
        };
        if foldable
            && let Some(v) = self.try_const(e)?
            && let Some(x) = val_expr(&v, e.ty, e.span, &self.p.types)
        {
            *e = x;
        }
        Ok(())
    }

    /// When an `if` is decided at compile time, keep only the arm that runs.
    fn select_arm(&mut self, i: &mut If) {
        let scrut = match &i.scrutinee {
            Some(s) if is_lit_tree(s) => match self.try_const(s) {
                Ok(Some(v)) => Some(v),
                _ => return,
            },
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
                    (Pat::Value(e), Some(v)) if is_lit_tree(e) => match self.try_const(e) {
                        Ok(Some(x)) => Some(val_eq(&x, v)),
                        _ => None,
                    },
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

    // ------------------------------------------------------------ release propagation
    //
    // Release mode only, after folding: walks each function body in order, tracking the
    // values of variables (mutable ones and top-level globals included) that are known at
    // each point. A statement whose inputs are known is run by the interpreter: when it
    // completes without effects (the interpreter refuses output, impure calls and anything
    // it cannot model, and its fuel bounds loops), its result replaces it. A loop that
    // finishes within the fuel is thereby proven to terminate, and becomes assignments of the
    // final values of the variables it changed. Statements that cannot run have their
    // expressions folded with what is known. Panics are left to happen at run time.

    fn propagate(&mut self) -> Vec<Func> {
        let p = self.p;
        // spawned tasks may change globals at any time
        let mut has_async = false;
        for f in &p.funcs {
            for s in &f.body.stmts {
                flow::visit_stmt(s, &mut |e| has_async |= matches!(e.kind, EK::Async(..)));
            }
        }
        self.gwrites = global_writes(p);
        let mut funcs = p.funcs.clone();
        for (fi, f) in funcs.iter_mut().enumerate() {
            let mut env = Env::new(f.locals.len());
            env.track_globals = !has_async;
            self.prop_block(&mut f.body, fi, &mut env);
        }
        funcs
    }

    /// Whether the value of `v` can be tracked: it is not shared with other code.
    fn trackable(&self, f: FuncId, v: VarRef, track_globals: bool) -> bool {
        match v {
            VarRef::Local(l) => {
                let local = &self.p.funcs[f].locals[l];
                matches!(local.kind, LocalKind::Normal | LocalKind::Param) && !local.boxed
            }
            VarRef::Global(g) => {
                track_globals
                    && !matches!(self.p.types.kind(self.p.globals[g].ty), TyKind::Mutex(_))
            }
        }
    }

    /// Drop what the interpreter learnt about variables that are not trackable.
    fn forget_untracked(&self, f: FuncId, env: &mut Env) {
        for l in 0..env.locals.len() {
            if env.locals[l].is_some() && !self.trackable(f, VarRef::Local(l), true) {
                env.forget(VarRef::Local(l));
            }
        }
        let tg = env.track_globals;
        env.forget_globals(|g| !self.trackable(f, VarRef::Global(g), tg));
    }

    fn var_ty(&self, f: FuncId, v: VarRef) -> TyId {
        match v {
            VarRef::Local(l) => self.p.funcs[f].locals[l].ty,
            VarRef::Global(g) => self.p.globals[g].ty,
        }
    }

    /// The globals that code called by `s` may change; `None` when any global may change.
    fn clobbers(
        &self,
        s: &Stmt,
        gwrites: &[Option<HashSet<GlobalId>>],
    ) -> Option<HashSet<GlobalId>> {
        let mut out = Some(HashSet::new());
        flow::visit_stmts(s, &mut |s| {
            if matches!(s.kind, SK::Lock { .. }) {
                out = None;
            }
        });
        flow::visit_stmt(s, &mut |e| match &e.kind {
            EK::Call(Callee::Direct(f), _) => match (&gwrites[*f], &mut out) {
                (Some(w), Some(o)) => o.extend(w),
                (None, _) => out = None,
                _ => {}
            },
            // C code cannot reach NC globals
            EK::Call(Callee::Extern(_), _) => {}
            EK::Call(..) | EK::Async(..) | EK::Await(_) => out = None,
            _ => {}
        });
        out
    }

    /// Run the interpreter with the fuel for one attempt, taken from the propagation budget.
    fn fueled<T>(&mut self, run: impl FnOnce(&mut Self) -> T) -> T {
        let fuel = CALL_FUEL.min(self.budget);
        self.fuel = fuel;
        let r = run(self);
        self.budget -= fuel - self.fuel;
        r
    }

    /// The value of `e` from what `env` knows, if the interpreter finds it.
    fn trial_expr(&mut self, e: &Expr, env: &mut Env) -> R<Val> {
        let r = self.fueled(|ev| ev.expr(e, env, false));
        env.dollar.clear();
        r
    }

    fn prop_block(&mut self, b: &mut Block, f: FuncId, env: &mut Env) {
        for s in &mut b.stmts {
            self.prop_stmt(s, f, env);
        }
    }

    /// `prop_block`, leaving `env` as it was.
    fn prop_scoped(&mut self, b: &mut Block, f: FuncId, env: &mut Env) {
        env.checkpoint();
        self.prop_block(b, f, env);
        env.rollback();
    }

    fn prop_stmt(&mut self, s: &mut Stmt, f: FuncId, env: &mut Env) {
        if let SK::Block(b) = &mut s.kind {
            return self.prop_block(b, f, env);
        }
        let (assigned, declared) = written(s);
        let runnable = matches!(
            s.kind,
            SK::Let(..) | SK::Assign(..) | SK::While { .. } | SK::For { .. } | SK::Expr(_)
        );
        if runnable {
            // a trial run, undone unless it completes
            env.checkpoint();
            let r = self.fueled(|ev| ev.stmt(s, env));
            if let Ok(Flow::Normal) = r {
                env.dollar.clear();
                let replaced = self.replacement(s, f, env, &assigned, &declared);
                self.forget_untracked(f, env);
                match replaced {
                    Some(kind) => {
                        s.kind = kind;
                        env.commit();
                    }
                    None => {
                        // the result is known but has no literal form: fold inside with what
                        // was known before the statement, then keep the result
                        let after = env.undo();
                        env.checkpoint();
                        env.forget_all(assigned.iter().chain(&declared));
                        self.prop_inside(s, f, env);
                        env.rollback();
                        env.redo(after);
                    }
                }
                return;
            }
            env.rollback();
        }
        let clobbers = self.clobbers(s, &self.gwrites);
        let nested = matches!(s.kind, SK::While { .. } | SK::For { .. } | SK::Lock { .. })
            || flow::has_nested_blocks(s);
        // what is known inside, undone afterwards
        env.checkpoint();
        if nested {
            env.forget_all(assigned.iter().chain(&declared));
        }
        forget_globals(env, &clobbers);
        // this holds what is true at the start of every iteration of a loop: a condition that
        // is true from that alone never becomes false
        if let SK::While { id, cond, .. } = &s.kind
            && let Ok(Val::Bool(true)) = self.trial_expr(cond, env)
        {
            self.endless.insert((f, *id));
        }
        self.prop_inside(s, f, env);
        env.rollback();
        env.forget_all(assigned.iter().chain(&declared));
        forget_globals(env, &clobbers);
        self.forget_untracked(f, env);
    }

    /// The statement replacing `s`, which ran to completion leaving `after`, when its
    /// results can be written as literals.
    fn replacement(
        &self,
        s: &Stmt,
        f: FuncId,
        after: &Env,
        assigned: &HashSet<VarRef>,
        declared: &HashSet<VarRef>,
    ) -> Option<SK> {
        let lit = |v: VarRef| -> Option<Expr> {
            let val = after.var(v).ok()?;
            if val_size(&val) > MAX_LITERAL {
                return None;
            }
            val_expr(&val, self.var_ty(f, v), s.span, &self.p.types)
        };
        let assign = |v: VarRef| -> Option<Stmt> {
            Some(Stmt {
                kind: SK::Assign(Place::Var(v), lit(v)?),
                span: s.span,
            })
        };
        match &s.kind {
            SK::Let(Some(v), _) => Some(SK::Let(Some(*v), lit(*v)?)),
            SK::Let(None, _) => Some(SK::Block(Block { stmts: vec![] })),
            SK::Assign(pl, _) => {
                let root = flow::place_root(pl)?;
                Some(assign(root)?.kind)
            }
            _ => {
                let mut outer: Vec<VarRef> = assigned.difference(declared).copied().collect();
                outer.sort_by_key(|v| match v {
                    VarRef::Global(g) => (0, *g),
                    VarRef::Local(l) => (1, *l),
                });
                let stmts = outer.into_iter().map(assign).collect::<Option<Vec<_>>>()?;
                Some(SK::Block(Block { stmts }))
            }
        }
    }

    /// Fold the expressions and nested blocks of `s` with what `env` knows on entry.
    fn prop_inside(&mut self, s: &mut Stmt, f: FuncId, env: &mut Env) {
        match &mut s.kind {
            SK::Let(_, e)
            | SK::Destructure(_, e)
            | SK::Expr(e)
            | SK::Throw(e)
            | SK::Assert(e)
            | SK::Return(Some(e))
            | SK::Break(_, Some(e)) => self.prop_expr(e, f, env),
            SK::Assign(pl, e) => {
                // a `$` in an index is unknown on its own, so it stays
                place_indices(pl, &mut |i| self.prop_expr(i, f, env));
                self.prop_expr(e, f, env);
            }
            SK::Return(None) | SK::Break(_, None) | SK::Continue(_) => {}
            SK::While { cond: e, body, .. }
            | SK::For { iter: e, body, .. }
            | SK::Lock { mutex: e, body, .. } => {
                // the loop changes what `env` no longer knows: every iteration starts alike
                self.prop_expr(e, f, env);
                self.prop_scoped(body, f, env);
            }
            SK::Block(b) => self.prop_block(b, f, env),
        }
    }

    /// Replace the largest known sub-expressions of `e` with their values.
    fn prop_expr(&mut self, e: &mut Expr, f: FuncId, env: &mut Env) {
        if is_lit_tree(e) {
            return;
        }
        if !contains_control(e)
            && !matches!(e.kind, EK::Closure(_) | EK::FnRef(_))
            && let Ok(v) = self.trial_expr(e, env)
            && val_size(&v) <= MAX_LITERAL
            && (cheap(&v) || !is_place_expr(e))
            && let Some(x) = val_expr(&v, e.ty, e.span, &self.p.types)
        {
            *e = x;
            return;
        }
        e.for_each_child_mut(&mut |c: &mut Expr| self.prop_expr(c, f, env));
        match &mut e.kind {
            EK::If(i) => {
                for arm in &mut i.arms {
                    for pt in &mut arm.patterns {
                        self.prop_pat(pt, f, env);
                    }
                    self.prop_scoped(&mut arm.body, f, env);
                }
                self.select_arm(i);
            }
            EK::Else(_, Fallback::Block(b, _)) | EK::Catch(_, _, b, _) => {
                self.prop_scoped(b, f, env);
            }
            _ => {}
        }
    }

    fn prop_pat(&mut self, pt: &mut Pat, f: FuncId, env: &mut Env) {
        pt.for_each_expr_mut(&mut |e| self.prop_expr(e, f, env));
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
        let mut env = Env::new(func.locals.len());
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
    fn invoke(&mut self, f: FuncId, args: Vec<Val>, caps: &[Val]) -> R<Val> {
        let ret = self.p.funcs[f].ret;
        let throws = matches!(self.p.types.kind(ret), TyKind::ErrU(_));
        match self.call_fn(f, args, caps)? {
            Flow::Return(v) => Ok(if throws { Val::Ok(Box::new(v)) } else { v }),
            Flow::Throw(e) if throws => Ok(Val::Err(e)),
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
                let x = value!(self.expr_flow(e, env));
                env.declare(*v, x)?;
                Ok(Flow::Normal)
            }
            SK::Destructure(ts, e) => {
                let Val::Tuple(items) = value!(self.expr_flow(e, env)) else {
                    return Err(Stop::Unknown);
                };
                for (v, start, count) in ts {
                    env.declare(*v, destructure_part(&items, *start, *count))?;
                }
                Ok(Flow::Normal)
            }
            SK::Assign(p, e) => {
                let x = value!(self.expr_flow(e, env));
                self.assign(p, x, env)?;
                Ok(Flow::Normal)
            }
            SK::Expr(e) => {
                value!(self.expr_flow(e, env));
                Ok(Flow::Normal)
            }
            SK::Return(Some(e)) => Ok(Flow::Return(value!(self.expr_flow(e, env)))),
            SK::Return(None) => Ok(Flow::Return(Val::Void)),
            SK::Throw(e) => match value!(self.expr_flow(e, env)) {
                Val::Str(s) => Ok(Flow::Throw(s)),
                _ => Err(Stop::Unknown),
            },
            SK::Break(id, Some(e)) => Ok(Flow::Break(*id, Some(value!(self.expr_flow(e, env))))),
            SK::Break(id, None) => Ok(Flow::Break(*id, None)),
            SK::Continue(id) => Ok(Flow::Continue(*id)),
            SK::While { id, cond, body } => loop {
                self.tick()?;
                let Val::Bool(c) = value!(self.expr_flow(cond, env)) else {
                    return Err(Stop::Unknown);
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
                let it = value!(self.expr_flow(iter, env));
                let keys: Vec<Val> = match (kind, &it) {
                    (ForKind::Keys, Val::Map(m)) => m.iter().map(|(k, _)| k.clone()).collect(),
                    (ForKind::Index, _) => (0..seq_len(&it)? as u64).map(Val::UInt).collect(),
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
        if path.is_empty() {
            // a whole-variable store does not need the old value
            return env.declare(Some(root), v);
        }
        set_path(env.slot(root)?, &path, v)
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
                let base = read_path(r, path, env)?;
                env.dollar.push(base);
                let i = self.expr(idx, env, false);
                env.dollar.pop();
                path.push(PathStep::Index(i?, idx.span));
                Ok(r)
            }
        }
    }

    /// Evaluate an expression; `Err(flow)` carries a control transfer (return/throw/break).
    fn expr_flow(&mut self, e: &Expr, env: &mut Env) -> R<Result<Val, Flow>> {
        match &e.kind {
            EK::If(i) => self.if_expr(i, env),
            EK::Catch(x, local, b, id) => match value!(expr self.expr_flow(x, env)) {
                Val::Ok(v) => Ok(Ok(*v)),
                Val::Err(msg) => {
                    if let Some(l) = local {
                        env.set(*l, Val::Str(msg));
                    }
                    self.block_value(b, *id, env)
                }
                _ => Err(Stop::Unknown),
            },
            EK::Else(o, fb) => match value!(expr self.expr_flow(o, env)) {
                Val::Some(v) => Ok(Ok(*v)),
                Val::None => match fb {
                    Fallback::Expr(x) => self.expr_flow(x, env),
                    Fallback::Block(b, id) => self.block_value(b, *id, env),
                },
                _ => Err(Stop::Unknown),
            },
            EK::Try(x) => match value!(expr self.expr_flow(x, env)) {
                Val::Ok(v) => Ok(Ok(*v)),
                Val::Err(m) => Ok(Err(Flow::Throw(m))),
                _ => Err(Stop::Unknown),
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
            Some(s) => Some(value!(expr self.expr_flow(s, env))),
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
                return self.block_value(&arm.body, i.id, env);
            }
        }
        Err(Stop::Unknown)
    }

    /// Run a block producing a value with `break <value>` to `id` (void when it completes).
    fn block_value(&mut self, b: &Block, id: JumpId, env: &mut Env) -> R<Result<Val, Flow>> {
        Ok(match self.block(b, env)? {
            Flow::Break(j, v) if j == id => Ok(v.unwrap_or(Val::Void)),
            Flow::Normal => Ok(Val::Void),
            f => Err(f),
        })
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
                self.pats_match(ps, xs, env)?
            }
            (Pat::Array(ps), Val::Arr(xs)) => {
                ps.len() == xs.len() && self.pats_match(ps, xs, env)?
            }
            (Pat::Variant(i, ps), Val::Variant(j, xs)) => i == j && self.pats_match(ps, xs, env)?,
            _ => return Err(Stop::Unknown),
        })
    }

    fn pats_match(&mut self, ps: &[Pat], xs: &[Val], env: &mut Env) -> R<bool> {
        for (p, x) in ps.iter().zip(xs) {
            if !self.pat_match(p, x, env)? {
                return Ok(false);
            }
        }
        Ok(true)
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
                env.get(*l)?
            }
            EK::Var(VarRef::Global(g)) => match env.globals.get(g) {
                Some(v) if !const_only => v.clone(),
                _ => self.gconst.get(g).cloned().ok_or(Stop::Unknown)?,
            },
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
                    caps.push(env.get(c.outer)?);
                }
                Val::Fn(*f, Rc::new(caps))
            }
            EK::Unary(op, a) => {
                let v = self.expr(a, env, const_only)?;
                match (op, v) {
                    (UnOp::Neg, Val::Int(i)) => match i.checked_neg() {
                        Some(x) => Val::Int(x),
                        None => return panic("integer overflow", span),
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
                binop(*op, x, y, span)?
            }
            EK::Call(c, args) => {
                if const_only && !args.iter().all(is_lit_tree) {
                    return Err(Stop::Unknown);
                }
                let vs = self.exprs(args, env, const_only)?;
                match c {
                    Callee::Direct(f) => self.invoke(*f, vs, &[])?,
                    Callee::Value(fe) => match self.expr(fe, env, const_only)? {
                        Val::Fn(f, caps) => self.invoke(f, vs, &caps)?,
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
                        None => return panic("key not found in map", span),
                    },
                    Val::Arr(a) => match a.get(idx_checked(&i, idx.span)? as usize) {
                        Some(v) => v.clone(),
                        None => return panic("index out of bounds", span),
                    },
                    Val::Str(s) => match s.chars().nth(idx_checked(&i, idx.span)? as usize) {
                        Some(c) => Val::Char(c as u32),
                        None => return panic("string index out of bounds", span),
                    },
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
                let len = seq_len(&base)?;
                // inclusive bounds: `l <= h < len`
                let h = idx_checked(&h?, hi.span)? as usize;
                if l > h || h >= len {
                    return panic("slice out of bounds", span);
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
                let len = seq_len(env.dollar.last().ok_or(Stop::Unknown)?)?;
                if len == 0 {
                    return panic("`$` used on an empty sequence", span);
                }
                Val::UInt(len as u64 - 1)
            }
            EK::Len(b) => match self.expr(b, env, const_only)? {
                Val::Map(m) => Val::UInt(m.len() as u64),
                v => Val::UInt(seq_len(&v)? as u64),
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
            EK::Embed(bytes, _) => byte_arr(bytes),
        })
    }

    fn exprs(&mut self, xs: &[Expr], env: &mut Env, const_only: bool) -> R<Vec<Val>> {
        xs.iter().map(|x| self.expr(x, env, const_only)).collect()
    }

    fn cast(&self, v: Val, from: TyId, to: TyId, span: Span) -> R<Val> {
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
                    return panic("negative value cannot be converted to uint", span);
                }
                Val::UInt(i as u64)
            }
            (Val::Int(i), TyKind::Float) => Val::Float(i as f64),
            (Val::UInt(u), TyKind::Int) => {
                if u > i64::MAX as u64 {
                    return panic("value out of range for int", span);
                }
                Val::Int(u as i64)
            }
            (Val::UInt(u), TyKind::Float) => Val::Float(u as f64),
            (Val::Float(f), TyKind::Int) => {
                if !(f > -9223372036854775809.0 && f < 9223372036854775808.0) {
                    return panic("float value out of range for int", span);
                }
                Val::Int(f as i64)
            }
            (Val::Float(f), TyKind::UInt) => {
                if !(f > -1.0 && f < 18446744073709551616.0) {
                    return panic("float value out of range for uint", span);
                }
                Val::UInt(f as u64)
            }
            (Val::Char(c), TyKind::Array(_)) => byte_arr(
                char::from_u32(c)
                    .ok_or(Stop::Unknown)?
                    .to_string()
                    .as_bytes(),
            ),
            (Val::Int(i), TyKind::Array(_)) => byte_arr(&i.to_le_bytes()),
            (Val::UInt(i), TyKind::Array(_)) => byte_arr(&i.to_le_bytes()),
            (Val::Float(f), TyKind::Array(_)) => byte_arr(&f.to_bits().to_le_bytes()),
            (Val::Str(s), TyKind::Array(e)) if *e == CHAR => {
                Val::Arr(Rc::new(s.chars().map(|c| Val::Char(c as u32)).collect()))
            }
            (Val::Str(s), TyKind::Array(_)) => byte_arr(s.as_bytes()),
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
                self.items_into(s, xs, |_| (None, *e))?;
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
                self.items_into(s, xs, |i| (None, ts[i]))?;
                s.push(')');
            }
            (Val::Struct(xs), TyKind::Struct(si)) => {
                let def = &t.structs[*si as usize];
                s.push_str(&def.name);
                s.push('{');
                self.items_into(s, xs, |i| (Some(&def.fields[i].0), def.fields[i].1))?;
                s.push('}');
            }
            (Val::Variant(vi, xs), TyKind::Enum(ei)) => {
                let (name, tys) = &t.enums[*ei as usize].variants[*vi];
                s.push_str(&t.enums[*ei as usize].name);
                s.push('.');
                s.push_str(name);
                if !xs.is_empty() {
                    s.push('(');
                    self.items_into(s, xs, |i| (None, tys[i]))?;
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

    /// Comma-separated values, each with its type and an optional field name (`.name = `).
    fn items_into<'a>(
        &self,
        s: &mut String,
        xs: &[Val],
        item: impl Fn(usize) -> (Option<&'a String>, TyId),
    ) -> R<()> {
        for (i, x) in xs.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            let (field, ty) = item(i);
            if let Some(f) = field {
                s.push_str(&format!(".{f} = "));
            }
            self.str_into(s, x, ty)?;
        }
        Ok(())
    }
}

/// Call `f` with each index expression of a place, innermost first.
fn place_indices(p: &mut Place, f: &mut dyn FnMut(&mut Expr)) {
    match p {
        Place::Var(_) => {}
        Place::Field(b, _) | Place::TupleIndex(b, _) => place_indices(b, f),
        Place::Index(b, i) => {
            place_indices(b, f);
            f(i);
        }
    }
}

/// The length of an array or (in characters) a string.
fn seq_len(v: &Val) -> R<usize> {
    match v {
        Val::Arr(a) => Ok(a.len()),
        Val::Str(s) => Ok(s.chars().count()),
        _ => Err(Stop::Unknown),
    }
}

struct Env {
    locals: Vec<Option<Val>>,
    dollar: Vec<Val>,
    /// known values of globals (release-mode propagation only, see `track_globals`)
    globals: HashMap<GlobalId, Val>,
    /// whether declarations and assignments of globals are tracked in `globals`
    track_globals: bool,
    /// open checkpoints, innermost last: the length of `dollar` and the value each variable
    /// had before its first change since
    checkpoints: Vec<(usize, HashMap<VarRef, Option<Val>>)>,
}

/// Changes to an environment, to apply again with `Env::redo`.
type Redo = Vec<(VarRef, Option<Val>)>;

impl Env {
    fn new(locals: usize) -> Self {
        Env {
            locals: vec![None; locals],
            dollar: vec![],
            globals: HashMap::new(),
            track_globals: false,
            checkpoints: vec![],
        }
    }

    /// Start recording changes, to undo them with `rollback` or keep them with `commit`.
    fn checkpoint(&mut self) {
        self.checkpoints.push((self.dollar.len(), HashMap::new()));
    }

    /// Keep the changes since the innermost checkpoint.
    fn commit(&mut self) {
        let (_, saved) = self.checkpoints.pop().unwrap();
        if let Some((_, outer)) = self.checkpoints.last_mut() {
            for (r, old) in saved {
                outer.entry(r).or_insert(old);
            }
        }
    }

    /// Undo the changes since the innermost checkpoint.
    fn rollback(&mut self) {
        self.undo();
    }

    /// Undo the changes since the innermost checkpoint, returning them.
    fn undo(&mut self) -> Redo {
        let (dollar, saved) = self.checkpoints.pop().unwrap();
        self.dollar.truncate(dollar);
        (saved.into_iter())
            .map(|(r, old)| {
                let now = self.raw(r);
                self.put(r, old);
                (r, now)
            })
            .collect()
    }

    /// Apply changes undone by `undo` again.
    fn redo(&mut self, changes: Redo) {
        for (r, v) in changes {
            self.write(r, v);
        }
    }

    fn raw(&self, r: VarRef) -> Option<Val> {
        match r {
            VarRef::Local(l) => self.locals.get(l).cloned().flatten(),
            VarRef::Global(g) => self.globals.get(&g).cloned(),
        }
    }

    /// Set a variable without recording the change.
    fn put(&mut self, r: VarRef, v: Option<Val>) {
        match (r, v) {
            (VarRef::Local(l), v) => {
                if l >= self.locals.len() {
                    self.locals.resize(l + 1, None);
                }
                self.locals[l] = v;
            }
            (VarRef::Global(g), Some(v)) => {
                self.globals.insert(g, v);
            }
            (VarRef::Global(g), None) => {
                self.globals.remove(&g);
            }
        }
    }

    /// Note that `r` is about to change.
    fn record(&mut self, r: VarRef) {
        if let Some((_, saved)) = self.checkpoints.last()
            && !saved.contains_key(&r)
        {
            let old = self.raw(r);
            self.checkpoints.last_mut().unwrap().1.insert(r, old);
        }
    }

    /// Set a variable, recording the change.
    fn write(&mut self, r: VarRef, v: Option<Val>) {
        self.record(r);
        self.put(r, v);
    }

    fn var(&self, r: VarRef) -> R<Val> {
        match r {
            VarRef::Local(l) => self.get(l),
            VarRef::Global(g) => self.globals.get(&g).cloned().ok_or(Stop::Unknown),
        }
    }

    fn slot(&mut self, r: VarRef) -> R<&mut Val> {
        self.record(r);
        match r {
            VarRef::Local(l) => self.locals.get_mut(l).and_then(|x| x.as_mut()),
            VarRef::Global(g) => self.globals.get_mut(&g),
        }
        .ok_or(Stop::Unknown)
    }

    fn forget(&mut self, r: VarRef) {
        if self.raw(r).is_some() {
            self.write(r, None);
        }
    }

    fn forget_all<'a>(&mut self, vs: impl Iterator<Item = &'a VarRef>) {
        vs.for_each(|v| self.forget(*v));
    }

    fn forget_globals(&mut self, pred: impl Fn(GlobalId) -> bool) {
        let gone: Vec<GlobalId> = self.globals.keys().copied().filter(|g| pred(*g)).collect();
        for g in gone {
            self.forget(VarRef::Global(g));
        }
    }

    fn get(&self, l: LocalId) -> R<Val> {
        self.locals.get(l).cloned().flatten().ok_or(Stop::Unknown)
    }

    fn set(&mut self, l: LocalId, v: Val) {
        self.write(VarRef::Local(l), Some(v));
    }

    /// Bind a declaration target: globals are only known through `gconst`, unless tracked.
    fn declare(&mut self, target: Option<VarRef>, v: Val) -> R<()> {
        match target {
            Some(VarRef::Local(l)) => self.set(l, v),
            Some(VarRef::Global(g)) if self.track_globals => {
                self.write(VarRef::Global(g), Some(v));
            }
            Some(VarRef::Global(_)) => return Err(Stop::Unknown),
            None => {}
        }
        Ok(())
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
                return panic("index out of bounds", *isp);
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
                return panic("string index out of bounds", *isp);
            }
            chars[k] = char::from_u32(c).ok_or(Stop::Unknown)?;
            *s = chars.into_iter().collect::<String>().into();
            Ok(())
        }
        _ => Err(Stop::Unknown),
    }
}

fn idx_checked(v: &Val, span: Span) -> R<u64> {
    match v {
        Val::UInt(u) => Ok(*u),
        Val::Int(i) if *i >= 0 => Ok(*i as u64),
        Val::Int(_) => panic("negative index", span),
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

/// Whether the operands of an operator expression are all literals (or `$`).
fn all_children_lit(e: &Expr) -> bool {
    let mut ok = true;
    e.for_each_child(&mut |c| ok &= is_lit_tree(c) || matches!(c.kind, EK::Dollar));
    ok
}

fn contains_control(e: &Expr) -> bool {
    let mut found = false;
    flow::visit_expr(e, &mut |x: &Expr| {
        found |= matches!(
            x.kind,
            EK::If(_) | EK::Catch(..) | EK::Else(..) | EK::Try(_)
        );
    });
    found
}

/// Convert a value back into an IR expression (literals only).
fn val_expr(v: &Val, ty: TyId, span: Span, t: &Types) -> Option<Expr> {
    let b = t.base(ty);
    let items = |xs: &[Val], tys: &mut dyn Iterator<Item = TyId>| -> Option<Vec<Expr>> {
        xs.iter()
            .zip(tys)
            .map(|(x, et)| val_expr(x, et, span, t))
            .collect()
    };
    let kind = match (v, t.kind(b)) {
        (Val::Bool(x), _) => EK::Lit(Lit::Bool(*x)),
        (Val::Byte(x), _) => EK::Lit(Lit::Byte(*x)),
        (Val::Int(x), _) => EK::Lit(Lit::Int(*x)),
        (Val::UInt(x), _) => EK::Lit(Lit::UInt(*x)),
        (Val::Float(x), _) => EK::Lit(Lit::Float(*x)),
        (Val::Char(x), _) => EK::Lit(Lit::Char(*x)),
        (Val::Str(s), _) if b == STR || b == ERROR => EK::Lit(Lit::Str(s.to_string())),
        (Val::None, TyKind::Opt(_)) => EK::None,
        (Val::Some(x), TyKind::Opt(i)) => EK::Some(Box::new(val_expr(x, *i, span, t)?)),
        (Val::Tuple(xs), TyKind::Tuple(ts)) => EK::Tuple(items(xs, &mut ts.iter().copied())?),
        (Val::Struct(xs), TyKind::Struct(si)) => {
            let fs = &t.structs[*si as usize].fields;
            EK::Struct(items(xs, &mut fs.iter().map(|f| f.1))?)
        }
        (Val::Arr(xs), _) if b != STR => EK::Array(items(xs, &mut std::iter::repeat(t.elem(b)?))?),
        (Val::Map(es), TyKind::Map(kt, vt)) => EK::Map(
            es.iter()
                .map(|(k, v)| Some((val_expr(k, *kt, span, t)?, val_expr(v, *vt, span, t)?)))
                .collect::<Option<Vec<_>>>()?,
        ),
        (Val::Variant(i, xs), TyKind::Enum(ei)) => {
            let ps = &t.enums[*ei as usize].variants[*i].1;
            EK::Variant(*i, items(xs, &mut ps.iter().copied())?)
        }
        _ => return None,
    };
    Some(Expr { kind, ty, span })
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
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if (-5..17).contains(&exp) {
        // place the decimal point
        if exp < 0 {
            out.push_str("0.");
            out.push_str(&"0".repeat((-exp - 1) as usize));
            out.push_str(&digits);
        } else if digits.len() > exp as usize + 1 {
            let (int, frac) = digits.split_at(exp as usize + 1);
            out.push_str(&format!("{int}.{frac}"));
        } else {
            out.push_str(&digits);
            out.push_str(&"0".repeat(exp as usize + 1 - digits.len()));
            out.push_str(".0");
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

fn panic<T>(msg: &str, span: Span) -> R<T> {
    Err(Stop::Panic(msg.to_string(), span))
}

fn read_path(r: VarRef, path: &[PathStep], env: &Env) -> R<Val> {
    let mut cur = env.var(r)?;
    for st in path {
        cur = match (st, &cur) {
            (PathStep::Elem(i), Val::Tuple(xs) | Val::Struct(xs)) => xs[*i].clone(),
            (PathStep::Index(k, _), Val::Map(m)) => m
                .iter()
                .find(|(kk, _)| kk == k)
                .map(|x| x.1.clone())
                .ok_or(Stop::Unknown)?,
            (PathStep::Index(i, sp), Val::Arr(a)) => a
                .get(idx_checked(i, *sp)? as usize)
                .cloned()
                .ok_or(Stop::Unknown)?,
            _ => return Err(Stop::Unknown),
        };
    }
    Ok(cur)
}

fn byte_arr(bytes: &[u8]) -> Val {
    Val::Arr(Rc::new(bytes.iter().map(|b| Val::Byte(*b)).collect()))
}

/// Part `start..start + count` of a destructured tuple: one value, or a tuple of several.
fn destructure_part(items: &[Val], start: usize, count: usize) -> Val {
    if count == 1 {
        items[start].clone()
    } else {
        Val::Tuple(Rc::new(items[start..start + count].to_vec()))
    }
}

/// An operator on integers of `bits` bits in `min..=max`, computed exactly in `i128` and
/// checked against the range.
fn int_op(
    op: BinOp,
    (a, b): (i128, i128),
    (min, max, bits): (i128, i128, i128),
    span: Span,
    mk: impl Fn(i128) -> Val,
) -> R<Val> {
    use BinOp::*;
    let fit = |v: Option<i128>| match v {
        Some(v) if (min..=max).contains(&v) => Ok(v),
        _ => panic("integer overflow", span),
    };
    if matches!(op, Div | Mod) && b == 0 {
        return panic("division by zero", span);
    }
    if matches!(op, Shl | Shr) && !(0..bits).contains(&b) {
        return panic("shift amount out of range", span);
    }
    let v = match op {
        Add => a.checked_add(b),
        Sub => a.checked_sub(b),
        Mul => a.checked_mul(b),
        Div => Some(a / b),
        Mod => Some(a % b),
        Pow if b < 0 => return panic("negative exponent", span),
        Pow => Some(match a {
            0 | 1 if b == 0 => 1,
            0 | 1 => a,
            -1 => 1 - 2 * (b % 2),
            // any other base overflows within `bits` steps
            _ => {
                let mut r: i128 = 1;
                for _ in 0..b {
                    r = fit(r.checked_mul(a))?;
                }
                r
            }
        }),
        Shl => Some(a << b),
        Shr => Some(a >> b),
        BitAnd => Some(a & b),
        BitOr => Some(a | b),
        BitXor => Some(a ^ b),
        _ => return ord(op, a, b),
    };
    fit(v).map(mk)
}

/// An ordering comparison.
fn ord<T: PartialOrd>(op: BinOp, a: T, b: T) -> R<Val> {
    Ok(Val::Bool(match op {
        BinOp::Lt => a < b,
        BinOp::Gt => a > b,
        BinOp::Le => a <= b,
        BinOp::Ge => a >= b,
        _ => return Err(Stop::Unknown),
    }))
}

fn binop(op: BinOp, x: Val, y: Val, span: Span) -> R<Val> {
    use BinOp::*;
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
        (op, Val::Int(a), Val::Int(b)) => {
            let range = (i64::MIN.into(), i64::MAX.into(), 64);
            int_op(op, (a.into(), b.into()), range, span, |v| {
                Val::Int(v as i64)
            })?
        }
        (op, Val::UInt(a), Val::UInt(b)) => {
            let range = (0, u64::MAX.into(), 64);
            int_op(op, (a.into(), b.into()), range, span, |v| {
                Val::UInt(v as u64)
            })?
        }
        (op, Val::Byte(a), Val::Byte(b)) => {
            int_op(op, (a.into(), b.into()), (0, 255, 8), span, |v| {
                Val::Byte(v as u8)
            })?
        }
        (op, Val::Float(a), Val::Float(b)) => match op {
            Add => Val::Float(a + b),
            Sub => Val::Float(a - b),
            Mul => Val::Float(a * b),
            Div => Val::Float(a / b),
            Pow => Val::Float(a.powf(b)),
            _ => ord(op, a, b)?,
        },
        (op, Val::Char(a), Val::Char(b)) => ord(op, a, b)?,
        _ => return Err(Stop::Unknown),
    })
}

/// Compile-time evaluation of constant expressions, for lints.
pub struct Consts<'p>(Evaluator<'p>);

impl<'p> Consts<'p> {
    pub fn new(p: &'p Program, sm: &'p SourceMap) -> Self {
        let mut ev = Evaluator::new(p, sm);
        ev.init_gconst();
        Consts(ev)
    }

    /// `while` loops whose condition stays true on every iteration, as found by propagating
    /// known values through the program (as release builds do, without changing it).
    pub fn endless_loops(&mut self) -> HashSet<(FuncId, JumpId)> {
        self.0.propagate();
        std::mem::take(&mut self.0.endless)
    }

    /// Whether `e` is a constant that is always `true`.
    pub fn always_true(&mut self, e: &Expr) -> bool {
        matches!(self.0.try_const(e), Ok(Some(Val::Bool(true))))
    }
}

/// Number of nodes in a value.
fn val_size(v: &Val) -> usize {
    1 + match v {
        Val::Arr(xs) | Val::Tuple(xs) | Val::Struct(xs) | Val::Variant(_, xs) => {
            xs.iter().map(val_size).sum()
        }
        Val::Map(m) => m.iter().map(|(k, v)| val_size(k) + val_size(v)).sum(),
        Val::Some(x) | Val::Ok(x) => val_size(x),
        _ => 0,
    }
}

/// Whether a value's literal costs no allocation, so copying it into each use is free.
fn cheap(v: &Val) -> bool {
    match v {
        Val::Arr(_) | Val::Map(_) | Val::Variant(..) | Val::Fn(..) => false,
        Val::Tuple(xs) | Val::Struct(xs) => xs.iter().all(cheap),
        Val::Some(x) | Val::Ok(x) => cheap(x),
        _ => true,
    }
}

/// A variable or a part of one: reading it shares the existing value instead of building one.
fn is_place_expr(e: &Expr) -> bool {
    match &e.kind {
        EK::Var(_) => true,
        EK::Field(b, _) | EK::TupleIndex(b, _) | EK::Index(b, _) | EK::Retype(b) => {
            is_place_expr(b)
        }
        _ => false,
    }
}

/// The variables a statement assigns, and those it declares (including in nested blocks).
fn written(s: &Stmt) -> (HashSet<VarRef>, HashSet<VarRef>) {
    let mut assigned = HashSet::new();
    let mut declared = HashSet::new();
    flow::visit_stmts(s, &mut |s| match &s.kind {
        SK::Assign(p, _) => {
            if let Some(v) = flow::place_root(p) {
                assigned.insert(v);
            }
        }
        SK::Let(Some(v), _) => {
            declared.insert(*v);
        }
        SK::Destructure(ts, _) => declared.extend(ts.iter().filter_map(|t| t.0)),
        SK::For { var, .. } => {
            declared.insert(VarRef::Local(*var));
        }
        SK::Lock { alias, .. } => {
            declared.insert(VarRef::Local(*alias));
        }
        _ => {}
    });
    fn binds(p: &Pat, out: &mut HashSet<VarRef>) {
        match p {
            Pat::Bind(l) => {
                out.insert(VarRef::Local(*l));
            }
            Pat::Tuple(ps) | Pat::Struct(ps) | Pat::Array(ps) | Pat::Variant(_, ps) => {
                ps.iter().for_each(|p| binds(p, out));
            }
            _ => {}
        }
    }
    flow::visit_stmt(s, &mut |e| match &e.kind {
        EK::If(i) => i
            .arms
            .iter()
            .flat_map(|a| &a.patterns)
            .for_each(|p| binds(p, &mut declared)),
        EK::Catch(_, Some(l), _, _) => {
            declared.insert(VarRef::Local(*l));
        }
        _ => {}
    });
    (assigned, declared)
}

/// Forget the globals in `which` (all of them when `None`).
fn forget_globals(env: &mut Env, which: &Option<HashSet<GlobalId>>) {
    env.forget_globals(|g| which.as_ref().is_none_or(|gs| gs.contains(&g)));
}

/// Per function, the globals it may change directly or through the functions it calls.
/// `None` when it calls unknown code (a function value) or spawns tasks.
fn global_writes(p: &Program) -> Vec<Option<HashSet<GlobalId>>> {
    let n = p.funcs.len();
    let mut out: Vec<Option<HashSet<GlobalId>>> = vec![Some(HashSet::new()); n];
    let mut calls: Vec<Vec<FuncId>> = vec![vec![]; n];
    for (i, f) in p.funcs.iter().enumerate() {
        let mut w = HashSet::new();
        let mut unknown = false;
        for s in &f.body.stmts {
            flow::writes_stmt(s, &mut w);
            flow::visit_stmt(s, &mut |e| match &e.kind {
                EK::Call(Callee::Direct(g), _) => calls[i].push(*g),
                EK::Call(Callee::Extern(_), _) => {}
                EK::Call(..) | EK::Async(..) | EK::Await(_) => unknown = true,
                _ => {}
            });
            flow::visit_stmts(s, &mut |s| unknown |= matches!(s.kind, SK::Lock { .. }));
        }
        out[i] = (!unknown).then_some(w);
    }
    loop {
        let mut changed = false;
        for i in 0..n {
            for &g in &calls[i] {
                let add = out[g].clone();
                match (&mut out[i], add) {
                    (Some(_), None) => {
                        out[i] = None;
                        changed = true;
                    }
                    (Some(mine), Some(theirs)) => {
                        let before = mine.len();
                        mine.extend(theirs);
                        changed |= mine.len() != before;
                    }
                    (None, _) => {}
                }
            }
        }
        if !changed {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a copy of the environment would hold.
    type Snapshot = (Vec<Option<Val>>, Vec<(GlobalId, Val)>, usize);

    fn snapshot(env: &Env) -> Snapshot {
        let mut globals: Vec<(GlobalId, Val)> =
            env.globals.iter().map(|(g, v)| (*g, v.clone())).collect();
        globals.sort_by_key(|(g, _)| *g);
        // trailing unset locals do not matter: `set` grows the list as needed
        let mut locals = env.locals.clone();
        while locals.last() == Some(&None) {
            locals.pop();
        }
        (locals, globals, env.dollar.len())
    }

    /// Checkpoints behave like copies: `rollback` (and `undo`) restore exactly what a copy made
    /// at the checkpoint would hold, `commit` keeps the changes for the checkpoint outside, and
    /// `redo` brings back what `undo` took away.
    #[test]
    fn checkpoints_match_copies() {
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut rand = move |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        for _ in 0..200 {
            let mut env = Env::new(4);
            env.track_globals = true;
            // copies made at the open checkpoints, innermost last
            let mut copies: Vec<Snapshot> = Vec::new();
            for _ in 0..300 {
                let var = match rand(2) {
                    0 => VarRef::Local(rand(8) as usize),
                    _ => VarRef::Global(rand(4) as usize),
                };
                let val = Val::Int(rand(1000) as i64);
                match rand(12) {
                    0..=2 => env.declare(Some(var), val).unwrap_or(()),
                    3 => env.forget(var),
                    4 => {
                        if let Ok(slot) = env.slot(var) {
                            *slot = val;
                        }
                    }
                    5 => env.dollar.push(val),
                    6 | 7 => {
                        copies.push(snapshot(&env));
                        env.checkpoint();
                    }
                    8 if !copies.is_empty() => {
                        env.rollback();
                        assert_eq!(snapshot(&env), copies.pop().unwrap());
                    }
                    9 if !copies.is_empty() => {
                        let kept = snapshot(&env);
                        env.commit();
                        copies.pop();
                        assert_eq!(snapshot(&env), kept);
                    }
                    10 if !copies.is_empty() => {
                        // undo, then put the changes back inside the same checkpoint
                        let after = snapshot(&env);
                        let changes = env.undo();
                        let before = copies.last().unwrap().clone();
                        assert_eq!(snapshot(&env), before);
                        env.checkpoint();
                        env.redo(changes);
                        let again = snapshot(&env);
                        assert_eq!((again.0, again.1), (after.0, after.1));
                    }
                    _ => {}
                }
            }
            // leaving every checkpoint by rolling back ends where the first copy was made
            while let Some(copy) = copies.pop() {
                env.rollback();
                assert_eq!(snapshot(&env), copy);
            }
        }
    }
}
