//! Expression checking.

use super::*;
use crate::ast::{BinOp, ExprKind as E, UnOp};

/// A resolved call target.
enum Target {
    Call(Callee, Vec<TyId>, TyId),
    Variant(TyId, usize, Vec<TyId>),
}

impl<'a> Checker<'a> {
    pub fn check_expr(&mut self, e: &ast::Expr, expected: Option<TyId>) -> Result<Expr> {
        self.check_expr_opts(e, expected, true)
    }

    /// Check `e` against `ty`, applying implicit coercions.
    pub fn expect_expr(&mut self, e: &ast::Expr, ty: TyId) -> Result<Expr> {
        let x = self.check_expr(e, Some(ty))?;
        self.coerce(x, ty)
    }

    pub fn coerce(&mut self, e: Expr, ty: TyId) -> Result<Expr> {
        if e.ty == ty || e.ty == NEVER {
            return Ok(e);
        }
        if let TyKind::Opt(inner) = *self.types.kind(ty)
            && (e.ty == inner || self.can_coerce(e.ty, inner))
        {
            let span = e.span;
            let inner_e = self.coerce(e, inner)?;
            return Ok(Expr {
                kind: EK::Some(Box::new(inner_e)),
                ty,
                span,
            });
        }
        if e.ty == VOID {
            return err(
                e.span,
                format!(
                    "expected a value of type `{}`, but this expression has no value",
                    self.types.show(ty)
                ),
            );
        }
        if self.types.kind(ty) == &TyKind::Void {
            return err(
                e.span,
                format!("unexpected value of type `{}`", self.types.show(e.ty)),
            );
        }
        let mut msg = format!(
            "expected `{}`, found `{}`",
            self.types.show(ty),
            self.types.show(e.ty)
        );
        if matches!(self.types.kind(e.ty), TyKind::Opt(_)) {
            msg.push_str("; optionals must be checked for `none` first (use `else`)");
        }
        if let (TyKind::Fixed(a, _), TyKind::Array(b)) =
            (self.types.kind(e.ty), self.types.kind(ty))
            && a == b
        {
            msg.push_str("; convert with `@as`");
        }
        err(e.span, msg)
    }

    fn can_coerce(&self, from: TyId, to: TyId) -> bool {
        if from == to {
            return true;
        }
        match self.types.kind(to) {
            TyKind::Opt(i) => self.can_coerce(from, *i),
            _ => false,
        }
    }

    /// Strip optional layers from an expected type (for literal typing).
    fn strip_opt(&self, mut t: TyId) -> TyId {
        while let TyKind::Opt(i) = self.types.kind(t) {
            t = *i;
        }
        t
    }

    /// Literal type: the expected type when its base matches `base`, else `base`.
    fn lit_ty(&self, expected: Option<TyId>, base: TyId) -> TyId {
        if let Some(t) = expected {
            let t = self.strip_opt(t);
            if self.types.base(t) == base {
                return t;
            }
        }
        base
    }

    fn is_untyped_int(e: &ast::Expr) -> bool {
        match &e.kind {
            E::Int(_, u) => !u,
            E::Unary(UnOp::Neg | UnOp::BitNot, x) => Self::is_untyped_int(x),
            E::Binary(op, a, b)
                if !matches!(
                    op,
                    BinOp::Eq
                        | BinOp::Ne
                        | BinOp::Lt
                        | BinOp::Gt
                        | BinOp::Le
                        | BinOp::Ge
                        | BinOp::In
                        | BinOp::And
                        | BinOp::Or
                        | BinOp::Concat
                ) =>
            {
                Self::is_untyped_int(a) && Self::is_untyped_int(b)
            }
            _ => false,
        }
    }

    fn int_lit(
        &mut self,
        v: i128,
        unsigned: bool,
        expected: Option<TyId>,
        span: Span,
    ) -> Result<Expr> {
        let ty = if unsigned {
            UINT
        } else {
            match expected.map(|t| self.strip_opt(t)) {
                Some(t) if self.types.is_integer(self.types.base(t)) => t,
                Some(t) if self.types.base(t) == FLOAT => {
                    return err(
                        span,
                        "integer literals cannot be used as `float`; write a decimal point (e.g. `1.0`)",
                    );
                }
                _ => INT,
            }
        };
        let base = self.types.base(ty);
        let lit = match base {
            BYTE => {
                if !(0..=255).contains(&v) {
                    return err(
                        span,
                        format!("literal `{v}` is out of range for `byte` (0 to 255)"),
                    );
                }
                Lit::Byte(v as u8)
            }
            INT => {
                if v < i64::MIN as i128 || v > i64::MAX as i128 {
                    return err(span, format!("literal `{v}` is out of range for `int`"));
                }
                Lit::Int(v as i64)
            }
            _ => {
                if v < 0 {
                    return err(span, format!("negative value `{v}` cannot be a `uint`"));
                }
                if v > u64::MAX as i128 {
                    return err(span, format!("literal `{v}` is out of range for `uint`"));
                }
                Lit::UInt(v as u64)
            }
        };
        Ok(Expr {
            kind: EK::Lit(lit),
            ty,
            span,
        })
    }

    pub fn check_expr_opts(
        &mut self,
        e: &ast::Expr,
        expected: Option<TyId>,
        value_needed: bool,
    ) -> Result<Expr> {
        let r = self.check_expr_raw(e, expected, value_needed);
        if let Ok(x) = &r {
            self.idx_expr(e.span, x.ty);
        }
        r
    }

    fn check_expr_raw(
        &mut self,
        e: &ast::Expr,
        expected: Option<TyId>,
        value_needed: bool,
    ) -> Result<Expr> {
        let span = e.span;
        let mk = |kind, ty| Ok(Expr { kind, ty, span });
        // named types can be created with the literal syntax of their underlying type
        if let Some(exp) = expected {
            let exp_s = self.strip_opt(exp);
            if let TyKind::Named(_) = self.types.kind(exp_s)
                && matches!(
                    e.kind,
                    E::Array(_) | E::Map(_) | E::Tuple(_) | E::Struct(..) | E::Str(_)
                )
            {
                let under = self.types.base(exp_s);
                let x = self.expect_expr(e, under)?;
                return Ok(Expr {
                    kind: EK::Retype(Box::new(x)),
                    ty: exp_s,
                    span,
                });
            }
        }
        match &e.kind {
            E::Int(v, u) => self.int_lit(*v as i128, *u, expected, span),
            E::Unary(UnOp::Neg, inner) if matches!(inner.kind, E::Int(..)) => {
                let E::Int(v, u) = inner.kind else {
                    unreachable!()
                };
                if u {
                    return err(span, "negative value cannot be a `uint`");
                }
                self.int_lit(-(v as i128), false, expected, span)
            }
            E::Float(f) => mk(EK::Lit(Lit::Float(*f)), self.lit_ty(expected, FLOAT)),
            E::Unary(UnOp::Neg, inner) if matches!(inner.kind, E::Float(_)) => {
                let E::Float(f) = inner.kind else {
                    unreachable!()
                };
                mk(EK::Lit(Lit::Float(-f)), self.lit_ty(expected, FLOAT))
            }
            E::Char(c) => mk(EK::Lit(Lit::Char(*c)), self.lit_ty(expected, CHAR)),
            E::Bool(b) => mk(EK::Lit(Lit::Bool(*b)), self.lit_ty(expected, BOOL)),
            E::Str(segs) => {
                let ty = self.lit_ty(expected, STR);
                if segs.iter().all(|s| matches!(s, ast::StrSeg::Lit(_))) {
                    let mut s = String::new();
                    for seg in segs {
                        if let ast::StrSeg::Lit(l) = seg {
                            s.push_str(l);
                        }
                    }
                    return mk(EK::Lit(Lit::Str(s)), ty);
                }
                let mut parts = Vec::new();
                for seg in segs {
                    match seg {
                        ast::StrSeg::Lit(l) => parts.push(Expr {
                            kind: EK::Lit(Lit::Str(l.clone())),
                            ty: STR,
                            span,
                        }),
                        ast::StrSeg::Expr(x) => {
                            let v = self.check_expr(x, None)?;
                            parts.push(self.stringify(v)?);
                        }
                    }
                }
                mk(EK::Format(parts), ty)
            }
            E::None => match expected {
                Some(t) if matches!(self.types.kind(t), TyKind::Opt(_)) => mk(EK::None, t),
                Some(t) => err(
                    span,
                    format!(
                        "cannot use `none` as `{}`, which is not optional",
                        self.types.show(t)
                    ),
                ),
                None => err(span, "cannot infer the optional type of `none` here"),
            },
            E::Ident(name) => self.check_ident(name, span),
            E::Wildcard => err(
                span,
                "`_` can only be used in patterns or as a discard target",
            ),
            E::Dollar => {
                if self.ctx_ref().dollar.is_empty() {
                    return err(span, "`$` can only be used inside an array or string index");
                }
                mk(EK::Dollar, UINT)
            }
            E::Unary(op, inner) => {
                let x = self.check_expr(inner, expected)?;
                let ok = match op {
                    UnOp::Neg => x.ty == INT || x.ty == FLOAT,
                    UnOp::BitNot => self.types.is_integer(x.ty),
                    UnOp::Not => x.ty == BOOL,
                };
                if !ok {
                    let sym = match op {
                        UnOp::Neg => "-",
                        UnOp::BitNot => "!",
                        UnOp::Not => "not",
                    };
                    return err(
                        span,
                        format!(
                            "operator `{sym}` cannot be applied to `{}`",
                            self.types.show(x.ty)
                        ),
                    );
                }
                let ty = x.ty;
                mk(EK::Unary(*op, Box::new(x)), ty)
            }
            E::Binary(op, l, r) => self.check_binary(*op, l, r, expected, span),
            E::Call(callee, args) => {
                let (target, args) = self.check_call_parts(callee, args)?;
                match target {
                    Target::Call(c, _, ret) => mk(EK::Call(c, args), ret),
                    Target::Variant(ty, idx, _) => mk(EK::Variant(idx, args), ty),
                }
            }
            E::Generic(..) => {
                if let Some(f) = self.generic_fn_ref(e)? {
                    let (ps, r) = self.sigs[f].clone();
                    let ty = self.types.intern(TyKind::Fn(ps, r));
                    return mk(EK::FnRef(f), ty);
                }
                err(span, "a type cannot be used as a value")
            }
            E::Builtin(name, ty, args) => self.check_builtin(name, ty.as_ref(), args, span),
            E::Field(base, name) => self.check_field(base, name, span),
            E::Index(base, idx) => {
                let b = self.check_expr(base, None)?;
                match self.types.kind(b.ty).clone() {
                    TyKind::Tuple(ts) => {
                        let i = self.tuple_index(idx, ts.len())?;
                        mk(EK::TupleIndex(Box::new(b), i), ts[i])
                    }
                    TyKind::Map(k, v) => {
                        let i = self.expect_expr(idx, k)?;
                        mk(EK::Index(Box::new(b), Box::new(i)), v)
                    }
                    TyKind::Array(el) | TyKind::Fixed(el, _) => {
                        let i = self.seq_index(idx, b.ty)?;
                        mk(EK::Index(Box::new(b), Box::new(i)), el)
                    }
                    TyKind::Str => {
                        let i = self.seq_index(idx, b.ty)?;
                        mk(EK::Index(Box::new(b), Box::new(i)), CHAR)
                    }
                    _ => err(
                        span,
                        format!("cannot index into `{}`", self.types.show(b.ty)),
                    ),
                }
            }
            E::Tuple(items) => {
                let exp =
                    expected
                        .map(|t| self.strip_opt(t))
                        .and_then(|t| match self.types.kind(t) {
                            TyKind::Tuple(ts) if ts.len() == items.len() => Some(ts.clone()),
                            _ => None,
                        });
                let mut xs = Vec::new();
                for (i, it) in items.iter().enumerate() {
                    let x = match &exp {
                        Some(ts) => self.expect_expr(it, ts[i])?,
                        None => self.check_expr(it, None)?,
                    };
                    if matches!(
                        self.types.kind(x.ty),
                        TyKind::Void | TyKind::Never | TyKind::Fut(_)
                    ) {
                        return err(
                            it.span,
                            format!("`{}` cannot be stored in a tuple", self.types.show(x.ty)),
                        );
                    }
                    xs.push(x);
                }
                let ty = self.types.tuple(xs.iter().map(|x| x.ty).collect());
                mk(EK::Tuple(xs), ty)
            }
            E::Array(items) => {
                let exp = expected.map(|t| self.strip_opt(t));
                let exp_kind = exp.map(|t| self.types.kind(t).clone());
                if items.is_empty() {
                    return match exp_kind {
                        Some(TyKind::Array(_)) => mk(EK::Array(vec![]), exp.unwrap()),
                        Some(TyKind::Fixed(_, 0)) => mk(EK::Array(vec![]), exp.unwrap()),
                        Some(TyKind::Map(..)) => mk(EK::Map(vec![]), exp.unwrap()),
                        _ => err(span, "cannot infer the type of an empty array literal here"),
                    };
                }
                let (elem, fixed) = match exp_kind {
                    Some(TyKind::Array(e)) => (Some(e), None),
                    Some(TyKind::Fixed(e, n)) => (Some(e), Some(n)),
                    _ => (None, None),
                };
                let mut xs = Vec::new();
                let mut et = elem;
                for it in items {
                    let x = match et {
                        Some(t) => self.expect_expr(it, t)?,
                        None => {
                            let x = self.check_expr(it, None)?;
                            et = Some(x.ty);
                            x
                        }
                    };
                    xs.push(x);
                }
                let et = et.unwrap();
                if matches!(
                    self.types.kind(et),
                    TyKind::Void | TyKind::Never | TyKind::Fut(_) | TyKind::Mutex(_)
                ) {
                    return err(
                        span,
                        format!("arrays cannot hold `{}`", self.types.show(et)),
                    );
                }
                let ty = match fixed {
                    Some(n) => {
                        if n as usize != xs.len() {
                            return err(
                                span,
                                format!(
                                    "expected exactly {n} elements for `{}`, found {}",
                                    self.types.show(exp.unwrap()),
                                    xs.len()
                                ),
                            );
                        }
                        exp.unwrap()
                    }
                    None => self.types.array(et),
                };
                mk(EK::Array(xs), ty)
            }
            E::Map(entries) => {
                let exp = expected.map(|t| self.strip_opt(t));
                let (mut kt, mut vt) = match exp.map(|t| self.types.kind(t).clone()) {
                    Some(TyKind::Map(k, v)) => (Some(k), Some(v)),
                    _ => (None, None),
                };
                let mut xs = Vec::new();
                for (k, v) in entries {
                    let kx = match kt {
                        Some(t) => self.expect_expr(k, t)?,
                        None => {
                            let x = self.check_expr(k, None)?;
                            kt = Some(x.ty);
                            x
                        }
                    };
                    let vx = match vt {
                        Some(t) => self.expect_expr(v, t)?,
                        None => {
                            let x = self.check_expr(v, None)?;
                            vt = Some(x.ty);
                            x
                        }
                    };
                    xs.push((kx, vx));
                }
                let (kt, vt) = (kt.unwrap(), vt.unwrap());
                if !self.comparable(kt) {
                    return err(
                        span,
                        format!("`{}` cannot be used as a map key", self.types.show(kt)),
                    );
                }
                let ty = self.types.intern(TyKind::Map(kt, vt));
                mk(EK::Map(xs), ty)
            }
            E::Struct(path, fields) => {
                let Some(ty) = self.type_path(path)? else {
                    return err(path.span, "expected a struct type");
                };
                let Some(def) = self.types.struct_def(ty).cloned() else {
                    return err(
                        path.span,
                        format!("`{}` is not a struct", self.types.show(ty)),
                    );
                };
                let mut vals: Vec<Option<Expr>> = vec![None; def.fields.len()];
                for (name, v, fsp) in fields {
                    let Some(i) = def.fields.iter().position(|(n, _)| n == name) else {
                        return err(*fsp, format!("`{}` has no field `{name}`", def.name));
                    };
                    // `.name`: the field reference covers the dot and the name
                    let name_span = Span {
                        end: fsp.end + 1 + name.len() as u32,
                        ..*fsp
                    };
                    self.idx_field_ref(ty, i, name_span);
                    if vals[i].is_some() {
                        return err(*fsp, format!("field `{name}` is set more than once"));
                    }
                    vals[i] = Some(self.expect_expr(v, def.fields[i].1)?);
                }
                let mut out = Vec::new();
                for (i, v) in vals.into_iter().enumerate() {
                    match v {
                        Some(v) => out.push(v),
                        None => {
                            return err(
                                span,
                                format!("missing field `{}` in `{}`", def.fields[i].0, def.name),
                            );
                        }
                    }
                }
                mk(EK::Struct(out), ty)
            }
            E::FnLit(sig, body) => {
                let (fid, fty) = self.check_nested_fn(sig, body, None)?;
                mk(EK::Closure(fid), fty)
            }
            E::If(ife) => {
                if value_needed {
                    let Some(t) = expected else {
                        return err(
                            span,
                            "cannot infer the type of this `if`; assign it to a variable with a declared type",
                        );
                    };
                    self.check_if(ife, Some(t), span)
                } else {
                    self.check_if(ife, None, span)
                }
            }
            E::Else(opt, fb) => {
                let oexp = expected.map(|t| self.types.opt(t));
                let o = self.check_expr(opt, oexp)?;
                let TyKind::Opt(inner) = *self.types.kind(o.ty) else {
                    return err(
                        opt.span,
                        format!(
                            "`else` can only be used on optional values, found `{}`",
                            self.types.show(o.ty)
                        ),
                    );
                };
                let fallback = match fb {
                    ast::ElseFallback::Expr(x) => {
                        Fallback::Expr(Box::new(self.expect_expr(x, inner)?))
                    }
                    ast::ElseFallback::Block(b) => {
                        let id = self.new_jump();
                        self.ctx().jumps.push(JumpCtx {
                            id,
                            kind: JumpKind::Value(inner),
                            label: None,
                        });
                        let blk = self.check_block(b);
                        self.ctx().jumps.pop();
                        let blk = blk?;
                        if value_needed && self.block_completes(&blk) {
                            return err(
                                b.span,
                                "the `else` block must provide a value with `break <value>` or leave the scope (`return`, `throw`, ...)",
                            );
                        }
                        Fallback::Block(blk, id)
                    }
                };
                mk(EK::Else(Box::new(o), fallback), inner)
            }
            E::Catch(x, name, name_span, b) => {
                let v = self.check_expr(x, None)?;
                let TyKind::ErrU(inner) = *self.types.kind(v.ty) else {
                    return err(
                        x.span,
                        format!(
                            "`catch` can only be used on error unions, found `{}`",
                            self.types.show(v.ty)
                        ),
                    );
                };
                let id = self.new_jump();
                self.push_scope();
                let local = if name == "_" {
                    None
                } else {
                    self.ix.scopes.push(b.span);
                    let l = self.declare_local(name, ERROR, false, LocalKind::Normal, *name_span);
                    self.ix.scopes.pop();
                    Some(l?)
                };
                if inner != VOID {
                    self.ctx().jumps.push(JumpCtx {
                        id,
                        kind: JumpKind::Value(inner),
                        label: None,
                    });
                }
                let blk = self.check_block(b);
                if inner != VOID {
                    self.ctx().jumps.pop();
                }
                self.pop_scope();
                let blk = blk?;
                if value_needed && inner != VOID && self.block_completes(&blk) {
                    return err(
                        b.span,
                        "the `catch` block must provide a value with `break <value>` or leave the scope (`return`, `throw`, ...)",
                    );
                }
                mk(EK::Catch(Box::new(v), local, blk, id), inner)
            }
            E::Try(x) => {
                self.require_throwing(span, "`try`")?;
                let v = self.check_expr(x, None)?;
                let TyKind::ErrU(inner) = *self.types.kind(v.ty) else {
                    return err(
                        x.span,
                        format!(
                            "`try` can only be used on error unions, found `{}`",
                            self.types.show(v.ty)
                        ),
                    );
                };
                mk(EK::Try(Box::new(v)), inner)
            }
            E::Async(call) => {
                let E::Call(callee, args) = &call.kind else {
                    return err(call.span, "`async` must be followed by a function call");
                };
                let (target, args) = self.check_call_parts(callee, args)?;
                match target {
                    Target::Call(c, _, ret) => {
                        if ret == NEVER {
                            return err(span, "cannot run a `never` function asynchronously");
                        }
                        let ty = self.types.intern(TyKind::Fut(ret));
                        mk(EK::Async(c, args), ty)
                    }
                    Target::Variant(..) => {
                        err(call.span, "`async` must be followed by a function call")
                    }
                }
            }
            E::Error => Err(Diag::new(span, String::new())),
            E::Await(x) => {
                let v = self.check_expr(x, None)?;
                let TyKind::Fut(inner) = *self.types.kind(v.ty) else {
                    return err(
                        x.span,
                        format!(
                            "`await` expects a future, found `{}`",
                            self.types.show(v.ty)
                        ),
                    );
                };
                mk(EK::Await(Box::new(v)), inner)
            }
        }
    }

    pub fn stringify(&mut self, v: Expr) -> Result<Expr> {
        if v.ty == STR || v.ty == ERROR {
            let span = v.span;
            return Ok(Expr {
                kind: EK::Retype(Box::new(v)),
                ty: STR,
                span,
            });
        }
        if !self.printable(v.ty) {
            return err(
                v.span,
                format!(
                    "`{}` cannot be converted to a string",
                    self.types.show(v.ty)
                ),
            );
        }
        let span = v.span;
        Ok(Expr {
            kind: EK::ToStr(Box::new(v)),
            ty: STR,
            span,
        })
    }

    fn check_ident(&mut self, name: &str, span: Span) -> Result<Expr> {
        if let Some((v, ty)) = self.resolve_var(name, span)? {
            self.idx_var_ref(v, span);
            if matches!(self.types.kind(ty), TyKind::Mutex(_)) {
                return err(
                    span,
                    format!("`{name}` is a mutex; access it inside `lock {name} {{ ... }}`"),
                );
            }
            return Ok(Expr {
                kind: EK::Var(v),
                ty,
                span,
            });
        }
        // a generic constant: `N` in `fn f<uint N>`
        if let Some(&t) = self.ctx_ref().tparams.get(name)
            && let TyKind::Const(ty, v) = *self.types.kind(t)
        {
            let def = self.ctx_ref().tspans.get(name).copied();
            let hover = format!("{} {name}", self.types.show(ty));
            self.idx_ref(span, def, hover);
            return self.int_lit(v, false, Some(ty), span);
        }
        let m = self.module();
        match self.mods[m].syms.get(name).map(|e| e.sym.clone()) {
            Some(Sym::Fn { func: Some(f), .. }) => {
                self.idx_fn_ref(f, name, span);
                let (ps, r) = self.sigs[f].clone();
                let ty = self.types.intern(TyKind::Fn(ps, r));
                Ok(Expr {
                    kind: EK::FnRef(f),
                    ty,
                    span,
                })
            }
            Some(Sym::Fn { func: None, .. }) => err(
                span,
                format!(
                    "generic function `{name}` requires explicit type arguments: `{name}<...>`"
                ),
            ),
            Some(Sym::Struct { .. }) | Some(Sym::Enum { .. }) | Some(Sym::Type(_)) => {
                err(span, format!("`{name}` is a type, not a value"))
            }
            Some(Sym::Globals(_)) => unreachable!(),
            None => {
                if self.mods[m].imports.contains_key(name)
                    || self.mods[m].externs.contains_key(name)
                {
                    err(span, format!("`{name}` is a module, not a value"))
                } else {
                    err(span, format!("unknown name `{name}`"))
                }
            }
        }
    }

    /// Is this identifier a module alias (not shadowed by a variable)?
    fn module_alias(&mut self, e: &ast::Expr) -> Result<Option<String>> {
        if let E::Ident(n) = &e.kind {
            let m = self.module();
            if (self.mods[m].imports.contains_key(n) || self.mods[m].externs.contains_key(n))
                && self.resolve_var(n, e.span)?.is_none()
            {
                self.ix.used_aliases.borrow_mut().insert((m, n.clone()));
                return Ok(Some(n.clone()));
            }
        }
        Ok(None)
    }

    /// Resolve an expression that names a type (`Foo`, `Foo<int>`, `mod.Foo`).
    pub fn type_path(&mut self, e: &ast::Expr) -> Result<Option<TyId>> {
        let (module, name, args, span) = match &e.kind {
            E::Ident(n) => {
                if self.resolve_var(n, e.span)?.is_some() {
                    return Ok(None);
                }
                (None, n.clone(), vec![], e.span)
            }
            E::Field(base, n) => match self.module_alias(base)? {
                Some(alias) => (Some(alias), n.clone(), vec![], e.span),
                None => return Ok(None),
            },
            E::Generic(inner, args) => match &inner.kind {
                E::Ident(n) => (None, n.clone(), args.clone(), e.span),
                E::Field(base, n) => match self.module_alias(base)? {
                    Some(alias) => (Some(alias), n.clone(), args.clone(), e.span),
                    None => return Ok(None),
                },
                _ => return Ok(None),
            },
            _ => return Ok(None),
        };
        let m = self.module();
        if module.is_none()
            && args.is_empty()
            && let Some(&t) = self.ctx_ref().tparams.get(&name)
        {
            // a generic constant is a value
            return Ok((!matches!(self.types.kind(t), TyKind::Const(..))).then_some(t));
        }
        let sym = match &module {
            Some(alias) if self.mods[m].externs.contains_key(alias) => return Ok(None),
            _ => match self.lookup_sym(m, module.as_deref(), &name, span) {
                Ok(s) => s,
                Err(d) => return if module.is_some() { Err(d) } else { Ok(None) },
            },
        };
        match sym {
            Some((_, Sym::Struct { .. }))
            | Some((_, Sym::Enum { .. }))
            | Some((_, Sym::Type(_))) => {
                let te = ast::TypeExpr::Named {
                    module,
                    name,
                    args,
                    span,
                };
                let tp = self.tparams();
                Ok(Some(self.resolve_type(&te, m, &tp)?))
            }
            _ => Ok(None),
        }
    }

    /// `name<T>` referring to a generic function: instantiate it.
    fn generic_fn_ref(&mut self, e: &ast::Expr) -> Result<Option<FuncId>> {
        let E::Generic(inner, targs) = &e.kind else {
            return Ok(None);
        };
        let (module, name) = match &inner.kind {
            E::Ident(n) => (None, n.clone()),
            E::Field(base, n) => match self.module_alias(base)? {
                Some(a) => (Some(a), n.clone()),
                None => return Ok(None),
            },
            _ => return Ok(None),
        };
        let m = self.module();
        match self.lookup_sym(m, module.as_deref(), &name, e.span)? {
            Some((dm, Sym::Fn { decl, func: None })) => {
                let mut args = Vec::new();
                let tp = self.tparams();
                for t in targs {
                    args.push(self.resolve_targ(t, m, &tp)?);
                }
                Ok(Some(self.instantiate_fn(dm, decl, args, e.span)?))
            }
            Some((_, Sym::Fn { func: Some(_), .. })) => {
                err(e.span, format!("`{name}` is not generic"))
            }
            _ => Ok(None),
        }
    }

    fn resolve_target(&mut self, callee: &ast::Expr) -> Result<Target> {
        let span = callee.span;
        // enum variant constructors and module members
        if let E::Field(base, name) = &callee.kind {
            if let Some(alias) = self.module_alias(base)? {
                let m = self.module();
                if let Some(tab) = self.mods[m].externs.get(&alias) {
                    let Some(&id) = tab.get(name) else {
                        return err(
                            span,
                            format!("extern block `{alias}` has no function `{name}`"),
                        );
                    };
                    let x = &self.externs[id];
                    let (ps, r) = (x.params.clone(), x.ret);
                    match self.ix.extern_decls.get(&id).cloned() {
                        Some((def, detail)) => {
                            self.idx_ref(span, Some(def), format!("extern {detail}"))
                        }
                        None => {
                            let hover = format!("extern fn {}", x.name);
                            self.idx_ref(span, None, hover)
                        }
                    }
                    return Ok(Target::Call(Callee::Extern(id), ps, r));
                }
                match self.lookup_sym(m, Some(&alias), name, span)? {
                    Some((_, Sym::Fn { func: Some(f), .. })) => {
                        self.idx_fn_ref(f, name, span);
                        let (ps, r) = self.sigs[f].clone();
                        return Ok(Target::Call(Callee::Direct(f), ps, r));
                    }
                    Some((_, Sym::Fn { func: None, .. })) => {
                        return err(
                            span,
                            format!(
                                "generic function `{alias}.{name}` requires explicit type arguments"
                            ),
                        );
                    }
                    _ => {}
                }
            } else if let Some(ty) = self.type_path(base)? {
                let Some(def) = self.types.enum_def(ty).cloned() else {
                    return err(span, format!("`{}` is not an enum", self.types.show(ty)));
                };
                let Some(idx) = def.variants.iter().position(|(n, _)| n == name) else {
                    return err(span, format!("enum `{}` has no member `{name}`", def.name));
                };
                self.idx_variant_ref(ty, idx, span);
                return Ok(Target::Variant(ty, idx, def.variants[idx].1.clone()));
            }
        }
        if let Some(f) = self.generic_fn_ref(callee)? {
            let fname = self
                .funcs
                .get(f)
                .and_then(|x| x.as_ref())
                .map(|x| x.name.clone())
                .unwrap_or_else(|| "fn".into());
            self.idx_fn_ref(f, &fname, callee.span);
            let (ps, r) = self.sigs[f].clone();
            return Ok(Target::Call(Callee::Direct(f), ps, r));
        }
        if let E::Ident(name) = &callee.kind
            && self.resolve_var(name, span)?.is_none()
        {
            let m = self.module();
            if let Some(Sym::Fn { func: Some(f), .. }) =
                self.mods[m].syms.get(name).map(|e| e.sym.clone())
            {
                self.idx_fn_ref(f, name, span);
                let (ps, r) = self.sigs[f].clone();
                return Ok(Target::Call(Callee::Direct(f), ps, r));
            }
        }
        let v = self.check_expr(callee, None)?;
        match self.types.kind(v.ty).clone() {
            TyKind::Fn(ps, r) => Ok(Target::Call(Callee::Value(Box::new(v)), ps, r)),
            _ => err(
                span,
                format!("`{}` is not a function", self.types.show(v.ty)),
            ),
        }
    }

    fn check_call_parts(
        &mut self,
        callee: &ast::Expr,
        args: &[ast::Expr],
    ) -> Result<(Target, Vec<Expr>)> {
        let target = self.resolve_target(callee)?;
        let params = match &target {
            Target::Call(_, ps, _) => ps.clone(),
            Target::Variant(_, _, ps) => ps.clone(),
        };
        if params.len() != args.len() {
            return err(
                callee.span,
                format!(
                    "expected {} argument(s), found {}",
                    params.len(),
                    args.len()
                ),
            );
        }
        if let Target::Call(Callee::Direct(f), ..) = &target {
            self.idx_call(*f, callee, args);
        }
        let mut xs = Vec::new();
        for (a, p) in args.iter().zip(&params) {
            let x = self.expect_expr(a, *p)?;
            if matches!(self.types.kind(x.ty), TyKind::Fut(_)) {
                return err(a.span, "futures cannot be passed to functions");
            }
            xs.push(x);
        }
        if let Target::Call(Callee::Direct(f), _, r) = &target
            && self.never_funcs.contains(f)
        {
            return Ok((Target::Call(Callee::Direct(*f), params, NEVER), xs)).map(|(t, x)| {
                let _ = r;
                (t, x)
            });
        }
        Ok((target, xs))
    }

    fn check_field(&mut self, base: &ast::Expr, name: &str, span: Span) -> Result<Expr> {
        if let Some(alias) = self.module_alias(base)? {
            let m = self.module();
            if self.mods[m].externs.contains_key(&alias) {
                return err(
                    span,
                    format!("extern functions must be called: `{alias}.{name}(...)`"),
                );
            }
            return match self.lookup_sym(m, Some(&alias), name, span)? {
                Some((tm, Sym::Globals(list))) => {
                    let g = list.last().unwrap().0;
                    self.idx_global_ref(g, span);
                    let ty = self.globals[g].ty;
                    let _ = tm;
                    if matches!(self.types.kind(ty), TyKind::Mutex(_)) {
                        return err(
                            span,
                            "mutexes from other modules cannot be accessed directly",
                        );
                    }
                    Ok(Expr {
                        kind: EK::Var(VarRef::Global(g)),
                        ty,
                        span,
                    })
                }
                Some((_, Sym::Fn { func: Some(f), .. })) => {
                    self.idx_fn_ref(f, name, span);
                    let (ps, r) = self.sigs[f].clone();
                    let ty = self.types.intern(TyKind::Fn(ps, r));
                    Ok(Expr {
                        kind: EK::FnRef(f),
                        ty,
                        span,
                    })
                }
                Some((_, Sym::Fn { func: None, .. })) => err(
                    span,
                    format!("generic function `{alias}.{name}` requires explicit type arguments"),
                ),
                _ => err(span, format!("`{alias}.{name}` is a type, not a value")),
            };
        }
        if let Some(ty) = self.type_path(base)? {
            let Some(def) = self.types.enum_def(ty).cloned() else {
                return err(
                    span,
                    format!("`{}` is a type, not a value", self.types.show(ty)),
                );
            };
            let Some(idx) = def.variants.iter().position(|(n, _)| n == name) else {
                return err(span, format!("enum `{}` has no member `{name}`", def.name));
            };
            if !def.variants[idx].1.is_empty() {
                return err(
                    span,
                    format!(
                        "`{}.{name}` holds a value; construct it with `{}.{name}(...)`",
                        def.name, def.name
                    ),
                );
            }
            self.idx_variant_ref(ty, idx, span);
            return Ok(Expr {
                kind: EK::Variant(idx, vec![]),
                ty,
                span,
            });
        }
        let b = self.check_expr(base, None)?;
        match self.types.kind(b.ty).clone() {
            TyKind::Struct(i) => {
                let fields = &self.types.structs[i as usize].fields;
                match fields.iter().position(|(n, _)| n == name) {
                    Some(idx) => {
                        let ty = fields[idx].1;
                        let bty = b.ty;
                        self.idx_field_ref(bty, idx, span);
                        Ok(Expr {
                            kind: EK::Field(Box::new(b), idx),
                            ty,
                            span,
                        })
                    }
                    None => err(
                        span,
                        format!("`{}` has no field `{name}`", self.types.show(b.ty)),
                    ),
                }
            }
            TyKind::Array(_) | TyKind::Fixed(..) | TyKind::Str | TyKind::Map(..)
                if name == "len" =>
            {
                Ok(Expr {
                    kind: EK::Len(Box::new(b)),
                    ty: UINT,
                    span,
                })
            }
            TyKind::Named(_) => err(
                span,
                format!(
                    "`{}` is a named type; convert it with `@as` to access `.{name}`",
                    self.types.show(b.ty)
                ),
            ),
            _ => err(
                span,
                format!("`{}` has no member `{name}`", self.types.show(b.ty)),
            ),
        }
    }

    fn check_pair(
        &mut self,
        l: &ast::Expr,
        r: &ast::Expr,
        exp: Option<TyId>,
    ) -> Result<(Expr, Expr)> {
        if Self::is_untyped_int(l) && !Self::is_untyped_int(r) {
            let b = self.check_expr(r, exp)?;
            let a = self.check_expr(l, Some(b.ty))?;
            let a = self.coerce_pair(a, b.ty)?;
            Ok((a, b))
        } else {
            let a = self.check_expr(l, exp)?;
            let b = self.check_expr(r, Some(a.ty))?;
            if b.ty != a.ty && self.can_coerce(a.ty, b.ty) {
                let a = self.coerce(a, b.ty)?;
                return Ok((a, b));
            }
            let b = self.coerce_pair(b, a.ty)?;
            Ok((a, b))
        }
    }

    fn coerce_pair(&mut self, e: Expr, ty: TyId) -> Result<Expr> {
        if e.ty == ty || self.can_coerce(e.ty, ty) {
            return self.coerce(e, ty);
        }
        Ok(e)
    }

    fn check_binary(
        &mut self,
        op: BinOp,
        l: &ast::Expr,
        r: &ast::Expr,
        expected: Option<TyId>,
        span: Span,
    ) -> Result<Expr> {
        use BinOp::*;
        let mk = |kind, ty| Ok(Expr { kind, ty, span });
        match op {
            And | Or => {
                let a = self.expect_expr(l, BOOL)?;
                let b = self.expect_expr(r, BOOL)?;
                mk(EK::Binary(op, Box::new(a), Box::new(b)), BOOL)
            }
            Concat => {
                let a = self.check_expr(l, expected.map(|t| self.strip_opt(t)))?;
                let rexp = match self.types.kind(a.ty).clone() {
                    TyKind::Fixed(e, _) => Some(self.types.array(e)),
                    _ => Some(a.ty),
                };
                let b = self.check_expr(r, rexp)?;
                let ka = self.types.kind(a.ty).clone();
                let kb = self.types.kind(b.ty).clone();
                let want_dyn = expected
                    .map(|t| self.strip_opt(t))
                    .map(|t| matches!(self.types.kind(t), TyKind::Array(_)))
                    .unwrap_or(false);
                let ty = match (&ka, &kb) {
                    (TyKind::Str, TyKind::Str) => STR,
                    (TyKind::Array(x), TyKind::Array(y)) if x == y => a.ty,
                    (TyKind::Fixed(x, n), TyKind::Fixed(y, m)) if x == y => {
                        if want_dyn {
                            self.types.array(*x)
                        } else {
                            self.types.intern(TyKind::Fixed(*x, n + m))
                        }
                    }
                    (TyKind::Fixed(x, _), TyKind::Array(y))
                    | (TyKind::Array(x), TyKind::Fixed(y, _))
                        if x == y =>
                    {
                        self.types.array(*x)
                    }
                    (TyKind::Map(..), TyKind::Map(..)) if a.ty == b.ty => a.ty,
                    (TyKind::Map(..), TyKind::Map(..)) => {
                        return err(span, "cannot concatenate maps of different types");
                    }
                    _ => {
                        return err(
                            span,
                            format!(
                                "cannot concatenate `{}` and `{}`",
                                self.types.show(a.ty),
                                self.types.show(b.ty)
                            ),
                        );
                    }
                };
                mk(EK::Binary(op, Box::new(a), Box::new(b)), ty)
            }
            In => {
                let b = self.check_expr(r, None)?;
                let a = match self.types.kind(b.ty).clone() {
                    TyKind::Array(e) | TyKind::Fixed(e, _) => self.expect_expr(l, e)?,
                    TyKind::Map(k, _) => self.expect_expr(l, k)?,
                    TyKind::Str => {
                        let a = self.check_expr(l, None)?;
                        if a.ty != CHAR && a.ty != STR {
                            return err(
                                l.span,
                                format!(
                                    "expected `char` or `str`, found `{}`",
                                    self.types.show(a.ty)
                                ),
                            );
                        }
                        a
                    }
                    _ => {
                        return err(
                            r.span,
                            format!(
                                "`in` expects an array, map or string, found `{}`",
                                self.types.show(b.ty)
                            ),
                        );
                    }
                };
                if !self.comparable(a.ty) {
                    return err(
                        l.span,
                        format!("`{}` values cannot be compared", self.types.show(a.ty)),
                    );
                }
                mk(EK::Binary(op, Box::new(a), Box::new(b)), BOOL)
            }
            Eq | Ne | Lt | Gt | Le | Ge => {
                let (a, b) = self.check_pair(l, r, None)?;
                if a.ty != b.ty {
                    return err(
                        span,
                        format!(
                            "cannot compare `{}` with `{}`",
                            self.types.show(a.ty),
                            self.types.show(b.ty)
                        ),
                    );
                }
                let ok = if matches!(op, Eq | Ne) {
                    self.comparable(a.ty)
                } else {
                    self.types.is_numeric(a.ty) || a.ty == CHAR
                };
                if !ok {
                    return err(
                        span,
                        format!(
                            "operator `{}` cannot be applied to `{}`",
                            op.symbol(),
                            self.types.show(a.ty)
                        ),
                    );
                }
                mk(EK::Binary(op, Box::new(a), Box::new(b)), BOOL)
            }
            _ => {
                let (a, b) = self.check_pair(l, r, expected.map(|t| self.strip_opt(t)))?;
                if a.ty != b.ty {
                    return err(
                        span,
                        format!(
                            "operator `{}` requires both operands to have the same type, found `{}` and `{}`",
                            op.symbol(),
                            self.types.show(a.ty),
                            self.types.show(b.ty)
                        ),
                    );
                }
                let t = a.ty;
                let ok = match op {
                    Add | Sub | Mul | Div | Pow => self.types.is_numeric(t),
                    Mod => {
                        if t == FLOAT {
                            return err(span, "`%` is not supported for `float`");
                        }
                        self.types.is_integer(t)
                    }
                    Shl | Shr | BitAnd | BitOr | BitXor => self.types.is_integer(t),
                    _ => unreachable!(),
                };
                if !ok {
                    return err(
                        span,
                        format!(
                            "operator `{}` cannot be applied to `{}`",
                            op.symbol(),
                            self.types.show(t)
                        ),
                    );
                }
                mk(EK::Binary(op, Box::new(a), Box::new(b)), t)
            }
        }
    }

    fn check_builtin(
        &mut self,
        name: &str,
        ty: Option<&ast::TypeExpr>,
        args: &[ast::Expr],
        span: Span,
    ) -> Result<Expr> {
        match name {
            "print" | "println" | "eprint" | "eprintln" => {
                let mut xs = Vec::new();
                for a in args {
                    let x = self.check_expr(a, None)?;
                    if !self.printable(x.ty) {
                        return err(
                            a.span,
                            format!("`{}` cannot be printed", self.types.show(x.ty)),
                        );
                    }
                    xs.push(x);
                }
                let to = if name.starts_with('e') {
                    PrintTo::Stderr
                } else {
                    PrintTo::Stdout
                };
                Ok(Expr {
                    kind: EK::Print(to, name.ends_with("ln"), xs),
                    ty: VOID,
                    span,
                })
            }
            "as" => {
                let Some(te) = ty else {
                    return err(span, "`@as` expects a type and a value");
                };
                if args.len() != 1 {
                    return err(span, "`@as` expects a type and a value: `@as(T, value)`");
                }
                let t = self.resolve_type_here(te)?;
                let x = self.check_expr(&args[0], Some(t))?;
                self.check_cast(x, t, span)
            }
            "args" | "env" | "target" => {
                if ty.is_some() || !args.is_empty() {
                    return err(span, format!("`@{name}` takes no arguments"));
                }
                let (kind, ty) = match name {
                    "args" => (EK::Args, self.types.array(STR)),
                    "env" => (EK::Env, self.types.intern(TyKind::Map(STR, STR))),
                    _ => (EK::Target, self.types.tuple(vec![STR, STR])),
                };
                Ok(Expr { kind, ty, span })
            }
            "embed" => {
                let [arg] = args else {
                    return err(span, "`@embed` expects one path: `@embed(\"./file\")`");
                };
                let path = match &arg.kind {
                    ast::ExprKind::Str(segs) => match segs.as_slice() {
                        [ast::StrSeg::Lit(s)] => s.clone(),
                        _ => return err(arg.span, "`@embed` paths must be plain string literals"),
                    },
                    _ => return err(arg.span, "`@embed` paths must be plain string literals"),
                };
                let file = self.resolve_embed_path(&path, arg.span)?;
                let bytes = std::fs::read(&file).map_err(|e| {
                    Diag::new(arg.span, format!("cannot read `{}`: {e}", file.display()))
                })?;
                let ty = self.types.array(BYTE);
                Ok(Expr {
                    kind: EK::Embed(std::rc::Rc::new(bytes), file),
                    ty,
                    span,
                })
            }
            _ => err(span, format!("unknown builtin function `@{name}`")),
        }
    }

    /// Resolve an `@embed` path like a relative import: `./` and `../` from the current file,
    /// `.../` from the project root.
    fn resolve_embed_path(&self, path: &str, span: Span) -> Result<std::path::PathBuf> {
        let m = self.module();
        let dir = self.mods[m].path.parent().unwrap().to_path_buf();
        let file = if let Some(rel) = path.strip_prefix(".../") {
            match crate::modules::find_root(&dir) {
                Some(root) => root.join(rel),
                None => {
                    return err(
                        span,
                        format!(
                            "cannot resolve `{path}`: no `.nc/config.toml` found in `{}` or any parent directory",
                            dir.display()
                        ),
                    );
                }
            }
        } else if path.starts_with("./") || path.starts_with("../") {
            dir.join(path.strip_prefix("./").unwrap_or(path))
        } else {
            return err(span, "`@embed` paths must start with `./`, `../` or `.../`");
        };
        if !file.is_file() {
            return err(
                span,
                format!("cannot find `{path}` (looked for `{}`)", file.display()),
            );
        }
        Ok(file)
    }

    fn check_cast(&mut self, x: Expr, t: TyId, span: Span) -> Result<Expr> {
        let retype = |x: Expr| {
            Ok(Expr {
                kind: EK::Retype(Box::new(x)),
                ty: t,
                span,
            })
        };
        if x.ty == t {
            return Ok(x);
        }
        // nominal types and their underlying type
        let named_ok = match (self.types.kind(t), self.types.kind(x.ty)) {
            (TyKind::Named(n), _) if self.types.named[*n as usize].underlying == x.ty => true,
            (_, TyKind::Named(n)) if self.types.named[*n as usize].underlying == t => true,
            _ => false,
        };
        if named_ok {
            return retype(x);
        }
        let from = self.types.kind(x.ty).clone();
        let to = self.types.kind(t).clone();
        let bytes = self.types.array(BYTE);
        let chars = self.types.array(CHAR);
        let ok = match (&from, &to) {
            (TyKind::Fixed(e, _), TyKind::Array(f)) if e == f => return retype(x),
            (TyKind::Array(_), TyKind::Fixed(..)) => {
                return err(span, "cannot convert a dynamic array to a fixed-size array");
            }
            (TyKind::Fixed(..), TyKind::Str) => true,
            (TyKind::Bool, TyKind::Int | TyKind::UInt | TyKind::Str) => true,
            (TyKind::Byte, TyKind::Char | TyKind::Int | TyKind::UInt | TyKind::Str) => true,
            (TyKind::Char, TyKind::Str) => true,
            (TyKind::Char, _) if t == bytes => true,
            (
                TyKind::Enum(_) | TyKind::Map(..) | TyKind::Struct(_) | TyKind::Tuple(_),
                TyKind::Str,
            ) => true,
            (TyKind::Int, TyKind::UInt | TyKind::Float | TyKind::Str) => true,
            (TyKind::UInt, TyKind::Int | TyKind::Float | TyKind::Str) => true,
            (TyKind::Float, TyKind::Int | TyKind::UInt | TyKind::Str) => true,
            (TyKind::Int | TyKind::UInt | TyKind::Float, _) if t == bytes => true,
            (TyKind::Str, _) if t == bytes || t == chars => true,
            _ => false,
        };
        if !ok {
            return err(
                span,
                format!(
                    "cannot convert `{}` to `{}`",
                    self.types.show(x.ty),
                    self.types.show(t)
                ),
            );
        }
        if to == TyKind::Str && !self.printable(x.ty) {
            return err(
                span,
                format!(
                    "`{}` cannot be converted to a string",
                    self.types.show(x.ty)
                ),
            );
        }
        Ok(Expr {
            kind: EK::Cast(Box::new(x)),
            ty: t,
            span,
        })
    }

    fn check_if(&mut self, ife: &ast::IfExpr, value_ty: Option<TyId>, span: Span) -> Result<Expr> {
        let id = self.new_jump();
        let scrut = match &ife.scrutinee {
            Some(s) => {
                let x = self.check_expr(s, None)?;
                match self.types.kind(x.ty) {
                    TyKind::Map(..) => {
                        return err(
                            s.span,
                            "maps cannot be matched directly; use a bare `if { ... }` with conditions",
                        );
                    }
                    TyKind::Fn(..) | TyKind::Fut(_) | TyKind::Void => {
                        return err(
                            s.span,
                            format!("`{}` values cannot be matched", self.types.show(x.ty)),
                        );
                    }
                    _ => {}
                }
                Some(x)
            }
            None => None,
        };
        let kind = match value_ty {
            Some(t) => JumpKind::Value(t),
            None => JumpKind::If,
        };
        self.ctx().jumps.push(JumpCtx {
            id,
            kind,
            label: ife.label.clone(),
        });
        let r = (|| {
            let mut arms = Vec::new();
            for arm in &ife.arms {
                self.push_scope();
                self.ix.scopes.push(arm.span);
                let res = (|| {
                    let mut pats = Vec::new();
                    for p in &arm.patterns {
                        let pat = match &scrut {
                            Some(s) => self.check_pattern(p, s.ty, arm.patterns.len() > 1)?,
                            None => {
                                if matches!(p.kind, E::Wildcard) {
                                    Pat::Wild
                                } else {
                                    Pat::Cond(self.expect_expr(p, BOOL)?)
                                }
                            }
                        };
                        pats.push(pat);
                    }
                    let body = self.check_block(&arm.body)?;
                    if value_ty.is_some() && self.block_completes(&body) {
                        return err(
                            arm.body.span,
                            "this branch must produce a value with `break <value>`",
                        );
                    }
                    Ok(Arm {
                        patterns: pats,
                        body,
                    })
                })();
                self.ix.scopes.pop();
                self.pop_scope();
                arms.push(res?);
            }
            Ok(arms)
        })();
        self.ctx().jumps.pop();
        let arms = r?;
        let exhaustive = match &scrut {
            Some(s) => self.exhaustive(&arms, s.ty),
            None => arms.iter().any(|a| {
                a.patterns.iter().any(|p| match p {
                    Pat::Wild => true,
                    Pat::Cond(e) => matches!(e.kind, EK::Lit(Lit::Bool(true))),
                    _ => false,
                })
            }),
        };
        if !exhaustive {
            return err(
                span,
                "`if` branches are not exhaustive; add a fallback `_ -> { ... }` branch",
            );
        }
        Ok(Expr {
            kind: EK::If(Box::new(If {
                id,
                is_value: value_ty.is_some(),
                scrutinee: scrut,
                arms,
            })),
            ty: value_ty.unwrap_or(VOID),
            span,
        })
    }
}
