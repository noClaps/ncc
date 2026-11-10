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
}

#[derive(Clone, Debug)]
pub struct SymRef {
    pub span: Span,
    pub def: Option<Span>,
    pub hover: String,
}

#[derive(Clone, Debug, Default)]
pub struct Index {
    pub decls: Vec<SymDecl>,
    pub refs: Vec<SymRef>,
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

    fn variant_text(&self, n: &str, ps: &[TyId]) -> String {
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
        });
        self.ix.index.refs.push(SymRef {
            span,
            def: Some(span),
            hover: detail,
        });
    }

    pub fn idx_ref(&mut self, span: Span, def: Option<Span>, hover: String) {
        self.ix.index.refs.push(SymRef { span, def, hover });
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
        self.idx_ref(span, def, hover);
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
        let def = self.ix.global_spans.get(&g).copied();
        self.idx_ref(span, def, hover);
    }

    pub fn idx_fn_ref(&mut self, f: FuncId, name: &str, span: Span) {
        let hover = self.fn_detail(name, f);
        let def = self.ix.fn_spans.get(&f).copied();
        self.idx_ref(span, def, hover);
    }

    pub fn idx_type_ref(&mut self, t: TyId, span: Span) {
        let hover = self.type_detail(t);
        let def = self.ix.type_spans.get(&t).copied();
        self.idx_ref(span, def, hover);
    }

    pub fn idx_field_ref(&mut self, t: TyId, i: usize, span: Span) {
        let Some(def) = self.types.struct_def(t) else {
            return;
        };
        let (n, ft) = &def.fields[i];
        let hover = format!("{} {n}", self.types.show(*ft));
        let d = self.ix.field_spans.get(&t).and_then(|v| v.get(i)).copied();
        self.idx_ref(span, d, hover);
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
}
