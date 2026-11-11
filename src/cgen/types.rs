//! C representations of NC types and their helper functions
//! (retain / release / equality / hashing / string conversion).

use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use crate::types::*;

pub struct TypeGen<'t> {
    pub t: &'t Types,
    /// forward declarations and struct definitions, in dependency order
    pub decls: String,
    /// enum payload definitions (heap allocated, so they can come last)
    pub late_decls: String,
    /// helper prototypes
    pub protos: String,
    /// helper definitions
    pub bodies: String,
    done: HashSet<TyId>,
    names: HashMap<TyId, String>,
    used_names: HashSet<String>,
}

fn ident(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_end_matches('_').to_string()
}

impl<'t> TypeGen<'t> {
    pub fn new(t: &'t Types) -> Self {
        TypeGen {
            t,
            decls: String::new(),
            late_decls: String::new(),
            protos: String::new(),
            bodies: String::new(),
            done: HashSet::new(),
            names: HashMap::new(),
            used_names: HashSet::new(),
        }
    }

    /// Helper id: nominal types share their underlying type's helpers.
    pub fn hid(&self, t: TyId) -> TyId {
        self.t.base(t)
    }

    pub fn managed(&self, t: TyId) -> bool {
        self.t.is_managed(t)
    }

    /// Readable mangled name for a type, unique within the program.
    pub fn mangle(&mut self, t: TyId) -> String {
        let t = self.hid(t);
        if let Some(n) = self.names.get(&t) {
            return n.clone();
        }
        let base = match self.t.kind(t).clone() {
            TyKind::Void
            | TyKind::Never
            | TyKind::Bool
            | TyKind::Byte
            | TyKind::Int
            | TyKind::UInt
            | TyKind::Float
            | TyKind::Char
            | TyKind::Str
            | TyKind::Error
            | TyKind::Struct(_)
            | TyKind::Enum(_) => ident(&self.t.show(t)),
            TyKind::Array(e) => format!("arr_{}", self.mangle(e)),
            TyKind::Fixed(e, n) => format!("arr{n}_{}", self.mangle(e)),
            TyKind::Map(k, v) => format!("map_{}_{}", self.mangle(k), self.mangle(v)),
            TyKind::Opt(i) => format!("opt_{}", self.mangle(i)),
            TyKind::ErrU(i) => format!("err_{}", self.mangle(i)),
            TyKind::Tuple(ts) => {
                let parts: Vec<String> = ts.iter().map(|x| self.mangle(*x)).collect();
                format!("tup_{}", parts.join("_"))
            }
            TyKind::Fn(ps, r) => {
                let mut parts: Vec<String> = ps.iter().map(|x| self.mangle(*x)).collect();
                parts.push(self.mangle(r));
                format!("fn_{}", parts.join("_"))
            }
            TyKind::Fut(i) => format!("fut_{}", self.mangle(i)),
            TyKind::Mutex(i) => format!("mutex_{}", self.mangle(i)),
            TyKind::Named(_) => unreachable!(),
            TyKind::Const(_, v) if v < 0 => format!("m{}", -v),
            TyKind::Const(_, v) => v.to_string(),
        };
        let mut name = base;
        if name.len() > 60 {
            name = format!("{}_{t}", &name[..40]);
        }
        if self.used_names.contains(&name) {
            name = format!("{name}_{t}");
        }
        self.used_names.insert(name.clone());
        self.names.insert(t, name.clone());
        name
    }

    /// The C type used for values of `t`.
    pub fn ct(&mut self, t: TyId) -> String {
        let t = self.hid(t);
        match self.t.kind(t).clone() {
            TyKind::Void => "nc_void".into(),
            TyKind::Never => "void".into(),
            TyKind::Bool => "bool".into(),
            TyKind::Byte => "uint8_t".into(),
            TyKind::Int => "int64_t".into(),
            TyKind::UInt => "uint64_t".into(),
            TyKind::Float => "double".into(),
            TyKind::Char => "uint32_t".into(),
            TyKind::Str | TyKind::Error => "nc_str *".into(),
            TyKind::Array(_) | TyKind::Fixed(..) => "nc_arr *".into(),
            TyKind::Map(..) => "nc_map *".into(),
            TyKind::Opt(_) if self.t.opt_boxed(t) => "nc_cell *".into(),
            TyKind::Opt(_) | TyKind::ErrU(_) | TyKind::Tuple(_) | TyKind::Struct(_) => {
                format!("nc_{}", self.mangle(t))
            }
            TyKind::Enum(_) if !self.t.enum_has_payload(t) => "uint32_t".into(),
            TyKind::Enum(_) => format!("nc_{} *", self.mangle(t)),
            TyKind::Fn(..) => "nc_fn".into(),
            TyKind::Fut(_) => "nc_fut *".into(),
            TyKind::Mutex(_) => "nc_mutex *".into(),
            TyKind::Named(_) | TyKind::Const(..) => unreachable!(),
        }
    }

    /// C function return type for an NC return type.
    pub fn ret_ct(&mut self, t: TyId) -> String {
        if t == VOID || t == NEVER {
            "void".into()
        } else {
            self.ct(t)
        }
    }

    pub fn fp_type(&mut self, t: TyId) -> String {
        format!("nc_fp_{}", self.mangle(t))
    }

    /// Ensure the C definition and helpers for `t` exist.
    pub fn need(&mut self, t: TyId) {
        let t = self.hid(t);
        if t == NEVER || !self.done.insert(t) {
            return;
        }
        let kind = self.t.kind(t).clone();
        let name = self.mangle(t);
        // forward declarations for aggregate types
        match &kind {
            TyKind::Opt(_) if self.t.opt_boxed(t) => {}
            TyKind::Opt(_) | TyKind::ErrU(_) | TyKind::Tuple(_) | TyKind::Struct(_) => {
                writeln!(self.decls, "typedef struct nc_{name} nc_{name};").unwrap();
            }
            TyKind::Enum(_) if self.t.enum_has_payload(t) => {
                writeln!(self.decls, "typedef struct nc_{name} nc_{name};").unwrap();
            }
            _ => {}
        }
        // components
        let comps: Vec<TyId> = match &kind {
            TyKind::Array(e)
            | TyKind::Fixed(e, _)
            | TyKind::Opt(e)
            | TyKind::ErrU(e)
            | TyKind::Fut(e)
            | TyKind::Mutex(e) => vec![*e],
            TyKind::Map(k, v) => vec![*k, *v],
            TyKind::Tuple(ts) => ts.clone(),
            TyKind::Fn(ps, r) => {
                let mut v = ps.clone();
                v.push(*r);
                v
            }
            TyKind::Struct(i) => self.t.structs[*i as usize]
                .fields
                .iter()
                .map(|f| f.1)
                .collect(),
            TyKind::Enum(i) => self.t.enums[*i as usize]
                .variants
                .iter()
                .flat_map(|v| v.1.clone())
                .collect(),
            _ => vec![],
        };
        for c in &comps {
            self.need(*c);
        }
        let ct = self.ct(t);
        // definitions
        match &kind {
            // `nc.h` defines the optionals and error unions of built-in types
            TyKind::Opt(i) | TyKind::ErrU(i) if in_header(&kind, *i, &name) => {}
            TyKind::Opt(i) if !self.t.opt_boxed(t) => {
                let ic = self.ct(*i);
                writeln!(
                    self.decls,
                    "struct nc_{name} {{\n  bool has;\n  {ic} val;\n}};"
                )
                .unwrap();
            }
            TyKind::ErrU(i) => {
                let ic = self.ct(*i);
                writeln!(
                    self.decls,
                    "struct nc_{name} {{\n  nc_str *err; /* NULL on success */\n  {ic} val;\n}};"
                )
                .unwrap();
            }
            TyKind::Tuple(ts) => {
                let mut s = format!("struct nc_{name} {{\n");
                for (i, x) in ts.iter().enumerate() {
                    let xc = self.ct(*x);
                    writeln!(s, "  {xc} f{i};").unwrap();
                }
                s.push_str("};\n");
                self.decls.push_str(&s);
            }
            TyKind::Struct(si) => {
                let fields = self.t.structs[*si as usize].fields.clone();
                let mut s = format!("struct nc_{name} {{\n");
                for (fname, x) in &fields {
                    let xc = self.ct(*x);
                    writeln!(s, "  {xc} {};", field_name(fname)).unwrap();
                }
                if fields.is_empty() {
                    s.push_str("  char _empty;\n");
                }
                s.push_str("};\n");
                self.decls.push_str(&s);
            }
            TyKind::Enum(ei) if self.t.enum_has_payload(t) => {
                let vars = self.t.enums[*ei as usize].variants.clone();
                let mut s = format!(
                    "struct nc_{name} {{\n  _Atomic int64_t rc;\n  uint32_t tag;\n  union {{\n"
                );
                for (vi, (vname, ps)) in vars.iter().enumerate() {
                    if ps.is_empty() {
                        continue;
                    }
                    s.push_str("    struct {");
                    for (k, p) in ps.iter().enumerate() {
                        let pc = self.ct(*p);
                        write!(s, " {pc} p{k};").unwrap();
                    }
                    writeln!(s, " }} v{vi}; /* {vname} */").unwrap();
                }
                s.push_str("  } u;\n};\n");
                self.late_decls.push_str(&s);
            }
            TyKind::Fn(ps, r) => {
                let rc = self.ret_ct(*r);
                let mut args = vec!["nc_env *".to_string()];
                for p in ps {
                    args.push(self.ct(*p));
                }
                let fp = self.fp_type(t);
                writeln!(self.decls, "typedef {rc} (*{fp})({});", args.join(", ")).unwrap();
            }
            _ => {}
        }
        self.helpers(t, &ct, &kind);
    }

    fn helpers(&mut self, t: TyId, ct: &str, kind: &TyKind) {
        let h = t;
        let managed = self.managed(t);
        writeln!(
            self.protos,
            "static {ct} rt_{h}({ct} x);\nstatic void rl_{h}({ct} x);\nstatic bool eq_{h}({ct} a, {ct} b);\nstatic uint64_t hs_{h}({ct} x);\nstatic void st_{h}(nc_sb *sb, {ct} x);\nstatic const nc_ti ti_{h};"
        )
        .unwrap();
        let (rt, rl, eq, hs, st) = self.helper_bodies(t, kind);
        let b = &mut self.bodies;
        writeln!(b, "/* {} */", self.t.show(t).replace("*/", "* /")).unwrap();
        writeln!(b, "static {ct} rt_{h}({ct} x) {{ {rt} return x; }}").unwrap();
        let (vx, vab, vsx) = (
            unused(&rl, &["x"]),
            unused(&eq, &["a", "b"]),
            unused(&st, &["sb", "x"]),
        );
        writeln!(b, "static void rl_{h}({ct} x) {{ {vx}{rl} }}").unwrap();
        writeln!(b, "static bool eq_{h}({ct} a, {ct} b) {{ {vab}{eq} }}").unwrap();
        let vx = unused(&hs, &["x"]);
        writeln!(b, "static uint64_t hs_{h}({ct} x) {{ {vx}{hs} }}").unwrap();
        writeln!(b, "static void st_{h}(nc_sb *sb, {ct} x) {{ {vsx}{st} }}").unwrap();
        if managed {
            writeln!(
                b,
                "static void vrt_{h}(void *p) {{ *({ct} *)p = rt_{h}(*({ct} *)p); }}"
            )
            .unwrap();
            writeln!(b, "static void vrl_{h}(void *p) {{ rl_{h}(*({ct} *)p); }}").unwrap();
        }
        writeln!(b, "static bool veq_{h}(const void *a, const void *b) {{ return eq_{h}(*({ct} const *)a, *({ct} const *)b); }}").unwrap();
        writeln!(
            b,
            "static uint64_t vhs_{h}(const void *a) {{ return hs_{h}(*({ct} const *)a); }}"
        )
        .unwrap();
        writeln!(
            b,
            "static void vst_{h}(nc_sb *sb, const void *a) {{ st_{h}(sb, *({ct} const *)a); }}"
        )
        .unwrap();
        let (r, l) = if managed {
            (format!("vrt_{h}"), format!("vrl_{h}"))
        } else {
            ("NULL".into(), "NULL".into())
        };
        writeln!(
            b,
            "static const nc_ti ti_{h} = {{sizeof({ct}), {r}, {l}, veq_{h}, vhs_{h}, vst_{h}}};\n"
        )
        .unwrap();
    }

    /// (retain stmts, release stmts, eq body, hash body, str body)
    fn helper_bodies(
        &mut self,
        t: TyId,
        kind: &TyKind,
    ) -> (String, String, String, String, String) {
        let s = |x: &str| x.to_string();
        // plain values: compared with `==`
        let prim = |hash: &str, st: &str| {
            (
                s(""),
                s(""),
                s("return a == b;"),
                format!("return {hash};"),
                s(st),
            )
        };
        // reference-counted runtime objects with their own helpers (`nc_str_retain`, ...)
        let object = |p: &str, st: &str| {
            (
                format!("{p}_retain(x);"),
                format!("{p}_release(x);"),
                format!("return {p}_eq(a, b);"),
                format!("return {p}_hash(x);"),
                s(st),
            )
        };
        // handles compared by identity
        let handle = |p: &str| {
            (
                format!("{p}_rt(x);"),
                format!("{p}_release(x);"),
                s("return a == b;"),
                s("return 0;"),
                s(""),
            )
        };
        match kind {
            TyKind::Void => (s(""), s(""), s("return true;"), s("return 0;"), s("")),
            TyKind::Bool => prim("nc_hash_u64(x)", "nc_sb_bool(sb, x);"),
            TyKind::Byte | TyKind::UInt => prim("nc_hash_u64(x)", "nc_sb_u64(sb, x);"),
            TyKind::Int => prim("nc_hash_u64((uint64_t)x)", "nc_sb_i64(sb, x);"),
            TyKind::Char => prim("nc_hash_u64(x)", "nc_sb_char(sb, x);"),
            TyKind::Float => prim("nc_hs_float(&x)", "nc_sb_f64(sb, x);"),
            TyKind::Str | TyKind::Error => object("nc_str", "nc_sb_str(sb, x);"),
            TyKind::Array(_) | TyKind::Fixed(..) => object("nc_arr", "nc_arr_str(sb, x);"),
            TyKind::Map(..) => object("nc_map", "nc_map_str(sb, x);"),
            TyKind::Opt(i) => {
                let i = self.hid(*i);
                let ic = self.ct(i);
                if self.t.opt_boxed(t) {
                    (
                        s("nc_cell_retain(x);"),
                        s("nc_cell_release(x);"),
                        format!(
                            "if (!a || !b) return a == b; return eq_{i}(*({ic} *)a->val, *({ic} *)b->val);"
                        ),
                        format!("return x ? nc_hash_mix(1, hs_{i}(*({ic} *)x->val)) : 0;"),
                        format!(
                            "if (x) st_{i}(sb, *({ic} *)x->val); else nc_sb_cstr(sb, \"none\");"
                        ),
                    )
                } else {
                    (
                        format!("if (x.has) x.val = rt_{i}(x.val);"),
                        format!("if (x.has) rl_{i}(x.val);"),
                        format!(
                            "if (a.has != b.has) return false; return !a.has || eq_{i}(a.val, b.val);"
                        ),
                        format!("return x.has ? nc_hash_mix(1, hs_{i}(x.val)) : 0;"),
                        format!("if (x.has) st_{i}(sb, x.val); else nc_sb_cstr(sb, \"none\");"),
                    )
                }
            }
            TyKind::ErrU(i) => {
                let i = self.hid(*i);
                (
                    format!("nc_str_retain(x.err); if (!x.err) x.val = rt_{i}(x.val);"),
                    format!("nc_str_release(x.err); if (!x.err) rl_{i}(x.val);"),
                    format!(
                        "if (!a.err != !b.err) return false; return a.err ? nc_str_eq(a.err, b.err) : eq_{i}(a.val, b.val);"
                    ),
                    format!("return x.err ? nc_str_hash(x.err) : hs_{i}(x.val);"),
                    format!(
                        "if (x.err) {{ nc_sb_cstr(sb, \"error: \"); nc_sb_str(sb, x.err); }} else st_{i}(sb, x.val);"
                    ),
                )
            }
            TyKind::Tuple(ts) => {
                let fields: Vec<(String, TyId)> = ts
                    .iter()
                    .enumerate()
                    .map(|(i, t)| (format!("f{i}"), *t))
                    .collect();
                self.product_bodies(&fields, "(", ")", None)
            }
            TyKind::Struct(si) => {
                let def = self.t.structs[*si as usize].clone();
                let fields: Vec<(String, TyId)> = def
                    .fields
                    .iter()
                    .map(|(n, t)| (field_name(n), *t))
                    .collect();
                let names: Vec<String> = def.fields.iter().map(|(n, _)| n.clone()).collect();
                self.product_bodies(&fields, &format!("{}{{", def.name), "}", Some(names))
            }
            TyKind::Enum(ei) => {
                let def = self.t.enums[*ei as usize].clone();
                if !self.t.enum_has_payload(t) {
                    let mut st = s("switch (x) {");
                    for (vi, (vn, _)) in def.variants.iter().enumerate() {
                        write!(
                            st,
                            " case {vi}: nc_sb_cstr(sb, \"{}.{vn}\"); break;",
                            c_escape_str(&def.name)
                        )
                        .unwrap();
                    }
                    st.push_str(" }");
                    return prim("nc_hash_u64(x)", &st);
                }
                let mut rl = s("if (x && nc_dec(&x->rc)) { switch (x->tag) {");
                let mut eq = s(
                    "if (a == b) return true; if (a->tag != b->tag) return false; switch (a->tag) {",
                );
                let mut hs = s("uint64_t h = nc_hash_u64(x->tag); switch (x->tag) {");
                let mut st = s("switch (x->tag) {");
                for (vi, (vn, ps)) in def.variants.iter().enumerate() {
                    write!(rl, " case {vi}:").unwrap();
                    write!(eq, " case {vi}: return true").unwrap();
                    write!(hs, " case {vi}:").unwrap();
                    write!(
                        st,
                        " case {vi}: nc_sb_cstr(sb, \"{}.{vn}\");",
                        c_escape_str(&def.name)
                    )
                    .unwrap();
                    if !ps.is_empty() {
                        st.push_str(" nc_sb_cstr(sb, \"(\");");
                    }
                    for (k, p) in ps.iter().enumerate() {
                        let p = self.hid(*p);
                        if self.managed(p) {
                            write!(rl, " rl_{p}(x->u.v{vi}.p{k});").unwrap();
                        }
                        write!(eq, " && eq_{p}(a->u.v{vi}.p{k}, b->u.v{vi}.p{k})").unwrap();
                        write!(hs, " h = nc_hash_mix(h, hs_{p}(x->u.v{vi}.p{k}));").unwrap();
                        if k > 0 {
                            st.push_str(" nc_sb_cstr(sb, \", \");");
                        }
                        write!(st, " st_{p}(sb, x->u.v{vi}.p{k});").unwrap();
                    }
                    if !ps.is_empty() {
                        st.push_str(" nc_sb_cstr(sb, \")\");");
                    }
                    rl.push_str(" break;");
                    eq.push(';');
                    hs.push_str(" break;");
                    st.push_str(" break;");
                }
                rl.push_str(" } free(x); }");
                eq.push_str(" } return false;");
                hs.push_str(" } return h;");
                st.push_str(" }");
                (s("if (x) nc_inc(&x->rc);"), rl, eq, hs, st)
            }
            TyKind::Fn(..) => (
                s("nc_env_rt(x.env);"),
                s("nc_env_release(x.env);"),
                s("return false;"),
                s("return 0;"),
                s(""),
            ),
            TyKind::Fut(_) => handle("nc_fut"),
            TyKind::Mutex(_) => handle("nc_mutex"),
            TyKind::Never | TyKind::Named(_) | TyKind::Const(..) => unreachable!(),
        }
    }

    fn product_bodies(
        &self,
        fields: &[(String, TyId)],
        open: &str,
        close: &str,
        names: Option<Vec<String>>,
    ) -> (String, String, String, String, String) {
        let mut rt = String::new();
        let mut rl = String::new();
        let mut eq = String::from("return true");
        let mut hs = String::from("uint64_t h = 0;");
        let mut st = format!("nc_sb_cstr(sb, \"{}\");", c_escape_str(open));
        for (k, (f, t)) in fields.iter().enumerate() {
            let t = self.hid(*t);
            if self.managed(t) {
                write!(rt, " x.{f} = rt_{t}(x.{f});").unwrap();
                write!(rl, " rl_{t}(x.{f});").unwrap();
            }
            write!(eq, " && eq_{t}(a.{f}, b.{f})").unwrap();
            write!(hs, " h = nc_hash_mix(h, hs_{t}(x.{f}));").unwrap();
            if k > 0 {
                st.push_str(" nc_sb_cstr(sb, \", \");");
            }
            if let Some(n) = &names {
                write!(st, " nc_sb_cstr(sb, \".{} = \");", n[k]).unwrap();
            }
            write!(st, " st_{t}(sb, x.{f});").unwrap();
        }
        eq.push(';');
        hs.push_str(" return h;");
        write!(st, " nc_sb_cstr(sb, \"{}\");", c_escape_str(close)).unwrap();
        (rt, rl, eq, hs, st)
    }
}

pub fn field_name(n: &str) -> String {
    format!("f_{n}")
}

/// Escape a string for a C string literal.
pub fn c_escape_str(s: &str) -> String {
    c_escape_bytes(s.as_bytes())
}

pub fn c_escape_bytes(b: &[u8]) -> String {
    let mut out = String::new();
    let mut prev_octal = false;
    for &c in b {
        match c {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\t' => out.push_str("\\t"),
            b'\r' => out.push_str("\\r"),
            b'?' => out.push_str("\\?"),
            0x20..=0x7e => {
                if prev_octal && c.is_ascii_digit() {
                    // break the octal escape
                    out.push_str("\"\"");
                }
                out.push(c as char);
            }
            _ => {
                write!(out, "\\{c:03o}").unwrap();
                prev_octal = true;
                continue;
            }
        }
        prev_octal = false;
    }
    out
}

pub fn c_ident(s: &str) -> String {
    let i = ident(s);
    if i.is_empty() { "anon".into() } else { i }
}

/// Is this optional or error union (named `name`) one that `nc.h` defines?
fn in_header(kind: &TyKind, inner: TyId, name: &str) -> bool {
    let prim = match inner {
        VOID => "void",
        BOOL => "bool",
        BYTE => "byte",
        INT => "int",
        UINT => "uint",
        FLOAT => "float",
        CHAR => "char",
        STR => "str",
        _ => return false,
    };
    match kind {
        TyKind::Opt(_) => inner != VOID && name == format!("opt_{prim}"),
        _ => name == format!("err_{prim}"),
    }
}

/// `(void)p; ` for each parameter `p` the body never mentions, to keep C compilers quiet.
fn unused(body: &str, params: &[&str]) -> String {
    let words: std::collections::HashSet<&str> = body
        .split(|c: char| !(c == '_' || c.is_ascii_alphanumeric()))
        .collect();
    (params.iter())
        .filter(|p| !words.contains(**p))
        .map(|p| format!("(void){p}; "))
        .collect()
}
