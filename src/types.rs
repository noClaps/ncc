//! Interned semantic types.

use std::collections::HashMap;

pub type TyId = u32;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TyKind {
    Void,
    Never,
    Bool,
    Byte,
    Int,
    UInt,
    Float,
    Char,
    Str,
    Error,
    Array(TyId),
    Fixed(TyId, u64),
    Map(TyId, TyId),
    Opt(TyId),
    ErrU(TyId),
    Tuple(Vec<TyId>),
    Fn(Vec<TyId>, TyId),
    Fut(TyId),
    Mutex(TyId),
    Struct(u32),
    Enum(u32),
    Named(u32),
    /// The value of a generic constant (`3` in `f<int, 3>`), with the constant's integer type.
    /// Only appears as a generic argument, never as the type of a value.
    Const(TyId, i128),
}

#[derive(Clone, Debug)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<(String, TyId)>,
    /// for an instance of a generic struct: the generic's name and the arguments
    pub generic: Option<(String, Vec<TyId>)>,
}

#[derive(Clone, Debug)]
pub struct EnumDef {
    pub name: String,
    pub variants: Vec<(String, Vec<TyId>)>,
    /// for an instance of a generic enum: the generic's name and the arguments
    pub generic: Option<(String, Vec<TyId>)>,
}

#[derive(Clone, Debug)]
pub struct NamedDef {
    pub name: String,
    pub underlying: TyId,
}

#[derive(Default)]
pub struct Types {
    kinds: Vec<TyKind>,
    map: HashMap<TyKind, TyId>,
    pub structs: Vec<StructDef>,
    pub enums: Vec<EnumDef>,
    pub named: Vec<NamedDef>,
    /// While a generic body is checked for the editor (see `Checker::index_generic_bodies`),
    /// the names of its constant parameters; their placeholder values are shown as names.
    pub sym_consts: Vec<String>,
}

/// The placeholder value of the `k`-th constant parameter of a generic body checked for the
/// editor: far apart powers of two, so sizes such as `N + M - 1` can be shown symbolically.
pub fn sym_const(k: usize) -> Option<i128> {
    (k < 3).then(|| 1i128 << (20 + 5 * k))
}

pub const VOID: TyId = 0;
pub const NEVER: TyId = 1;
pub const BOOL: TyId = 2;
pub const BYTE: TyId = 3;
pub const INT: TyId = 4;
pub const UINT: TyId = 5;
pub const FLOAT: TyId = 6;
pub const CHAR: TyId = 7;
pub const STR: TyId = 8;
pub const ERROR: TyId = 9;

/// The builtin types by name.
pub const BUILTIN_TYPES: &[(&str, TyId)] = &[
    ("int", INT),
    ("uint", UINT),
    ("byte", BYTE),
    ("float", FLOAT),
    ("bool", BOOL),
    ("char", CHAR),
    ("str", STR),
    ("void", VOID),
    ("error", ERROR),
    ("never", NEVER),
];

pub fn builtin_type(name: &str) -> Option<TyId> {
    BUILTIN_TYPES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, t)| *t)
}

impl Types {
    pub fn new() -> Self {
        let mut t = Self::default();
        for k in [
            TyKind::Void,
            TyKind::Never,
            TyKind::Bool,
            TyKind::Byte,
            TyKind::Int,
            TyKind::UInt,
            TyKind::Float,
            TyKind::Char,
            TyKind::Str,
            TyKind::Error,
        ] {
            t.intern(k);
        }
        t
    }

    pub fn intern(&mut self, k: TyKind) -> TyId {
        if let Some(&id) = self.map.get(&k) {
            return id;
        }
        let id = self.kinds.len() as TyId;
        self.kinds.push(k.clone());
        self.map.insert(k, id);
        id
    }

    pub fn kind(&self, t: TyId) -> &TyKind {
        &self.kinds[t as usize]
    }

    /// The id of a type that has been interned.
    pub fn lookup(&self, k: &TyKind) -> Option<TyId> {
        self.map.get(k).copied()
    }

    pub fn array(&mut self, t: TyId) -> TyId {
        self.intern(TyKind::Array(t))
    }
    pub fn opt(&mut self, t: TyId) -> TyId {
        self.intern(TyKind::Opt(t))
    }
    pub fn erru(&mut self, t: TyId) -> TyId {
        self.intern(TyKind::ErrU(t))
    }
    pub fn tuple(&mut self, ts: Vec<TyId>) -> TyId {
        self.intern(TyKind::Tuple(ts))
    }

    /// A new struct (the instance of a generic struct with `generic`), its fields to be filled.
    pub fn new_struct(&mut self, name: String, generic: Option<(String, Vec<TyId>)>) -> TyId {
        self.structs.push(StructDef {
            name,
            fields: vec![],
            generic,
        });
        self.intern(TyKind::Struct(self.structs.len() as u32 - 1))
    }
    /// A new enum (the instance of a generic enum with `generic`), its variants to be filled.
    pub fn new_enum(&mut self, name: String, generic: Option<(String, Vec<TyId>)>) -> TyId {
        self.enums.push(EnumDef {
            name,
            variants: vec![],
            generic,
        });
        self.intern(TyKind::Enum(self.enums.len() as u32 - 1))
    }
    pub fn new_named(&mut self, name: String, underlying: TyId) -> TyId {
        self.named.push(NamedDef { name, underlying });
        self.intern(TyKind::Named(self.named.len() as u32 - 1))
    }

    /// Strip nominal type definitions.
    pub fn base(&self, mut t: TyId) -> TyId {
        while let TyKind::Named(n) = self.kind(t) {
            t = self.named[*n as usize].underlying;
        }
        t
    }

    /// Element type for indexable sequences (arrays, fixed arrays, str).
    pub fn elem(&self, t: TyId) -> Option<TyId> {
        match self.kind(self.base(t)) {
            TyKind::Array(e) | TyKind::Fixed(e, _) => Some(*e),
            TyKind::Str => Some(CHAR),
            _ => None,
        }
    }

    /// The types `t` is directly built from: elements, keys and values, fields, payloads,
    /// parameters and results. A named type's underlying type is not one of them.
    pub fn components(&self, t: TyId) -> Vec<TyId> {
        match self.kind(t) {
            TyKind::Array(e)
            | TyKind::Fixed(e, _)
            | TyKind::Opt(e)
            | TyKind::ErrU(e)
            | TyKind::Fut(e)
            | TyKind::Mutex(e) => vec![*e],
            TyKind::Map(k, v) => vec![*k, *v],
            TyKind::Tuple(ts) => ts.clone(),
            TyKind::Fn(ps, r) => ps.iter().chain([r]).copied().collect(),
            TyKind::Struct(i) => self.structs[*i as usize]
                .fields
                .iter()
                .map(|f| f.1)
                .collect(),
            TyKind::Enum(i) => (self.enums[*i as usize].variants.iter())
                .flat_map(|v| v.1.iter().copied())
                .collect(),
            _ => vec![],
        }
    }

    pub fn struct_def(&self, t: TyId) -> Option<&StructDef> {
        match self.kind(t) {
            TyKind::Struct(i) => Some(&self.structs[*i as usize]),
            _ => None,
        }
    }
    pub fn enum_def(&self, t: TyId) -> Option<&EnumDef> {
        match self.kind(t) {
            TyKind::Enum(i) => Some(&self.enums[*i as usize]),
            _ => None,
        }
    }

    /// Whether values of this type need reference counting.
    pub fn is_managed(&self, t: TyId) -> bool {
        match self.kind(t) {
            TyKind::Void
            | TyKind::Never
            | TyKind::Bool
            | TyKind::Byte
            | TyKind::Int
            | TyKind::UInt
            | TyKind::Float
            | TyKind::Char => false,
            TyKind::Named(_) => self.is_managed(self.base(t)),
            TyKind::Opt(i) => self.opt_boxed(t) || self.is_managed(*i),
            TyKind::ErrU(_) => true,
            TyKind::Tuple(ts) => ts.iter().any(|t| self.is_managed(*t)),
            TyKind::Struct(i) => self.structs[*i as usize]
                .fields
                .iter()
                .any(|(_, t)| self.is_managed(*t)),
            TyKind::Enum(_) => self.enum_has_payload(t),
            _ => true,
        }
    }

    pub fn enum_has_payload(&self, t: TyId) -> bool {
        match self.kind(t) {
            TyKind::Enum(i) => self.enums[*i as usize]
                .variants
                .iter()
                .any(|(_, p)| !p.is_empty()),
            _ => false,
        }
    }

    /// Optionals of a type that (by value) contains the optional itself are heap allocated.
    pub fn opt_boxed(&self, t: TyId) -> bool {
        match self.kind(t) {
            TyKind::Opt(inner) => self.contains_by_value(*inner, t, true, &mut vec![]),
            _ => false,
        }
    }

    /// Does `t` contain `target` by value (not behind a heap pointer)? Optionals are only
    /// looked into with `opts`.
    pub fn contains_by_value(
        &self,
        t: TyId,
        target: TyId,
        opts: bool,
        seen: &mut Vec<TyId>,
    ) -> bool {
        if t == target {
            return true;
        }
        if seen.contains(&t) {
            return false;
        }
        seen.push(t);
        let r = match self.kind(t) {
            TyKind::Named(_) => self.contains_by_value(self.base(t), target, opts, seen),
            TyKind::Tuple(ts) => {
                (ts.iter()).any(|x| self.contains_by_value(*x, target, opts, seen))
            }
            TyKind::Struct(i) => (self.structs[*i as usize].fields.iter())
                .any(|f| self.contains_by_value(f.1, target, opts, seen)),
            // a boxed optional breaks the cycle, but boxedness is what `opt_boxed` computes:
            // there, optionals count as by-value containers
            TyKind::Opt(i) if opts => self.contains_by_value(*i, target, opts, seen),
            TyKind::ErrU(i) => self.contains_by_value(*i, target, opts, seen),
            _ => false,
        };
        seen.pop();
        r
    }

    /// An array size, written in terms of the placeholder constants when there are any.
    fn show_size(&self, n: u64) -> String {
        let k = self.sym_consts.len().min(3);
        if k == 0 || (n as i128) < sym_const(0).unwrap() / 2 {
            return n.to_string();
        }
        let mut rest = n as i128;
        let mut terms = Vec::new();
        for i in (0..k).rev() {
            let unit = sym_const(i).unwrap();
            let c = (rest + unit / 2).div_euclid(unit);
            if !(0..32).contains(&c) {
                return n.to_string();
            }
            rest -= c * unit;
            match c {
                0 => {}
                1 => terms.push(self.sym_consts[i].clone()),
                _ => terms.push(format!("{c} * {}", self.sym_consts[i])),
            }
        }
        terms.reverse();
        let mut s = terms.join(" + ");
        match rest {
            0 => {}
            r if r > 0 => s.push_str(&format!(" + {r}")),
            r => s.push_str(&format!(" - {}", -r)),
        }
        s
    }

    /// The types, comma separated.
    pub fn show_list(&self, ts: &[TyId]) -> String {
        let shown: Vec<_> = ts.iter().map(|t| self.show(*t)).collect();
        shown.join(", ")
    }

    pub fn show(&self, t: TyId) -> String {
        match self.kind(t) {
            TyKind::Void
            | TyKind::Never
            | TyKind::Bool
            | TyKind::Byte
            | TyKind::Int
            | TyKind::UInt
            | TyKind::Float
            | TyKind::Char
            | TyKind::Str
            | TyKind::Error => BUILTIN_TYPES
                .iter()
                .find(|(_, b)| *b == t)
                .unwrap()
                .0
                .into(),
            TyKind::Array(e) => format!("{}[]", self.show(*e)),
            TyKind::Fixed(e, n) => format!("{}[{}]", self.show(*e), self.show_size(*n)),
            TyKind::Map(k, v) => format!("[{}]{}", self.show(*k), self.show(*v)),
            TyKind::Opt(i) => format!("{}?", self.show(*i)),
            TyKind::ErrU(i) => format!("{}!", self.show(*i)),
            TyKind::Tuple(ts) => format!("({})", self.show_list(ts)),
            TyKind::Fn(ps, r) => {
                let ps = self.show_list(ps);
                if *r == VOID {
                    format!("(fn({ps}))")
                } else {
                    format!("(fn({ps}) {})", self.show(*r))
                }
            }
            TyKind::Fut(i) => format!("fut {}", self.show(*i)),
            TyKind::Mutex(i) => format!("mutex {}", self.show(*i)),
            TyKind::Struct(i) => self.structs[*i as usize].name.clone(),
            TyKind::Enum(i) => self.enums[*i as usize].name.clone(),
            TyKind::Named(i) => self.named[*i as usize].name.clone(),
            TyKind::Const(_, v) => v.to_string(),
        }
    }
}

impl Types {
    /// The type as a syntax tree, as it would be written (`Box<int>[]`, `(fn(int) str)`).
    pub fn to_type_expr(&self, t: TyId) -> crate::ast::TypeExpr {
        use crate::ast::{ConstExpr, TypeExpr as T};
        let sp = crate::diag::Span::default();
        let b = |t: TyId| Box::new(self.to_type_expr(t));
        let named = |name: String, args: &[TyId]| T::Named {
            module: None,
            name,
            args: args.iter().map(|a| self.to_type_expr(*a)).collect(),
            span: sp,
        };
        // an instance of a generic is written with its arguments
        let nominal = |generic: &Option<(String, Vec<TyId>)>| match generic {
            Some((n, args)) => named(n.clone(), args),
            None => named(self.show(t), &[]),
        };
        match self.kind(t) {
            TyKind::Array(e) => T::Array(b(*e), None, sp),
            TyKind::Fixed(e, n) => {
                let size = match self.show_size(*n).parse() {
                    Ok(n) => ConstExpr::Int(n, sp),
                    // a size in terms of generic constants
                    Err(_) => ConstExpr::Name(self.show_size(*n), sp),
                };
                T::Array(b(*e), Some(size), sp)
            }
            TyKind::Map(k, v) => T::Map(b(*k), b(*v), sp),
            TyKind::Opt(i) => T::Optional(b(*i), sp),
            TyKind::ErrU(i) => T::ErrorUnion(b(*i), sp),
            TyKind::Tuple(ts) => T::Tuple(ts.iter().map(|t| self.to_type_expr(*t)).collect(), sp),
            TyKind::Fn(ps, r) => T::Fn(
                ps.iter().map(|t| self.to_type_expr(*t)).collect(),
                (*r != VOID).then(|| b(*r)),
                sp,
            ),
            TyKind::Fut(i) => T::Fut(b(*i), sp),
            TyKind::Const(_, v) => T::Const(ConstExpr::Int(*v as u128, sp)),
            TyKind::Struct(i) => nominal(&self.structs[*i as usize].generic),
            TyKind::Enum(i) => nominal(&self.enums[*i as usize].generic),
            // builtins, aliases and mutexes, written as they show
            _ => named(self.show(t), &[]),
        }
    }
}

pub fn is_integer(t: TyId) -> bool {
    matches!(t, BYTE | INT | UINT)
}

pub fn is_numeric(t: TyId) -> bool {
    matches!(t, BYTE | INT | UINT | FLOAT)
}
