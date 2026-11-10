//! Statement checking.

use super::*;

impl<'a> Checker<'a> {
    pub fn check_block(&mut self, b: &ast::Block) -> Result<Block> {
        self.push_scope();
        let mut out = Vec::new();
        for s in &b.stmts {
            // report the error and keep checking the following statements
            let r = self.check_stmt(s, &mut out);
            self.report(r);
        }
        self.pop_scope();
        Ok(Block { stmts: out })
    }

    pub fn tparams(&self) -> HashMap<String, TyId> {
        self.ctx_ref().tparams.clone()
    }

    pub fn resolve_type_here(&mut self, te: &ast::TypeExpr) -> Result<TyId> {
        let m = self.module();
        let tp = self.tparams();
        self.resolve_type(te, m, &tp)
    }

    fn in_throwing_ctx(&self) -> bool {
        let c = self.ctx_ref();
        c.is_main || c.is_test || matches!(self.types.kind(c.ret), TyKind::ErrU(_))
    }

    pub fn require_throwing(&self, span: Span, what: &str) -> Result<()> {
        if !self.in_throwing_ctx() {
            let name = self.funcs.len();
            let _ = name;
            return err(span, format!("{what} requires the enclosing function to be throwing (add `!` to its return type)"));
        }
        Ok(())
    }

    fn in_test(&self) -> bool {
        self.fns.iter().any(|f| f.is_test)
    }

    /// Declare a variable either as a global (module top level) or a local.
    fn declare_var(&mut self, name: &str, ty: TyId, mutable: bool, public: bool, span: Span) -> Result<Option<VarRef>> {
        if name == "_" {
            return Ok(None);
        }
        if crate::parser::is_keyword(name) {
            return err(span, format!("`{name}` is a keyword"));
        }
        if self.at_module_level() {
            let g = self.declare_global(name, ty, mutable, span)?;
            self.set_global_public(name, public);
            Ok(Some(VarRef::Global(g)))
        } else {
            if public {
                return err(span, "only module-level declarations can be `pub`");
            }
            Ok(Some(VarRef::Local(self.declare_local(name, ty, mutable, LocalKind::Normal, span)?)))
        }
    }

    pub fn check_stmt(&mut self, s: &ast::Stmt, out: &mut Vec<Stmt>) -> Result<()> {
        use ast::StmtKind as S;
        let span = s.span;
        let push = |out: &mut Vec<Stmt>, kind| out.push(Stmt { kind, span });
        match &s.kind {
            S::VarDecl { public, mutable, mutex, targets, value } => {
                if targets.len() == 1 {
                    let t = &targets[0];
                    let ty = self.resolve_type_here(&t.ty)?;
                    if let TyKind::Fut(_) = self.types.kind(ty) {
                        if *mutable {
                            return err(span, "futures cannot be mutable");
                        }
                        if !matches!(value.kind, ast::ExprKind::Async(_)) {
                            return err(value.span, "a future can only be declared for an `async` function call");
                        }
                    }
                    if *mutex && matches!(self.types.kind(ty), TyKind::Fut(_)) {
                        return err(span, "futures cannot be placed in a mutex");
                    }
                    let v = self.expect_expr(value, ty);
                    let vty = if *mutex { self.types.intern(TyKind::Mutex(ty)) } else { ty };
                    // declare the variable even if its value is invalid, to avoid follow-up errors
                    let var = self.declare_var(&t.name, vty, *mutable, *public, t.span)?;
                    push(out, SK::Let(var, v?));
                } else {
                    if *mutex {
                        return err(span, "a mutex cannot be destructured");
                    }
                    let mut tys = Vec::new();
                    for t in targets {
                        tys.push(self.resolve_type_here(&t.ty)?);
                    }
                    // expected type when the value is a literal tuple with one entry per target
                    let v = match &value.kind {
                        ast::ExprKind::Tuple(items) if items.len() == targets.len() => {
                            let tt = self.types.tuple(tys.clone());
                            self.expect_expr(value, tt)?
                        }
                        _ => self.check_expr(value, None)?,
                    };
                    let elems = match self.types.kind(v.ty) {
                        TyKind::Tuple(ts) => ts.clone(),
                        TyKind::Opt(_) => {
                            return err(value.span, "cannot destructure an optional tuple without first checking for `none`")
                        }
                        _ => return err(value.span, format!("cannot destructure `{}`", self.types.show(v.ty))),
                    };
                    let mut parts = Vec::new();
                    let mut i = 0;
                    for (t, ty) in targets.iter().zip(&tys) {
                        let count = if i < elems.len() && elems[i] == *ty {
                            1
                        } else if let TyKind::Tuple(sub) = self.types.kind(*ty).clone() {
                            if i + sub.len() <= elems.len() && elems[i..i + sub.len()] == sub[..] {
                                sub.len()
                            } else {
                                return err(t.span, self.destructure_msg(*ty, &elems, i));
                            }
                        } else {
                            return err(t.span, self.destructure_msg(*ty, &elems, i));
                        };
                        parts.push((t, *ty, i, count));
                        i += count;
                    }
                    if i != elems.len() {
                        return err(span, format!("tuple has {} elements but only {} are destructured", elems.len(), i));
                    }
                    let mut tgts = Vec::new();
                    for (t, ty, start, count) in parts {
                        let var = self.declare_var(&t.name, ty, *mutable, *public, t.span)?;
                        tgts.push((var, start, count));
                    }
                    push(out, SK::Destructure(tgts, v));
                }
            }
            S::FnVar { name, value } => {
                let ast::ExprKind::FnLit(sig, body) = &value.kind else {
                    return err(value.span, "expected a function literal `fn(...) { ... }`");
                };
                let (fid, fty) = self.check_nested_fn(sig, body, None)?;
                let var = self.declare_var(name, fty, false, false, span)?;
                push(out, SK::Let(var, Expr { kind: EK::Closure(fid), ty: fty, span: value.span }));
            }
            S::Assign { target, value } => {
                let (place, ty) = self.check_place(target)?;
                let v = self.expect_expr(value, ty)?;
                push(out, SK::Assign(place, v));
            }
            S::Discard(e) => {
                let x = self.check_expr_opts(e, None, false)?;
                if matches!(self.types.kind(x.ty), TyKind::Fut(_)) {
                    return err(e.span, "futures cannot be discarded; `await` them");
                }
                push(out, SK::Expr(x));
            }
            S::Expr(e) => {
                let x = self.check_expr_opts(e, None, false)?;
                let ok = x.ty == VOID
                    || x.ty == NEVER
                    || matches!(x.kind, EK::Else(_, Fallback::Block(..)) | EK::Catch(..) | EK::If(_))
                        && !matches!(self.types.kind(x.ty), TyKind::ErrU(_));
                if !ok {
                    let msg = match self.types.kind(x.ty) {
                        TyKind::ErrU(_) => format!(
                            "error union `{}` must be handled with `catch` or `try`",
                            self.types.show(x.ty)
                        ),
                        _ if matches!(x.kind, EK::Call(..)) => format!(
                            "return value of function not used (type `{}`); assign it to `_` to discard it",
                            self.types.show(x.ty)
                        ),
                        _ => format!("unused value of type `{}`", self.types.show(x.ty)),
                    };
                    return err(e.span, msg);
                }
                push(out, SK::Expr(x));
            }
            S::Return(v) => {
                let c = self.ctx_ref();
                if c.is_main {
                    return err(span, "`return` outside of a function");
                }
                if c.is_never {
                    return err(span, "cannot `return` from a function declared `never`");
                }
                let ret = c.ret;
                let target = match self.types.kind(ret) {
                    TyKind::ErrU(i) => *i,
                    _ => ret,
                };
                match v {
                    None => {
                        if target != VOID {
                            return err(span, format!("expected a return value of type `{}`", self.types.show(target)));
                        }
                        push(out, SK::Return(None));
                    }
                    Some(v) => {
                        if target == VOID {
                            return err(v.span, "this function does not return a value");
                        }
                        let x = self.expect_expr(v, target)?;
                        push(out, SK::Return(Some(x)));
                    }
                }
            }
            S::Throw(e) => {
                self.require_throwing(span, "`throw`")?;
                let x = self.check_expr(e, Some(STR))?;
                if x.ty != STR && x.ty != ERROR {
                    return err(e.span, format!("can only throw `str` or `error` values, found `{}`", self.types.show(x.ty)));
                }
                push(out, SK::Throw(x));
            }
            S::Break { label, value } => {
                let jumps = self.ctx_ref().jumps.clone();
                if let Some(v) = value {
                    let Some(j) = jumps.iter().rev().find(|j| matches!(j.kind, JumpKind::Value(_))) else {
                        return err(span, "`break` with a value outside of a value assignment");
                    };
                    let JumpKind::Value(ty) = j.kind else { unreachable!() };
                    let x = self.expect_expr(v, ty)?;
                    self.broken.insert(j.id);
                    push(out, SK::Break(j.id, Some(x)));
                } else if let Some(l) = label {
                    let Some(j) = jumps.iter().rev().find(|j| j.label.as_deref() == Some(l)) else {
                        return err(span, format!("unknown label `{l}`"));
                    };
                    if matches!(j.kind, JumpKind::Value(_)) {
                        return err(span, format!("breaking out of `{l}` requires a value"));
                    }
                    self.broken.insert(j.id);
                    push(out, SK::Break(j.id, None));
                } else {
                    let Some(j) = jumps.iter().rev().find(|j| matches!(j.kind, JumpKind::Loop | JumpKind::Lock)) else {
                        return err(span, "`break` outside of a loop");
                    };
                    self.broken.insert(j.id);
                    push(out, SK::Break(j.id, None));
                }
            }
            S::Continue(label) => {
                let jumps = &self.ctx_ref().jumps;
                let j = match label {
                    Some(l) => match jumps.iter().rev().find(|j| j.label.as_deref() == Some(l)) {
                        Some(j) if j.kind == JumpKind::Loop => j.id,
                        Some(_) => return err(span, format!("`{l}` is not a loop")),
                        None => return err(span, format!("unknown label `{l}`")),
                    },
                    None => match jumps.iter().rev().find(|j| j.kind == JumpKind::Loop) {
                        Some(j) => j.id,
                        None => return err(span, "`continue` outside of a loop"),
                    },
                };
                push(out, SK::Continue(j));
            }
            S::While { label, cond, body } => {
                let c = self.expect_expr(cond, BOOL)?;
                let id = self.new_jump();
                self.ctx().jumps.push(JumpCtx { id, kind: JumpKind::Loop, label: label.clone() });
                let b = self.check_block(body);
                self.ctx().jumps.pop();
                push(out, SK::While { id, cond: c, body: b? });
            }
            S::For { label, var, var_span, iter, body } => {
                let it = self.check_expr(iter, None)?;
                let (kind, vty) = match self.types.kind(it.ty) {
                    TyKind::Array(_) | TyKind::Fixed(..) | TyKind::Str => (ForKind::Index, UINT),
                    TyKind::Map(k, _) => (ForKind::Keys, *k),
                    _ => {
                        return err(
                            iter.span,
                            format!("cannot loop over `{}`; expected an array, string or map", self.types.show(it.ty)),
                        )
                    }
                };
                let id = self.new_jump();
                self.push_scope();
                let v = self.declare_local(var, vty, false, LocalKind::Normal, *var_span)?;
                self.ctx().jumps.push(JumpCtx { id, kind: JumpKind::Loop, label: label.clone() });
                let b = self.check_block(body);
                self.ctx().jumps.pop();
                self.pop_scope();
                push(out, SK::For { id, var: v, kind, iter: it, body: b? });
            }
            S::Lock { label, name, name_span, body } => {
                let (vr, ty) = match self.resolve_var(name, *name_span)? {
                    Some(x) => x,
                    None => return err(*name_span, format!("unknown variable `{name}`")),
                };
                self.idx_var_ref(vr, *name_span);
                let TyKind::Mutex(inner) = *self.types.kind(ty) else {
                    return err(*name_span, format!("`{name}` is not a mutex"));
                };
                let mutex = Expr { kind: EK::Var(vr), ty, span: *name_span };
                let id = self.new_jump();
                self.push_scope();
                let alias = self.declare_local(name, inner, true, LocalKind::LockAlias, *name_span)?;
                self.ctx().jumps.push(JumpCtx { id, kind: JumpKind::Lock, label: label.clone() });
                let b = self.check_block(body);
                self.ctx().jumps.pop();
                self.pop_scope();
                push(out, SK::Lock { id, mutex, alias, body: b? });
            }
            S::Block(b) => {
                let b = self.check_block(b)?;
                push(out, SK::Block(b));
            }
            S::Fn(f) => {
                if f.public {
                    return err(span, "nested functions cannot be `pub`");
                }
                let (fid, fty) = self.check_nested_fn(&f.sig, &f.body, Some(&f.sig.name))?;
                let var = self.declare_var(&f.sig.name, fty, false, false, f.sig.span)?;
                push(out, SK::Let(var, Expr { kind: EK::Closure(fid), ty: fty, span: f.sig.span }));
            }
            S::Assert(e) => {
                if !self.in_test() {
                    return err(span, "`assert` can only be used inside a `test` block");
                }
                let x = self.expect_expr(e, BOOL)?;
                push(out, SK::Assert(x));
            }
        }
        Ok(())
    }

    fn destructure_msg(&self, ty: TyId, elems: &[TyId], i: usize) -> String {
        match elems.get(i) {
            Some(e) => format!(
                "cannot destructure element `{}` of the tuple into `{}`",
                self.types.show(*e),
                self.types.show(ty)
            ),
            None => "too many destructuring targets for this tuple".into(),
        }
    }

    /// Check a nested function or function literal; returns its id and function type.
    pub fn check_nested_fn(&mut self, sig: &ast::FnSig, body: &ast::Block, name: Option<&str>) -> Result<(FuncId, TyId)> {
        if !sig.type_params.is_empty() {
            return err(sig.span, "generic functions must be declared at module level");
        }
        let m = self.module();
        let tp = self.tparams();
        let (params, ret) = self.resolve_sig(sig, m, &tp)?;
        let fty = self.types.intern(TyKind::Fn(params.clone(), ret));
        let display = name.map(|s| s.to_string()).unwrap_or_else(|| "fn literal".to_string());
        let fid = self.reserve_fn(display.clone(), params.clone(), ret);
        self.ix.fn_spans.insert(fid, sig.span);
        if ret == NEVER {
            self.never_funcs.insert(fid);
        }
        let parent = self.ctx_ref().func;
        let mut ctx = self.new_ctx(fid, m, ret, tp);
        ctx.is_never = ret == NEVER;
        self.fns.push(ctx);
        let r = (|| {
            if let Some(n) = name {
                let sl = self.declare_local(n, fty, false, LocalKind::Normal, sig.span)?;
                self.ctx().self_local = Some(sl);
            }
            let mut pids = Vec::new();
            for (i, p) in sig.params.iter().enumerate() {
                pids.push(self.declare_local(&p.name, params[i], false, LocalKind::Param, p.span)?);
            }
            let before = self.errors.len();
            let b = self.check_block(body)?;
            if self.errors.len() == before {
                self.check_fn_end(&b, ret, sig.span, &display)?;
            }
            Ok((pids, b))
        })();
        let ctx = self.fns.pop().unwrap();
        let (pids, b) = r?;
        self.finish_fn(ctx, display, pids, b, sig.span, Some(parent));
        Ok((fid, fty))
    }

    /// Resolve a variable name to a reference (without the mutex access check).
    pub fn resolve_var(&mut self, name: &str, span: Span) -> Result<Option<(VarRef, TyId)>> {
        if let Some(l) = self.lookup_local(name, span)? {
            let ty = self.ctx_ref().locals[l].ty;
            return Ok(Some((VarRef::Local(l), ty)));
        }
        let m = self.module();
        if let Some(SymEntry { sym: Sym::Globals(list), .. }) = self.mods[m].syms.get(name) {
            let in_main = self.fns.len() == 1 && self.ctx_ref().is_main;
            let g = Self::pick_global(list, span.start, span.file == self.mods[m].file || in_main);
            return Ok(Some((VarRef::Global(g), self.globals[g].ty)));
        }
        Ok(None)
    }

    pub fn var_mutable(&self, v: VarRef) -> bool {
        match v {
            VarRef::Local(l) => {
                let loc = &self.ctx_ref().locals[l];
                loc.mutable
            }
            VarRef::Global(g) => self.globals[g].mutable,
        }
    }

    fn check_place(&mut self, e: &ast::Expr) -> Result<(Place, TyId)> {
        use ast::ExprKind as E;
        match &e.kind {
            E::Ident(name) => {
                let Some((v, ty)) = self.resolve_var(name, e.span)? else {
                    return err(e.span, format!("unknown variable `{name}`"));
                };
                self.idx_var_ref(v, e.span);
                if matches!(self.types.kind(ty), TyKind::Mutex(_)) {
                    return err(e.span, format!("`{name}` is a mutex; access it inside `lock {name} {{ ... }}`"));
                }
                if !self.var_mutable(v) {
                    let what = match v {
                        VarRef::Local(l) if self.ctx_ref().locals[l].kind == LocalKind::Param => "function argument",
                        _ => "immutable variable",
                    };
                    return err(e.span, format!("cannot assign to {what} `{name}`; declare it with `mut`"));
                }
                Ok((Place::Var(v), ty))
            }
            E::Field(base, name) => {
                let (p, ty) = self.check_place(base)?;
                match self.types.kind(ty).clone() {
                    TyKind::Struct(i) => {
                        let fields = &self.types.structs[i as usize].fields;
                        match fields.iter().position(|(n, _)| n == name) {
                            Some(idx) => Ok((Place::Field(Box::new(p), idx), fields[idx].1)),
                            None => err(e.span, format!("`{}` has no field `{name}`", self.types.show(ty))),
                        }
                    }
                    _ if name == "len" => err(e.span, "`.len` cannot be assigned"),
                    _ => err(e.span, format!("`{}` has no field `{name}`", self.types.show(ty))),
                }
            }
            E::Index(base, idx) => {
                let (p, ty) = self.check_place(base)?;
                match self.types.kind(ty).clone() {
                    TyKind::Tuple(ts) => {
                        let i = self.tuple_index(idx, ts.len())?;
                        Ok((Place::TupleIndex(Box::new(p), i), ts[i]))
                    }
                    TyKind::Map(k, v) => {
                        let i = self.expect_expr(idx, k)?;
                        Ok((Place::Index(Box::new(p), Box::new(i)), v))
                    }
                    TyKind::Array(el) | TyKind::Fixed(el, _) => {
                        let i = self.seq_index(idx, ty)?;
                        Ok((Place::Index(Box::new(p), Box::new(i)), el))
                    }
                    TyKind::Str => {
                        let i = self.seq_index(idx, ty)?;
                        Ok((Place::Index(Box::new(p), Box::new(i)), CHAR))
                    }
                    _ => err(e.span, format!("cannot index into `{}`", self.types.show(ty))),
                }
            }
            _ => err(e.span, "invalid assignment target"),
        }
    }

    pub fn tuple_index(&mut self, idx: &ast::Expr, len: usize) -> Result<usize> {
        match idx.kind {
            ast::ExprKind::Int(v, _) if (v as usize) < len => Ok(v as usize),
            ast::ExprKind::Int(v, _) => err(idx.span, format!("tuple index {v} out of range for a tuple of {len} elements")),
            _ => err(idx.span, "tuple indexes must be integer literals"),
        }
    }

    /// Index into an array or string; `$` refers to the base's last index.
    pub fn seq_index(&mut self, idx: &ast::Expr, base_ty: TyId) -> Result<Expr> {
        self.ctx().dollar.push(base_ty);
        let r = self.check_expr(idx, None);
        self.ctx().dollar.pop();
        let i = r?;
        if i.ty != INT && i.ty != UINT {
            return err(idx.span, format!("indexes must be `int` or `uint`, found `{}`", self.types.show(i.ty)));
        }
        Ok(i)
    }
}
