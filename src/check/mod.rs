//! Semantic analysis: name resolution, type checking, generic instantiation and lowering
//! of the AST into the typed IR.

mod expr;
pub mod flow;
mod pat;
mod stmt;

use std::collections::{HashMap, HashSet};

use crate::ast;
use crate::diag::{err, Diag, Result, Span};
use crate::modules::ModuleGraph;
use crate::tir::*;
use crate::types::*;

#[derive(Clone, Debug)]
pub enum Sym {
    Fn { decl: usize, func: Option<FuncId> },
    Struct { decl: usize, ty: Option<TyId> },
    Enum { decl: usize, ty: Option<TyId> },
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
    pub is_test: bool,
    /// The module top-level context: declarations at scope depth 1 become globals.
    pub is_main: bool,
    pub is_never: bool,
    pub self_local: Option<LocalId>,
    /// `$` contexts: types of the bases of enclosing index expressions
    pub dollar: Vec<TyId>,
}

struct PendingFn {
    func: FuncId,
    module: usize,
    decl: ast::FnDecl,
    tparams: HashMap<String, TyId>,
}

pub struct Checker<'a> {
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
    _p: std::marker::PhantomData<&'a ()>,
}

pub fn check(graph: ModuleGraph) -> Result<Program> {
    let c = Checker {
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
        order: graph.order.clone(),
        _p: std::marker::PhantomData,
    };
    c.run()
}

impl<'a> Checker<'a> {
    pub fn new_jump(&mut self) -> JumpId {
        self.next_jump += 1;
        self.next_jump
    }

    fn run(mut self) -> Result<Program> {
        // Phase A: declare names
        for m in 0..self.mods.len() {
            self.declare_names(m)?;
        }
        // Phase B: resolve type definitions and function signatures
        for m in 0..self.mods.len() {
            self.resolve_decls(m)?;
        }
        // Phase C: top-level statements, module by module in dependency order
        let main_ret = self.types.erru(VOID);
        let main = self.reserve_fn("main".into(), vec![], main_ret);
        let mut main_ctx = self.new_ctx(main, 0, main_ret, HashMap::new());
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
                    self.check_stmt(s, &mut init)?;
                }
            }
            self.mods[m].init = init;
        }
        let main_ctx = self.fns.pop().unwrap();
        // tests (entry module only are run, but all are checked)
        for m in 0..self.mods.len() {
            let items = self.mods[m].ast.items.clone();
            for it in &items {
                if let ast::Item::Test(name, body, span) = it {
                    let erru = self.types.erru(VOID);
                    let f = self.reserve_fn(format!("test {name}"), vec![], erru);
                    let mut ctx = self.new_ctx(f, m, erru, HashMap::new());
                    ctx.is_test = true;
                    self.fns.push(ctx);
                    let body = self.check_block(body)?;
                    let ctx = self.fns.pop().unwrap();
                    self.finish_fn(ctx, format!("test {name}"), vec![], body, *span, None);
                    if m == 0 {
                        self.tests.push(Test { name: name.clone(), func: f });
                    }
                }
            }
        }
        // Phase D: function bodies (including generic instances discovered on the way)
        while let Some(p) = self.pending.pop() {
            self.check_fn_body(p)?;
        }
        // assemble main
        let main_body = self.assemble_main();
        self.finish_fn(main_ctx, "main".into(), vec![], main_body, Span::default(), None);
        let funcs = self.funcs.into_iter().map(|f| f.expect("unfinished function")).collect();
        Ok(Program {
            types: self.types,
            funcs,
            globals: self.globals,
            externs: self.externs,
            main,
            tests: self.tests,
        })
    }

    // ------------------------------------------------------------ declarations

    fn add_sym(&mut self, m: usize, name: &str, sym: Sym, public: bool, span: Span) -> Result<()> {
        if matches!(name, "int" | "uint" | "byte" | "float" | "bool" | "char" | "str" | "void" | "error" | "never") {
            return err(span, format!("`{name}` is a builtin type and cannot be redefined"));
        }
        if self.mods[m].imports.contains_key(name) {
            return err(span, format!("`{name}` is already used as an import alias"));
        }
        if self.mods[m].syms.contains_key(name) {
            return err(span, format!("`{name}` is already defined in this module"));
        }
        self.mods[m].syms.insert(name.to_string(), SymEntry { sym, public });
        Ok(())
    }

    fn declare_names(&mut self, m: usize) -> Result<()> {
        let items = self.mods[m].ast.items.clone();
        let r = (|| {
            for (i, it) in items.iter().enumerate() {
                match it {
                    ast::Item::Fn(f) => {
                        self.add_sym(m, &f.sig.name, Sym::Fn { decl: i, func: None }, f.public, f.sig.span)?
                    }
                    ast::Item::Struct(s) => {
                        let ty = if s.type_params.is_empty() { Some(self.types.new_struct(s.name.clone())) } else { None };
                        self.add_sym(m, &s.name, Sym::Struct { decl: i, ty }, s.public, s.span)?
                    }
                    ast::Item::Enum(e) => {
                        let ty = if e.type_params.is_empty() { Some(self.types.new_enum(e.name.clone())) } else { None };
                        self.add_sym(m, &e.name, Sym::Enum { decl: i, ty }, e.public, e.span)?
                    }
                    ast::Item::Type(t) => {
                        let ty = self.types.new_named(t.name.clone(), VOID);
                        self.add_sym(m, &t.name, Sym::Type(ty), t.public, t.span)?
                    }
                    ast::Item::Extern(e) => {
                        if self.mods[m].externs.contains_key(&e.alias) || self.mods[m].imports.contains_key(&e.alias) {
                            return err(e.span, format!("duplicate alias `{}`", e.alias));
                        }
                        self.mods[m].externs.insert(e.alias.clone(), HashMap::new());
                    }
                    _ => {}
                }
            }
            Ok(())
        })();
        r
    }

    fn resolve_decls(&mut self, m: usize) -> Result<()> {
        let items = self.mods[m].ast.items.clone();
        let r = (|| {
            let none = HashMap::new();
            for it in items.iter() {
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
                        // reject cycles
                        let mut seen = vec![ty];
                        let mut cur = under;
                        while let TyKind::Named(n) = self.types.kind(cur).clone() {
                            if seen.contains(&cur) {
                                return err(t.span, format!("type `{}` is defined in terms of itself", t.name));
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
                    }
                    ast::Item::Enum(e) if e.type_params.is_empty() => {
                        let ty = match self.mods[m].syms[&e.name].sym {
                            Sym::Enum { ty, .. } => ty.unwrap(),
                            _ => unreachable!(),
                        };
                        self.fill_enum(ty, e, m, &none)?;
                    }
                    _ => {}
                }
            }
            for (i, it) in items.iter().enumerate() {
                match it {
                    ast::Item::Fn(f) if f.sig.type_params.is_empty() => {
                        let (params, ret) = self.resolve_sig(&f.sig, m, &none)?;
                        let id = self.reserve_fn(f.sig.name.clone(), params, ret);
                        if is_never_ret(&f.sig) {
                            self.never_funcs.insert(id);
                        }
                        if let Some(SymEntry { sym: Sym::Fn { func, .. }, .. }) = self.mods[m].syms.get_mut(&f.sig.name) {
                            *func = Some(id);
                        }
                        self.pending.push(PendingFn { func: id, module: m, decl: f.clone(), tparams: HashMap::new() });
                        let _ = i;
                    }
                    ast::Item::Extern(e) => {
                        let dir = self.mods[m].path.parent().unwrap().to_path_buf();
                        for f in &e.fns {
                            let (params, ret) = self.resolve_sig(&f.sig, m, &none)?;
                            let id = self.externs.len();
                            self.externs.push(Extern {
                                name: format!("{}.{}", e.alias, f.sig.name),
                                symbol: f.symbol.clone(),
                                params,
                                ret,
                                file: dir.join(&e.path),
                            });
                            let tab = self.mods[m].externs.get_mut(&e.alias).unwrap();
                            if tab.insert(f.sig.name.clone(), id).is_some() {
                                return err(f.sig.span, format!("duplicate extern function `{}`", f.sig.name));
                            }
                        }
                    }
                    _ => {}
                }
            }
            // reject infinitely sized structs
            for it in items.iter() {
                if let ast::Item::Struct(s) = it {
                    if let Some(SymEntry { sym: Sym::Struct { ty: Some(ty), .. }, .. }) = self.mods[m].syms.get(&s.name) {
                        self.check_finite(*ty, s.span)?;
                    }
                }
            }
            Ok(())
        })();
        r
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
                TyKind::Struct(i) => t.structs[*i as usize].fields.iter().any(|(_, x)| walk(t, *x, target, false, seen)),
                // optionals that contain themselves are boxed; arrays, maps and enums are on the heap
                _ => false,
            };
            seen.pop();
            r
        }
        if walk(&self.types, ty, ty, true, &mut vec![]) {
            return err(span, format!("struct `{}` contains itself and would have infinite size", self.types.show(ty)));
        }
        Ok(())
    }

    fn fill_struct(&mut self, ty: TyId, s: &ast::StructDecl, m: usize, tp: &HashMap<String, TyId>) -> Result<()> {
        let mut fields = Vec::new();
        for (fty, name, sp) in &s.fields {
            if fields.iter().any(|(n, _): &(String, TyId)| n == name) {
                return err(*sp, format!("duplicate field `{name}`"));
            }
            let t = self.resolve_type(fty, m, tp)?;
            if matches!(self.types.kind(t), TyKind::Void | TyKind::Never | TyKind::Mutex(_) | TyKind::Fut(_)) {
                return err(*sp, format!("struct fields cannot have type `{}`", self.types.show(t)));
            }
            fields.push((name.clone(), t));
        }
        if let TyKind::Struct(i) = *self.types.kind(ty) {
            self.types.structs[i as usize].fields = fields;
        }
        Ok(())
    }

    fn fill_enum(&mut self, ty: TyId, e: &ast::EnumDecl, m: usize, tp: &HashMap<String, TyId>) -> Result<()> {
        let mut variants = Vec::new();
        for (name, tys, sp) in &e.variants {
            if variants.iter().any(|(n, _): &(String, Vec<TyId>)| n == name) {
                return err(*sp, format!("duplicate enum member `{name}`"));
            }
            let mut ts = Vec::new();
            for t in tys {
                ts.push(self.resolve_type(t, m, tp)?);
            }
            variants.push((name.clone(), ts));
        }
        if let TyKind::Enum(i) = *self.types.kind(ty) {
            self.types.enums[i as usize].variants = variants;
        }
        Ok(())
    }

    pub fn resolve_sig(&mut self, sig: &ast::FnSig, m: usize, tp: &HashMap<String, TyId>) -> Result<(Vec<TyId>, TyId)> {
        let mut params = Vec::new();
        for p in &sig.params {
            let t = self.resolve_type(&p.ty, m, tp)?;
            if matches!(self.types.kind(t), TyKind::Void | TyKind::Never | TyKind::Fut(_)) {
                return err(p.span, format!("parameters cannot have type `{}`", self.types.show(t)));
            }
            params.push(t);
        }
        let ret = match &sig.ret {
            Some(t) => self.resolve_type(t, m, tp)?,
            None => VOID,
        };
        if matches!(self.types.kind(ret), TyKind::Fut(_)) {
            return err(sig.span, "futures cannot be returned from functions");
        }
        if let TyKind::ErrU(i) = self.types.kind(ret) {
            if matches!(self.types.kind(*i), TyKind::Fut(_)) {
                return err(sig.span, "futures cannot be returned from functions");
            }
        }
        Ok((params, ret))
    }

    pub fn reserve_fn(&mut self, _name: String, params: Vec<TyId>, ret: TyId) -> FuncId {
        self.funcs.push(None);
        self.sigs.push((params, ret));
        self.funcs.len() - 1
    }

    pub fn new_ctx(&mut self, func: FuncId, module: usize, ret: TyId, tparams: HashMap<String, TyId>) -> FnCtx {
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
            is_test: false,
            is_main: false,
            is_never: false,
            self_local: None,
            dollar: vec![],
        }
    }

    pub fn finish_fn(
        &mut self,
        ctx: FnCtx,
        name: String,
        params: Vec<LocalId>,
        body: Block,
        span: Span,
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
            span,
            parent,
            self_local: ctx.self_local,
            is_never: ctx.is_never,
        });
    }

    fn check_fn_body(&mut self, p: PendingFn) -> Result<()> {
        let (params, ret) = self.sigs[p.func].clone();
        let mut ctx = self.new_ctx(p.func, p.module, ret, p.tparams);
        ctx.is_never = ret == NEVER;
        self.fns.push(ctx);
        let mut pids = Vec::new();
        for (i, prm) in p.decl.sig.params.iter().enumerate() {
            let id = self.declare_local(&prm.name, params[i], false, LocalKind::Param, prm.span)?;
            pids.push(id);
        }
        let body = self.check_block(&p.decl.body)?;
        self.check_fn_end(&body, ret, p.decl.sig.span, &p.decl.sig.name)?;
        let ctx = self.fns.pop().unwrap();
        let name = self.fn_display_name(&p.decl.sig.name, p.func);
        self.finish_fn(ctx, name, pids, body, p.decl.sig.span, None);
        Ok(())
    }

    fn fn_display_name(&self, base: &str, _f: FuncId) -> String {
        base.to_string()
    }

    pub fn check_fn_end(&mut self, body: &Block, ret: TyId, span: Span, name: &str) -> Result<()> {
        let completes = self.block_completes(body);
        if completes {
            if ret == NEVER {
                return err(span, format!("function `{name}` is declared `never` but can return"));
            }
            let needs = match self.types.kind(ret) {
                TyKind::Void => false,
                TyKind::ErrU(i) => *i != VOID,
                _ => true,
            };
            if needs {
                return err(span, format!("function `{name}` does not return a value on every path"));
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------ generics

    pub fn instantiate_fn(&mut self, m: usize, decl: usize, args: Vec<TyId>, span: Span) -> Result<FuncId> {
        let key = (m, decl, args.clone());
        if let Some(&f) = self.fn_insts.get(&key) {
            return Ok(f);
        }
        let f = match &self.mods[m].ast.items[decl] {
            ast::Item::Fn(f) => f.clone(),
            _ => unreachable!(),
        };
        if f.sig.type_params.len() != args.len() {
            return err(
                span,
                format!(
                    "`{}` expects {} type argument(s), found {}",
                    f.sig.name,
                    f.sig.type_params.len(),
                    args.len()
                ),
            );
        }
        let tp: HashMap<String, TyId> = f.sig.type_params.iter().cloned().zip(args.iter().cloned()).collect();
        let (params, ret) = self.resolve_sig(&f.sig, m, &tp)?;
        let name = format!("{}<{}>", f.sig.name, args.iter().map(|a| self.types.show(*a)).collect::<Vec<_>>().join(", "));
        let id = self.reserve_fn(name, params, ret);
        if is_never_ret(&f.sig) {
            self.never_funcs.insert(id);
        }
        self.fn_insts.insert(key, id);
        // generic bodies are type-checked at instantiation; check right away so that errors
        // point at the instantiation that caused them.
        let saved = std::mem::take(&mut self.fns);
        let mut decl2 = f.clone();
        decl2.sig.name = self.types_name(&f.sig.name, &args);
        let r = self.check_fn_body(PendingFn { func: id, module: m, decl: decl2, tparams: tp });
        self.fns = saved;
        let inst = self.types_name(&f.sig.name, &args);
        r.map_err(|mut d| {
            d.msg = format!("{} (in instantiation `{inst}`)", d.msg);
            d
        })?;
        Ok(id)
    }

    fn types_name(&self, base: &str, args: &[TyId]) -> String {
        format!("{}<{}>", base, args.iter().map(|a| self.types.show(*a)).collect::<Vec<_>>().join(", "))
    }

    fn instantiate_type(&mut self, m: usize, decl: usize, args: Vec<TyId>, span: Span) -> Result<TyId> {
        let key = (m, decl, args.clone());
        if let Some(&t) = self.type_insts.get(&key) {
            return Ok(t);
        }
        let item = self.mods[m].ast.items[decl].clone();
        let (tparams, name) = match &item {
            ast::Item::Struct(s) => (s.type_params.clone(), s.name.clone()),
            ast::Item::Enum(e) => (e.type_params.clone(), e.name.clone()),
            _ => unreachable!(),
        };
        if tparams.len() != args.len() {
            return err(span, format!("`{name}` expects {} type argument(s), found {}", tparams.len(), args.len()));
        }
        let display = self.types_name(&name, &args);
        let tp: HashMap<String, TyId> = tparams.into_iter().zip(args.iter().cloned()).collect();
        let ty = match &item {
            ast::Item::Struct(s) => {
                let ty = self.types.new_struct(display);
                self.type_insts.insert(key, ty);
                self.fill_struct(ty, s, m, &tp)?;
                self.check_finite(ty, span)?;
                ty
            }
            ast::Item::Enum(e) => {
                let ty = self.types.new_enum(display);
                self.type_insts.insert(key, ty);
                self.fill_enum(ty, e, m, &tp)?;
                ty
            }
            _ => unreachable!(),
        };
        Ok(ty)
    }

    // ------------------------------------------------------------ types

    /// Look up a symbol in module `m`, or in an imported module when `module` is given.
    pub fn lookup_sym(&self, m: usize, module: Option<&str>, name: &str, span: Span) -> Result<Option<(usize, Sym)>> {
        match module {
            Some(alias) => {
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

    pub fn resolve_type(&mut self, te: &ast::TypeExpr, m: usize, tp: &HashMap<String, TyId>) -> Result<TyId> {
        use ast::TypeExpr as T;
        Ok(match te {
            T::Named { module, name, args, span } => {
                if module.is_none() {
                    if let Some(t) = tp.get(name) {
                        if !args.is_empty() {
                            return err(*span, "type parameters cannot take type arguments");
                        }
                        return Ok(*t);
                    }
                    let b = match name.as_str() {
                        "void" => Some(VOID),
                        "never" => Some(NEVER),
                        "bool" => Some(BOOL),
                        "byte" => Some(BYTE),
                        "int" => Some(INT),
                        "uint" => Some(UINT),
                        "float" => Some(FLOAT),
                        "char" => Some(CHAR),
                        "str" => Some(STR),
                        "error" => Some(ERROR),
                        _ => None,
                    };
                    if let Some(b) = b {
                        if !args.is_empty() {
                            return err(*span, format!("`{name}` does not take type arguments"));
                        }
                        return Ok(b);
                    }
                }
                let mut targs = Vec::new();
                for a in args {
                    targs.push(self.resolve_type(a, m, tp)?);
                }
                match self.lookup_sym(m, module.as_deref(), name, *span)? {
                    Some((dm, Sym::Struct { decl, ty })) | Some((dm, Sym::Enum { decl, ty })) => match ty {
                        Some(t) => {
                            if !targs.is_empty() {
                                return err(*span, format!("`{name}` does not take type arguments"));
                            }
                            t
                        }
                        None => {
                            if targs.is_empty() {
                                return err(*span, format!("generic type `{name}` requires type arguments"));
                            }
                            self.instantiate_type(dm, decl, targs, *span)?
                        }
                    },
                    Some((_, Sym::Type(t))) => {
                        if !targs.is_empty() {
                            return err(*span, format!("`{name}` does not take type arguments"));
                        }
                        t
                    }
                    Some(_) => return err(*span, format!("`{name}` is not a type")),
                    None => return err(*span, format!("unknown type `{name}`")),
                }
            }
            T::Array(e, n, sp) => {
                let et = self.resolve_type(e, m, tp)?;
                self.check_elem_type(et, *sp)?;
                match n {
                    None => self.types.array(et),
                    Some(n) => self.types.intern(TyKind::Fixed(et, *n)),
                }
            }
            T::Map(k, v, sp) => {
                let kt = self.resolve_type(k, m, tp)?;
                let vt = self.resolve_type(v, m, tp)?;
                self.check_elem_type(vt, *sp)?;
                if !self.comparable(kt) {
                    return err(*sp, format!("`{}` cannot be used as a map key", self.types.show(kt)));
                }
                self.types.intern(TyKind::Map(kt, vt))
            }
            T::Optional(i, sp) => {
                let it = self.resolve_type(i, m, tp)?;
                if matches!(self.types.kind(it), TyKind::Void | TyKind::Never | TyKind::ErrU(_) | TyKind::Mutex(_)) {
                    return err(*sp, format!("`{}` cannot be optional", self.types.show(it)));
                }
                self.types.opt(it)
            }
            T::ErrorUnion(i, sp) => {
                let it = self.resolve_type(i, m, tp)?;
                if matches!(self.types.kind(it), TyKind::Never | TyKind::ErrU(_) | TyKind::Mutex(_)) {
                    return err(*sp, format!("`{}` cannot be an error union", self.types.show(it)));
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
                let mut out = Vec::new();
                for p in ps {
                    out.push(self.resolve_type(p, m, tp)?);
                }
                let r = match r {
                    Some(r) => self.resolve_type(r, m, tp)?,
                    None => VOID,
                };
                self.types.intern(TyKind::Fn(out, r))
            }
            T::Fut(i, sp) => {
                let it = self.resolve_type(i, m, tp)?;
                if matches!(self.types.kind(it), TyKind::Fut(_) | TyKind::Never | TyKind::Mutex(_)) {
                    return err(*sp, format!("invalid future type `fut {}`", self.types.show(it)));
                }
                self.types.intern(TyKind::Fut(it))
            }
        })
    }

    fn check_elem_type(&self, t: TyId, sp: Span) -> Result<()> {
        if matches!(self.types.kind(t), TyKind::Void | TyKind::Never | TyKind::Mutex(_) | TyKind::Fut(_)) {
            return err(sp, format!("`{}` cannot be stored in a container", self.types.show(t)));
        }
        Ok(())
    }

    /// Types supporting `==`.
    pub fn comparable(&self, t: TyId) -> bool {
        self.comparable_guard(t, &mut vec![])
    }

    fn comparable_guard(&self, t: TyId, seen: &mut Vec<TyId>) -> bool {
        if seen.contains(&t) {
            return true;
        }
        match self.types.kind(t) {
            TyKind::Struct(i) => {
                seen.push(t);
                let fs: Vec<TyId> = self.types.structs[*i as usize].fields.iter().map(|f| f.1).collect();
                let r = fs.iter().all(|f| self.comparable_guard(*f, seen));
                seen.pop();
                r
            }
            TyKind::Enum(i) => {
                seen.push(t);
                let vs: Vec<TyId> = self.types.enums[*i as usize].variants.iter().flat_map(|v| v.1.clone()).collect();
                let r = vs.iter().all(|f| self.comparable_guard(*f, seen));
                seen.pop();
                r
            }
            TyKind::Array(e) | TyKind::Fixed(e, _) | TyKind::Opt(e) => self.comparable_guard(*e, seen),
            TyKind::Map(k, v) => self.comparable_guard(*k, seen) && self.comparable_guard(*v, seen),
            TyKind::Tuple(ts) => ts.clone().iter().all(|x| self.comparable_guard(*x, seen)),
            TyKind::Named(_) => self.comparable_guard(self.types.base(t), seen),
            TyKind::ErrU(e) => *e == VOID || self.comparable_guard(*e, seen),
            TyKind::Fn(..) | TyKind::Fut(_) | TyKind::Mutex(_) | TyKind::Void | TyKind::Never => false,
            _ => true,
        }
    }

    /// Types that can be converted to `str` (printing, format strings).
    pub fn printable(&self, t: TyId) -> bool {
        self.printable_guard(t, &mut vec![])
    }
    fn printable_guard(&self, t: TyId, seen: &mut Vec<TyId>) -> bool {
        if seen.contains(&t) {
            return true;
        }
        match self.types.kind(t) {
            TyKind::Fn(..) | TyKind::Fut(_) | TyKind::Mutex(_) | TyKind::Void | TyKind::Never => false,
            TyKind::ErrU(e) => *e == VOID || self.printable_guard(*e, seen),
            TyKind::Named(_) => self.printable_guard(self.types.base(t), seen),
            TyKind::Array(e) | TyKind::Fixed(e, _) | TyKind::Opt(e) => self.printable_guard(*e, seen),
            TyKind::Map(k, v) => self.printable_guard(*k, seen) && self.printable_guard(*v, seen),
            TyKind::Tuple(ts) => ts.clone().iter().all(|x| self.printable_guard(*x, seen)),
            TyKind::Struct(i) => {
                seen.push(t);
                let fs: Vec<TyId> = self.types.structs[*i as usize].fields.iter().map(|f| f.1).collect();
                let r = fs.iter().all(|f| self.printable_guard(*f, seen));
                seen.pop();
                r
            }
            TyKind::Enum(i) => {
                seen.push(t);
                let vs: Vec<TyId> = self.types.enums[*i as usize].variants.iter().flat_map(|v| v.1.clone()).collect();
                let r = vs.iter().all(|f| self.printable_guard(*f, seen));
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

    pub fn declare_local(&mut self, name: &str, ty: TyId, mutable: bool, kind: LocalKind, _span: Span) -> Result<LocalId> {
        let ctx = self.ctx();
        let id = ctx.locals.len();
        ctx.locals.push(Local { name: name.to_string(), ty, mutable, kind, boxed: false });
        if name != "_" {
            ctx.scopes.last_mut().unwrap().push((name.to_string(), id));
        }
        Ok(id)
    }

    /// Whether a declaration in the current scope should create a global.
    pub fn at_module_level(&self) -> bool {
        let c = self.ctx_ref();
        self.fns.len() == 1 && c.is_main && c.scopes.len() == 1
    }

    pub fn declare_global(&mut self, name: &str, ty: TyId, mutable: bool, span: Span) -> Result<GlobalId> {
        let m = self.module();
        let id = self.globals.len();
        self.globals.push(Global { name: name.to_string(), ty, mutable, module: m });
        if name == "_" {
            return Ok(id);
        }
        match self.mods[m].syms.get_mut(name) {
            Some(SymEntry { sym: Sym::Globals(list), .. }) => list.push((id, span.start)),
            Some(_) => return err(span, format!("`{name}` is already defined in this module")),
            None => {
                if self.mods[m].imports.contains_key(name) || self.mods[m].externs.contains_key(name) {
                    return err(span, format!("`{name}` is already used as a module alias"));
                }
                self.mods[m]
                    .syms
                    .insert(name.to_string(), SymEntry { sym: Sym::Globals(vec![(id, span.start)]), public: false });
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
            if let Some(&(_, id)) = ctx.scopes.iter().rev().flat_map(|s| s.iter().rev()).find(|(n, _)| n == name) {
                found = Some((d, id));
                break;
            }
            if ctx.is_main && d == 0 && !ctx.scopes.is_empty() {
                // main's locals are visible to nested functions declared in main
            }
        }
        let Some((d, mut id)) = found else { return Ok(None) };
        if d == top {
            return Ok(Some(id));
        }
        // module-level functions are never nested in another function context
        for k in d + 1..=top {
            let outer = &self.fns[k - 1].locals[id];
            if outer.kind == LocalKind::LockAlias {
                return err(span, format!("`{name}` is locked in an enclosing scope and cannot be captured"));
            }
            let by_ref = outer.mutable;
            let oty = outer.ty;
            let omut = outer.mutable;
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
                mutable: omut,
                kind: if by_ref { LocalKind::CapRef(ci) } else { LocalKind::CapVal(ci) },
                boxed: false,
            });
            inner.cap_map.insert(id, lid);
            id = lid;
        }
        Ok(Some(id))
    }

    /// Pick the global for a name: the latest declaration at or before `pos`, else the first.
    pub fn pick_global(list: &[(GlobalId, u32)], pos: u32, same_file: bool) -> GlobalId {
        if same_file {
            if let Some((g, _)) = list.iter().rev().find(|(_, p)| *p <= pos) {
                return *g;
            }
        }
        if same_file { list[0].0 } else { list.last().unwrap().0 }
    }

    // ------------------------------------------------------------ main assembly

    fn assemble_main(&mut self) -> Block {
        // reachability from the entry module's top level and its tests
        let entry = 0;
        let mut reach_g: HashSet<GlobalId> = HashSet::new();
        let mut reach_f: HashSet<FuncId> = HashSet::new();
        let mut work_f: Vec<FuncId> = Vec::new();
        let collect = |stmts: &[Stmt], rg: &mut HashSet<GlobalId>, wf: &mut Vec<FuncId>| {
            let mut g = HashSet::new();
            let mut f = HashSet::new();
            for s in stmts {
                flow::refs_stmt(s, &mut g, &mut f);
            }
            rg.extend(g);
            wf.extend(f);
        };
        let entry_init = self.mods[entry].init.clone();
        collect(&entry_init, &mut reach_g, &mut work_f);
        for t in &self.tests {
            work_f.push(t.func);
        }
        let mut included: Vec<Vec<bool>> = self.mods.iter().map(|m| vec![false; m.init.len()]).collect();
        loop {
            while let Some(f) = work_f.pop() {
                if !reach_f.insert(f) {
                    continue;
                }
                let func = self.funcs[f].as_ref().unwrap();
                let mut g = HashSet::new();
                let mut fs = HashSet::new();
                for s in &func.body.stmts {
                    flow::refs_stmt(s, &mut g, &mut fs);
                }
                reach_g.extend(g);
                work_f.extend(fs);
            }
            let mut changed = false;
            for m in 0..self.mods.len() {
                if m == entry {
                    continue;
                }
                for (i, s) in self.mods[m].init.iter().enumerate() {
                    if included[m][i] {
                        continue;
                    }
                    let mut writes = HashSet::new();
                    flow::writes_stmt(s, &mut writes);
                    if writes.iter().any(|g| reach_g.contains(g)) {
                        included[m][i] = true;
                        changed = true;
                        let mut g = HashSet::new();
                        let mut fs = HashSet::new();
                        flow::refs_stmt(s, &mut g, &mut fs);
                        reach_g.extend(g);
                        work_f.extend(fs);
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
