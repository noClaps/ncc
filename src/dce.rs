//! Release-mode dead code elimination on the typed IR.
//!
//! Removes statements whose only effect is to compute a value nobody observes:
//! - stores (`let` and assignments) to variables that are never read by live code;
//! - discarded expressions (`_ = e`, expression statements) without side effects.
//!
//! A store is only removed when its value (and any index in its target) is effect-free: it
//! cannot print, call, loop, panic or transfer control. Allocation counts as effect-free:
//! running out of memory is not an observable behaviour of an NC program.
//!
//! Liveness is computed over the whole program: a variable is live when code that is kept
//! reads it. Reads inside a store to `x` only matter once `x` itself is live, so
//! self-referencing chains such as `a = a <> [4]` are removed together.

use std::collections::{HashMap, HashSet};

use crate::ast::{BinOp, UnOp};
use crate::check::flow::place_root;
use crate::tir::*;
use crate::types::TyKind;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Key {
    Global(GlobalId),
    Local(FuncId, LocalId),
}

pub fn run(p: &mut Program) {
    // removing statements can empty blocks and make more variables dead: repeat until stable
    while pass(p) {}
}

/// One round of elimination; whether anything was removed.
fn pass(p: &mut Program) -> bool {
    let live = {
        let mut a = Analysis {
            p,
            func: 0,
            live: HashSet::new(),
            deps: HashMap::new(),
        };
        for f in 0..p.funcs.len() {
            a.func = f;
            // every statement, including those in nested bodies and in blocks of expressions
            for s in &p.funcs[f].body.stmts {
                crate::check::flow::visit_stmts(s, &mut |s| a.stmt(s));
            }
        }
        a.propagate()
    };
    let mut changed = false;
    for f in 0..p.funcs.len() {
        let mut body = std::mem::replace(&mut p.funcs[f].body, Block { stmts: vec![] });
        let mut pr = Prune {
            p,
            func: f,
            live: &live,
            changed: false,
        };
        pr.block(&mut body);
        changed |= pr.changed;
        p.funcs[f].body = body;
    }
    changed
}

struct Analysis<'p> {
    p: &'p Program,
    func: FuncId,
    /// variables read by code that is kept
    live: HashSet<Key>,
    /// variables read by removable stores to a variable: live once that variable is
    deps: HashMap<Key, HashSet<Key>>,
}

impl Analysis<'_> {
    fn stmt(&mut self, s: &Stmt) {
        let mut reads = HashSet::new();
        for e in own_exprs(s) {
            self.reads(e, &mut reads);
        }
        match removable(self.p, self.func, s) {
            Some(Removable::Discard) => return,
            Some(Removable::Store(k)) => {
                self.deps.entry(k).or_default().extend(reads);
                return;
            }
            None => {}
        }
        // a kept store, `let` or destructuring keeps its variables declared
        match &s.kind {
            SK::Assign(pl, _) => reads.extend(place_root(pl).map(|v| self.key(v))),
            SK::Let(Some(v), _) => {
                reads.insert(self.key(*v));
            }
            SK::Destructure(ts, _) => {
                reads.extend(ts.iter().filter_map(|(t, _, _)| t.map(|v| self.key(v))));
            }
            _ => {}
        }
        self.live.extend(reads);
    }

    fn key(&self, v: VarRef) -> Key {
        match v {
            VarRef::Global(g) => Key::Global(g),
            VarRef::Local(l) => Key::Local(self.func, l),
        }
    }

    /// Record the variables `e` reads. Statements in its nested blocks are analysed on their
    /// own, so their reads are not included.
    fn reads(&self, e: &Expr, out: &mut HashSet<Key>) {
        for c in children(e) {
            self.reads(c, out);
        }
        match &e.kind {
            EK::Var(v) => {
                out.insert(self.key(*v));
            }
            EK::FnRef(f)
            | EK::Closure(f)
            | EK::Call(Callee::Direct(f), _)
            | EK::Async(Callee::Direct(f), _) => {
                // creating or calling a nested function reads the variables it captures
                let func = &self.p.funcs[*f];
                if let Some(parent) = func.parent {
                    out.extend(func.captures.iter().map(|c| Key::Local(parent, c.outer)));
                }
            }
            _ => {}
        }
    }

    fn propagate(mut self) -> HashSet<Key> {
        let mut work: Vec<Key> = self.live.iter().copied().collect();
        while let Some(k) = work.pop() {
            if let Some(ds) = self.deps.remove(&k) {
                for d in ds {
                    if self.live.insert(d) {
                        work.push(d);
                    }
                }
            }
        }
        self.live
    }
}

/// The expressions of a statement itself (including indices of its target), not those of the
/// statements nested in it.
fn own_exprs(s: &Stmt) -> Vec<&Expr> {
    match &s.kind {
        SK::Let(_, e)
        | SK::Destructure(_, e)
        | SK::Expr(e)
        | SK::Throw(e)
        | SK::Assert(e)
        | SK::While { cond: e, .. }
        | SK::For { iter: e, .. }
        | SK::Lock { mutex: e, .. } => vec![e],
        SK::Return(e) | SK::Break(_, e) => e.iter().collect(),
        SK::Assign(pl, e) => {
            let mut pl = pl;
            let mut out = vec![e];
            loop {
                match pl {
                    Place::Var(_) => return out,
                    Place::Field(b, _) | Place::TupleIndex(b, _) => pl = b,
                    Place::Index(b, i) => {
                        out.push(i);
                        pl = b;
                    }
                }
            }
        }
        SK::Continue(_) | SK::Block(_) => vec![],
    }
}

enum Removable {
    /// An effect-free expression whose value is discarded.
    Discard,
    /// An effect-free store to a variable: removable when the variable is not live.
    Store(Key),
}

/// The key of a variable whose stores may be removed: captured or aliased variables are
/// shared with other code, so their stores are kept.
fn store_key(p: &Program, func: FuncId, v: VarRef) -> Option<Key> {
    match v {
        VarRef::Global(g) => Some(Key::Global(g)),
        VarRef::Local(l) => {
            let local = &p.funcs[func].locals[l];
            (matches!(local.kind, LocalKind::Normal | LocalKind::Param) && !local.boxed)
                .then_some(Key::Local(func, l))
        }
    }
}

fn removable(p: &Program, func: FuncId, s: &Stmt) -> Option<Removable> {
    let store_key = |v: VarRef| store_key(p, func, v);
    match &s.kind {
        SK::Expr(e) | SK::Let(None, e) if pure(p, e) => Some(Removable::Discard),
        SK::Block(b) if b.stmts.is_empty() => Some(Removable::Discard),
        SK::Let(Some(v), e) if pure(p, e) => store_key(*v).map(Removable::Store),
        SK::Assign(pl, e) if pure(p, e) && place_pure(pl) => {
            store_key(place_root(pl)?).map(Removable::Store)
        }
        _ => None,
    }
}

fn place_pure(pl: &Place) -> bool {
    match pl {
        Place::Var(_) => true,
        Place::Field(b, _) | Place::TupleIndex(b, _) => place_pure(b),
        // an index out of bounds panics
        Place::Index(..) => false,
    }
}

/// Whether evaluating `e` has no observable effect: no output, calls, control transfer or
/// panics. Allocation is not observable.
fn pure(p: &Program, e: &Expr) -> bool {
    let float = |e: &Expr| matches!(p.types.kind(e.ty), TyKind::Float);
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
        | EK::Embed(..) => true,
        EK::Unary(op, a) => (*op != UnOp::Neg || float(a)) && pure(p, a),
        EK::Binary(op, a, b) => {
            let ok = match op {
                BinOp::Concat
                | BinOp::Eq
                | BinOp::Ne
                | BinOp::Lt
                | BinOp::Gt
                | BinOp::Le
                | BinOp::Ge
                | BinOp::In
                | BinOp::And
                | BinOp::Or
                | BinOp::BitAnd
                | BinOp::BitOr
                | BinOp::BitXor => true,
                // integer arithmetic can overflow or divide by zero, shifts can be out of range
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod | BinOp::Pow => {
                    float(a) && float(b)
                }
                BinOp::Shl | BinOp::Shr => false,
            };
            ok && pure(p, a) && pure(p, b)
        }
        EK::ToStr(a)
        | EK::Field(a, _)
        | EK::TupleIndex(a, _)
        | EK::Len(a)
        | EK::Some(a)
        | EK::Retype(a) => pure(p, a),
        EK::Format(xs) | EK::Tuple(xs) | EK::Array(xs) | EK::Struct(xs) | EK::Variant(_, xs) => {
            xs.iter().all(|x| pure(p, x))
        }
        EK::Map(es) => es.iter().all(|(k, v)| pure(p, k) && pure(p, v)),
        // an `if` with nothing left in its arms (the checker ensures one arm matches)
        EK::If(i) => {
            i.scrutinee.as_ref().is_none_or(|s| pure(p, s))
                && i.arms
                    .iter()
                    .all(|a| a.body.stmts.is_empty() && a.patterns.iter().all(|pt| pat_pure(p, pt)))
        }
        EK::Else(a, Fallback::Expr(b)) => pure(p, a) && pure(p, b),
        // calls may print, loop forever or panic; casts, indexing and slicing may panic
        EK::Call(..)
        | EK::Print(..)
        | EK::Cast(_)
        | EK::Index(..)
        | EK::Slice(..)
        | EK::Else(_, Fallback::Block(..))
        | EK::Catch(..)
        | EK::Try(_)
        | EK::Async(..)
        | EK::Await(_) => false,
    }
}

fn pat_pure(p: &Program, pt: &Pat) -> bool {
    match pt {
        Pat::Wild | Pat::Bind(_) => true,
        Pat::Value(e) | Pat::Cond(e) => pure(p, e),
        Pat::Tuple(ps) | Pat::Struct(ps) | Pat::Array(ps) | Pat::Variant(_, ps) => {
            ps.iter().all(|pt| pat_pure(p, pt))
        }
    }
}

/// The expressions directly inside `e`, including those in `if` patterns but not the
/// statements of its nested blocks.
fn children(e: &Expr) -> Vec<&Expr> {
    let mut out: Vec<&Expr> = vec![];
    e.for_each_child(&mut |c| out.push(c));
    if let EK::If(i) = &e.kind {
        for a in &i.arms {
            for pt in &a.patterns {
                pt.for_each_expr(&mut |x| out.push(x));
            }
        }
    }
    out
}

struct Prune<'a> {
    p: &'a Program,
    func: FuncId,
    live: &'a HashSet<Key>,
    changed: bool,
}

impl Prune<'_> {
    fn block(&mut self, b: &mut Block) {
        // blocks that declare nothing need no scope of their own
        if b.stmts
            .iter()
            .any(|s| matches!(&s.kind, SK::Block(x) if flat(x)))
        {
            let mut out = Vec::with_capacity(b.stmts.len());
            for s in std::mem::take(&mut b.stmts) {
                match s.kind {
                    SK::Block(x) if flat(&x) => out.extend(x.stmts),
                    kind => out.push(Stmt { kind, ..s }),
                }
            }
            b.stmts = out;
            self.changed = true;
        }
        let before = b.stmts.len();
        b.stmts.retain(|s| match removable(self.p, self.func, s) {
            Some(Removable::Discard) => false,
            Some(Removable::Store(k)) => self.live.contains(&k),
            None => true,
        });
        self.overwritten(b);
        if b.stmts.len() != before {
            self.changed = true;
        }
        for s in &mut b.stmts {
            self.stmt(s);
        }
    }

    /// Remove effect-free assignments that a later statement of the block overwrites before
    /// anything reads them.
    fn overwritten(&self, b: &mut Block) {
        // variables fully assigned later in the block, with no read in between: the index of
        // that assignment
        let mut over: HashMap<Key, usize> = HashMap::new();
        let mut keep = vec![true; b.stmts.len()];
        // assignments that become the declaration of their variable
        let mut declare = vec![];
        for (i, s) in b.stmts.iter().enumerate().rev() {
            let simple = matches!(s.kind, SK::Let(..) | SK::Assign(..) | SK::Expr(_))
                && !crate::check::flow::has_nested_blocks(s);
            if !simple {
                over.clear();
                continue;
            }
            if let Some(Removable::Store(k)) = removable(self.p, self.func, s)
                && let Some(&later) = over.get(&k)
            {
                keep[i] = false;
                // the later assignment declares the variable instead
                if let SK::Let(..) = s.kind {
                    declare.push(later);
                    over.remove(&k);
                }
                continue;
            }
            // the store happens after the reads of its own value
            match &s.kind {
                SK::Assign(Place::Var(v), _) => {
                    if let Some(k) = store_key(self.p, self.func, *v) {
                        over.insert(k, i);
                    }
                }
                SK::Let(Some(v), _) => {
                    if let Some(k) = store_key(self.p, self.func, *v) {
                        over.remove(&k);
                    }
                }
                _ => {}
            }
            // a partial store reads the rest of the variable
            if let SK::Assign(pl, _) = &s.kind
                && !matches!(pl, Place::Var(_))
                && let Some(v) = place_root(pl)
                && let Some(k) = store_key(self.p, self.func, v)
            {
                over.remove(&k);
            }
            crate::check::flow::visit_stmt(s, &mut |e| match &e.kind {
                EK::Var(v) => {
                    if let Some(k) = store_key(self.p, self.func, *v) {
                        over.remove(&k);
                    }
                }
                // called code may read any global; locals reach it only as captured values,
                // read when the closure is created
                EK::Call(..) | EK::Async(..) | EK::Await(_) | EK::FnRef(_) => {
                    over.retain(|k, _| matches!(k, Key::Local(..)));
                }
                EK::Closure(f) => {
                    over.retain(|k, _| matches!(k, Key::Local(..)));
                    for c in &self.p.funcs[*f].captures {
                        over.remove(&Key::Local(self.func, c.outer));
                    }
                }
                _ => {}
            });
        }
        for d in declare {
            let s = &mut b.stmts[d];
            if let SK::Assign(Place::Var(v), e) = &mut s.kind {
                let e = std::mem::replace(
                    e,
                    Expr {
                        kind: EK::None,
                        ty: e.ty,
                        span: e.span,
                    },
                );
                s.kind = SK::Let(Some(*v), e);
            }
        }
        let mut k = keep.iter();
        b.stmts.retain(|_| *k.next().unwrap());
    }

    fn stmt(&mut self, s: &mut Stmt) {
        match &mut s.kind {
            SK::Let(_, e) | SK::Destructure(_, e) | SK::Expr(e) | SK::Throw(e) | SK::Assert(e) => {
                self.expr(e)
            }
            SK::Assign(pl, e) => {
                self.place(pl);
                self.expr(e);
            }
            SK::Return(e) | SK::Break(_, e) => {
                if let Some(e) = e {
                    self.expr(e);
                }
            }
            SK::Continue(_) => {}
            SK::While { cond: e, body, .. }
            | SK::For { iter: e, body, .. }
            | SK::Lock { mutex: e, body, .. } => {
                self.expr(e);
                self.block(body);
            }
            SK::Block(b) => self.block(b),
        }
    }

    fn place(&mut self, pl: &mut Place) {
        match pl {
            Place::Var(_) => {}
            Place::Field(b, _) | Place::TupleIndex(b, _) => self.place(b),
            Place::Index(b, i) => {
                self.place(b);
                self.expr(i);
            }
        }
    }

    /// Prune the blocks nested in `e`.
    fn expr(&mut self, e: &mut Expr) {
        e.for_each_child_mut(&mut |c| self.expr(c));
        match &mut e.kind {
            EK::If(i) => {
                for a in &mut i.arms {
                    for pt in &mut a.patterns {
                        pt.for_each_expr_mut(&mut |x| self.expr(x));
                    }
                    self.block(&mut a.body);
                }
            }
            EK::Catch(_, _, b, _) | EK::Else(_, Fallback::Block(b, _)) => self.block(b),
            _ => {}
        }
    }
}

fn flat(b: &Block) -> bool {
    !b.stmts
        .iter()
        .any(|s| matches!(s.kind, SK::Let(..) | SK::Destructure(..)))
}
