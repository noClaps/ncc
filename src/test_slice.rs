//! Conservative, source-order dependency slicing before test-mode semantic checks.
use crate::ast::*;
use std::collections::{HashMap, HashSet};

type Names = HashSet<String>;

#[derive(Clone, Default, PartialEq, Eq)]
struct Effects {
    uses: Names,
    writes: Names,
    calls: Names,
    opaque: bool,
    awaits: bool,
    assigns: bool,
    captured_write: bool,
}
impl Effects {
    fn merge(&mut self, other: &Self) {
        self.uses.extend(other.uses.iter().cloned());
        self.writes.extend(other.writes.iter().cloned());
        self.calls.extend(other.calls.iter().cloned());
        self.opaque |= other.opaque;
        self.awaits |= other.awaits;
        self.assigns |= other.assigns;
        self.captured_write |= other.captured_write;
    }
}

fn definitions(item: &Item) -> Vec<String> {
    match item {
        Item::Function(f) => vec![f.name.clone()],
        Item::Global(v) => v.binding_names().into_iter().map(str::to_owned).collect(),
        Item::Struct(s) => vec![s.name.clone()],
        Item::Enum(e) => vec![e.name.clone()],
        Item::TypeAlias { name, .. } => vec![name.clone()],
        Item::Extern { functions, .. } => functions.iter().map(|f| f.name.clone()).collect(),
        _ => vec![],
    }
}

/// Return selected item identities; callers retain the original AST and locations.
pub(crate) fn select(
    items: &[Item],
    errors: &[Option<crate::diagnostic::Diagnostics>],
) -> Vec<bool> {
    let Some(last_test) = items.iter().rposition(|i| matches!(i, Item::Test { .. })) else {
        return vec![false; items.len()];
    };
    let mut declarations = HashMap::<String, Vec<usize>>::new();
    for (index, item) in items.iter().enumerate() {
        for name in definitions(item) {
            declarations.entry(name).or_default().push(index);
        }
    }
    let globals: Names = declarations.keys().cloned().collect();
    let callable: Names = items
        .iter()
        .filter(|item| match item {
            Item::Function(_) | Item::Extern { .. } => true,
            Item::Global(v) => {
                matches!(v.ty, Type::Function(..))
                    || matches!(v.ty, Type::Named(ref n, _) if n == "fn")
                    || matches!(v.value.unlocated(), Expr::Lambda(_) | Expr::Name(_))
            }
            _ => false,
        })
        .flat_map(definitions)
        .collect();
    let enums: Names = items
        .iter()
        .filter_map(|item| match item {
            Item::Enum(e) => Some(e.name.clone()),
            _ => None,
        })
        .collect();
    let mutable: Names = items
        .iter()
        .filter_map(|item| match item {
            Item::Global(v) if v.mutable || v.mutex => Some(v.binding_names()),
            _ => None,
        })
        .flatten()
        .map(str::to_owned)
        .collect();
    let mut effects: Vec<_> = items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let mut scan = Scan {
                globals: &globals,
                enums: &enums,
                local: Names::new(),
                captures: Names::new(),
                effects: Effects::default(),
            };
            scan.item(item);
            // Qualification can stop partway through a bad item, leaving names
            // unresolved. Never infer that its remaining assignments are harmless.
            if errors[index].is_some()
                && (scan.effects.assigns || matches!(item, Item::Function(_)))
            {
                scan.effects.opaque = true;
            }
            scan.effects
        })
        .collect();
    // Function values, aliases, returned closures and recursive call chains all
    // contribute dependencies. Unknown callees are conservatively opaque.
    loop {
        let previous = effects.clone();
        for effect in &mut effects {
            for callee in effect.calls.clone() {
                if let Some(indices) = declarations.get(&callee) {
                    for &index in indices {
                        effect.merge(&previous[index]);
                        if previous[index].captured_write {
                            // Escaped cells have no module name. Associate their
                            // mutations with the callable exposing those cells.
                            effect.writes.insert(callee.clone());
                        }
                        if matches!(items[index], Item::Global(_)) {
                            // A function-valued binding may have been replaced.
                            // Join every possible assignment before deciding a
                            // call is irrelevant; do not trust its initializer.
                            for writer in &previous {
                                if writer.writes.contains(&callee) {
                                    effect.merge(writer);
                                    effect
                                        .calls
                                        .extend(writer.uses.intersection(&callable).cloned());
                                }
                            }
                        }
                        if matches!(items[index], Item::Global(_)) {
                            effect
                                .calls
                                .extend(previous[index].uses.intersection(&callable).cloned());
                        }
                        if matches!(items[index], Item::Extern { .. }) {
                            effect.opaque = true;
                        }
                    }
                } else {
                    effect.opaque = true;
                }
            }
            if effect.opaque {
                effect.writes.extend(mutable.iter().cloned());
            }
        }
        if effects == previous {
            break;
        }
    }
    let mut selected: Vec<_> = items
        .iter()
        .map(|item| matches!(item, Item::Test { .. }))
        .collect();
    loop {
        let previous = selected.clone();
        let mut needed = Names::new();
        for (index, effect) in effects.iter().enumerate() {
            if selected[index] {
                needed.extend(effect.uses.iter().cloned());
                needed.extend(definitions(&items[index]));
            }
        }
        for name in &needed {
            if let Some(indices) = declarations.get(name) {
                for &index in indices {
                    selected[index] = true;
                }
            }
        }
        // Selected native sources need their complete ABI: C helper calls are
        // invisible to the NC graph and may use another extern block's signature.
        let native: Names = items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| match item {
                Item::Extern { path, .. } if selected[index] => Some(path.clone()),
                _ => None,
            })
            .collect();
        for (index, item) in items.iter().enumerate() {
            if let Item::Extern { path, .. } = item {
                selected[index] |= native.contains(path);
            }
            if index <= last_test && matches!(item, Item::Global(_) | Item::Statement(_)) {
                selected[index] |= effects[index].opaque
                    || !effects[index].writes.is_disjoint(&needed)
                    || (effects[index].awaits
                        && synchronization_depends_on(
                            &effects[index].uses,
                            &needed,
                            &declarations,
                            &effects,
                        ));
            }
        }
        if selected == previous {
            return selected;
        }
    }
}

fn synchronization_depends_on(
    uses: &Names,
    needed: &Names,
    declarations: &HashMap<String, Vec<usize>>,
    effects: &[Effects],
) -> bool {
    let mut pending: Vec<_> = uses.iter().cloned().collect();
    let mut seen = Names::new();
    while let Some(name) = pending.pop() {
        if needed.contains(&name) {
            return true;
        }
        if seen.insert(name.clone())
            && let Some(indices) = declarations.get(&name)
        {
            for &index in indices {
                pending.extend(effects[index].uses.iter().cloned());
            }
        }
    }
    false
}

struct Scan<'a> {
    globals: &'a Names,
    enums: &'a Names,
    local: Names,
    captures: Names,
    effects: Effects,
}
impl Scan<'_> {
    fn name(&mut self, name: &str) {
        if !self.local.contains(name) && self.globals.contains(name) {
            self.effects.uses.insert(name.into());
        }
    }
    fn ty(&mut self, ty: &Type) {
        match ty {
            Type::Named(name, args) => {
                self.name(name);
                for arg in args {
                    self.ty(arg);
                }
            }
            Type::Array(t, _) | Type::Optional(t) | Type::ErrorUnion(t) | Type::Future(t) => {
                self.ty(t)
            }
            Type::Map(k, v) => {
                self.ty(k);
                self.ty(v);
            }
            Type::Tuple(ts) => {
                for t in ts {
                    self.ty(t);
                }
            }
            Type::Function(ts, result) => {
                for t in ts {
                    self.ty(t);
                }
                self.ty(result);
            }
        }
    }
    fn function(&mut self, f: &Function) {
        let local = self.local.clone();
        let captures = std::mem::replace(&mut self.captures, local.clone());
        for p in &f.params {
            self.captures.remove(&p.name);
        }

        self.local.extend(f.generics.iter().cloned());
        for p in &f.params {
            self.ty(&p.ty);
        }
        self.ty(&f.return_type);
        self.local.extend(f.params.iter().map(|p| p.name.clone()));
        self.block(&f.body);
        self.local = local;
        self.captures = captures;
    }
    fn item(&mut self, item: &Item) {
        match item {
            Item::Function(f) => self.function(f),
            Item::Global(v) => {
                self.ty(&v.ty);
                self.expr(&v.value);
            }
            Item::Statement(s) => self.stmt(s),
            Item::Test { body, .. } => self.block(body),
            Item::TypeAlias { ty, .. } => self.ty(ty),
            Item::Struct(s) => {
                self.local.extend(s.generics.iter().cloned());
                for field in &s.fields {
                    self.ty(&field.ty);
                }
            }
            Item::Enum(e) => {
                self.local.extend(e.generics.iter().cloned());
                for variant in &e.variants {
                    for ty in &variant.values {
                        self.ty(ty);
                    }
                }
            }
            Item::Extern { functions, .. } => {
                for f in functions {
                    for p in &f.params {
                        self.ty(&p.ty);
                    }
                    self.ty(&f.return_type);
                }
            }
            Item::Import { .. } => {}
        }
    }
    fn bind(&mut self, p: &Pattern) {
        match p {
            Pattern::Name(n) => {
                self.captures.remove(n);
                self.local.insert(n.clone());
            }
            Pattern::Tuple(ps) | Pattern::Array(ps) => {
                for p in ps {
                    self.bind(p);
                }
            }
            Pattern::Variant { values, .. } => {
                for p in values {
                    self.bind(p);
                }
            }
            Pattern::Struct { fields, .. } => {
                for (_, p) in fields {
                    self.bind(p);
                }
            }
            _ => {}
        }
    }
    fn pattern(&mut self, p: &Pattern) {
        match p {
            Pattern::Literal(e) => self.expr(e),
            Pattern::Name(n) => {
                self.captures.remove(n);
                self.local.insert(n.clone());
            }
            Pattern::Tuple(ps) | Pattern::Array(ps) => {
                for p in ps {
                    self.pattern(p);
                }
            }
            Pattern::Variant { name, values } => {
                if let Some((owner, _)) = name.rsplit_once('.') {
                    self.name(owner);
                }
                for p in values {
                    self.pattern(p);
                }
            }
            Pattern::Struct { name, fields } => {
                self.name(name);
                for (_, p) in fields {
                    self.pattern(p);
                }
            }
            Pattern::Wildcard => {}
        }
    }
    fn block(&mut self, b: &Block) {
        let local = self.local.clone();
        let captures = self.captures.clone();
        for s in &b.statements {
            self.stmt(s);
        }
        self.local = local;
        self.captures = captures;
    }
    fn target(&mut self, e: &Expr) {
        match e.unlocated() {
            Expr::Name(n) if !self.local.contains(n) && self.globals.contains(n) => {
                self.effects.writes.insert(n.clone());
            }
            Expr::Name(n) if self.captures.contains(n) => {
                self.effects.captured_write = true;
            }
            Expr::Tuple(es) => {
                for e in es {
                    self.target(e);
                }
            }
            Expr::Member { object, .. } | Expr::Index { object, .. } => self.target(object),
            _ => {}
        }
        self.expr(e);
    }
    fn stmt(&mut self, s: &Stmt) {
        match s.unlocated() {
            Stmt::Var(v) => {
                self.ty(&v.ty);
                self.expr(&v.value);
                self.bind(&v.pattern);
            }
            Stmt::Block(b) => self.block(b),
            Stmt::Assign { target, value } => {
                self.effects.assigns = true;
                self.expr(value);
                self.target(target);
            }
            Stmt::Expr(e) | Stmt::Throw(e) | Stmt::Assert(e) | Stmt::LabeledIf { value: e, .. } => {
                self.expr(e)
            }
            Stmt::Return(e) | Stmt::Break(e, _) => {
                if let Some(e) = e {
                    self.expr(e);
                }
            }
            Stmt::For {
                name,
                iterable,
                body,
                ..
            } => {
                self.expr(iterable);
                let local = self.local.clone();
                let captures = self.captures.clone();
                self.captures.remove(name);
                self.local.insert(name.clone());
                self.block(body);
                self.local = local;
                self.captures = captures;
            }
            Stmt::While {
                condition, body, ..
            } => {
                self.expr(condition);
                self.block(body);
            }
            Stmt::Lock { name, body, .. } => {
                self.name(name);
                self.block(body);
            }
            Stmt::Continue(_) => {}
            Stmt::Located(..) => unreachable!(),
        }
    }
    fn expr(&mut self, e: &Expr) {
        match e.unlocated() {
            Expr::Name(n) => self.name(n),
            Expr::Lambda(f) => self.function(f),
            Expr::Embed { path, .. } => self.expr(path),
            Expr::Cast { ty, value, .. } => {
                self.ty(ty);
                self.expr(value);
            }
            Expr::Array(es) | Expr::Tuple(es) => {
                for e in es {
                    self.expr(e);
                }
            }
            Expr::Map(es) => {
                for (k, v) in es {
                    self.expr(k);
                    self.expr(v);
                }
            }
            Expr::StructInit { name, fields } => {
                self.name(name);
                for (_, e) in fields {
                    self.expr(e);
                }
            }
            Expr::Call {
                callee,
                args,
                generics,
            } => {
                self.expr(callee);
                for arg in args {
                    self.expr(arg);
                }
                for ty in generics {
                    self.ty(ty);
                }
                match callee.unlocated() {
                    Expr::Name(n) if n.starts_with('@') => {}
                    Expr::Name(n) if !self.local.contains(n) && self.globals.contains(n) => {
                        self.effects.calls.insert(n.clone());
                    }
                    Expr::Lambda(_) => {}

                    Expr::Name(n) if !self.local.contains(n) && !self.globals.contains(n) => {}
                    // Enum constructors are pure; member/index/dynamic callees
                    // otherwise require conservative effects before type checking.
                    Expr::Member { object, .. } if matches!(object.unlocated(), Expr::Name(n) if self.enums.contains(n) && !self.local.contains(n)) =>
                        {}
                    _ => self.effects.opaque = true,
                }
            }
            Expr::Index { object, index } => {
                self.expr(object);
                self.expr(index);
            }
            Expr::Member { object, .. } => self.expr(object),
            Expr::Unary { value, .. } | Expr::Async(value) | Expr::Try(value) => self.expr(value),
            Expr::Await(value) => {
                self.effects.awaits = true;
                self.expr(value);
            }
            Expr::Binary { left, right, .. } => {
                self.expr(left);
                self.expr(right);
            }
            Expr::If { subject, arms } => {
                if let Some(e) = subject {
                    self.expr(e);
                }
                for (ps, b) in arms {
                    let local = self.local.clone();
                    let captures = self.captures.clone();
                    for p in ps {
                        self.pattern(p);
                    }
                    self.block(b);
                    self.local = local;
                    self.captures = captures;
                }
            }
            Expr::Else { value, fallback } => {
                self.expr(value);
                self.block(fallback);
            }
            Expr::Catch { value, name, body } => {
                self.expr(value);
                let local = self.local.clone();
                let captures = self.captures.clone();
                self.captures.remove(name);
                self.local.insert(name.clone());
                self.block(body);
                self.local = local;
                self.captures = captures;
            }
            _ => {}
        }
    }
}
