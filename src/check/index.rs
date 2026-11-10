//! Symbol index recorded during checking, used by the language server for hover,
//! go-to-definition and completion. It is filled even when checking fails part-way.

use std::collections::HashMap;

use super::Checker;
use crate::diag::Span;
use crate::tir::{FuncId, GlobalId, LocalId};
use crate::types::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeclKind {
    Variable,
    Function,
    Struct,
    Enum,
    Type,
    Field,
    Variant,
}

#[derive(Clone, Debug)]
pub struct SymDecl {
    pub name: String,
    pub span: Span,
    pub kind: DeclKind,
    /// hover text
    pub detail: String,
    /// members available after `.` on values of this declaration: (name, detail)
    pub members: Vec<(String, String)>,
    pub mutable: bool,
    pub param: bool,
    /// the block a local variable is visible in (None for module-level declarations)
    pub scope: Option<Span>,
    /// the variable's type, a key into `Index::types`
    pub ty: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SymRef {
    pub span: Span,
    pub def: Option<Span>,
    pub hover: String,
    /// declaration of the referenced value's type (for go-to-type-definition)
    pub type_def: Option<Span>,
}

/// The type of a checked expression.
#[derive(Clone, Debug)]
pub struct ExprTy {
    pub span: Span,
    pub ty: String,
    pub type_def: Option<Span>,
}

/// A member reachable with `.` on values of a type.
#[derive(Clone, Debug)]
pub struct Member {
    pub name: String,
    pub detail: String,
    pub ty: String,
    pub def: Option<Span>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HintKind {
    Type,
    Param,
}

/// An inlay hint: `label` is shown before the offset `at.start`.
#[derive(Clone, Debug)]
pub struct Hint {
    pub at: Span,
    pub label: String,
    pub kind: HintKind,
}

/// A direct call from one function to another.
#[derive(Clone, Debug)]
pub struct Call {
    /// the calling function's signature span (None at the top level of a file or in a test)
    pub caller: Option<Span>,
    pub callee: Span,
    /// the callee's name at the call site
    pub at: Span,
}

#[derive(Clone, Debug, Default)]
pub struct Index {
    pub decls: Vec<SymDecl>,
    pub refs: Vec<SymRef>,
    pub exprs: Vec<ExprTy>,
    /// members by type name
    pub types: HashMap<String, Vec<Member>>,
    /// element (or map value) type by type name, for indexing with `[...]`
    pub elems: HashMap<String, String>,
    /// return type by function type name
    pub returns: HashMap<String, String>,
    pub hints: Vec<Hint>,
    pub calls: Vec<Call>,
}

#[derive(Default)]
pub struct IndexState {
    pub index: Index,
    pub local_spans: HashMap<(FuncId, LocalId), Span>,
    pub global_spans: HashMap<GlobalId, Span>,
    pub fn_spans: HashMap<FuncId, Span>,
    pub type_spans: HashMap<TyId, Span>,
    pub field_spans: HashMap<TyId, Vec<Span>>,
    pub variant_spans: HashMap<TyId, Vec<Span>>,
    pub fn_params: HashMap<FuncId, Vec<String>>,
    /// innermost scopes being checked, for the visibility of local variables
    pub scopes: Vec<Span>,
}

impl<'a> Checker<'a> {
    pub fn members_of(&self, t: TyId) -> Vec<(String, String)> {
        match self.types.kind(t) {
            TyKind::Struct(i) => self.types.structs[*i as usize]
                .fields
                .iter()
                .map(|(n, ft)| (n.clone(), format!("{} {n}", self.types.show(*ft))))
                .collect(),
            TyKind::Array(_) | TyKind::Fixed(..) | TyKind::Str | TyKind::Map(..) => {
                vec![("len".into(), "uint len".into())]
            }
            _ => vec![],
        }
    }

    pub fn fn_detail(&self, name: &str, f: FuncId) -> String {
        let (ps, r) = &self.sigs[f];
        let ps: Vec<String> = ps.iter().map(|p| self.types.show(*p)).collect();
        if *r == VOID {
            format!("fn {name}({})", ps.join(", "))
        } else {
            format!("fn {name}({}) {}", ps.join(", "), self.types.show(*r))
        }
    }

    /// Like `fn_detail`, with parameter names (`fn add(int a, int b) int`).
    pub fn fn_detail_named(&self, name: &str, f: FuncId, params: &[String]) -> String {
        let (ps, r) = &self.sigs[f];
        let ps: Vec<String> = ps
            .iter()
            .zip(params)
            .map(|(p, n)| format!("{} {n}", self.types.show(*p)))
            .collect();
        if *r == VOID {
            format!("fn {name}({})", ps.join(", "))
        } else {
            format!("fn {name}({}) {}", ps.join(", "), self.types.show(*r))
        }
    }

    pub fn type_detail(&self, t: TyId) -> String {
        match self.types.kind(t) {
            TyKind::Struct(i) => {
                let d = &self.types.structs[*i as usize];
                let fields: Vec<String> = d
                    .fields
                    .iter()
                    .map(|(n, ft)| format!("  {} {n}", self.types.show(*ft)))
                    .collect();
                format!("struct {} {{\n{}\n}}", d.name, fields.join("\n"))
            }
            TyKind::Enum(i) => {
                let d = &self.types.enums[*i as usize];
                let vs: Vec<String> = d
                    .variants
                    .iter()
                    .map(|(n, ps)| format!("  {}", self.variant_text(n, ps)))
                    .collect();
                format!("enum {} {{\n{}\n}}", d.name, vs.join("\n"))
            }
            TyKind::Named(i) => {
                let d = &self.types.named[*i as usize];
                format!("type {} = {}", d.name, self.types.show(d.underlying))
            }
            _ => self.types.show(t),
        }
    }

    pub fn variant_text(&self, n: &str, ps: &[TyId]) -> String {
        if ps.is_empty() {
            n.to_string()
        } else {
            format!(
                "{n}({})",
                ps.iter()
                    .map(|p| self.types.show(*p))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }

    pub fn idx_decl(
        &mut self,
        name: &str,
        span: Span,
        kind: DeclKind,
        detail: String,
        members: Vec<(String, String)>,
    ) {
        if name == "_" || name.is_empty() {
            return;
        }
        self.ix.index.decls.push(SymDecl {
            name: name.to_string(),
            span,
            kind,
            detail: detail.clone(),
            members,
            mutable: false,
            param: false,
            scope: None,
            ty: None,
        });
        self.ix.index.refs.push(SymRef {
            span,
            def: Some(span),
            hover: detail,
            type_def: None,
        });
    }

    /// Declaration of a variable: like `idx_decl`, with its type, mutability and scope.
    pub fn idx_var_decl(
        &mut self,
        name: &str,
        span: Span,
        ty: TyId,
        mutable: bool,
        param: bool,
        scope: Option<Span>,
    ) {
        let detail = self.var_detail(name, ty, mutable);
        let members = self.members_of(ty);
        let before = self.ix.index.decls.len();
        self.idx_decl(name, span, DeclKind::Variable, detail, members);
        if self.ix.index.decls.len() == before {
            return;
        }
        let key = self.idx_type(ty);
        let type_def = self.type_def_of(ty);
        let d = self.ix.index.decls.last_mut().unwrap();
        d.mutable = mutable;
        d.param = param;
        d.scope = scope;
        d.ty = Some(key);
        self.ix.index.refs.last_mut().unwrap().type_def = type_def;
    }

    /// Set the type of the declaration just recorded (a function's return type, a type's own).
    pub fn idx_last_ty(&mut self, name: &str, t: TyId) {
        if name == "_" || name.is_empty() {
            return;
        }
        let key = self.idx_type(t);
        if let Some(d) = self.ix.index.decls.last_mut() {
            d.ty = Some(key);
        }
    }

    pub fn idx_ref(&mut self, span: Span, def: Option<Span>, hover: String) {
        self.ix.index.refs.push(SymRef {
            span,
            def,
            hover,
            type_def: None,
        });
    }

    fn idx_typed_ref(&mut self, span: Span, def: Option<Span>, hover: String, ty: TyId) {
        let type_def = self.type_def_of(ty);
        self.ix.index.refs.push(SymRef {
            span,
            def,
            hover,
            type_def,
        });
    }

    /// The declaration of the struct, enum or named type inside `t` (through arrays, optionals,
    /// error unions, futures and mutexes).
    pub fn type_def_of(&self, mut t: TyId) -> Option<Span> {
        loop {
            match self.types.kind(t) {
                TyKind::Array(x)
                | TyKind::Fixed(x, _)
                | TyKind::Opt(x)
                | TyKind::ErrU(x)
                | TyKind::Fut(x)
                | TyKind::Mutex(x) => t = *x,
                TyKind::Map(_, v) => t = *v,
                TyKind::Struct(_) | TyKind::Enum(_) | TyKind::Named(_) => {
                    return self.ix.type_spans.get(&t).copied();
                }
                _ => return None,
            }
        }
    }

    /// Record the members of a type (and of their types) and return its key.
    pub fn idx_type(&mut self, t: TyId) -> String {
        let key = self.types.show(t);
        if self.ix.index.types.contains_key(&key) {
            return key;
        }
        self.ix.index.types.insert(key.clone(), vec![]);
        let base = self.types.base(t);
        let elem = match self.types.kind(base).clone() {
            TyKind::Array(e) | TyKind::Fixed(e, _) | TyKind::Map(_, e) => Some(e),
            TyKind::Str => Some(CHAR),
            TyKind::Opt(e) | TyKind::Mutex(e) | TyKind::Fut(e) => {
                // members of the inner type are reachable after unwrapping
                let inner = self.idx_type(e);
                let ms = self.ix.index.types.get(&inner).cloned().unwrap_or_default();
                self.ix.index.types.insert(key.clone(), ms);
                None
            }
            _ => None,
        };
        if let Some(e) = elem {
            let ek = self.idx_type(e);
            self.ix.index.elems.insert(key.clone(), ek);
        }
        if let TyKind::Fn(_, r) = self.types.kind(base).clone() {
            let rk = self.idx_type(r);
            self.ix.index.returns.insert(key.clone(), rk);
        }
        if !self.ix.index.types[&key].is_empty() {
            return key;
        }
        let mut members = Vec::new();
        match self.types.kind(base).clone() {
            TyKind::Struct(i) => {
                let fields = self.types.structs[i as usize].fields.clone();
                for (k, (n, ft)) in fields.iter().enumerate() {
                    let fty = self.idx_type(*ft);
                    members.push(Member {
                        name: n.clone(),
                        detail: format!("{fty} {n}"),
                        ty: fty,
                        def: self
                            .ix
                            .field_spans
                            .get(&base)
                            .and_then(|v| v.get(k))
                            .copied(),
                    });
                }
            }
            TyKind::Array(_) | TyKind::Fixed(..) | TyKind::Str | TyKind::Map(..) => {
                members.push(Member {
                    name: "len".into(),
                    detail: "uint len".into(),
                    ty: "uint".into(),
                    def: None,
                });
            }
            _ => {}
        }
        self.ix.index.types.insert(key.clone(), members);
        key
    }

    /// Record the type of a checked expression.
    pub fn idx_expr(&mut self, span: Span, t: TyId) {
        if span.end <= span.start {
            return;
        }
        let ty = self.idx_type(t);
        let type_def = self.type_def_of(t);
        self.ix.index.exprs.push(ExprTy { span, ty, type_def });
    }

    pub fn idx_hint(&mut self, at: u32, file: u32, label: String, kind: HintKind) {
        self.ix.index.hints.push(Hint {
            at: Span {
                file,
                start: at,
                end: at,
            },
            label,
            kind,
        });
    }

    pub fn var_detail(&self, name: &str, ty: TyId, mutable: bool) -> String {
        format!(
            "{}{} {name}",
            if mutable { "mut " } else { "" },
            self.types.show(ty)
        )
    }

    /// Record a reference to a local variable of the current function.
    pub fn idx_local_ref(&mut self, l: LocalId, span: Span) {
        let Some(ctx) = self.fns.last() else { return };
        let loc = &ctx.locals[l];
        let def = self.ix.local_spans.get(&(ctx.func, l)).copied();
        let hover = self.var_detail(&loc.name, loc.ty, loc.mutable);
        let ty = loc.ty;
        self.idx_typed_ref(span, def, hover, ty);
    }

    pub fn idx_var_ref(&mut self, v: crate::tir::VarRef, span: Span) {
        match v {
            crate::tir::VarRef::Local(l) => self.idx_local_ref(l, span),
            crate::tir::VarRef::Global(g) => self.idx_global_ref(g, span),
        }
    }

    pub fn idx_global_ref(&mut self, g: GlobalId, span: Span) {
        let gl = &self.globals[g];
        let hover = self.var_detail(&gl.name, gl.ty, gl.mutable);
        let ty = gl.ty;
        let def = self.ix.global_spans.get(&g).copied();
        self.idx_typed_ref(span, def, hover, ty);
    }

    pub fn idx_fn_ref(&mut self, f: FuncId, name: &str, span: Span) {
        let hover = self.fn_detail(name, f);
        let def = self.ix.fn_spans.get(&f).copied();
        self.idx_ref(span, def, hover);
    }

    pub fn idx_type_ref(&mut self, t: TyId, span: Span) {
        let hover = self.type_detail(t);
        let def = self.ix.type_spans.get(&t).copied();
        self.idx_typed_ref(span, def, hover, t);
    }

    pub fn idx_field_ref(&mut self, t: TyId, i: usize, span: Span) {
        let Some(def) = self.types.struct_def(t) else {
            return;
        };
        let (n, ft) = &def.fields[i];
        let ft = *ft;
        let hover = format!("{} {n}", self.types.show(ft));
        let d = self.ix.field_spans.get(&t).and_then(|v| v.get(i)).copied();
        self.idx_typed_ref(span, d, hover, ft);
    }

    pub fn idx_variant_ref(&mut self, t: TyId, i: usize, span: Span) {
        let Some(def) = self.types.enum_def(t) else {
            return;
        };
        let (n, ps) = &def.variants[i];
        let hover = format!("{}.{}", def.name, self.variant_text(n, ps));
        let d = self
            .ix
            .variant_spans
            .get(&t)
            .and_then(|v| v.get(i))
            .copied();
        self.idx_ref(span, d, hover);
    }

    /// Record a direct call: parameter-name inlay hints and the call-hierarchy edge.
    pub fn idx_call(&mut self, f: FuncId, callee: &crate::ast::Expr, args: &[crate::ast::Expr]) {
        use crate::ast::ExprKind as E;
        let mut c = callee;
        while let E::Generic(inner, _) = &c.kind {
            c = inner;
        }
        let at = match &c.kind {
            E::Field(_, n) => Span {
                file: c.span.file,
                start: c.span.end.saturating_sub(n.len() as u32),
                end: c.span.end,
            },
            _ => c.span,
        };
        if let Some(callee_def) = self.ix.fn_spans.get(&f).copied() {
            let caller = self
                .fns
                .last()
                .and_then(|ctx| self.ix.fn_spans.get(&ctx.func))
                .copied();
            self.ix.index.calls.push(Call {
                caller,
                callee: callee_def,
                at,
            });
        }
        let Some(names) = self.ix.fn_params.get(&f).cloned() else {
            return;
        };
        for (a, n) in args.iter().zip(&names) {
            let same = match &a.kind {
                E::Ident(x) | E::Field(_, x) => x == n,
                _ => false,
            };
            if n == "_" || same || a.span.end <= a.span.start {
                continue;
            }
            self.idx_hint(a.span.start, a.span.file, format!("{n}:"), HintKind::Param);
        }
    }
}
