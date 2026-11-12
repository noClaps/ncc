//! Symbol index recorded during checking, used by the language server for hover,
//! go-to-definition and completion. It is filled even when checking fails part-way.

use std::collections::HashMap;

use super::Checker;
use crate::ast::TypeExpr;
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
    /// the declaration; references point at it by this span
    pub span: Span,
    /// the declared name
    pub name_span: Span,
    pub kind: DeclKind,
    /// hover text
    pub detail: String,
    /// members available after `.` on values of this declaration: (name, detail)
    pub members: Vec<(String, String)>,
    pub mutable: bool,
    pub param: bool,
    /// the block a local variable is visible in (None for module-level declarations)
    pub scope: Option<Span>,
    /// the variable's type; a function's return type; a type's own
    pub ty: Option<TyId>,
    /// for a function of an `extern` block: the block's alias (`alias.name(...)`)
    pub extern_alias: Option<String>,
    /// the names of a generic declaration's parameters (`T`, `N`)
    pub type_params: Vec<String>,
    /// for a generic struct: its fields' types as declared, in the order of `members`
    pub field_tys: Vec<TypeExpr>,
    /// a function's signature
    pub sig: Option<Sig>,
}

/// A function's signature, as editor features show and fill in calls.
#[derive(Clone, Debug, Default)]
pub struct Sig {
    /// the generic parameters as declared (`type T`, `uint N`)
    pub type_params: Vec<String>,
    /// (type, name); a builtin's parameters have only a name (`value`, `type T`, `...`)
    pub params: Vec<(Option<TypeExpr>, String)>,
    /// the return type, if any
    pub ret: Option<TypeExpr>,
}

impl Sig {
    /// `fn name<type T>(T a, int b) int`, and the range of each parameter in it.
    pub fn label(&self, name: &str) -> (String, Vec<(usize, usize)>) {
        let mut s = format!("fn {name}");
        if !self.type_params.is_empty() {
            s.push_str(&format!("<{}>", self.type_params.join(", ")));
        }
        s.push('(');
        let mut ranges = Vec::new();
        for (i, (t, n)) in self.params.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            let start = s.len();
            let part = match t {
                Some(t) if n.is_empty() => t.show(),
                Some(t) => format!("{} {n}", t.show()),
                None => n.clone(),
            };
            s.push_str(&part);
            ranges.push((start, s.len()));
        }
        s.push(')');
        if let Some(r) = &self.ret {
            s.push(' ');
            s.push_str(&r.show());
        }
        (s, ranges)
    }

    /// The function's type as a value: `(fn(int, int) int)`.
    pub fn value_ty(&self) -> Option<TypeExpr> {
        let ps = (self.params.iter())
            .map(|(t, _)| t.clone())
            .collect::<Option<_>>()?;
        let ret = self.ret.clone().map(Box::new);
        Some(TypeExpr::Fn(ps, ret, Span::default()))
    }
}

#[derive(Clone, Debug)]
pub struct SymRef {
    pub span: Span,
    /// the referenced name inside `span`
    pub name_span: Span,
    pub def: Option<Span>,
    pub hover: String,
    /// declaration of the referenced value's type (for go-to-type-definition)
    pub type_def: Option<Span>,
}

/// The type of a checked expression.
#[derive(Clone, Debug)]
pub struct ExprTy {
    pub span: Span,
    pub ty: TyId,
    pub type_def: Option<Span>,
}

/// A member reachable with `.` on values of a type.
#[derive(Clone, Debug)]
pub struct Member {
    pub name: String,
    pub detail: String,
    pub ty: TyId,
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
    /// the declarations of each struct's fields
    pub field_spans: HashMap<TyId, Vec<Span>>,
    /// the declarations of each enum's variants
    pub variant_spans: HashMap<TyId, Vec<Span>>,
    pub hints: Vec<Hint>,
    pub calls: Vec<Call>,
    /// per module, for the unused-code lint
    pub modules: Vec<ModFacts>,
    /// Bodies of generic functions that are never instantiated, checked only for the editor
    /// with placeholder types. Their declarations are left out of the unused-code lint, since
    /// statements that need a real type are skipped there.
    pub editor_only: Vec<Span>,
}

/// What the unused-code lint needs to know about a module.
#[derive(Clone, Debug, Default)]
pub struct ModFacts {
    pub file: u32,
    /// part of the project (not a dependency under `.nc/deps`)
    pub project: bool,
    /// names of the module's `pub` items
    pub public: std::collections::HashSet<String>,
    /// imports: alias, the alias's span, and whether the alias is used
    pub imports: Vec<(String, Span, bool)>,
}

#[derive(Default)]
pub struct IndexState {
    pub index: Index,
    pub local_spans: HashMap<(FuncId, LocalId), Span>,
    pub global_spans: HashMap<GlobalId, Span>,
    pub fn_spans: HashMap<FuncId, Span>,
    pub type_spans: HashMap<TyId, Span>,
    pub fn_params: HashMap<FuncId, Vec<String>>,
    /// import aliases used in each module: (module, alias)
    pub used_aliases: std::cell::RefCell<std::collections::HashSet<(usize, String)>>,
    /// extern functions: declaration span and hover text
    pub extern_decls: HashMap<usize, (Span, String)>,
    /// innermost scopes being checked, for the visibility of local variables
    pub scopes: Vec<Span>,
}

impl Checker {
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

    /// Declare a function (one of an `extern` block when `alias` is given) with its signature.
    pub fn idx_fn_decl(&mut self, name: &str, span: Span, alias: Option<&str>, sig: Sig) {
        let shown = alias.map_or_else(|| name.to_string(), |a| format!("{a}.{name}"));
        let (detail, _) = sig.label(&shown);
        let before = self.ix.index.decls.len();
        self.idx_decl(name, span, span, DeclKind::Function, detail, vec![]);
        if let Some(d) = self.ix.index.decls.get_mut(before) {
            d.type_params = (sig.type_params.iter())
                .map(|t| t.rsplit(' ').next().unwrap_or(t).to_string())
                .collect();
            d.extern_alias = alias.map(String::from);
            d.sig = Some(sig);
        }
    }

    /// The signature of the declared function `f`, with its parameter names.
    pub fn fn_sig(&self, f: FuncId) -> Sig {
        let (ps, r) = &self.sigs[f];
        let names = self.ix.fn_params.get(&f).cloned().unwrap_or_default();
        Sig {
            type_params: vec![],
            params: (ps.iter().enumerate())
                .map(|(i, p)| {
                    let name = names.get(i).cloned().unwrap_or_default();
                    (Some(self.types.to_type_expr(*p)), name)
                })
                .collect(),
            ret: (*r != VOID).then(|| self.types.to_type_expr(*r)),
        }
    }

    /// `fn add(int, int) int`
    pub fn fn_detail(&self, name: &str, f: FuncId) -> String {
        self.fn_detail_named(name, f, &[])
    }

    /// Like `fn_detail`, with the parameter names that are known (`fn add(int a, int b) int`).
    pub fn fn_detail_named(&self, name: &str, f: FuncId, params: &[String]) -> String {
        let (ps, r) = &self.sigs[f];
        let ps: Vec<String> = (ps.iter().enumerate())
            .map(|(i, p)| match params.get(i) {
                Some(n) => format!("{} {n}", self.types.show(*p)),
                None => self.types.show(*p),
            })
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
        name_span: Span,
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
            name_span,
            kind,
            detail: detail.clone(),
            members,
            mutable: false,
            param: false,
            scope: None,
            ty: None,
            extern_alias: None,
            type_params: vec![],
            field_tys: vec![],
            sig: None,
        });
        self.ix.index.refs.push(SymRef {
            span,
            name_span,
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
        // the name ends the declaration: `int x`
        let name_span = name_suffix(span, name.len());
        self.idx_decl(name, span, name_span, DeclKind::Variable, detail, members);
        if self.ix.index.decls.len() == before {
            return;
        }
        let type_def = self.type_def_of(ty);
        let d = self.ix.index.decls.last_mut().unwrap();
        d.mutable = mutable;
        d.param = param;
        d.scope = scope;
        d.ty = Some(ty);
        self.ix.index.refs.last_mut().unwrap().type_def = type_def;
    }

    /// Set the type of the declaration just recorded (a function's return type, a type's own).
    pub fn idx_last_ty(&mut self, name: &str, t: TyId) {
        if name == "_" || name.is_empty() {
            return;
        }
        if let Some(d) = self.ix.index.decls.last_mut() {
            d.ty = Some(t);
        }
    }

    /// A reference ending with the referenced name (`x`, `m.x`, `.x`); see `name_refs`.
    pub fn idx_ref(&mut self, span: Span, def: Option<Span>, hover: String) {
        self.ix.index.refs.push(SymRef {
            span,
            name_span: span,
            def,
            hover,
            type_def: None,
        });
    }

    fn idx_typed_ref(&mut self, span: Span, def: Option<Span>, hover: String, ty: TyId) {
        let type_def = self.type_def_of(ty);
        self.ix.index.refs.push(SymRef {
            span,
            name_span: span,
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

    /// Record the type of a checked expression.
    pub fn idx_expr(&mut self, span: Span, ty: TyId) {
        // only the editor asks for the types of expressions
        if !self.editor || span.end <= span.start {
            return;
        }
        let type_def = self.type_def_of(ty);
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

    /// A reference to a type, written `span` with its name at `name_span` (`m.Name<T>`).
    pub fn idx_type_ref(&mut self, t: TyId, span: Span, name_span: Span) {
        let hover = self.type_detail(t);
        let def = self.ix.type_spans.get(&t).copied();
        self.idx_typed_ref(span, def, hover, t);
        self.ix.index.refs.last_mut().unwrap().name_span = name_span;
    }

    pub fn idx_field_ref(&mut self, t: TyId, i: usize, span: Span) {
        let Some(def) = self.types.struct_def(t) else {
            return;
        };
        let (n, ft) = &def.fields[i];
        let ft = *ft;
        let hover = format!("{} {n}", self.types.show(ft));
        let d = (self.ix.index.field_spans.get(&t))
            .and_then(|v| v.get(i))
            .copied();
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
            .index
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

/// The last `len` bytes of `span`.
pub fn name_suffix(span: Span, len: usize) -> Span {
    Span {
        start: span.end.saturating_sub(len as u32).max(span.start),
        ..span
    }
}

impl Index {
    /// Narrow the name spans of references recorded with their whole span to the referenced
    /// declaration's name, which ends them.
    pub fn name_refs(&mut self) {
        let lens: HashMap<Span, usize> = (self.decls.iter())
            .map(|d| (d.span, d.name.len()))
            .collect();
        for r in &mut self.refs {
            if r.name_span == r.span
                && let Some(len) = r.def.and_then(|d| lens.get(&d))
            {
                r.name_span = name_suffix(r.span, *len);
            }
        }
    }
}

impl Index {
    /// The members reachable with `.` on values of type `t` (through optionals, mutexes and
    /// futures, which are unwrapped first).
    pub fn members(&self, types: &Types, t: TyId) -> Vec<Member> {
        let base = types.base(t);
        match types.kind(base) {
            TyKind::Opt(e) | TyKind::Mutex(e) | TyKind::Fut(e) => self.members(types, *e),
            TyKind::Struct(i) => (types.structs[*i as usize].fields.iter().enumerate())
                .map(|(k, (n, ft))| Member {
                    name: n.clone(),
                    detail: format!("{} {n}", types.show(*ft)),
                    ty: *ft,
                    def: (self.field_spans.get(&base))
                        .and_then(|v| v.get(k))
                        .copied(),
                })
                .collect(),
            TyKind::Array(_) | TyKind::Fixed(..) | TyKind::Str | TyKind::Map(..) => vec![Member {
                name: "len".into(),
                detail: "uint len".into(),
                ty: UINT,
                def: None,
            }],
            _ => vec![],
        }
    }
}

/// The element (or map value) type of `t`, for indexing with `[...]`.
pub fn elem_of(types: &Types, t: TyId) -> Option<TyId> {
    match types.kind(types.base(t)) {
        TyKind::Array(e) | TyKind::Fixed(e, _) | TyKind::Map(_, e) => Some(*e),
        TyKind::Str => Some(CHAR),
        _ => None,
    }
}

/// The return type of the function type `t`.
pub fn return_of(types: &Types, t: TyId) -> Option<TyId> {
    match types.kind(types.base(t)) {
        TyKind::Fn(_, r) => Some(*r),
        _ => None,
    }
}
