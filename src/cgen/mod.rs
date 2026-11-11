//! C backend: lowers the typed IR into a single C23 translation unit.
//!
//! Ownership discipline: every managed value stored in a variable holds one reference.
//! Expressions yield either *borrowed* C expressions or *owned* temporaries that are
//! registered on the cleanup stack and released at the end of the enclosing statement
//! unless moved. Function arguments are borrowed; return values are owned.

mod prune;
mod types;

use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use crate::ast::{BinOp, UnOp};
use crate::check::flow;
use crate::diag::{SourceMap, Span};
use crate::tir::*;
use crate::types::*;
use types::{TypeGen, c_escape_bytes, c_escape_str, c_ident, field_name};

/// The public header: NC's types and the runtime functions C code may call. Extern C files
/// include it as `nc.h`.
pub const HEADER: &str = include_str!("../runtime/nc.h");
/// The runtime implementation, pasted into every generated program after [`HEADER`].
pub const RUNTIME: &str = include_str!("../runtime/nc_impl.h");

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Run,
    Test,
}

struct CVal {
    code: String,
    owned: bool,
}

impl CVal {
    fn b(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            owned: false,
        }
    }
}

#[derive(Clone)]
enum CleanKind {
    /// `rl_<helper id>`
    Release(TyId),
    /// a raw `nc_cell`
    Cell,
    /// a raw `nc_arr`
    Arr,
    Unlock,
}

#[derive(Clone)]
struct Clean {
    var: String,
    kind: CleanKind,
    temp: bool,
    moved: bool,
}

#[derive(Clone)]
struct JumpInfo {
    /// cleanup height to unwind to when breaking
    height: usize,
    brk: String,
    /// for loops: continue label and cleanup height of the body
    cont: Option<(String, usize)>,
    /// result variable for value contexts
    result: Option<(String, TyId)>,
}

pub struct CGen<'p> {
    p: &'p Program,
    sm: &'p SourceMap,
    tg: TypeGen<'p>,
    strs: HashMap<String, String>,
    str_defs: String,
    env_defs: String,
    fut_defs: String,
    funcs_out: String,
    protos: String,
    // per-function state
    out: String,
    ind: usize,
    cleanup: Vec<Clean>,
    tmp: usize,
    func: FuncId,
    jumps: HashMap<JumpId, JumpInfo>,
    dollar: Vec<String>,
    fut_sites: usize,
    /// `main` cannot throw, so it is generated as a plain `void` function
    main_void: bool,
    /// functions used as values (`nc_fn`), which take a closure environment parameter
    value_fns: HashSet<FuncId>,
    /// the program uses `@args`, so `main` records `argc`/`argv`
    uses_args: bool,
    embeds: usize,
}

pub fn generate(p: &Program, sm: &SourceMap, mode: Mode) -> (String, String) {
    let mut g = CGen {
        p,
        sm,
        tg: TypeGen::new(&p.types),
        strs: HashMap::new(),
        str_defs: String::new(),
        env_defs: String::new(),
        fut_defs: String::new(),
        funcs_out: String::new(),
        protos: String::new(),
        out: String::new(),
        ind: 0,
        cleanup: vec![],
        tmp: 0,
        func: 0,
        jumps: HashMap::new(),
        dollar: vec![],
        fut_sites: 0,
        main_void: false,
        value_fns: HashSet::new(),
        uses_args: false,
        embeds: 0,
    };
    g.run(mode)
}

impl CGen<'_> {
    fn run(&mut self, mode: Mode) -> (String, String) {
        self.main_void = mode == Mode::Run && !block_throws(&self.p.funcs[self.p.main].body);
        for f in &self.p.funcs {
            for st in &f.body.stmts {
                flow::visit_stmt(st, &mut |e: &Expr| {
                    if let EK::FnRef(g) | EK::Closure(g) = e.kind {
                        self.value_fns.insert(g);
                    }
                    if matches!(e.kind, EK::Args) {
                        self.uses_args = true;
                    }
                });
            }
        }
        // primitive helpers are always available
        for t in [VOID, BOOL, BYTE, INT, UINT, FLOAT, CHAR, STR, ERROR] {
            self.tg.need(t);
        }
        // signatures
        for (i, f) in self.p.funcs.iter().enumerate() {
            let proto = self.fn_proto(i, f);
            writeln!(self.protos, "{proto};").unwrap();
        }
        for x in &self.p.externs {
            let ps = c_params(x.params.iter().map(|t| self.tg.ct(*t)).collect());
            let rc = self.tg.ret_ct(x.ret);
            writeln!(
                self.protos,
                "{rc} {}({ps}); /* extern {} */",
                x.symbol, x.name
            )
            .unwrap();
            for t in x.params.iter().chain([&x.ret]) {
                self.tg.need(*t);
            }
        }
        // globals
        let mut globals = String::new();
        for (i, gl) in self.p.globals.iter().enumerate() {
            let c = self.ct(gl.ty);
            writeln!(globals, "static {c} {}; /* {} */", self.gname(i), gl.name).unwrap();
        }
        // function bodies
        for i in 0..self.p.funcs.len() {
            self.gen_fn(i, None);
        }
        // test-mode initialiser: only statements writing globals
        let entry = match mode {
            Mode::Run => self.run_main(),
            Mode::Test => {
                let main = &self.p.funcs[self.p.main];
                let stmts = main
                    .body
                    .stmts
                    .iter()
                    .filter(|s| {
                        let mut w = HashSet::new();
                        flow::writes_stmt(s, &mut w);
                        !w.is_empty()
                    })
                    .cloned()
                    .collect();
                let body = Block { stmts };
                let proto = format!("static {} nc_test_init(void)", self.tg.ct(main.ret));
                writeln!(self.protos, "{proto};").unwrap();
                self.gen_fn(self.p.main, Some(("nc_test_init", body)));
                self.test_main()
            }
        };
        let mut out = String::new();
        out.push_str(HEADER);
        out.push_str(RUNTIME);
        let types_section = format!("{}{}", self.tg.decls, self.tg.late_decls);
        out.push_str(&types_section);
        out.push_str(&self.tg.protos);
        out.push('\n');
        out.push_str(&self.tg.bodies);
        out.push_str(&self.str_defs);
        out.push_str(&self.protos);
        out.push_str(&self.env_defs);
        out.push_str(&self.fut_defs);
        out.push('\n');
        out.push_str(&globals);
        out.push('\n');
        out.push_str(&self.funcs_out);
        out.push_str(&entry);
        // header for extern C files
        let header = format!(
            "/* Generated by ncc: NC types for extern implementations. */\n#ifndef NC_GEN_H\n#define NC_GEN_H\n#include \"nc.h\"\n{types_section}\n#endif\n"
        );
        // extern C files may call the runtime's public functions
        let roots = if self.p.externs.is_empty() {
            vec![]
        } else {
            prune::public_api(RUNTIME)
        };
        let pruned = prune::prune(&out, &roots);
        (format!("/* Generated by ncc. */\n{pruned}"), header)
    }

    fn run_main(&mut self) -> String {
        let m = self.p.main;
        let f = self.fname(m);
        let (params, init) = self.main_params();
        if self.main_void {
            return format!("int main({params}) {{\n{init}  {f}();\n  return 0;\n}}\n");
        }
        let rc = self.tg.ct(self.p.funcs[m].ret);
        format!(
            "int main({params}) {{\n{init}  {rc} r = {f}();\n  if (r.err) nc_uncaught(r.err);\n  return 0;\n}}\n"
        )
    }

    /// `main`'s parameters and the code recording them (only when `@args` is used).
    fn main_params(&self) -> (&'static str, &'static str) {
        if self.uses_args {
            (
                "int argc, char **argv",
                "  nc_argc = argc;\n  nc_argv = argv;\n",
            )
        } else {
            ("void", "")
        }
    }

    fn test_main(&mut self) -> String {
        let erru = self.tg.ct(self.p.funcs[self.p.main].ret);
        let mut s = String::new();
        let (params, init) = self.main_params();
        writeln!(
            s,
            "int main({params}) {{\n{init}  nc_test_runner_init();\n  int passed = 0, failed = 0;"
        )
        .unwrap();
        writeln!(
            s,
            "  {{\n    {erru} r = nc_test_init();\n    if (r.err) nc_uncaught(r.err);\n  }}"
        )
        .unwrap();
        for t in &self.p.tests {
            let name = c_escape_str(&t.name);
            writeln!(
                s,
                "  {{\n    jmp_buf jb;\n    nc_test_jmp = &jb;\n    if (setjmp(jb) == 0) {{\n      {erru} r = {}();\n      nc_test_jmp = NULL;\n      fflush(stderr);\n      if (r.err) {{ printf(\"FAIL {name}: %.*s\\n\", (int)r.err->blen, r.err->data); failed++; }}\n      else {{ printf(\"PASS {name}\\n\"); passed++; }}\n    }} else {{\n      nc_test_jmp = NULL;\n      printf(\"FAIL {name}: %s\\n\", nc_test_msg);\n      failed++;\n    }}\n    fflush(stdout);\n  }}",
                self.fname(t.func)
            )
            .unwrap();
        }
        writeln!(
            s,
            "  printf(\"%d passed, %d failed\\n\", passed, failed);\n  return failed ? 1 : 0;\n}}"
        )
        .unwrap();
        s
    }

    // ------------------------------------------------------------ naming

    fn fname(&self, f: FuncId) -> String {
        let name = &self.p.funcs[f].name;
        format!("f{f}_{}", c_ident(name))
    }
    fn gname(&self, g: GlobalId) -> String {
        format!("g{g}_{}", c_ident(&self.p.globals[g].name))
    }
    fn local(&self, l: LocalId) -> &Local {
        &self.p.funcs[self.func].locals[l]
    }
    fn lname(&self, l: LocalId) -> String {
        format!("v{l}_{}", c_ident(&self.local(l).name))
    }
    fn env_name(&self, f: FuncId) -> String {
        format!("nc_env_{}", self.fname(f))
    }

    /// The source location of a span as a C string literal.
    fn loc(&self, span: Span) -> String {
        format!("\"{}\"", c_escape_str(&self.sm.location(span)))
    }

    // ------------------------------------------------------------ output helpers

    fn line(&mut self, s: impl AsRef<str>) {
        for _ in 0..self.ind {
            self.out.push_str("  ");
        }
        self.out.push_str(s.as_ref());
        self.out.push('\n');
    }

    fn fresh(&mut self, prefix: &str) -> String {
        self.tmp += 1;
        format!("{prefix}{}", self.tmp)
    }

    fn managed(&self, t: TyId) -> bool {
        self.p.types.is_managed(t)
    }
    fn h(&self, t: TyId) -> TyId {
        self.p.types.base(t)
    }

    /// The C type of `t`, making sure it is declared.
    fn ct(&mut self, t: TyId) -> String {
        self.tg.need(t);
        self.tg.ct(t)
    }

    fn push_clean(&mut self, var: String, kind: CleanKind, temp: bool) {
        self.cleanup.push(Clean {
            var,
            kind,
            temp,
            moved: false,
        });
    }

    fn var_ty(&self, v: VarRef) -> TyId {
        match v {
            VarRef::Local(l) => self.local(l).ty,
            VarRef::Global(g) => self.p.globals[g].ty,
        }
    }

    /// Declare a temporary holding an owned value.
    fn temp(&mut self, ty: TyId, init: &str) -> CVal {
        let c = self.ct(ty);
        let v = self.fresh("t");
        self.line(format!("{c} {v} = {init};"));
        self.owned_temp(v, ty)
    }

    /// An owned value: in a temporary when it needs releasing, otherwise as is.
    fn owned(&mut self, ty: TyId, code: String) -> CVal {
        if self.managed(ty) {
            self.temp(ty, &code)
        } else {
            CVal::b(code)
        }
    }

    /// The temporary variable `v`, released at the end of the statement if `ty` is managed.
    fn owned_temp(&mut self, v: String, ty: TyId) -> CVal {
        if !self.managed(ty) {
            return CVal::b(v);
        }
        self.push_clean(v.clone(), CleanKind::Release(self.h(ty)), true);
        CVal {
            code: v,
            owned: true,
        }
    }

    /// Materialise a value into a plain (non-owning) variable so it can be referenced repeatedly.
    fn bind(&mut self, ty: TyId, v: &CVal) -> String {
        if is_simple(&v.code) {
            return v.code.clone();
        }
        let c = self.tg.ct(ty);
        let n = self.fresh("b");
        self.line(format!("{c} {n} = {};", v.code));
        n
    }

    /// Obtain an owned reference to the value (moving temporaries, retaining borrows).
    fn own(&mut self, v: CVal, ty: TyId) -> String {
        if !self.managed(ty) {
            return v.code;
        }
        if v.owned
            && let Some(c) = self
                .cleanup
                .iter_mut()
                .rev()
                .find(|c| c.var == v.code && c.temp && !c.moved)
        {
            c.moved = true;
            return v.code;
        }
        format!("rt_{}({})", self.h(ty), v.code)
    }

    /// Emit releases for all live entries above `height` (without popping them).
    fn unwind_to(&mut self, height: usize) {
        let codes: Vec<String> = (self.cleanup[height..].iter().rev())
            .filter(|c| !c.moved)
            .map(release_code)
            .collect();
        for code in codes {
            self.line(code);
        }
    }

    /// Release and pop temporaries above `height`, keeping variables.
    fn end_stmt(&mut self, height: usize) {
        let entries = self.cleanup.split_off(height);
        for c in entries.iter().rev().filter(|c| c.temp && !c.moved) {
            self.line(release_code(c));
        }
        self.cleanup.extend(entries.into_iter().filter(|c| !c.temp));
    }

    /// Release and pop everything above `height` (end of scope).
    fn end_scope(&mut self, height: usize) {
        self.unwind_to(height);
        self.cleanup.truncate(height);
    }

    // ------------------------------------------------------------ functions

    /// Return type of a function as generated (`main` may be generated as `void`).
    fn fn_ret(&self, i: FuncId) -> TyId {
        if i == self.p.main && self.main_void {
            VOID
        } else {
            self.p.funcs[i].ret
        }
    }

    fn fn_proto(&mut self, i: FuncId, f: &Func) -> String {
        let ret = self.fn_ret(i);
        let rc = self.tg.ret_ct(ret);
        self.tg.need(ret);
        let mut ps = if self.value_fns.contains(&i) {
            vec!["nc_env *env".to_string()]
        } else {
            vec![]
        };
        self.func = i;
        for &p in &f.params {
            let t = f.locals[p].ty;
            let c = self.ct(t);
            ps.push(format!("{c} {}", self.lname(p)));
        }
        format!("static {rc} {}({})", self.fname(i), c_params(ps))
    }

    fn gen_fn(&mut self, i: FuncId, init_body: Option<(&str, Block)>) {
        let f = &self.p.funcs[i];
        self.func = i;
        self.out.clear();
        self.ind = 1;
        self.cleanup.clear();
        self.jumps.clear();
        self.dollar.clear();
        let mut proto = self.fn_proto(i, f);
        let is_init = init_body.is_some();
        let body = match init_body {
            Some((name, b)) => {
                proto = proto.replacen(&self.fname(i), name, 1);
                b
            }
            None => f.body.clone(),
        };
        // environment struct
        if !f.captures.is_empty() && !is_init {
            let en = self.env_name(i);
            let mut s = format!("struct {en} {{\n  nc_env h;\n");
            let mut drop = format!(
                "static void {en}_drop(nc_env *e_) {{\n  struct {en} *e = (struct {en} *)e_;\n"
            );
            for (ci, cap) in f.captures.iter().enumerate() {
                let parent = f.parent.unwrap();
                let oty = self.p.funcs[parent].locals[cap.outer].ty;
                self.tg.need(oty);
                if cap.by_ref {
                    writeln!(
                        s,
                        "  nc_cell *c{ci}; /* {} */",
                        self.p.funcs[parent].locals[cap.outer].name
                    )
                    .unwrap();
                    writeln!(drop, "  nc_cell_release(e->c{ci});").unwrap();
                } else {
                    let c = self.tg.ct(oty);
                    writeln!(
                        s,
                        "  {c} c{ci}; /* {} */",
                        self.p.funcs[parent].locals[cap.outer].name
                    )
                    .unwrap();
                    if self.managed(oty) {
                        writeln!(drop, "  rl_{}(e->c{ci});", self.h(oty)).unwrap();
                    }
                }
            }
            s.push_str("};\n");
            drop.push_str("}\n");
            self.env_defs.push_str(&s);
            self.env_defs.push_str(&drop);
            self.line(format!("struct {en} *E = (struct {en} *)env;"));
        }
        if self.value_fns.contains(&i) && !is_init {
            self.line("(void)env;");
        }
        if let Some(sl) = f.self_local {
            let n = self.lname(sl);
            let fname = self.fname(i);
            self.line(format!("nc_fn {n} = {{(void *){fname}, env}};"));
            self.line(format!("(void){n};"));
        }
        let ret = if is_init { f.ret } else { self.fn_ret(i) };
        let h = self.cleanup.len();
        self.block_stmts(&body);
        self.end_scope(h);
        // implicit return
        match self.p.types.kind(ret) {
            TyKind::Void => {}
            TyKind::ErrU(_) => {
                let c = self.tg.ct(ret);
                self.line(format!("return ({c}){{0}};"));
            }
            _ => self.line("unreachable();"),
        }
        let body_text = std::mem::take(&mut self.out);
        let name = if is_init {
            "test initialiser".to_string()
        } else {
            self.p.funcs[i].name.clone()
        };
        writeln!(
            self.funcs_out,
            "/* {} */\n{proto} {{\n{body_text}}}\n",
            name.replace("*/", "* /")
        )
        .unwrap();
    }

    // ------------------------------------------------------------ variables

    fn var_access(&mut self, v: VarRef) -> String {
        match v {
            VarRef::Global(g) => self.gname(g),
            VarRef::Local(l) => {
                let loc = self.local(l).clone();
                let n = self.lname(l);
                let c = self.tg.ct(loc.ty);
                match loc.kind {
                    LocalKind::Normal | LocalKind::Param => {
                        if loc.boxed {
                            format!("(*({c} *){n}->val)")
                        } else {
                            n
                        }
                    }
                    LocalKind::CapVal(i) => format!("E->c{i}"),
                    LocalKind::CapRef(i) => format!("(*({c} *)E->c{i}->val)"),
                    LocalKind::LockAlias => format!("(*{n})"),
                }
            }
        }
    }

    /// Declare a new variable for a `let` target and initialise it with an owned value.
    fn declare(&mut self, v: VarRef, value: String) {
        match v {
            VarRef::Global(g) => {
                let n = self.gname(g);
                let ty = self.p.globals[g].ty;
                if self.managed(ty) {
                    let h = self.h(ty);
                    // re-running an initialiser releases the previous value
                    let c = self.tg.ct(ty);
                    self.line(format!("{{ {c} old = {n}; {n} = {value}; rl_{h}(old); }}"));
                } else {
                    self.line(format!("{n} = {value};"));
                }
            }
            VarRef::Local(l) => {
                let loc = self.local(l).clone();
                let n = self.lname(l);
                let c = self.ct(loc.ty);
                if loc.boxed {
                    let h = self.h(loc.ty);
                    self.line(format!("nc_cell *{n} = nc_cell_new(&ti_{h});"));
                    self.line(format!("*({c} *){n}->val = {value};"));
                    self.push_clean(n, CleanKind::Cell, false);
                } else {
                    self.line(format!("{c} {n} = {value};"));
                    self.line(format!("(void){n};"));
                    if self.managed(loc.ty) {
                        self.push_clean(n, CleanKind::Release(self.h(loc.ty)), false);
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------ statements

    fn block_stmts(&mut self, b: &Block) {
        for s in &b.stmts {
            self.stmt(s);
        }
    }

    fn block(&mut self, b: &Block) {
        self.line("{");
        self.ind += 1;
        let h = self.cleanup.len();
        self.block_stmts(b);
        self.end_scope(h);
        self.ind -= 1;
        self.line("}");
    }

    fn stmt(&mut self, s: &Stmt) {
        let h = self.cleanup.len();
        match &s.kind {
            SK::Let(v, e) => {
                let val = self.expr(e);
                if let Some(var) = *v
                    && e.ty != NEVER
                {
                    if let TyKind::Mutex(inner) = *self.p.types.kind(self.var_ty(var))
                        && !matches!(self.p.types.kind(e.ty), TyKind::Mutex(_))
                    {
                        let o = self.own(val, inner);
                        let ih = self.h(inner);
                        let ic = self.ct(inner);
                        let m = self.fresh("m");
                        self.line(format!("nc_mutex *{m} = nc_mutex_new(&ti_{ih});"));
                        self.line(format!("*({ic} *){m}->val = {o};"));
                        self.declare(var, m);
                    } else {
                        let o = self.own(val, e.ty);
                        self.declare(var, o);
                    }
                }
            }
            SK::Destructure(targets, e) => {
                let val = self.expr(e);
                let tv = self.bind(e.ty, &val);
                let elems = match self.p.types.kind(e.ty) {
                    TyKind::Tuple(ts) => ts.clone(),
                    _ => unreachable!(),
                };
                for (var, start, count) in targets {
                    let Some(var) = var else { continue };
                    let code = if *count == 1 {
                        let t = elems[*start];
                        self.own(CVal::b(format!("{tv}.f{start}")), t)
                    } else {
                        let sub: Vec<TyId> = elems[*start..*start + *count].to_vec();
                        let sty = self.var_ty(*var);
                        let mut parts = Vec::new();
                        for (k, t) in sub.iter().enumerate() {
                            parts.push(self.own(CVal::b(format!("{tv}.f{}", start + k)), *t));
                        }
                        let c = self.ct(sty);
                        format!("({c}){{{}}}", parts.join(", "))
                    };
                    self.declare(*var, code);
                }
            }
            SK::Assign(place, e) => self.assign(place, e),
            SK::Expr(e) => {
                let v = self.expr(e);
                if !v.owned
                    && !v.code.is_empty()
                    && e.ty != VOID
                    && e.ty != NEVER
                    && !is_simple(&v.code)
                {
                    self.line(format!("(void)({});", v.code));
                }
            }
            SK::Return(v) => {
                let fret = self.p.funcs[self.func].ret;
                if let Some(e) = v {
                    let val = self.expr(e);
                    let o = self.own(val, e.ty);
                    let rc = self.tg.ct(fret);
                    let r = self.fresh("r");
                    if let TyKind::ErrU(_) = self.p.types.kind(fret) {
                        self.line(format!("{rc} {r} = ({rc}){{NULL, {o}}};"));
                    } else {
                        self.line(format!("{rc} {r} = {o};"));
                    }
                    self.unwind_to(0);
                    self.line(format!("return {r};"));
                } else {
                    self.unwind_to(0);
                    if let TyKind::ErrU(_) = self.p.types.kind(fret) {
                        let rc = self.tg.ct(fret);
                        self.line(format!("return ({rc}){{0}};"));
                    } else {
                        self.line("return;");
                    }
                }
            }
            SK::Throw(e) => {
                let val = self.expr(e);
                let o = self.own(val, STR);
                self.return_err(&o);
            }
            SK::Break(id, v) => {
                let j = self.jumps[id].clone();
                if let Some(e) = v {
                    let (rv, rty) = j.result.clone().unwrap();
                    let val = self.expr(e);
                    if e.ty != NEVER {
                        self.store(Some(&rv), val, rty);
                    }
                }
                self.unwind_to(j.height);
                self.line(format!("goto {};", j.brk));
            }
            SK::Continue(id) => {
                let j = self.jumps[id].clone();
                let (lbl, height) = j.cont.unwrap();
                self.unwind_to(height);
                self.line(format!("goto {lbl};"));
            }
            SK::While { id, cond, body } => {
                self.line("for (;;) {");
                self.ind += 1;
                let hh = self.cleanup.len();
                self.line("{");
                self.ind += 1;
                let c = self.expr(cond);
                let cv = self.fresh("c");
                self.line(format!("bool {cv} = {};", c.code));
                self.end_stmt(hh);
                self.line(format!("if (!{cv}) break;"));
                self.ind -= 1;
                self.line("}");
                let (brk, cont) = self.jump(*id, h, Some(hh));
                self.block(body);
                self.line(format!("{cont}:;"));
                self.ind -= 1;
                self.line("}");
                self.line(format!("{brk}:;"));
            }
            SK::For {
                id,
                var,
                kind,
                iter,
                body,
            } => {
                self.line("{");
                self.ind += 1;
                let it = self.expr(iter);
                // snapshot of the iterated value
                let snap = self.own(it, iter.ty);
                let sv = self.fresh("it");
                let ic = self.tg.ct(iter.ty);
                let ih = self.h(iter.ty);
                self.line(format!("{ic} {sv} = {snap};"));
                self.push_clean(sv.clone(), CleanKind::Release(ih), false);
                let n = self.fresh("n");
                let i = self.fresh("i");
                // the loop variable's type and value
                let (vc, vval) = match kind {
                    ForKind::Index => {
                        let len = if self.p.types.base(iter.ty) == STR {
                            "clen"
                        } else {
                            "len"
                        };
                        self.line(format!("uint64_t {n} = (uint64_t){sv}->{len};"));
                        ("uint64_t".to_string(), i.clone())
                    }
                    ForKind::Keys => {
                        let kt = self.local(*var).ty;
                        let kh = self.h(kt);
                        let kc = self.ct(kt);
                        let keys = self.fresh("keys");
                        self.line(format!("nc_arr *{keys} = nc_map_keys({sv}, &ti_{kh});"));
                        self.push_clean(keys.clone(), CleanKind::Arr, false);
                        self.line(format!("uint64_t {n} = (uint64_t){keys}->len;"));
                        let val = format!("(({kc} *){keys}->data)[{i}]");
                        (kc, val)
                    }
                };
                self.line(format!("for (uint64_t {i} = 0; {i} < {n}; {i}++) {{"));
                self.ind += 1;
                let vn = self.lname(*var);
                self.line(format!("{vc} {vn} = {vval};"));
                self.line(format!("(void){vn};"));
                let (brk, cont) = self.jump(*id, h, Some(self.cleanup.len()));
                self.block(body);
                self.line(format!("{cont}:;"));
                self.ind -= 1;
                self.line("}");
                self.end_scope(h);
                self.ind -= 1;
                self.line("}");
                self.line(format!("{brk}:;"));
            }
            SK::Lock {
                id,
                mutex,
                alias,
                body,
            } => {
                let inner = self.local(*alias).ty;
                let ic = self.tg.ct(inner);
                self.line("{");
                self.ind += 1;
                let m = self.expr(mutex);
                let mv = self.fresh("mx");
                self.line(format!("nc_mutex *{mv} = {};", m.code));
                self.line(format!("pthread_mutex_lock(&{mv}->mu);"));
                self.push_clean(mv.clone(), CleanKind::Unlock, false);
                let an = self.lname(*alias);
                self.line(format!("{ic} *{an} = ({ic} *){mv}->val;"));
                let (brk, _) = self.jump(*id, h, None);
                self.block(body);
                self.end_scope(h);
                self.ind -= 1;
                self.line("}");
                self.line(format!("{brk}:;"));
            }
            SK::Block(b) => self.block(b),
            SK::Assert(e) => {
                let c = self.expr(e);
                let loc = self.loc(s.span);
                self.line(format!("if (!({})) nc_assert_fail({loc});", c.code));
            }
        }
        self.end_stmt(h);
    }

    /// Store `v` (of type `ty`) in the result variable `r`, if there is one.
    fn store(&mut self, r: Option<&str>, v: CVal, ty: TyId) {
        if let Some(r) = r {
            let o = self.own(v, ty);
            self.line(format!("{r} = {o};"));
        }
    }

    /// Return the error `err` (an owned string) from the current function.
    fn return_err(&mut self, err: &str) {
        let er = self.fresh("err");
        self.line(format!("nc_str *{er} = {err};"));
        self.unwind_to(0);
        let rc = self.tg.ct(self.p.funcs[self.func].ret);
        self.line(format!("return ({rc}){{.err = {er}}};"));
    }

    /// Register a loop or `lock` as a jump target, with the cleanup heights to unwind to when
    /// breaking and (for loops) continuing; returns its break and continue labels.
    fn jump(&mut self, id: JumpId, height: usize, cont: Option<usize>) -> (String, String) {
        let (brk, cont_lbl) = (format!("brk_{id}"), format!("cont_{id}"));
        let info = JumpInfo {
            height,
            brk: brk.clone(),
            cont: cont.map(|h| (cont_lbl.clone(), h)),
            result: None,
        };
        self.jumps.insert(id, info);
        (brk, cont_lbl)
    }

    // ------------------------------------------------------------ assignment

    fn assign(&mut self, place: &Place, e: &Expr) {
        // string element assignment
        if let Place::Index(base, idx) = place {
            let bty = self.place_ty(base);
            if self.p.types.base(bty) == STR {
                let val = self.expr(e);
                let cv = self.bind(CHAR, &val);
                let p = self.place_ptr(base);
                let i = self.index_value(idx);
                let loc = self.loc(e.span);
                self.line(format!("nc_str_set({p}, {i}, {cv}, {loc});"));
                return;
            }
        }
        let ty = self.place_ty(place);
        if e.ty == NEVER {
            self.expr(e);
            return;
        }
        let val = self.expr(e);
        let o = self.own(val, ty);
        let c = self.ct(ty);
        let nv = self.fresh("nv");
        self.line(format!("{c} {nv} = {o};"));
        let p = self.place_ptr(place);
        if self.managed(ty) {
            let h = self.h(ty);
            self.line(format!(
                "{{ {c} *p_ = {p}; {c} old_ = *p_; *p_ = {nv}; rl_{h}(old_); }}"
            ));
        } else {
            self.line(format!("*{p} = {nv};"));
        }
    }

    fn place_ty(&self, p: &Place) -> TyId {
        match p {
            Place::Var(v) => self.var_ty(*v),
            Place::Field(b, i) => {
                let t = self.place_ty(b);
                self.p.types.struct_def(t).unwrap().fields[*i].1
            }
            Place::TupleIndex(b, i) => match self.p.types.kind(self.place_ty(b)) {
                TyKind::Tuple(ts) => ts[*i],
                _ => unreachable!(),
            },
            Place::Index(b, _) => {
                let t = self.place_ty(b);
                match self.p.types.kind(t) {
                    TyKind::Map(_, v) => *v,
                    _ => self.p.types.elem(t).unwrap(),
                }
            }
        }
    }

    /// C expression for a pointer to the place's storage, making containers unique on the way.
    fn place_ptr(&mut self, p: &Place) -> String {
        match p {
            Place::Var(v) => format!("&{}", self.var_access(*v)),
            Place::Field(b, i) => {
                let t = self.place_ty(b);
                let fname = field_name(&self.p.types.struct_def(t).unwrap().fields[*i].0);
                let bp = self.place_ptr(b);
                format!("&({bp})->{fname}")
            }
            Place::TupleIndex(b, i) => {
                let bp = self.place_ptr(b);
                format!("&({bp})->f{i}")
            }
            Place::Index(b, idx) => {
                let t = self.place_ty(b);
                let et = self.place_ty(p);
                let ec = self.ct(et);
                let bp = self.place_ptr(b);
                let bpv = self.fresh("pp");
                let bc = self.tg.ct(t);
                self.line(format!("{bc} *{bpv} = {bp};"));
                if let TyKind::Map(k, _) = self.p.types.kind(t).clone() {
                    let kv = self.expr(idx);
                    let kc = self.tg.ct(k);
                    let kn = self.fresh("k");
                    self.line(format!("{kc} {kn} = {};", kv.code));
                    format!("(({ec} *)nc_map_slot({bpv}, &{kn}))")
                } else {
                    let i = self.dollar_index(format!("(*{bpv})->len"), idx);
                    let loc = self.loc(idx.span);
                    format!("(({ec} *)nc_arr_slot({bpv}, {i}, {loc}))")
                }
            }
        }
    }

    /// An index into a sequence of length `len`, where `$` is the last index.
    fn dollar_index(&mut self, len: String, idx: &Expr) -> String {
        let loc = self.loc(idx.span);
        self.dollar.push(format!("nc_dollar({len}, {loc})"));
        let i = self.index_value(idx);
        self.dollar.pop();
        i
    }

    /// Owned values of expressions (for aggregate initialisers).
    fn owned_all(&mut self, items: &[Expr]) -> Vec<String> {
        items
            .iter()
            .map(|it| {
                let v = self.expr(it);
                self.own(v, it.ty)
            })
            .collect()
    }

    /// Evaluate an index (int or uint) into a uint64 expression.
    fn index_value(&mut self, idx: &Expr) -> String {
        let v = self.expr(idx);
        if idx.ty == INT {
            let loc = self.loc(idx.span);
            format!("nc_index({}, {loc})", v.code)
        } else {
            v.code
        }
    }

    // ------------------------------------------------------------ expressions

    fn lit_str(&mut self, s: &str) -> String {
        if let Some(n) = self.strs.get(s) {
            return n.clone();
        }
        let n = format!("ls{}", self.strs.len());
        let clen = s.chars().count();
        writeln!(
            self.str_defs,
            "static nc_str {n} = {{-1, {}, {clen}, \"{}\"}};",
            s.len(),
            c_escape_bytes(s.as_bytes())
        )
        .unwrap();
        self.strs.insert(s.to_string(), n.clone());
        n
    }

    fn zero(&mut self, ty: TyId) -> String {
        let c = self.ct(ty);
        format!("({c}){{0}}")
    }

    fn expr(&mut self, e: &Expr) -> CVal {
        let ty = e.ty;
        match &e.kind {
            EK::Lit(l) => CVal::b(match l {
                Lit::Bool(b) => b.to_string(),
                Lit::Byte(b) => format!("(uint8_t){b}"),
                Lit::Int(i) => {
                    if *i == i64::MIN {
                        "INT64_MIN".into()
                    } else {
                        format!("INT64_C({i})")
                    }
                }
                Lit::UInt(u) => format!("UINT64_C({u})"),
                Lit::Float(f) => c_float(*f),
                Lit::Char(c) => format!("(uint32_t){c}"),
                Lit::Str(s) => format!("(&{})", self.lit_str(s)),
            }),
            EK::None => {
                if self.p.types.opt_boxed(ty) {
                    CVal::b("(nc_cell *)NULL")
                } else {
                    CVal::b(self.zero(ty))
                }
            }
            EK::Var(v) => CVal::b(self.var_access(*v)),
            EK::FnRef(f) => CVal::b(format!("(nc_fn){{(void *){}, NULL}}", self.fname(*f))),
            EK::Closure(f) => self.closure(*f),
            EK::Unary(op, a) => {
                let v = self.expr(a);
                let loc = self.loc(e.span);
                let b = self.p.types.base(ty);
                CVal::b(match (op, b) {
                    (UnOp::Neg, INT) => format!("nc_neg_i64({}, {loc})", v.code),
                    (UnOp::Neg, _) => format!("(-{})", v.code),
                    (UnOp::Not, _) => format!("(!{})", v.code),
                    (UnOp::BitNot, BYTE) => format!("((uint8_t)~{})", v.code),
                    (UnOp::BitNot, _) => format!("(~{})", v.code),
                })
            }
            EK::Binary(op, a, b) => self.binary(*op, a, b, ty, e.span),
            EK::Call(c, args) => self.call(c, args, ty),
            EK::Print(to, nl, args) if args.iter().all(|a| lit_text(a).is_some()) => {
                // everything is known at compile time: write the finished text directly
                let mut text: String = args.iter().map(|a| lit_text(a).unwrap()).collect();
                if *nl {
                    text.push('\n');
                }
                if !text.is_empty() {
                    let lit = c_escape_bytes(text.as_bytes());
                    if *to == PrintTo::Stderr {
                        self.line("fflush(stdout);");
                        self.line(format!("fwrite(\"{lit}\", 1, {}, stderr);", text.len()));
                    } else if text.ends_with('\n') && !text.contains('\0') {
                        // `puts` appends the newline itself
                        let body = c_escape_bytes(&text.as_bytes()[..text.len() - 1]);
                        self.line(format!("puts(\"{body}\");"));
                    } else {
                        self.line(format!("fwrite(\"{lit}\", 1, {}, stdout);", text.len()));
                    }
                }
                CVal::b("")
            }
            EK::Print(to, nl, args) => {
                let sb = self.new_sb();
                for a in args {
                    let v = self.expr(a);
                    self.str_into(&sb, a.ty, &v.code);
                }
                self.line(format!(
                    "nc_sb_write(&{sb}, {}, {});",
                    *to == PrintTo::Stderr,
                    nl
                ));
                CVal::b("")
            }
            EK::Cast(x) => self.cast(x, ty, e.span),
            EK::ToStr(x) => {
                let v = self.expr(x);
                self.str_of(x.ty, &v.code)
            }
            EK::Format(parts) => {
                let sb = self.new_sb();
                for p in parts {
                    if let (EK::Lit(Lit::Str(s)), _) = (&p.kind, p.ty) {
                        if !s.is_empty() {
                            self.line(format!(
                                "nc_sb_put(&{sb}, \"{}\", {});",
                                c_escape_bytes(s.as_bytes()),
                                s.len()
                            ));
                        }
                    } else {
                        let v = self.expr(p);
                        self.line(format!("nc_sb_str(&{sb}, {});", v.code));
                    }
                }
                self.temp(STR, &format!("nc_sb_finish(&{sb})"))
            }
            EK::Field(b, i) => {
                let bt = b.ty;
                let v = self.expr(b);
                let fname = field_name(
                    &self
                        .p
                        .types
                        .struct_def(self.p.types.base(bt))
                        .unwrap()
                        .fields[*i]
                        .0,
                );
                CVal::b(format!("({}).{fname}", v.code))
            }
            EK::TupleIndex(b, i) => {
                let v = self.expr(b);
                CVal::b(format!("({}).f{i}", v.code))
            }
            EK::Index(b, idx) => {
                let bv = self.expr(b);
                let bn = self.bind(b.ty, &bv);
                let ec = self.ct(ty);
                match self.p.types.kind(self.p.types.base(b.ty)).clone() {
                    TyKind::Map(k, _) => {
                        let kv = self.expr(idx);
                        let kc = self.tg.ct(k);
                        let kn = self.fresh("k");
                        self.line(format!("{kc} {kn} = {};", kv.code));
                        let loc = self.loc(e.span);
                        CVal::b(format!("(*({ec} *)nc_map_get({bn}, &{kn}, {loc}))"))
                    }
                    TyKind::Str => {
                        let i = self.dollar_index(format!("{bn}->clen"), idx);
                        let loc = self.loc(e.span);
                        CVal::b(format!("nc_str_at({bn}, {i}, {loc})"))
                    }
                    _ => {
                        let i = self.dollar_index(format!("{bn}->len"), idx);
                        let loc = self.loc(e.span);
                        CVal::b(format!("(*({ec} *)nc_arr_at({bn}, {i}, {loc}))"))
                    }
                }
            }
            EK::Slice(b, lo, hi) => {
                let bv = self.expr(b);
                let bn = self.bind(b.ty, &bv);
                let is_str = matches!(self.p.types.kind(self.p.types.base(b.ty)), TyKind::Str);
                let len = if is_str { "clen" } else { "len" };
                let loc = self.loc(e.span);
                self.dollar.push(format!("nc_dollar({bn}->{len}, {loc})"));
                let l = self.index_value(lo);
                let h = self.index_value(hi);
                self.dollar.pop();
                let f = if is_str {
                    "nc_str_slice"
                } else {
                    "nc_arr_slice"
                };
                self.temp(ty, &format!("{f}({bn}, {l}, {h}, {loc})"))
            }
            EK::Dollar => CVal::b(self.dollar.last().cloned().unwrap_or_else(|| "0".into())),
            EK::Len(b) => {
                let v = self.expr(b);
                CVal::b(match self.p.types.kind(self.p.types.base(b.ty)) {
                    TyKind::Str => format!("((uint64_t)({})->clen)", v.code),
                    _ => format!("((uint64_t)({})->len)", v.code),
                })
            }
            EK::Tuple(items) | EK::Struct(items) => {
                let mut parts = self.owned_all(items);
                let c = self.ct(ty);
                if parts.is_empty() {
                    parts.push("0".into());
                }
                self.temp(ty, &format!("({c}){{{}}}", parts.join(", ")))
            }
            EK::Array(items) => {
                let et = self.p.types.elem(ty).unwrap();
                let eh = self.h(et);
                let ec = self.ct(et);
                let a = self.temp(ty, &format!("nc_arr_new(&ti_{eh}, {})", items.len()));
                for it in items {
                    let v = self.expr(it);
                    let o = self.own(v, it.ty);
                    self.line(format!(
                        "{{ {ec} el_ = {o}; nc_arr_push(&{}, &el_); }}",
                        a.code
                    ));
                }
                a
            }
            EK::Map(entries) => {
                let (k, v) = match self.p.types.kind(self.p.types.base(ty)) {
                    TyKind::Map(k, v) => (*k, *v),
                    _ => unreachable!(),
                };
                let (kh, vh) = (self.h(k), self.h(v));
                let (kc, vc) = (self.ct(k), self.ct(v));
                let m = self.temp(
                    ty,
                    &format!("nc_map_new(&ti_{kh}, &ti_{vh}, {})", entries.len()),
                );
                for (ke, ve) in entries {
                    let kv = self.expr(ke);
                    let vv = self.expr(ve);
                    let vo = self.own(vv, v);
                    self.line(format!(
                        "{{ {kc} k_ = {}; {vc} v_ = {vo}; nc_map_set(&{}, &k_, &v_); }}",
                        kv.code, m.code
                    ));
                }
                m
            }
            EK::Variant(idx, payload) => {
                if !self.p.types.enum_has_payload(self.p.types.base(ty)) {
                    return CVal::b(format!("UINT32_C({idx})"));
                }
                let parts = self.owned_all(payload);
                self.tg.need(ty);
                let name = self.tg.mangle(ty);
                let v = self.temp(ty, &format!("nc_alloc(sizeof(nc_{name}))"));
                self.line(format!("atomic_init(&{}->rc, 1);", v.code));
                self.line(format!("{}->tag = {idx};", v.code));
                for (k, p) in parts.iter().enumerate() {
                    self.line(format!("{}->u.v{idx}.p{k} = {p};", v.code));
                }
                v
            }
            EK::Some(x) => {
                if x.ty == NEVER {
                    self.expr(x);
                    return CVal::b(self.zero(ty));
                }
                let v = self.expr(x);
                let o = self.own(v, x.ty);
                if self.p.types.opt_boxed(self.p.types.base(ty)) {
                    let ih = self.h(x.ty);
                    let ic = self.ct(x.ty);
                    let c = self.temp(ty, &format!("nc_cell_new(&ti_{ih})"));
                    self.line(format!("*({ic} *){}->val = {o};", c.code));
                    c
                } else {
                    let c = self.ct(ty);
                    self.owned(ty, format!("({c}){{true, {o}}}"))
                }
            }
            EK::Retype(x) => self.expr(x),
            EK::If(i) => self.if_expr(i, ty),
            EK::Else(o, fb) => self.else_expr(o, fb, ty),
            EK::Catch(x, local, b, id) => self.catch_expr(x, *local, b, *id, ty),
            EK::Try(x) => {
                let v = self.expr(x);
                let rv = self.bind(x.ty, &v);
                self.line(format!("if ({rv}.err) {{"));
                self.ind += 1;
                self.return_err(&format!("nc_str_rt({rv}.err)"));
                self.ind -= 1;
                self.line("}");
                if ty == VOID {
                    return CVal::b("(nc_void)0");
                }
                CVal::b(format!("{rv}.val"))
            }
            EK::Async(c, args) => self.async_call(c, args, ty),
            EK::Args => {
                self.tg.need(STR);
                self.temp(ty, &format!("nc_args(&ti_{STR})"))
            }
            EK::Env => {
                self.tg.need(STR);
                self.temp(ty, &format!("nc_environ(&ti_{STR})"))
            }
            EK::Target => {
                let (os, cpu) = self
                    .p
                    .target
                    .clone()
                    .expect("target is detected before code generation");
                let (os, cpu) = (self.lit_str(&os), self.lit_str(&cpu));
                let c = self.ct(ty);
                CVal::b(format!("(({c}){{&{os}, &{cpu}}})"))
            }
            EK::Embed(bytes, path) => {
                // an immortal array over the file's bytes, embedded with C23 `#embed`
                self.tg.need(BYTE);
                let n = self.embeds;
                self.embeds += 1;
                let len = bytes.len();
                if len == 0 {
                    writeln!(
                        self.str_defs,
                        "static nc_arr nc_embed{n} = {{-1, 0, 0, &ti_{BYTE}, NULL}};"
                    )
                    .unwrap();
                } else {
                    let p = c_escape_str(&path.display().to_string());
                    writeln!(
                        self.str_defs,
                        "static const uint8_t nc_embed{n}_data[] = {{\n#embed \"{p}\"\n}};\nstatic nc_arr nc_embed{n} = {{-1, {len}, {len}, &ti_{BYTE}, (void *)nc_embed{n}_data}};"
                    )
                    .unwrap();
                }
                CVal::b(format!("(&nc_embed{n})"))
            }
            EK::Await(x) => {
                let v = self.expr(x);
                let c = self.ct(ty);
                let r = format!("(*({c} *)nc_fut_await({}))", v.code);
                if ty == VOID {
                    self.line(format!("(void){r};"));
                    return CVal::b("(nc_void)0");
                }
                let o = self.own(CVal::b(r), ty);
                self.owned(ty, o)
            }
        }
    }

    /// Declare a new string builder.
    fn new_sb(&mut self) -> String {
        let sb = self.fresh("sb");
        self.line(format!("nc_sb {sb} = nc_sb_new();"));
        sb
    }

    /// The string form of a value of type `ty`.
    fn str_of(&mut self, ty: TyId, code: &str) -> CVal {
        let sb = self.new_sb();
        self.str_into(&sb, ty, code);
        self.temp(STR, &format!("nc_sb_finish(&{sb})"))
    }

    /// Append the string form of a value to a string builder.
    fn str_into(&mut self, sb: &str, ty: TyId, code: &str) {
        self.tg.need(ty);
        let h = self.h(ty);
        self.line(format!("st_{h}(&{sb}, {code});"));
    }

    fn closure(&mut self, f: FuncId) -> CVal {
        let func = &self.p.funcs[f];
        let fname = self.fname(f);
        if func.captures.is_empty() {
            return CVal::b(format!("(nc_fn){{(void *){fname}, NULL}}"));
        }
        let en = self.env_name(f);
        let ev = self.fresh("env");
        self.line(format!(
            "struct {en} *{ev} = nc_alloc(sizeof(struct {en}));"
        ));
        self.line(format!("atomic_init(&{ev}->h.rc, 1);"));
        self.line(format!("{ev}->h.drop = {en}_drop;"));
        for (ci, cap) in func.captures.clone().iter().enumerate() {
            let outer = self.local(cap.outer).clone();
            if cap.by_ref {
                let cell = match outer.kind {
                    LocalKind::CapRef(j) => format!("E->c{j}"),
                    _ => self.lname(cap.outer),
                };
                self.line(format!("{ev}->c{ci} = nc_cell_rt({cell});"));
            } else {
                let acc = self.var_access(VarRef::Local(cap.outer));
                let o = self.own(CVal::b(acc), outer.ty);
                self.line(format!("{ev}->c{ci} = {o};"));
            }
        }
        let fnty = self.fn_type_of(f);
        self.temp(fnty, &format!("(nc_fn){{(void *){fname}, &{ev}->h}}"))
    }

    fn fn_type_of(&self, f: FuncId) -> TyId {
        let k = TyKind::Fn(self.param_tys(f), self.p.funcs[f].ret);
        self.p.types.lookup(&k).expect("function type is interned")
    }

    fn param_tys(&self, f: FuncId) -> Vec<TyId> {
        let func = &self.p.funcs[f];
        func.params.iter().map(|p| func.locals[*p].ty).collect()
    }

    fn callee_params(&self, c: &Callee) -> Vec<TyId> {
        match c {
            Callee::Direct(f) => self.param_tys(*f),
            Callee::Extern(x) => self.p.externs[*x].params.clone(),
            Callee::Value(f) => match self.p.types.kind(f.ty) {
                TyKind::Fn(ps, _) => ps.clone(),
                _ => unreachable!(),
            },
        }
    }

    /// The C call of `c` with `args`; `fv` is the C value of a `Callee::Value`.
    fn call_code(&mut self, c: &Callee, fv: &str, args: Vec<String>) -> String {
        match c {
            Callee::Direct(f) => {
                let mut all = if self.value_fns.contains(f) {
                    vec!["NULL".to_string()]
                } else {
                    vec![]
                };
                all.extend(args);
                format!("{}({})", self.fname(*f), all.join(", "))
            }
            Callee::Extern(x) => format!("{}({})", self.p.externs[*x].symbol, args.join(", ")),
            Callee::Value(f) => {
                self.tg.need(f.ty);
                let fp = self.tg.fp_type(f.ty);
                let mut all = vec![format!("{fv}.env")];
                all.extend(args);
                format!("(({fp}){fv}.fp)({})", all.join(", "))
            }
        }
    }

    fn call(&mut self, c: &Callee, args: &[Expr], ty: TyId) -> CVal {
        let fv = match c {
            Callee::Value(f) => {
                let v = self.expr(f);
                self.bind(f.ty, &v)
            }
            _ => String::new(),
        };
        let mut avs = Vec::new();
        for (a, pt) in args.iter().zip(self.callee_params(c)) {
            let v = self.expr(a);
            avs.push(if a.ty == NEVER { self.zero(pt) } else { v.code });
        }
        let call = self.call_code(c, &fv, avs);
        if ty == NEVER {
            self.line(format!("{call};"));
            self.line("unreachable();");
            return CVal::b("0");
        }
        if ty == VOID {
            self.line(format!("{call};"));
            return CVal::b("(nc_void)0");
        }
        self.temp(ty, &call)
    }

    fn async_call(&mut self, c: &Callee, args: &[Expr], ty: TyId) -> CVal {
        let TyKind::Fut(rt) = *self.p.types.kind(ty) else {
            unreachable!()
        };
        self.fut_sites += 1;
        let sname = format!("nc_fut_site{}", self.fut_sites);
        let params = self.callee_params(c);
        let rc = self.ct(rt);
        // struct definition
        let mut def = format!("struct {sname} {{\n  nc_fut h;\n  nc_fn fn;\n");
        for (i, p) in params.iter().enumerate() {
            let pc = self.ct(*p);
            writeln!(def, "  {pc} a{i};").unwrap();
        }
        writeln!(def, "  {rc} result;\n}};").unwrap();
        // run function
        let call_args = (0..params.len()).map(|i| format!("s->a{i}")).collect();
        let call = self.call_code(c, "s->fn", call_args);
        let store = if rt == VOID {
            format!("{call};")
        } else {
            format!("s->result = {call};")
        };
        writeln!(def, "static void {sname}_run(nc_fut *f) {{\n  struct {sname} *s = (struct {sname} *)f;\n  {store}\n}}").unwrap();
        let mut drop = format!(
            "static void {sname}_drop(nc_fut *f) {{\n  struct {sname} *s = (struct {sname} *)f;\n  nc_fn_release(s->fn);\n"
        );
        for (i, p) in params.iter().enumerate() {
            if self.managed(*p) {
                writeln!(drop, "  rl_{}(s->a{i});", self.h(*p)).unwrap();
            }
        }
        if self.managed(rt) {
            writeln!(drop, "  if (s->h.state == 2) rl_{}(s->result);", self.h(rt)).unwrap();
        }
        drop.push_str("}\n");
        self.fut_defs.push_str(&def);
        self.fut_defs.push_str(&drop);
        // call site
        let fnv = match c {
            Callee::Value(f) => {
                let v = self.expr(f);
                self.own(v, f.ty)
            }
            _ => "(nc_fn){NULL, NULL}".to_string(),
        };
        let sv = self.fresh("fs");
        self.line(format!(
            "struct {sname} *{sv} = nc_zalloc(sizeof(struct {sname}));"
        ));
        self.line(format!(
            "nc_fut_init(&{sv}->h, {sname}_run, {sname}_drop, &{sv}->result);"
        ));
        self.line(format!("{sv}->fn = {fnv};"));
        for (i, a) in args.iter().enumerate() {
            let v = self.expr(a);
            let o = self.own(v, a.ty);
            self.line(format!("{sv}->a{i} = {o};"));
        }
        self.line(format!("nc_fut_submit(&{sv}->h);"));
        self.temp(ty, &format!("&{sv}->h"))
    }

    fn binary(&mut self, op: BinOp, a: &Expr, b: &Expr, ty: TyId, span: Span) -> CVal {
        use BinOp::*;
        if matches!(op, And | Or) {
            let av = self.expr(a);
            let r = self.fresh("l");
            self.line(format!("bool {r} = {};", av.code));
            if op == And {
                self.line(format!("if ({r}) {{"));
            } else {
                self.line(format!("if (!{r}) {{"));
            }
            self.ind += 1;
            let h = self.cleanup.len();
            let bv = self.expr(b);
            self.line(format!("{r} = {};", bv.code));
            self.end_stmt(h);
            self.ind -= 1;
            self.line("}");
            return CVal::b(r);
        }
        let av = self.expr(a);
        let bv = self.expr(b);
        let (x, y) = (av.code, bv.code);
        let ot = self.p.types.base(a.ty);
        let loc = self.loc(span);
        let s = match ot {
            INT => "i64",
            UINT => "u64",
            BYTE => "u8",
            _ => "",
        };
        let code = match op {
            // checked integer arithmetic: `nc_add_i64` and so on
            Add | Sub | Mul | Div | Mod | Pow | Shl | Shr if !s.is_empty() => {
                let name = format!("{op:?}").to_lowercase();
                format!("nc_{name}_{s}({x}, {y}, {loc})")
            }
            Pow => format!("pow({x}, {y})"),
            Mod | Shl | Shr | And | Or => unreachable!(),
            Add | Sub | Mul | Div | BitAnd | BitOr | BitXor | Lt | Gt | Le | Ge => {
                format!("({x} {} {y})", op.symbol())
            }
            Eq | Ne => {
                let eq = if self.is_prim(a.ty) {
                    format!("({x} == {y})")
                } else {
                    self.tg.need(a.ty);
                    format!("eq_{}({x}, {y})", self.h(a.ty))
                };
                if op == Ne { format!("(!{eq})") } else { eq }
            }
            Concat => {
                let f = match self.p.types.kind(ot) {
                    TyKind::Str => "nc_str_cat",
                    TyKind::Map(..) => "nc_map_cat",
                    _ => "nc_arr_cat",
                };
                return self.temp(ty, &format!("{f}({x}, {y})"));
            }
            In => {
                let bt = self.p.types.base(b.ty);
                match self.p.types.kind(bt).clone() {
                    TyKind::Str => {
                        if a.ty == CHAR {
                            format!("nc_str_has_char({y}, {x})")
                        } else {
                            format!("nc_str_has_str({y}, {x})")
                        }
                    }
                    kind => {
                        let f = match kind {
                            TyKind::Map(..) => "nc_map_has",
                            _ => "nc_arr_has",
                        };
                        let c = self.tg.ct(a.ty);
                        let k = self.fresh("k");
                        self.line(format!("{c} {k} = {x};"));
                        format!("{f}({y}, &{k})")
                    }
                }
            }
        };
        CVal::b(code)
    }

    fn is_prim(&self, t: TyId) -> bool {
        let b = self.p.types.base(t);
        matches!(b, BOOL | BYTE | INT | UINT | FLOAT | CHAR)
            || matches!(self.p.types.kind(b), TyKind::Enum(_)) && !self.p.types.enum_has_payload(b)
    }

    fn cast(&mut self, x: &Expr, to: TyId, span: Span) -> CVal {
        let v = self.expr(x);
        let from = self.p.types.base(x.ty);
        let loc = self.loc(span);
        let tk = self.p.types.kind(to).clone();
        if to == STR {
            return self.str_of(x.ty, &v.code);
        }
        let c = &v.code;
        match (from, &tk) {
            (BOOL, TyKind::Int) => CVal::b(format!("(int64_t)({c} ? 1 : 0)")),
            (BOOL, TyKind::UInt) => CVal::b(format!("(uint64_t)({c} ? 1 : 0)")),
            (BYTE, TyKind::Char) => CVal::b(format!("(uint32_t){c}")),
            (BYTE, TyKind::Int) => CVal::b(format!("(int64_t){c}")),
            (BYTE, TyKind::UInt) => CVal::b(format!("(uint64_t){c}")),
            (INT, TyKind::UInt) => CVal::b(format!("nc_i64_to_u64({c}, {loc})")),
            (INT | UINT, TyKind::Float) => CVal::b(format!("(double){c}")),
            (UINT, TyKind::Int) => CVal::b(format!("nc_u64_to_i64({c}, {loc})")),
            (FLOAT, TyKind::Int) => CVal::b(format!("nc_f64_to_i64({c}, {loc})")),
            (FLOAT, TyKind::UInt) => CVal::b(format!("nc_f64_to_u64({c}, {loc})")),
            (CHAR, TyKind::Array(_)) => self.temp(to, &format!("nc_char_bytes({c}, &ti_{BYTE})")),
            (INT, TyKind::Array(_)) => {
                self.temp(to, &format!("nc_u64_le_bytes((uint64_t){c}, &ti_{BYTE})"))
            }
            (UINT, TyKind::Array(_)) => self.temp(to, &format!("nc_u64_le_bytes({c}, &ti_{BYTE})")),
            (FLOAT, TyKind::Array(_)) => {
                let b = self.fresh("fb");
                self.line(format!(
                    "uint64_t {b}; {{ double d_ = {c}; memcpy(&{b}, &d_, 8); }}"
                ));
                self.temp(to, &format!("nc_u64_le_bytes({b}, &ti_{BYTE})"))
            }
            (STR, TyKind::Array(e)) if *e == CHAR => {
                self.temp(to, &format!("nc_str_to_chars({c}, &ti_{CHAR})"))
            }
            (STR, TyKind::Array(_)) => self.temp(
                to,
                &format!("nc_bytes_of({c}->data, {c}->blen, &ti_{BYTE})"),
            ),
            _ => v,
        }
    }

    // ------------------------------------------------------------ control-flow expressions

    fn result_var(&mut self, ty: TyId) -> Option<String> {
        if ty == VOID || ty == NEVER {
            return None;
        }
        let c = self.ct(ty);
        let r = self.fresh("res");
        let z = self.zero(ty);
        self.line(format!("{c} {r} = {z};"));
        Some(r)
    }

    fn finish_result(&mut self, r: Option<String>, ty: TyId) -> CVal {
        match r {
            Some(r) => self.owned_temp(r, ty),
            None => CVal::b("(nc_void)0"),
        }
    }

    /// Register the jump `id` of a value-producing construct (`if`, `else` or `catch` block),
    /// storing its value in `r`; returns its end label.
    fn value_jump(&mut self, id: JumpId, height: usize, r: &Option<String>, ty: TyId) -> String {
        let end = format!("end_{id}");
        self.jumps.insert(
            id,
            JumpInfo {
                height,
                brk: end.clone(),
                cont: None,
                result: r.clone().map(|r| (r, ty)),
            },
        );
        end
    }

    fn if_expr(&mut self, i: &If, ty: TyId) -> CVal {
        let r = self.result_var(ty);
        let h = self.cleanup.len();
        let end = self.value_jump(i.id, h, &r, ty);
        self.line("{");
        self.ind += 1;
        let scrut = match &i.scrutinee {
            Some(s) => {
                let v = self.expr(s);
                Some((self.bind(s.ty, &v), s.ty))
            }
            None => None,
        };
        for arm in &i.arms {
            let next = self.fresh("arm");
            let body_lbl = self.fresh("body");
            self.line("{");
            self.ind += 1;
            let ha = self.cleanup.len();
            let n = arm.patterns.len();
            for (k, p) in arm.patterns.iter().enumerate() {
                let fail = if k + 1 == n {
                    next.clone()
                } else {
                    format!("{body_lbl}_alt{k}")
                };
                self.line("{");
                self.ind += 1;
                let hp = self.cleanup.len();
                match (p, &scrut) {
                    (Pat::Cond(c), _) => {
                        let v = self.expr(c);
                        let cv = self.fresh("c");
                        self.line(format!("bool {cv} = {};", v.code));
                        self.end_stmt(hp);
                        self.line(format!("if (!{cv}) goto {fail};"));
                    }
                    (_, Some((sv, st))) => {
                        let ok = self.fresh("ok");
                        self.line(format!("bool {ok} = true;"));
                        self.pat_test(p, sv, *st, &ok);
                        self.end_stmt(hp);
                        self.line(format!("if (!{ok}) goto {fail};"));
                    }
                    (Pat::Wild, None) => {}
                    _ => unreachable!(),
                }
                self.ind -= 1;
                self.line("}");
                if n > 1 {
                    self.line(format!("goto {body_lbl};"));
                    if k + 1 < n {
                        self.line(format!("{body_lbl}_alt{k}:;"));
                    }
                }
            }
            if n > 1 {
                self.line(format!("{body_lbl}:;"));
            }
            // bindings
            if let Some((sv, st)) = &scrut {
                for p in &arm.patterns {
                    self.pat_bind(p, sv, *st);
                }
            }
            self.block(&arm.body);
            self.end_scope(ha);
            self.line(format!("goto {end};"));
            self.ind -= 1;
            self.line("}");
            self.line(format!("{next}:;"));
        }
        // exhaustive: unreachable, but keep C happy
        self.line("unreachable();");
        self.end_scope(h);
        self.ind -= 1;
        self.line("}");
        self.line(format!("{end}:;"));
        self.finish_result(r, ty)
    }

    /// Emit code clearing `ok` when the pattern does not match the subject `sv`.
    fn pat_test(&mut self, p: &Pat, sv: &str, st: TyId, ok: &str) {
        let st = self.p.types.base(st);
        match p {
            Pat::Wild | Pat::Bind(_) => return,
            Pat::Cond(_) => unreachable!(),
            Pat::Value(e) => {
                self.line(format!("if ({ok}) {{"));
                self.ind += 1;
                let v = self.expr(e);
                let eq = if self.is_prim(st) {
                    format!("({sv} == {})", v.code)
                } else {
                    self.tg.need(st);
                    format!("eq_{st}({sv}, {})", v.code)
                };
                self.line(format!("{ok} = {eq};"));
                self.ind -= 1;
                self.line("}");
                return;
            }
            Pat::Array(ps) if matches!(self.p.types.kind(st), TyKind::Array(_)) => {
                self.line(format!("if ({sv}->len != {}) {ok} = false;", ps.len()));
            }
            Pat::Variant(idx, _) if !self.p.types.enum_has_payload(st) => {
                self.line(format!("if ({sv} != {idx}) {ok} = false;"));
                return;
            }
            Pat::Variant(idx, _) => self.line(format!("if ({sv}->tag != {idx}) {ok} = false;")),
            _ => {}
        }
        for (sp, ssv, sty) in self.pat_parts(p, sv, st) {
            if matches!(sp, Pat::Wild | Pat::Bind(_)) {
                continue;
            }
            self.line(format!("if ({ok}) {{"));
            self.ind += 1;
            self.pat_test(sp, &ssv, sty, ok);
            self.ind -= 1;
            self.line("}");
        }
    }

    fn pat_bind(&mut self, p: &Pat, sv: &str, st: TyId) {
        if let Pat::Bind(l) = p {
            let ty = self.local(*l).ty;
            let o = self.own(CVal::b(sv.to_string()), ty);
            self.declare(VarRef::Local(*l), o);
            return;
        }
        let st = self.p.types.base(st);
        for (sp, ssv, sty) in self.pat_parts(p, sv, st) {
            self.pat_bind(sp, &ssv, sty);
        }
    }

    /// The sub-patterns of a tuple, struct, array or variant pattern on the subject `sv` (of
    /// base type `st`), each with its part of the subject and that part's type.
    fn pat_parts<'a>(&mut self, p: &'a Pat, sv: &str, st: TyId) -> Vec<(&'a Pat, String, TyId)> {
        let types = &self.p.types;
        match p {
            Pat::Tuple(ps) => {
                let TyKind::Tuple(ts) = types.kind(st) else {
                    unreachable!()
                };
                (ps.iter().zip(ts).enumerate())
                    .map(|(k, (sp, t))| (sp, format!("({sv}).f{k}"), *t))
                    .collect()
            }
            Pat::Struct(ps) => (ps.iter().zip(&types.struct_def(st).unwrap().fields))
                .map(|(sp, (name, t))| (sp, format!("({sv}).{}", field_name(name)), *t))
                .collect(),
            Pat::Array(ps) => {
                let et = types.elem(st).unwrap();
                let ec = self.tg.ct(et);
                (ps.iter().enumerate())
                    .map(|(k, sp)| (sp, format!("(({ec} *){sv}->data)[{k}]"), et))
                    .collect()
            }
            Pat::Variant(idx, ps) if !ps.is_empty() => {
                let payload = &types.enum_def(st).unwrap().variants[*idx].1;
                (ps.iter().zip(payload).enumerate())
                    .map(|(k, (sp, t))| (sp, format!("{sv}->u.v{idx}.p{k}"), *t))
                    .collect()
            }
            _ => vec![],
        }
    }

    fn else_expr(&mut self, o: &Expr, fb: &Fallback, ty: TyId) -> CVal {
        let ov = self.expr(o);
        let on = self.bind(o.ty, &ov);
        let r = self.result_var(ty);
        let boxed = self.p.types.opt_boxed(self.p.types.base(o.ty));
        let (has, val) = if boxed {
            let ic = self.tg.ct(ty);
            (format!("({on} != NULL)"), format!("(*({ic} *){on}->val)"))
        } else {
            (format!("{on}.has"), format!("{on}.val"))
        };
        self.line(format!("if ({has}) {{"));
        self.ind += 1;
        self.store(r.as_deref(), CVal::b(val), ty);
        self.ind -= 1;
        self.line("} else {");
        self.ind += 1;
        let h = self.cleanup.len();
        match fb {
            Fallback::Expr(x) => {
                let v = self.expr(x);
                if x.ty != NEVER {
                    self.store(r.as_deref(), v, ty);
                }
                self.end_stmt(h);
            }
            Fallback::Block(b, id) => {
                let end = self.value_jump(*id, h, &r, ty);
                self.block(b);
                self.line(format!("{end}:;"));
            }
        }
        self.ind -= 1;
        self.line("}");
        self.finish_result(r, ty)
    }

    fn catch_expr(
        &mut self,
        x: &Expr,
        local: Option<LocalId>,
        b: &Block,
        id: JumpId,
        ty: TyId,
    ) -> CVal {
        let xv = self.expr(x);
        let xn = self.bind(x.ty, &xv);
        let r = self.result_var(ty);
        self.line(format!("if ({xn}.err) {{"));
        self.ind += 1;
        let h = self.cleanup.len();
        if let Some(l) = local {
            self.declare(VarRef::Local(l), format!("nc_str_rt({xn}.err)"));
        }
        let end = self.value_jump(id, h, &r, ty);
        self.block(b);
        self.end_scope(h);
        self.line(format!("goto {end};"));
        self.ind -= 1;
        self.line("}");
        self.store(r.as_deref(), CVal::b(format!("{xn}.val")), ty);
        self.line(format!("{end}:;"));
        self.finish_result(r, ty)
    }
}

fn is_simple(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn c_float(f: f64) -> String {
    if f.is_nan() {
        return "NAN".into();
    }
    if f.is_infinite() {
        return if f < 0.0 {
            "(-INFINITY)".into()
        } else {
            "INFINITY".into()
        };
    }
    let s = format!("{f:?}");
    if s.contains('.') || s.contains('e') {
        s
    } else {
        format!("{s}.0")
    }
}

/// The printed text of a literal argument, if it is one.
fn lit_text(e: &Expr) -> Option<String> {
    match &e.kind {
        EK::Lit(Lit::Str(s)) => Some(s.clone()),
        EK::Lit(Lit::Int(i)) => Some(i.to_string()),
        EK::Lit(Lit::UInt(u)) => Some(u.to_string()),
        EK::Lit(Lit::Byte(b)) => Some(b.to_string()),
        EK::Lit(Lit::Bool(b)) => Some(b.to_string()),
        EK::Lit(Lit::Char(c)) => char::from_u32(*c).map(|c| c.to_string()),
        EK::Lit(Lit::Float(f)) => Some(crate::eval::fmt_float(*f)),
        EK::Retype(x) => lit_text(x),
        _ => None,
    }
}

/// Whether a block can throw (contains `throw` or `try` outside nested functions).
fn block_throws(b: &Block) -> bool {
    let mut found = false;
    for s in &b.stmts {
        flow::visit_stmts(s, &mut |s| found |= matches!(s.kind, SK::Throw(_)));
        flow::visit_stmt(s, &mut |e| found |= matches!(e.kind, EK::Try(_)));
    }
    found
}

fn release_code(c: &Clean) -> String {
    match c.kind {
        CleanKind::Cell => format!("nc_cell_release({});", c.var),
        CleanKind::Arr => format!("nc_arr_release({});", c.var),
        CleanKind::Release(h) => format!("rl_{h}({});", c.var),
        CleanKind::Unlock => format!("pthread_mutex_unlock(&{}->mu);", c.var),
    }
}

/// A C parameter list: `void` when there are none.
fn c_params(ps: Vec<String>) -> String {
    if ps.is_empty() {
        "void".into()
    } else {
        ps.join(", ")
    }
}
