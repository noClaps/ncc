//! Semantic analysis: name resolution, type checking, generic instantiation and lowering
//! of the AST into the typed IR.

mod expr;
pub mod flow;
pub mod index;
mod pat;
mod stmt;

use std::collections::{HashMap, HashSet};

use crate::ast;
use crate::diag::{Diag, Result, Span, err};
use crate::modules::ModuleGraph;
use crate::tir::*;
use crate::types::*;

#[derive(Clone, Debug)]
pub enum Sym {
    Fn {
        decl: usize,
        func: Option<FuncId>,
    },
    Struct {
        decl: usize,
        ty: Option<TyId>,
    },
    Enum {
        decl: usize,
        ty: Option<TyId>,
    },
    Type(TyId),
    /// globals with that name, with declaration position (shadowing at top level)
    Globals(Vec<(GlobalId, u32)>),
}

#[derive(Clone, Debug)]
pub struct SymEntry {
    pub sym: Sym,
    pub public: bool,
}

pub struct ModInfo {
    pub ast: ast::Module,
    pub file: u32,
    pub path: std::path::PathBuf,
    pub imports: HashMap<String, usize>,
    pub syms: HashMap<String, SymEntry>,
    pub externs: HashMap<String, HashMap<String, ExternId>>,
    /// top-level statements, checked, with whether they belong to the entry module
    pub init: Vec<Stmt>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum JumpKind {
    Loop,
    If,
    Lock,
    /// value-producing context with its value type
    Value(TyId),
}

#[derive(Clone, Debug)]
pub struct JumpCtx {
    pub id: JumpId,
    pub kind: JumpKind,
    pub label: Option<String>,
}

pub struct FnCtx {
    pub func: FuncId,
    pub locals: Vec<Local>,
    pub scopes: Vec<Vec<(String, LocalId)>>,
    pub captures: Vec<Capture>,
    /// outer local id -> local id in this function
    pub cap_map: HashMap<LocalId, LocalId>,
    pub ret: TyId,
    pub jumps: Vec<JumpCtx>,
    pub module: usize,
    pub tparams: HashMap<String, TyId>,
    /// Where the generic parameters in `tparams` are declared
    pub tspans: HashMap<String, Span>,
    pub is_test: bool,
    /// The module top-level context: declarations at scope depth 1 become globals.
    pub is_main: bool,
    pub is_never: bool,
    pub self_local: Option<LocalId>,
    /// `$` contexts: types of the bases of enclosing index expressions
    pub dollar: Vec<TyId>,
}

impl FnCtx {
    pub fn new(func: FuncId, module: usize, ret: TyId, tparams: HashMap<String, TyId>) -> FnCtx {
        FnCtx {
            func,
            locals: vec![],
            scopes: vec![vec![]],
            captures: vec![],
            cap_map: HashMap::new(),
            ret,
            jumps: vec![],
            module,
            tparams,
            tspans: HashMap::new(),
            is_test: false,
            is_main: false,
            is_never: false,
            self_local: None,
            dollar: vec![],
        }
    }
}

struct PendingFn {
    func: FuncId,
    module: usize,
    decl: ast::FnDecl,
    tparams: HashMap<String, TyId>,
}

pub struct Checker {
    pub mods: Vec<ModInfo>,
    pub types: Types,
    pub funcs: Vec<Option<Func>>,
    /// signatures of reserved functions: (params, ret)
    pub sigs: Vec<(Vec<TyId>, TyId)>,
    pub globals: Vec<Global>,
    pub externs: Vec<Extern>,
    fn_insts: HashMap<(usize, usize, Vec<TyId>), FuncId>,
    type_insts: HashMap<(usize, usize, Vec<TyId>), TyId>,
    pending: Vec<PendingFn>,
    pub fns: Vec<FnCtx>,
    next_jump: JumpId,
    /// jump ids targeted by at least one `break`
    pub broken: HashSet<JumpId>,
    pub never_funcs: HashSet<FuncId>,
    tests: Vec<Test>,
    order: Vec<usize>,
    pub ix: index::IndexState,
    pub errors: Vec<Diag>,
    /// Checking for the language server: also index generic bodies that are never instantiated
    pub editor: bool,
}

/// Check a program, returning the (possibly partial) program, every error found and the
/// symbol index. Statements that failed to check are left out of the program.
///
/// For the language server (`editor`), generic functions that are never instantiated are also
/// checked with placeholder types, so their bodies are indexed; the program must then not be
/// compiled.
pub fn check(graph: ModuleGraph, editor: bool) -> (Program, Vec<Diag>, index::Index) {
    let mut c = Checker {
        mods: graph
            .modules
            .into_iter()
            .map(|m| ModInfo {
                ast: m.ast,
                file: m.file,
                path: m.path,
                imports: m.imports,
                syms: HashMap::new(),
                externs: HashMap::new(),
                init: vec![],
            })
            .collect(),
        types: Types::new(),
        funcs: vec![],
        sigs: vec![],
        globals: vec![],
        externs: vec![],
        fn_insts: HashMap::new(),
        type_insts: HashMap::new(),
        pending: vec![],
        fns: vec![],
        next_jump: 0,
        broken: HashSet::new(),
        never_funcs: HashSet::new(),
        tests: vec![],
        order: graph.order,
        ix: Default::default(),
        errors: vec![],
        editor,
    };
    let p = c.run();
    c.record_module_facts();
    let mut errors = std::mem::take(&mut c.errors);
    errors.retain(|d| !d.msg.is_empty());
    errors.sort_by_key(|d| (d.span.file, d.span.start));
    errors.dedup_by(|a, b| a.span == b.span && a.msg == b.msg);
    (p, errors, std::mem::take(&mut c.ix.index))
}

impl Checker {
    /// Public names, imports (and whether they are used) and project membership of each
    /// module, for the unused-code lint.
    fn record_module_facts(&mut self) {
        let used = self.ix.used_aliases.borrow().clone();
        let mut facts = Vec::new();
        for (m, info) in self.mods.iter().enumerate() {
            let parts: Vec<String> = info
                .path
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            let dependency = parts.windows(2).any(|w| w[0] == ".nc" && w[1] == "deps");
            let mut f = index::ModFacts {
                file: info.file,
                project: !dependency,
                ..Default::default()
            };
            for it in &info.ast.items {
                match it {
                    ast::Item::Fn(x) if x.public => {
                        f.public.insert(x.sig.name.clone());
                    }
                    ast::Item::Struct(x) if x.public => {
                        f.public.insert(x.name.clone());
                    }
                    ast::Item::Enum(x) if x.public => {
                        f.public.insert(x.name.clone());
                    }
                    ast::Item::Type(x) if x.public => {
                        f.public.insert(x.name.clone());
                    }
                    ast::Item::Stmt(ast::Stmt {
                        kind:
                            ast::StmtKind::VarDecl {
                                public: true,
                                targets,
                                ..
                            },
                        ..
                    }) => {
                        f.public.extend(targets.iter().map(|t| t.name.clone()));
                    }
                    ast::Item::Import(im) => f.imports.push((
                        im.alias.clone(),
                        im.span,
                        used.contains(&(m, im.alias.clone())),
                    )),
                    _ => {}
                }
            }
            facts.push(f);
        }
        self.ix.index.modules = facts;
    }

    pub fn new_jump(&mut self) -> JumpId {
        self.next_jump += 1;
        self.next_jump
    }

    pub fn report(&mut self, r: Result<()>) {
        if let Err(d) = r {
            // empty messages mark errors that were already reported by the parser
            if !d.msg.is_empty() {
                self.errors.push(d);
            }
        }
    }

    fn run(&mut self) -> Program {
        // Phase A: declare names
        for m in 0..self.mods.len() {
            self.declare_names(m);
        }
        // Phase B: resolve type definitions and function signatures
        for m in 0..self.mods.len() {
            self.resolve_decls(m);
        }
        // Phase C: top-level statements, module by module in dependency order
        let main_ret = self.types.erru(VOID);
        let main = self.reserve_fn(vec![], main_ret);
        let mut main_ctx = FnCtx::new(main, 0, main_ret, HashMap::new());
        main_ctx.is_main = true;
        self.fns.push(main_ctx);
        let order = self.order.clone();
        for &m in &order {
            self.fns.last_mut().unwrap().module = m;
            self.fns.last_mut().unwrap().scopes = vec![vec![]];
            let items = self.mods[m].ast.items.clone();
            let mut init = vec![];
            for it in &items {
                if let ast::Item::Stmt(s) = it {
                    let r = self.check_stmt(s, &mut init);
                    self.report(r);
                }
            }
            self.mods[m].init = init;
        }
        let main_ctx = self.fns.pop().unwrap();
        // tests (entry module only are run, but all are checked)
        for m in 0..self.mods.len() {
            let items = self.mods[m].ast.items.clone();
            for it in &items {
                if let ast::Item::Test(name, body, _) = it {
                    let erru = self.types.erru(VOID);
                    let f = self.reserve_fn(vec![], erru);
                    let mut ctx = FnCtx::new(f, m, erru, HashMap::new());
                    ctx.is_test = true;
                    self.fns.push(ctx);
                    let body = self.check_block(body);
                    let ctx = self.fns.pop().unwrap();
                    self.finish_fn(ctx, format!("test {name}"), vec![], body, None);
                    if m == 0 {
                        self.tests.push(Test {
                            name: name.clone(),
                            func: f,
                        });
                    }
                }
            }
        }
        // Phase D: function bodies (including generic instances discovered on the way)
        while let Some(p) = self.pending.pop() {
            self.check_fn_body(p);
        }
        if self.editor {
            self.index_generic_bodies();
        }

        // assemble main
        let main_body = self.assemble_main();
        self.finish_fn(main_ctx, "main".into(), vec![], main_body, None);
        // functions whose checking failed early get empty bodies
        let sigs = self.sigs.clone();
        let funcs = std::mem::take(&mut self.funcs)
            .into_iter()
            .enumerate()
            .map(|(i, f)| {
                f.unwrap_or_else(|| Func {
                    name: "<invalid>".into(),
                    params: vec![],
                    ret: sigs[i].1,
                    locals: vec![],
                    captures: vec![],
                    body: Block { stmts: vec![] },
                    parent: None,
                    self_local: None,
                })
            })
            .collect();
        Program {
            types: std::mem::take(&mut self.types),
            funcs,
            globals: std::mem::take(&mut self.globals),
            externs: std::mem::take(&mut self.externs),
            main,
            tests: std::mem::take(&mut self.tests),
            target: None,
        }
    }

    // ------------------------------------------------------------ declarations

    fn add_sym(&mut self, m: usize, name: &str, sym: Sym, public: bool, span: Span) -> Result<()> {
        if builtin_type(name).is_some() {
            return err(
                span,
                format!("`{name}` is a builtin type and cannot be redefined"),
            );
        }
        if self.mods[m].imports.contains_key(name) {
            return err(span, format!("`{name}` is already used as an import alias"));
        }
        if self.mods[m].syms.contains_key(name) {
            return err(span, format!("`{name}` is already defined in this module"));
        }
        self.mods[m]
            .syms
            .insert(name.to_string(), SymEntry { sym, public });
        Ok(())
    }

    fn declare_names(&mut self, m: usize) {
        let items = self.mods[m].ast.items.clone();
        for (i, it) in items.iter().enumerate() {
            let item_r: Result<()> = (|| {
                match it {
                    ast::Item::Fn(f) => self.add_sym(
                        m,
                        &f.sig.name,
                        Sym::Fn {
                            decl: i,
                            func: None,
                        },
                        f.public,
                        f.sig.span,
                    )?,
                    ast::Item::Struct(s) => {
                        let ty = if s.type_params.is_empty() {
                            Some(self.types.new_struct(s.name.clone()))
                        } else {
                            None
                        };
                        self.add_sym(m, &s.name, Sym::Struct { decl: i, ty }, s.public, s.span)?;
                    }
                    ast::Item::Enum(e) => {
                        let ty = if e.type_params.is_empty() {
                            Some(self.types.new_enum(e.name.clone()))
                        } else {
                            None
                        };
                        self.add_sym(m, &e.name, Sym::Enum { decl: i, ty }, e.public, e.span)?;
                    }
                    ast::Item::Type(t) => {
                        let ty = self.types.new_named(t.name.clone(), VOID);
                        self.add_sym(m, &t.name, Sym::Type(ty), t.public, t.span)?;
                    }
                    ast::Item::Extern(e) => {
                        if self.mods[m].externs.contains_key(&e.alias)
                            || self.mods[m].imports.contains_key(&e.alias)
                        {
                            return err(e.span, format!("duplicate alias `{}`", e.alias));
                        }
                        self.mods[m].externs.insert(e.alias.clone(), HashMap::new());
                    }
                    _ => {}
                }
                Ok(())
            })();
            self.report(item_r);
        }
    }

    fn resolve_decls(&mut self, m: usize) {
        let items = self.mods[m].ast.items.clone();
        let none = HashMap::new();
        for it in &items {
            let item_r: Result<()> = (|| {
                match it {
                    ast::Item::Type(t) => {
                        let ty = match self.mods[m].syms[&t.name].sym {
                            Sym::Type(ty) => ty,
                            _ => unreachable!(),
                        };
                        let under = self.resolve_type(&t.ty, m, &none)?;
                        if let TyKind::Named(n) = self.types.kind(ty).clone() {
                            self.types.named[n as usize].underlying = under;
                        }
                        self.ix.type_spans.insert(ty, t.span);
                        let detail = self.type_detail(ty);
                        let members = self.members_of(ty);
                        self.idx_decl(&t.name, t.span, index::DeclKind::Type, detail, members);
                        self.idx_last_ty(&t.name, ty);
                        // reject cycles
                        let mut seen = vec![ty];
                        let mut cur = under;
                        while let TyKind::Named(n) = self.types.kind(cur).clone() {
                            if seen.contains(&cur) {
                                return err(
                                    t.span,
                                    format!("type `{}` is defined in terms of itself", t.name),
                                );
                            }
                            seen.push(cur);
                            cur = self.types.named[n as usize].underlying;
                        }
                    }
                    ast::Item::Struct(s) if s.type_params.is_empty() => {
                        let ty = match self.mods[m].syms[&s.name].sym {
                            Sym::Struct { ty, .. } => ty.unwrap(),
                            _ => unreachable!(),
                        };
                        self.fill_struct(ty, s, m, &none)?;
                        let detail = self.type_detail(ty);
                        self.idx_decl(&s.name, s.span, index::DeclKind::Struct, detail, vec![]);
                        self.idx_last_ty(&s.name, ty);
                    }
                    ast::Item::Enum(e) if e.type_params.is_empty() => {
                        let ty = match self.mods[m].syms[&e.name].sym {
                            Sym::Enum { ty, .. } => ty.unwrap(),
                            _ => unreachable!(),
                        };
                        self.fill_enum(ty, e, m, &none)?;
                        let detail = self.type_detail(ty);
                        let members: Vec<(String, String)> = e
                            .variants
                            .iter()
                            .map(|v| {
                                let ps = self.types.enum_def(ty).map(|d| {
                                    d.variants
                                        .iter()
                                        .find(|x| x.0 == v.0)
                                        .map(|x| x.1.clone())
                                        .unwrap_or_default()
                                });
                                let text = self.variant_text(&v.0, &ps.unwrap_or_default());
                                (v.0.clone(), format!("{}.{text}", e.name))
                            })
                            .collect();
                        self.idx_decl(&e.name, e.span, index::DeclKind::Enum, detail, members);
                        self.idx_last_ty(&e.name, ty);
                    }
                    _ => {}
                }
                Ok(())
            })();
            self.report(item_r);
        }
        for it in &items {
            let item_r: Result<()> = (|| {
                match it {
                    ast::Item::Fn(f) if f.sig.type_params.is_empty() => {
                        let (params, ret) = self.resolve_sig(&f.sig, m, &none)?;
                        let id = self.reserve_decl(&f.sig, params, ret);
                        let names = self.ix.fn_params[&id].clone();
                        let detail = self.fn_detail_named(&f.sig.name, id, &names);
                        self.idx_decl(
                            &f.sig.name,
                            f.sig.span,
                            index::DeclKind::Function,
                            detail,
                            vec![],
                        );
                        let ret = self.sigs[id].1;
                        self.idx_last_ty(&f.sig.name, ret);
                        if let Some(SymEntry {
                            sym: Sym::Fn { func, .. },
                            ..
                        }) = self.mods[m].syms.get_mut(&f.sig.name)
                        {
                            *func = Some(id);
                        }
                        self.pending.push(PendingFn {
                            func: id,
                            module: m,
                            decl: f.clone(),
                            tparams: HashMap::new(),
                        });
                    }
                    // generic declarations are only checked when instantiated; index
                    // them so that uses link to them (and unused ones are found)
                    ast::Item::Fn(f) => {
                        // `fn map<type I, type O>(I[] arr, (fn(I) O) f) O[]`
                        let tps: Vec<String> = f
                            .sig
                            .type_params
                            .iter()
                            .map(ast::GenericParam::show)
                            .collect();
                        for t in &f.sig.type_params {
                            self.idx_ref(t.span, Some(t.span), t.show());
                        }
                        let ps: Vec<String> = f
                            .sig
                            .params
                            .iter()
                            .map(|p| format!("{} {}", p.ty.show(), p.name))
                            .collect();
                        let ret = f.sig.ret.as_ref().map(|r| format!(" {}", r.show()));
                        let detail = format!(
                            "fn {}<{}>({}){}",
                            f.sig.name,
                            tps.join(", "),
                            ps.join(", "),
                            ret.unwrap_or_default()
                        );
                        self.idx_decl(
                            &f.sig.name,
                            f.sig.span,
                            index::DeclKind::Function,
                            detail,
                            vec![],
                        );
                    }
                    ast::Item::Struct(st) if !st.type_params.is_empty() => {
                        for t in &st.type_params {
                            self.idx_ref(t.span, Some(t.span), t.show());
                        }
                        let detail = format!(
                            "struct {}{}",
                            st.name,
                            ast::GenericParam::list(&st.type_params)
                        );
                        self.idx_decl(&st.name, st.span, index::DeclKind::Struct, detail, vec![]);
                    }
                    ast::Item::Enum(en) if !en.type_params.is_empty() => {
                        for t in &en.type_params {
                            self.idx_ref(t.span, Some(t.span), t.show());
                        }
                        let detail = format!(
                            "enum {}{}",
                            en.name,
                            ast::GenericParam::list(&en.type_params)
                        );
                        self.idx_decl(&en.name, en.span, index::DeclKind::Enum, detail, vec![]);
                    }
                    ast::Item::Extern(e) => {
                        let dir = self.mods[m].path.parent().unwrap().to_path_buf();
                        for f in &e.fns {
                            let (params, ret) = self.resolve_sig(&f.sig, m, &none)?;
                            let id = self.externs.len();
                            let ps: Vec<String> = params
                                .iter()
                                .zip(&f.sig.params)
                                .map(|(t, p)| format!("{} {}", self.types.show(*t), p.name))
                                .collect();
                            let detail = format!(
                                "fn {}.{}({}) {}",
                                e.alias,
                                f.sig.name,
                                ps.join(", "),
                                if ret == VOID {
                                    String::new()
                                } else {
                                    self.types.show(ret)
                                }
                            );
                            self.idx_decl(
                                &f.sig.name,
                                f.sig.span,
                                index::DeclKind::Function,
                                detail.trim_end().to_string(),
                                vec![],
                            );
                            self.idx_last_ty(&f.sig.name, ret);
                            self.ix
                                .extern_decls
                                .insert(id, (f.sig.span, detail.trim_end().to_string()));
                            if let Some(d) = self.ix.index.decls.last_mut() {
                                d.extern_alias = Some(e.alias.clone());
                            }
                            self.externs.push(Extern {
                                name: format!("{}.{}", e.alias, f.sig.name),
                                symbol: f.symbol.clone(),
                                params,
                                ret,
                                file: dir.join(&e.path),
                            });
                            let tab = self.mods[m].externs.get_mut(&e.alias).unwrap();
                            if tab.insert(f.sig.name.clone(), id).is_some() {
                                return err(
                                    f.sig.span,
                                    format!("duplicate extern function `{}`", f.sig.name),
                                );
                            }
                        }
                    }
                    _ => {}
                }
                Ok(())
            })();
            self.report(item_r);
        }
        // reject infinitely sized structs
        for it in &items {
            if let ast::Item::Struct(s) = it
                && let Some(SymEntry {
                    sym: Sym::Struct { ty: Some(ty), .. },
                    ..
                }) = self.mods[m].syms.get(&s.name)
                && let Err(d) = self.check_finite(*ty, s.span)
            {
                self.errors.push(d);
            }
        }
    }

    fn check_finite(&self, ty: TyId, span: Span) -> Result<()> {
        fn walk(t: &Types, cur: TyId, target: TyId, first: bool, seen: &mut Vec<TyId>) -> bool {
            if !first && cur == target {
                return true;
            }
            if seen.contains(&cur) {
                return false;
            }
            seen.push(cur);
            let r = match t.kind(cur) {
                TyKind::Named(_) => walk(t, t.base(cur), target, false, seen),
                TyKind::Tuple(ts) => ts.iter().any(|x| walk(t, *x, target, false, seen)),
                TyKind::ErrU(i) => walk(t, *i, target, false, seen),
                TyKind::Struct(i) => t.structs[*i as usize]
                    .fields
                    .iter()
                    .any(|(_, x)| walk(t, *x, target, false, seen)),
                // optionals that contain themselves are boxed; arrays, maps and enums are on the heap
                _ => false,
            };
            seen.pop();
            r
        }
        if walk(&self.types, ty, ty, true, &mut vec![]) {
            return err(
                span,
                format!(
                    "struct `{}` contains itself and would have infinite size",
                    self.types.show(ty)
                ),
            );
        }
        Ok(())
    }

    fn fill_struct(
        &mut self,
        ty: TyId,
        s: &ast::StructDecl,
        m: usize,
        tp: &HashMap<String, TyId>,
    ) -> Result<()> {
        let mut fields = Vec::new();
        for (fty, name, sp) in &s.fields {
            if fields.iter().any(|(n, _): &(String, TyId)| n == name) {
                return err(*sp, format!("duplicate field `{name}`"));
            }
            let t = self.resolve_type(fty, m, tp)?;
            if matches!(
                self.types.kind(t),
                TyKind::Void | TyKind::Never | TyKind::Mutex(_) | TyKind::Fut(_)
            ) {
                return err(
                    *sp,
                    format!("struct fields cannot have type `{}`", self.types.show(t)),
                );
            }
            fields.push((name.clone(), t));
        }
        self.ix
            .field_spans
            .insert(ty, s.fields.iter().map(|f| f.2).collect());
        self.ix.type_spans.insert(ty, s.span);
        if let TyKind::Struct(i) = *self.types.kind(ty) {
            self.types.structs[i as usize].fields = fields;
        }
        for (_, name, sp) in &s.fields {
            let fdetail = self
                .types
                .struct_def(ty)
                .and_then(|d| d.fields.iter().find(|f| &f.0 == name))
                .map(|f| format!("{} {name}", self.types.show(f.1)))
                .unwrap_or_default();
            self.idx_decl(name, *sp, index::DeclKind::Field, fdetail, vec![]);
        }
        Ok(())
    }

    fn fill_enum(
        &mut self,
        ty: TyId,
        e: &ast::EnumDecl,
        m: usize,
        tp: &HashMap<String, TyId>,
    ) -> Result<()> {
        let mut variants = Vec::new();
        for (name, tys, sp) in &e.variants {
            if variants
                .iter()
                .any(|(n, _): &(String, Vec<TyId>)| n == name)
            {
                return err(*sp, format!("duplicate enum member `{name}`"));
            }
            let ts = tys
                .iter()
                .map(|t| self.resolve_type(t, m, tp))
                .collect::<Result<_>>()?;
            variants.push((name.clone(), ts));
        }
        self.ix
            .variant_spans
            .insert(ty, e.variants.iter().map(|v| v.2).collect());
        self.ix.type_spans.insert(ty, e.span);
        if let TyKind::Enum(i) = *self.types.kind(ty) {
            self.types.enums[i as usize].variants = variants;
        }
        let ename = self.types.show(ty);
        for (vn, _, sp) in &e.variants {
            self.idx_decl(
                vn,
                *sp,
                index::DeclKind::Variant,
                format!("{ename}.{vn}"),
                vec![],
            );
        }
        Ok(())
    }

    pub fn resolve_sig(
        &mut self,
        sig: &ast::FnSig,
        m: usize,
        tp: &HashMap<String, TyId>,
    ) -> Result<(Vec<TyId>, TyId)> {
        let mut params = Vec::new();
        for p in &sig.params {
            let t = self.resolve_type(&p.ty, m, tp)?;
            if matches!(
                self.types.kind(t),
                TyKind::Void | TyKind::Never | TyKind::Fut(_)
            ) {
                return err(
                    p.span,
                    format!("parameters cannot have type `{}`", self.types.show(t)),
                );
            }
            params.push(t);
        }
        let ret = match &sig.ret {
            Some(t) => self.resolve_type(t, m, tp)?,
            None => VOID,
        };
        let value = match self.types.kind(ret) {
            TyKind::ErrU(i) => *i,
            _ => ret,
        };
        if matches!(self.types.kind(value), TyKind::Fut(_)) {
            return err(sig.span, "futures cannot be returned from functions");
        }
        Ok((params, ret))
    }

    pub fn reserve_fn(&mut self, params: Vec<TyId>, ret: TyId) -> FuncId {
        self.funcs.push(None);
        self.sigs.push((params, ret));
        self.funcs.len() - 1
    }

    /// Reserve a declared function, recording what the index needs of its signature.
    pub fn reserve_decl(&mut self, sig: &ast::FnSig, params: Vec<TyId>, ret: TyId) -> FuncId {
        let id = self.reserve_fn(params, ret);
        if is_never_ret(sig) {
            self.never_funcs.insert(id);
        }
        self.ix.fn_spans.insert(id, sig.span);
        self.ix
            .fn_params
            .insert(id, sig.params.iter().map(|p| p.name.clone()).collect());
        id
    }

    /// Check a function body from inside another one (a generic instance).
    fn check_nested_body(&mut self, p: PendingFn) {
        let saved = std::mem::take(&mut self.fns);
        self.check_fn_body(p);
        self.fns = saved;
    }

    pub fn finish_fn(
        &mut self,
        ctx: FnCtx,
        name: String,
        params: Vec<LocalId>,
        body: Block,
        parent: Option<FuncId>,
    ) {
        let (_, ret) = self.sigs[ctx.func].clone();
        self.funcs[ctx.func] = Some(Func {
            name,
            params,
            ret,
            locals: ctx.locals,
            captures: ctx.captures,
            body,
            parent,
            self_local: ctx.self_local,
        });
    }

    fn check_fn_body(&mut self, p: PendingFn) {
        let (params, ret) = self.sigs[p.func].clone();
        let mut ctx = FnCtx::new(p.func, p.module, ret, p.tparams);
        ctx.tspans = (p.decl.sig.type_params.iter())
            .map(|t| (t.name.clone(), t.span))
            .collect();
        ctx.is_never = ret == NEVER;
        self.fns.push(ctx);
        self.ix.scopes.push(p.decl.body.span);
        let pids: Vec<_> = (p.decl.sig.params.iter().zip(&params))
            .map(|(prm, &ty)| self.declare_local(&prm.name, ty, false, LocalKind::Param, prm.span))
            .collect();
        self.ix.scopes.pop();
        let before = self.errors.len();
        let body = self.check_block(&p.decl.body);
        // a missing-return error would only be noise when statements failed to check
        if self.errors.len() == before {
            let r = self.check_fn_end(&body, ret, p.decl.sig.span, &p.decl.sig.name);
            self.report(r);
        }
        let ctx = self.fns.pop().unwrap();
        self.finish_fn(ctx, p.decl.sig.name, pids, body, None);
    }

    pub fn check_fn_end(&mut self, body: &Block, ret: TyId, span: Span, name: &str) -> Result<()> {
        let completes = self.block_completes(body);
        if completes {
            if ret == NEVER {
                return err(
                    span,
                    format!("function `{name}` is declared `never` but can return"),
                );
            }
            let needs = match self.types.kind(ret) {
                TyKind::Void => false,
                TyKind::ErrU(i) => *i != VOID,
                _ => true,
            };
            if needs {
                return err(
                    span,
                    format!("function `{name}` does not return a value on every path"),
                );
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------ generics

    pub fn instantiate_fn(
        &mut self,
        m: usize,
        decl: usize,
        args: Vec<TyId>,
        span: Span,
    ) -> Result<FuncId> {
        let f = match &self.mods[m].ast.items[decl] {
            ast::Item::Fn(f) => f.clone(),
            _ => unreachable!(),
        };
        let (args, tp) = self.bind_generics(&f.sig.name, &f.sig.type_params, args, span)?;
        let key = (m, decl, args.clone());
        if let Some(&f) = self.fn_insts.get(&key) {
            return Ok(f);
        }
        let (params, ret) = self
            .resolve_sig(&f.sig, m, &tp)
            .map_err(|d| self.in_instance(d, &f.sig.name, &args))?;
        let id = self.reserve_decl(&f.sig, params, ret);
        self.fn_insts.insert(key, id);
        // generic bodies are type-checked at instantiation; check right away so that errors
        // point at the instantiation that caused them.
        let inst = self.types_name(&f.sig.name, &args);
        let mut decl = f;
        decl.sig.name = inst.clone();
        let before = self.errors.len();
        self.check_nested_body(PendingFn {
            func: id,
            module: m,
            decl,
            tparams: tp,
        });
        for d in &mut self.errors[before..] {
            d.msg = format!("{} (in instantiation `{inst}`)", d.msg);
        }
        Ok(id)
    }

    /// Match generic arguments to parameters: types to `type T`, constants to `uint N`.
    /// Constants are checked against their declared type and retagged with it.
    fn bind_generics(
        &mut self,
        name: &str,
        params: &[ast::GenericParam],
        args: Vec<TyId>,
        span: Span,
    ) -> Result<(Vec<TyId>, HashMap<String, TyId>)> {
        if params.len() != args.len() {
            return err(
                span,
                format!(
                    "`{name}` expects {} generic argument(s), found {}",
                    params.len(),
                    args.len()
                ),
            );
        }
        let mut out = Vec::new();
        let mut tp = HashMap::new();
        for (p, a) in params.iter().zip(args) {
            let a = match (&p.konst, self.types.kind(a).clone()) {
                (None, TyKind::Const(_, v)) => {
                    return err(
                        span,
                        format!(
                            "`{}` is a type parameter of `{name}`, but `{v}` is a constant",
                            p.name
                        ),
                    );
                }
                (None, _) => a,
                (Some(k), TyKind::Const(_, v)) => {
                    let ty = builtin_type(k).unwrap_or(UINT);
                    let (lo, hi) = match ty {
                        BYTE => (0, 255),
                        INT => (i64::MIN as i128, i64::MAX as i128),
                        _ => (0, u64::MAX as i128),
                    };
                    if v < lo || v > hi {
                        return err(
                            span,
                            format!("`{v}` is out of range for `{k} {}` of `{name}`", p.name),
                        );
                    }
                    self.types.intern(TyKind::Const(ty, v))
                }
                (Some(k), _) => {
                    return err(
                        span,
                        format!(
                            "`{} {}` of `{name}` is a constant, but `{}` is a type",
                            k,
                            p.name,
                            self.types.show(a)
                        ),
                    );
                }
            };
            tp.insert(p.name.clone(), a);
            out.push(a);
        }
        Ok((out, tp))
    }

    /// Evaluate a compile-time integer (an array size or a generic constant argument).
    pub fn const_value(&self, c: &ast::ConstExpr, tp: &HashMap<String, TyId>) -> Result<i128> {
        use ast::ConstExpr as C;
        Ok(match c {
            C::Int(n, sp) => {
                i128::try_from(*n).map_err(|_| Diag::new(*sp, "constant is too large"))?
            }
            C::Name(n, sp) => match tp.get(n).map(|t| self.types.kind(*t)) {
                Some(TyKind::Const(_, v)) => *v,
                Some(_) => return err(*sp, format!("`{n}` is a type, not a constant")),
                None => return err(*sp, format!("unknown constant `{n}`")),
            },
            C::Neg(e, _) => -self.const_value(e, tp)?,
            C::Paren(e, _) => self.const_value(e, tp)?,
            C::Bin(op, a, b) => {
                let (x, y) = (self.const_value(a, tp)?, self.const_value(b, tp)?);
                let r = match op {
                    ast::BinOp::Add => x.checked_add(y),
                    ast::BinOp::Sub => x.checked_sub(y),
                    ast::BinOp::Mul => x.checked_mul(y),
                    ast::BinOp::Div | ast::BinOp::Mod if y == 0 => {
                        return err(c.span(), format!("division by zero in `{}`", c.show()));
                    }
                    ast::BinOp::Div => x.checked_div(y),
                    ast::BinOp::Mod => x.checked_rem(y),
                    _ => unreachable!("not a constant operator"),
                };
                match r {
                    Some(r) if i64::MIN as i128 <= r && r <= u64::MAX as i128 => r,
                    _ => return err(c.span(), format!("`{}` overflows", c.show())),
                }
            }
        })
    }

    /// A generic argument: a type, or a constant (`3`, `N - 1`, or a bare constant `N`).
    pub fn resolve_targ(
        &mut self,
        te: &ast::TypeExpr,
        m: usize,
        tp: &HashMap<String, TyId>,
    ) -> Result<TyId> {
        match te {
            ast::TypeExpr::Const(c) => {
                let v = self.const_value(c, tp)?;
                Ok(self.types.intern(TyKind::Const(INT, v)))
            }
            ast::TypeExpr::Named {
                module: None,
                name,
                args,
                ..
            } if args.is_empty()
                && tp
                    .get(name)
                    .is_some_and(|t| matches!(self.types.kind(*t), TyKind::Const(..))) =>
            {
                Ok(tp[name])
            }
            _ => self.resolve_type(te, m, tp),
        }
    }

    /// Name the instantiation an error in a generic declaration came from.
    fn in_instance(&self, mut d: Diag, name: &str, args: &[TyId]) -> Diag {
        if !d.msg.contains("(in instantiation") {
            d.msg = format!(
                "{} (in instantiation `{}`)",
                d.msg,
                self.types_name(name, args)
            );
        }
        d
    }

    /// For the editor: check each generic function that was never instantiated once, with
    /// its type parameters as opaque types named after them and its constants as placeholder
    /// values, so that hover, completion and navigation work inside the body. Errors are
    /// discarded: operations that need a real type (`value.x` on a `T`) just aren't indexed.
    fn index_generic_bodies(&mut self) {
        for m in 0..self.mods.len() {
            for decl in 0..self.mods[m].ast.items.len() {
                let f = match &self.mods[m].ast.items[decl] {
                    ast::Item::Fn(f) if !f.sig.type_params.is_empty() => f.clone(),
                    _ => continue,
                };
                if self
                    .fn_insts
                    .keys()
                    .any(|(mm, d, _)| *mm == m && *d == decl)
                {
                    continue;
                }
                let mut tp = HashMap::new();
                let mut consts = Vec::new();
                for p in &f.sig.type_params {
                    let t = match p.konst.as_deref() {
                        None => self.types.new_struct(p.name.clone()),
                        Some(k) => {
                            let ty = builtin_type(k).unwrap_or(UINT);
                            let v = match sym_const(consts.len()) {
                                Some(v) if ty != BYTE => v,
                                _ => 64,
                            };
                            consts.push(p.name.clone());
                            self.types.intern(TyKind::Const(ty, v))
                        }
                    };
                    tp.insert(p.name.clone(), t);
                }
                self.types.sym_consts = consts;
                let before = self.errors.len();
                if let Ok((params, ret)) = self.resolve_sig(&f.sig, m, &tp) {
                    let id = self.reserve_decl(&f.sig, params, ret);
                    self.check_nested_body(PendingFn {
                        func: id,
                        module: m,
                        decl: f.clone(),
                        tparams: tp,
                    });
                }
                self.errors.truncate(before);
                self.types.sym_consts.clear();
                self.ix.index.editor_only.push(f.body.span);
            }
        }
    }

    fn types_name(&self, base: &str, args: &[TyId]) -> String {
        format!(
            "{}<{}>",
            base,
            args.iter()
                .map(|a| self.types.show(*a))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }

    fn instantiate_type(
        &mut self,
        m: usize,
        decl: usize,
        args: Vec<TyId>,
        span: Span,
    ) -> Result<TyId> {
        let item = self.mods[m].ast.items[decl].clone();
        let (tparams, name) = match &item {
            ast::Item::Struct(s) => (s.type_params.clone(), s.name.clone()),
            ast::Item::Enum(e) => (e.type_params.clone(), e.name.clone()),
            _ => unreachable!(),
        };
        let (args, tp) = self.bind_generics(&name, &tparams, args, span)?;
        let key = (m, decl, args.clone());
        if let Some(&t) = self.type_insts.get(&key) {
            return Ok(t);
        }
        let display = self.types_name(&name, &args);
        let ty = match &item {
            ast::Item::Struct(s) => {
                let ty = self.types.new_struct(display);
                self.type_insts.insert(key, ty);
                self.fill_struct(ty, s, m, &tp)
                    .map_err(|d| self.in_instance(d, &name, &args))?;
                self.check_finite(ty, span)?;
                ty
            }
            ast::Item::Enum(e) => {
                let ty = self.types.new_enum(display);
                self.type_insts.insert(key, ty);
                self.fill_enum(ty, e, m, &tp)
                    .map_err(|d| self.in_instance(d, &name, &args))?;
                ty
            }
            _ => unreachable!(),
        };
        Ok(ty)
    }

    // ------------------------------------------------------------ types

    /// Look up a symbol in module `m`, or in an imported module when `module` is given.
    pub fn lookup_sym(
        &self,
        m: usize,
        module: Option<&str>,
        name: &str,
        span: Span,
    ) -> Result<Option<(usize, Sym)>> {
        match module {
            Some(alias) => {
                self.ix
                    .used_aliases
                    .borrow_mut()
                    .insert((m, alias.to_string()));
                let Some(&target) = self.mods[m].imports.get(alias) else {
                    return err(span, format!("unknown module `{alias}`"));
                };
                match self.mods[target].syms.get(name) {
                    Some(e) if e.public => Ok(Some((target, e.sym.clone()))),
                    Some(_) => err(span, format!("`{name}` is not public in module `{alias}`")),
                    None => err(span, format!("module `{alias}` has no member `{name}`")),
                }
            }
            None => Ok(self.mods[m].syms.get(name).map(|e| (m, e.sym.clone()))),
        }
    }

    pub fn resolve_type(
        &mut self,
        te: &ast::TypeExpr,
        m: usize,
        tp: &HashMap<String, TyId>,
    ) -> Result<TyId> {
        use ast::TypeExpr as T;
        Ok(match te {
            T::Named {
                module,
                name,
                args,
                span,
            } => {
                if module.is_none() {
                    if let Some(t) = tp.get(name) {
                        if let TyKind::Const(..) = self.types.kind(*t) {
                            return err(*span, format!("`{name}` is a constant, not a type"));
                        }
                        if !args.is_empty() {
                            return err(*span, "type parameters cannot take type arguments");
                        }
                        return Ok(*t);
                    }
                    if let Some(b) = builtin_type(name) {
                        if !args.is_empty() {
                            return err(*span, format!("`{name}` does not take type arguments"));
                        }
                        return Ok(b);
                    }
                }
                let targs = args
                    .iter()
                    .map(|a| self.resolve_targ(a, m, tp))
                    .collect::<Result<Vec<_>>>()?;
                let resolved = match self.lookup_sym(m, module.as_deref(), name, *span)? {
                    Some((dm, Sym::Struct { decl, ty } | Sym::Enum { decl, ty })) => {
                        if let Some(t) = ty {
                            if !targs.is_empty() {
                                return err(
                                    *span,
                                    format!("`{name}` does not take type arguments"),
                                );
                            }
                            t
                        } else {
                            if targs.is_empty() {
                                return err(
                                    *span,
                                    format!("generic type `{name}` requires type arguments"),
                                );
                            }
                            self.instantiate_type(dm, decl, targs, *span)?
                        }
                    }
                    Some((_, Sym::Type(t))) => {
                        if !targs.is_empty() {
                            return err(*span, format!("`{name}` does not take type arguments"));
                        }
                        t
                    }
                    Some(_) => return err(*span, format!("`{name}` is not a type")),
                    None => return err(*span, format!("unknown type `{name}`")),
                };
                self.idx_type_ref(resolved, *span);
                resolved
            }
            T::Array(e, n, sp) => {
                let et = self.resolve_type(e, m, tp)?;
                self.check_elem_type(et, *sp)?;
                match n {
                    None => self.types.array(et),
                    Some(n) => {
                        let v = self.const_value(n, tp)?;
                        if v < 0 || v > u32::MAX as i128 {
                            return err(
                                n.span(),
                                format!(
                                    "array size `{}` is {v}, which is not a valid size",
                                    n.show()
                                ),
                            );
                        }
                        self.types.intern(TyKind::Fixed(et, v as u64))
                    }
                }
            }
            T::Map(k, v, sp) => {
                let kt = self.resolve_type(k, m, tp)?;
                let vt = self.resolve_type(v, m, tp)?;
                self.check_elem_type(vt, *sp)?;
                if !self.plain(kt) {
                    return err(
                        *sp,
                        format!("`{}` cannot be used as a map key", self.types.show(kt)),
                    );
                }
                self.types.intern(TyKind::Map(kt, vt))
            }
            T::Optional(i, sp) => {
                let it = self.resolve_type(i, m, tp)?;
                if matches!(
                    self.types.kind(it),
                    TyKind::Void | TyKind::Never | TyKind::ErrU(_) | TyKind::Mutex(_)
                ) {
                    return err(*sp, format!("`{}` cannot be optional", self.types.show(it)));
                }
                self.types.opt(it)
            }
            T::ErrorUnion(i, sp) => {
                let it = self.resolve_type(i, m, tp)?;
                if matches!(
                    self.types.kind(it),
                    TyKind::Never | TyKind::ErrU(_) | TyKind::Mutex(_)
                ) {
                    return err(
                        *sp,
                        format!("`{}` cannot be an error union", self.types.show(it)),
                    );
                }
                self.types.erru(it)
            }
            T::Tuple(ts, sp) => {
                let mut out = Vec::new();
                for t in ts {
                    let x = self.resolve_type(t, m, tp)?;
                    self.check_elem_type(x, *sp)?;
                    out.push(x);
                }
                self.types.tuple(out)
            }
            T::Fn(ps, r, _) => {
                let out = ps
                    .iter()
                    .map(|p| self.resolve_type(p, m, tp))
                    .collect::<Result<_>>()?;
                let r = match r {
                    Some(r) => self.resolve_type(r, m, tp)?,
                    None => VOID,
                };
                self.types.intern(TyKind::Fn(out, r))
            }
            T::Fut(i, sp) => {
                let it = self.resolve_type(i, m, tp)?;
                if matches!(
                    self.types.kind(it),
                    TyKind::Fut(_) | TyKind::Never | TyKind::Mutex(_)
                ) {
                    return err(
                        *sp,
                        format!("invalid future type `fut {}`", self.types.show(it)),
                    );
                }
                self.types.intern(TyKind::Fut(it))
            }
            T::Const(c) => {
                return err(
                    c.span(),
                    format!("`{}` is a constant, not a type", c.show()),
                );
            }
        })
    }

    fn check_elem_type(&self, t: TyId, sp: Span) -> Result<()> {
        if matches!(
            self.types.kind(t),
            TyKind::Void | TyKind::Never | TyKind::Mutex(_) | TyKind::Fut(_)
        ) {
            return err(
                sp,
                format!("`{}` cannot be stored in a container", self.types.show(t)),
            );
        }
        Ok(())
    }

    /// Types supporting `==` and conversion to `str` (printing, format strings): those with no
    /// functions, futures, mutexes, `void` or `never` (except as the value of `!void`).
    pub fn plain(&self, t: TyId) -> bool {
        self.plain_in(t, &mut vec![])
    }

    fn plain_in(&self, t: TyId, seen: &mut Vec<TyId>) -> bool {
        if seen.contains(&t) {
            return true;
        }
        let ty = &self.types;
        match ty.kind(t) {
            TyKind::Fn(..) | TyKind::Fut(_) | TyKind::Mutex(_) | TyKind::Void | TyKind::Never => {
                false
            }
            TyKind::ErrU(e) => *e == VOID || self.plain_in(*e, seen),
            TyKind::Named(_) => self.plain_in(ty.base(t), seen),
            TyKind::Array(e) | TyKind::Fixed(e, _) | TyKind::Opt(e) => self.plain_in(*e, seen),
            TyKind::Map(k, v) => self.plain_in(*k, seen) && self.plain_in(*v, seen),
            TyKind::Tuple(ts) => ts.iter().all(|x| self.plain_in(*x, seen)),
            TyKind::Struct(i) => {
                seen.push(t);
                let r = (ty.structs[*i as usize].fields.iter()).all(|f| self.plain_in(f.1, seen));
                seen.pop();
                r
            }
            TyKind::Enum(i) => {
                seen.push(t);
                let r = (ty.enums[*i as usize].variants.iter())
                    .all(|v| v.1.iter().all(|f| self.plain_in(*f, seen)));
                seen.pop();
                r
            }
            _ => true,
        }
    }

    // ------------------------------------------------------------ scopes

    pub fn ctx(&mut self) -> &mut FnCtx {
        self.fns.last_mut().unwrap()
    }
    pub fn ctx_ref(&self) -> &FnCtx {
        self.fns.last().unwrap()
    }
    pub fn module(&self) -> usize {
        self.ctx_ref().module
    }

    pub fn push_scope(&mut self) {
        self.ctx().scopes.push(vec![]);
    }
    pub fn pop_scope(&mut self) {
        self.ctx().scopes.pop();
    }

    pub fn declare_local(
        &mut self,
        name: &str,
        ty: TyId,
        mutable: bool,
        kind: LocalKind,
        span: Span,
    ) -> LocalId {
        let ctx = self.ctx();
        let id = ctx.locals.len();
        ctx.locals.push(Local {
            name: name.to_string(),
            ty,
            mutable,
            kind,
            boxed: false,
        });
        if name != "_" {
            ctx.scopes.last_mut().unwrap().push((name.to_string(), id));
        }
        let func = ctx.func;
        self.ix.local_spans.insert((func, id), span);
        let scope = self.ix.scopes.last().copied();
        let param = kind == LocalKind::Param;
        self.idx_var_decl(name, span, ty, mutable, param, scope);
        id
    }

    /// Whether a declaration in the current scope should create a global.
    pub fn at_module_level(&self) -> bool {
        let c = self.ctx_ref();
        self.fns.len() == 1 && c.is_main && c.scopes.len() == 1
    }

    pub fn declare_global(
        &mut self,
        name: &str,
        ty: TyId,
        mutable: bool,
        span: Span,
    ) -> Result<GlobalId> {
        let m = self.module();
        let id = self.globals.len();
        self.globals.push(Global {
            name: name.to_string(),
            ty,
            mutable,
        });
        self.ix.global_spans.insert(id, span);
        self.idx_var_decl(name, span, ty, mutable, false, None);
        if name == "_" {
            return Ok(id);
        }
        match self.mods[m].syms.get_mut(name) {
            Some(SymEntry {
                sym: Sym::Globals(list),
                ..
            }) => list.push((id, span.start)),
            Some(_) => return err(span, format!("`{name}` is already defined in this module")),
            None => {
                if self.mods[m].imports.contains_key(name)
                    || self.mods[m].externs.contains_key(name)
                {
                    return err(span, format!("`{name}` is already used as a module alias"));
                }
                self.mods[m].syms.insert(
                    name.to_string(),
                    SymEntry {
                        sym: Sym::Globals(vec![(id, span.start)]),
                        public: false,
                    },
                );
            }
        }
        Ok(id)
    }

    pub fn set_global_public(&mut self, name: &str, public: bool) {
        let m = self.module();
        if let Some(e) = self.mods[m].syms.get_mut(name) {
            e.public = e.public || public;
        }
    }

    /// Resolve a local variable name, capturing through enclosing functions as needed.
    pub fn lookup_local(&mut self, name: &str, span: Span) -> Result<Option<LocalId>> {
        let top = self.fns.len() - 1;
        let mut found = None;
        for d in (0..=top).rev() {
            let ctx = &self.fns[d];
            if let Some(&(_, id)) = ctx
                .scopes
                .iter()
                .rev()
                .flat_map(|s| s.iter().rev())
                .find(|(n, _)| n == name)
            {
                found = Some((d, id));
                break;
            }
        }
        let Some((d, mut id)) = found else {
            return Ok(None);
        };
        if d == top {
            return Ok(Some(id));
        }
        // module-level functions are never nested in another function context
        for k in d + 1..=top {
            let outer = &self.fns[k - 1].locals[id];
            if outer.kind == LocalKind::LockAlias {
                return err(
                    span,
                    format!("`{name}` is locked in an enclosing scope and cannot be captured"),
                );
            }
            // mutable variables are captured by reference
            let (by_ref, oty) = (outer.mutable, outer.ty);
            if by_ref {
                self.fns[k - 1].locals[id].boxed = true;
            }
            let inner = &mut self.fns[k];
            if let Some(&l) = inner.cap_map.get(&id) {
                id = l;
                continue;
            }
            let ci = inner.captures.len();
            inner.captures.push(Capture { outer: id, by_ref });
            let lid = inner.locals.len();
            inner.locals.push(Local {
                name: name.to_string(),
                ty: oty,
                mutable: by_ref,
                kind: if by_ref {
                    LocalKind::CapRef(ci)
                } else {
                    LocalKind::CapVal(ci)
                },
                boxed: false,
            });
            inner.cap_map.insert(id, lid);
            let (of, inf) = (self.fns[k - 1].func, self.fns[k].func);
            if let Some(sp) = self.ix.local_spans.get(&(of, id)).copied() {
                self.ix.local_spans.insert((inf, lid), sp);
            }
            id = lid;
        }
        Ok(Some(id))
    }

    /// Pick the global for a name: the latest declaration at or before `pos`, else the first.
    pub fn pick_global(list: &[(GlobalId, u32)], pos: u32, same_file: bool) -> GlobalId {
        if same_file {
            list.iter()
                .rev()
                .find(|(_, p)| *p <= pos)
                .unwrap_or(&list[0])
                .0
        } else {
            list.last().unwrap().0
        }
    }

    // ------------------------------------------------------------ main assembly

    fn assemble_main(&mut self) -> Block {
        // reachability from the entry module's top level and its tests
        let entry = 0;
        let mut reach_g: HashSet<GlobalId> = HashSet::new();
        let mut reach_f: HashSet<FuncId> = HashSet::new();
        let mut work_f: Vec<FuncId> = Vec::new();
        // globals and functions referenced by statements
        let refs = |stmts: &[Stmt], rg: &mut HashSet<GlobalId>, wf: &mut Vec<FuncId>| {
            let mut fs = HashSet::new();
            for s in stmts {
                flow::refs_stmt(s, rg, &mut fs);
            }
            wf.extend(fs);
        };
        refs(&self.mods[entry].init, &mut reach_g, &mut work_f);
        for t in &self.tests {
            work_f.push(t.func);
        }
        let mut included: Vec<Vec<bool>> = self
            .mods
            .iter()
            .map(|m| vec![false; m.init.len()])
            .collect();
        loop {
            while let Some(f) = work_f.pop() {
                if !reach_f.insert(f) {
                    continue;
                }
                let func = self.funcs[f].as_ref().unwrap();
                refs(&func.body.stmts, &mut reach_g, &mut work_f);
            }
            let mut changed = false;
            for (m, (module, inc)) in self.mods.iter().zip(included.iter_mut()).enumerate() {
                if m == entry {
                    continue;
                }
                for (i, s) in module.init.iter().enumerate() {
                    if inc[i] {
                        continue;
                    }
                    let mut writes = HashSet::new();
                    flow::writes_stmt(s, &mut writes);
                    if writes.iter().any(|g| reach_g.contains(g)) {
                        inc[i] = true;
                        changed = true;
                        refs(std::slice::from_ref(s), &mut reach_g, &mut work_f);
                    }
                }
            }
            if !changed && work_f.is_empty() {
                break;
            }
        }
        let mut stmts = Vec::new();
        for &m in &self.order {
            for (i, s) in self.mods[m].init.iter().enumerate() {
                if m == entry || included[m][i] {
                    stmts.push(s.clone());
                }
            }
        }
        Block { stmts }
    }
}

fn is_never_ret(sig: &ast::FnSig) -> bool {
    matches!(&sig.ret, Some(ast::TypeExpr::Named { module: None, name, args, .. }) if name == "never" && args.is_empty())
}
