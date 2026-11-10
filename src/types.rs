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
}

#[derive(Clone, Debug)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<(String, TyId)>,
}

#[derive(Clone, Debug)]
pub struct EnumDef {
    pub name: String,
    pub variants: Vec<(String, Vec<TyId>)>,
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

impl Types {
    pub fn new() -> Types {
        let mut t = Types::default();
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

    pub fn count(&self) -> usize {
        self.kinds.len()
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

    pub fn new_struct(&mut self, name: String) -> TyId {
        self.structs.push(StructDef { name, fields: vec![] });
        self.intern(TyKind::Struct(self.structs.len() as u32 - 1))
    }
    pub fn new_enum(&mut self, name: String) -> TyId {
        self.enums.push(EnumDef { name, variants: vec![] });
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

    pub fn is_integer(&self, t: TyId) -> bool {
        matches!(t, BYTE | INT | UINT)
    }
    pub fn is_numeric(&self, t: TyId) -> bool {
        matches!(t, BYTE | INT | UINT | FLOAT)
    }

    /// Element type for indexable sequences (arrays, fixed arrays, str).
    pub fn elem(&self, t: TyId) -> Option<TyId> {
        match self.kind(self.base(t)) {
            TyKind::Array(e) | TyKind::Fixed(e, _) => Some(*e),
            TyKind::Str => Some(CHAR),
            _ => None,
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
            TyKind::Struct(i) => self.structs[*i as usize].fields.iter().any(|(_, t)| self.is_managed(*t)),
            TyKind::Enum(_) => self.enum_has_payload(t),
            _ => true,
        }
    }

    pub fn enum_has_payload(&self, t: TyId) -> bool {
        match self.kind(t) {
            TyKind::Enum(i) => self.enums[*i as usize].variants.iter().any(|(_, p)| !p.is_empty()),
            _ => false,
        }
    }

    /// Optionals of a type that (by value) contains the optional itself are heap allocated.
    pub fn opt_boxed(&self, t: TyId) -> bool {
        match self.kind(t) {
            TyKind::Opt(inner) => self.contains_by_value(*inner, t, &mut vec![]),
            _ => false,
        }
    }

    /// Does `t` contain `target` by value (not behind a heap pointer)?
    pub fn contains_by_value(&self, t: TyId, target: TyId, seen: &mut Vec<TyId>) -> bool {
        if t == target {
            return true;
        }
        if seen.contains(&t) {
            return false;
        }
        seen.push(t);
        let r = match self.kind(t) {
            TyKind::Named(_) => self.contains_by_value(self.base(t), target, seen),
            TyKind::Tuple(ts) => ts.clone().iter().any(|x| self.contains_by_value(*x, target, seen)),
            TyKind::Struct(i) => {
                let fs: Vec<TyId> = self.structs[*i as usize].fields.iter().map(|f| f.1).collect();
                fs.iter().any(|x| self.contains_by_value(*x, target, seen))
            }
            TyKind::Opt(i) => {
                // a boxed optional breaks the cycle, but we are computing boxedness itself:
                // optionals are considered by-value containers here.
                self.contains_by_value(*i, target, seen)
            }
            TyKind::ErrU(i) => self.contains_by_value(*i, target, seen),
            _ => false,
        };
        seen.pop();
        r
    }

    pub fn show(&self, t: TyId) -> String {
        match self.kind(t) {
            TyKind::Void => "void".into(),
            TyKind::Never => "never".into(),
            TyKind::Bool => "bool".into(),
            TyKind::Byte => "byte".into(),
            TyKind::Int => "int".into(),
            TyKind::UInt => "uint".into(),
            TyKind::Float => "float".into(),
            TyKind::Char => "char".into(),
            TyKind::Str => "str".into(),
            TyKind::Error => "error".into(),
            TyKind::Array(e) => format!("{}[]", self.show(*e)),
            TyKind::Fixed(e, n) => format!("{}[{n}]", self.show(*e)),
            TyKind::Map(k, v) => format!("[{}]{}", self.show(*k), self.show(*v)),
            TyKind::Opt(i) => format!("{}?", self.show(*i)),
            TyKind::ErrU(i) => {
                if *i == VOID {
                    "void!".into()
                } else {
                    format!("{}!", self.show(*i))
                }
            }
            TyKind::Tuple(ts) => format!("({})", ts.iter().map(|t| self.show(*t)).collect::<Vec<_>>().join(", ")),
            TyKind::Fn(ps, r) => {
                let ps = ps.iter().map(|t| self.show(*t)).collect::<Vec<_>>().join(", ");
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
        }
    }
}
