//! Conservative, source-order dependency slicing before test-mode semantic checks.
use crate::ast::{BinaryOp, Block, Expr, Function, Item, Pattern, Stmt, Type};
use std::collections::{HashMap, HashSet};

type Names = HashSet<String>;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum EffectKind {
    Opaque,
    Await,
    Assignment,
    CapturedWrite,
}
#[derive(Clone, Default, PartialEq, Eq)]
struct Effects {
    uses: Names,
    runtime_uses: Names,
    writes: Names,
    calls: Names,
    kinds: HashSet<EffectKind>,
}
impl Effects {
    fn mark(&mut self, kind: EffectKind) {
        self.kinds.insert(kind);
    }
    fn contains(&self, kind: EffectKind) -> bool {
        self.kinds.contains(&kind)
    }
    fn merge(&mut self, other: &Self) {
        self.uses.extend(other.uses.iter().cloned());
        self.runtime_uses.extend(other.runtime_uses.iter().cloned());
        self.writes.extend(other.writes.iter().cloned());
        self.calls.extend(other.calls.iter().cloned());
        self.kinds.extend(other.kinds.iter().copied());
    }
}

fn exits_plain_block(statement: &Stmt) -> bool {
    match statement.unlocated() {
        Stmt::Return(_) | Stmt::Throw(_) | Stmt::Break(..) | Stmt::Continue(_) => true,
        Stmt::Block(block) => block.statements.iter().any(exits_plain_block),
        // Loops, labeled constructs, and value expressions can consume jumps.
        // Their continuation requires more context than this untyped scan has.
        _ => false,
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
    let mut effects = scan_effects(items, errors, &globals, &enums);
    summarize_effects(items, &declarations, &callable, &mutable, &mut effects);
    select_dependencies(items, &declarations, &effects, last_test)
}

fn scan_effects(
    items: &[Item],
    errors: &[Option<crate::diagnostic::Diagnostics>],
    globals: &Names,
    enums: &Names,
) -> Vec<Effects> {
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let mut scan = Scan {
                globals,
                enums,
                local: Names::new(),
                captures: Names::new(),
                effects: Effects::default(),
                runtime_effects: true,
            };
            scan.item(item);
            // Qualification can stop partway through a bad item, leaving names
            // unresolved. Never infer that its remaining assignments are harmless.
            if errors[index].is_some()
                && (scan.effects.contains(EffectKind::Assignment)
                    || matches!(item, Item::Function(_)))
            {
                scan.effects.mark(EffectKind::Opaque);
            }
            scan.effects
        })
        .collect()
}

fn summarize_effects(
    items: &[Item],
    declarations: &HashMap<String, Vec<usize>>,
    callable: &Names,
    mutable: &Names,
    effects: &mut [Effects],
) {
    // Function values, aliases, returned closures and recursive call chains all
    // contribute dependencies. Unknown callees are conservatively opaque.
    loop {
        let previous = effects.to_vec();
        for effect in effects.iter_mut() {
            for callee in effect.calls.clone() {
                if let Some(indices) = declarations.get(&callee) {
                    for &index in indices {
                        effect.merge(&previous[index]);
                        if previous[index].contains(EffectKind::CapturedWrite) {
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
                                    effect.calls.extend(
                                        writer.runtime_uses.intersection(callable).cloned(),
                                    );
                                }
                            }
                        }
                        if matches!(items[index], Item::Global(_)) {
                            effect.calls.extend(
                                previous[index].runtime_uses.intersection(callable).cloned(),
                            );
                        }
                        if matches!(items[index], Item::Extern { .. }) {
                            effect.mark(EffectKind::Opaque);
                        }
                    }
                } else {
                    effect.mark(EffectKind::Opaque);
                }
            }
            if effect.contains(EffectKind::Opaque) {
                effect.writes.extend(mutable.iter().cloned());
            }
        }
        if effects == previous {
            break;
        }
    }
}

fn select_dependencies(
    items: &[Item],
    declarations: &HashMap<String, Vec<usize>>,
    effects: &[Effects],
    last_test: usize,
) -> Vec<bool> {
    let mut selected: Vec<_> = items
        .iter()
        .map(|item| matches!(item, Item::Test { .. }))
        .collect();
    loop {
        let previous = selected.clone();
        let mut needed = Names::new();
        let mut runtime_needed = Names::new();
        let mut opaque_runtime = false;
        for (index, effect) in effects.iter().enumerate() {
            if selected[index] {
                needed.extend(effect.uses.iter().cloned());
                needed.extend(definitions(&items[index]));
                // Declaration bodies execute through call summaries, not merely
                // because they are retained for checking. Global initializers run.
                if matches!(
                    items[index],
                    Item::Global(_) | Item::Statement(_) | Item::Test { .. }
                ) {
                    runtime_needed.extend(effect.runtime_uses.iter().cloned());
                    opaque_runtime |= effect.contains(EffectKind::Opaque);
                }
            }
        }
        if opaque_runtime {
            // Unknown callbacks can read globals and escaped cells through
            // immutable callable bindings. Preserve the full dependency demand.
            runtime_needed.extend(needed.iter().cloned());
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
                selected[index] |= effects[index].contains(EffectKind::Opaque)
                    || !effects[index].writes.is_disjoint(&runtime_needed)
                    || (effects[index].contains(EffectKind::Await)
                        && synchronization_depends_on(
                            &effects[index].runtime_uses,
                            &runtime_needed,
                            declarations,
                            effects,
                        ));
            }
        }
        if selected == previous {
            return selected;
        }
    }
}

fn synchronization_depends_on(
    runtime_uses: &Names,
    needed: &Names,
    declarations: &HashMap<String, Vec<usize>>,
    effects: &[Effects],
) -> bool {
    let mut pending: Vec<_> = runtime_uses.iter().cloned().collect();
    let mut seen = Names::new();
    while let Some(name) = pending.pop() {
        if needed.contains(&name) {
            return true;
        }
        if seen.insert(name.clone())
            && let Some(indices) = declarations.get(&name)
        {
            for &index in indices {
                pending.extend(effects[index].runtime_uses.iter().cloned());
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
    runtime_effects: bool,
}
impl Scan<'_> {
    fn mark_runtime_effect(&mut self, kind: EffectKind) {
        if self.runtime_effects {
            self.effects.mark(kind);
        }
    }
    fn with_runtime_effects(&mut self, reachable: bool, scan: impl FnOnce(&mut Self)) {
        let runtime_effects = self.runtime_effects;
        self.runtime_effects &= reachable;
        scan(self);
        self.runtime_effects = runtime_effects;
    }
    fn name(&mut self, name: &str) {
        if !self.local.contains(name) && self.globals.contains(name) {
            self.effects.uses.insert(name.into());
            if self.runtime_effects {
                self.effects.runtime_uses.insert(name.into());
            }
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
                self.ty(t);
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
        let runtime_effects = self.runtime_effects;
        for s in &b.statements {
            self.stmt(s);
            if exits_plain_block(s) {
                // Keep dead syntax dependencies for checking, but exclude its
                // executable effects after jumps through plain lexical blocks.
                self.runtime_effects = false;
            }
        }
        self.local = local;
        self.captures = captures;
        self.runtime_effects = runtime_effects;
    }
    fn target(&mut self, e: &Expr) {
        match e.unlocated() {
            Expr::Name(n)
                if self.runtime_effects && !self.local.contains(n) && self.globals.contains(n) =>
            {
                self.effects.writes.insert(n.clone());
            }
            Expr::Name(n) if self.captures.contains(n) => {
                self.mark_runtime_effect(EffectKind::CapturedWrite);
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
                self.effects.mark(EffectKind::Assignment);
                self.expr(value);
                self.target(target);
            }
            Stmt::Expr(e) | Stmt::Throw(e) | Stmt::Assert(e) | Stmt::LabeledIf { value: e, .. } => {
                self.expr(e);
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
                self.with_runtime_effects(
                    crate::flow::constant_bool(condition) != Some(false),
                    |scan| scan.block(body),
                );
            }
            Stmt::Lock { name, body, .. } => {
                self.name(name);
                self.block(body);
            }
            Stmt::Continue(_) => {}
            Stmt::Located(..) => unreachable!(),
        }
    }
    fn conditional(&mut self, subject: Option<&Expr>, arms: &[(Vec<Pattern>, Block)]) {
        if let Some(subject) = subject {
            self.expr(subject);
        }
        let known = subject.map_or(Some(true), crate::flow::constant_bool);
        // Unknown patterns may match or fall through; only definite matches
        // make subsequent patterns and arms unreachable.
        let mut remaining = true;
        for (patterns, body) in arms {
            let local = self.local.clone();
            let captures = self.captures.clone();
            let mut can_match = false;
            for pattern in patterns {
                self.with_runtime_effects(remaining, |scan| scan.pattern(pattern));
                let matches = match pattern {
                    Pattern::Wildcard => Some(true),
                    Pattern::Literal(value) => known.and_then(|known| {
                        crate::flow::constant_bool(value).map(|value| value == known)
                    }),
                    _ => None,
                };
                if remaining {
                    can_match |= matches != Some(false);
                    remaining &= matches != Some(true);
                }
            }
            self.with_runtime_effects(can_match, |scan| scan.block(body));
            self.local = local;
            self.captures = captures;
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
                        if self.runtime_effects {
                            self.effects.calls.insert(n.clone());
                        }
                    }
                    Expr::Lambda(_) => {}

                    Expr::Name(n) if !self.local.contains(n) && !self.globals.contains(n) => {}
                    // Enum constructors are pure; member/index/dynamic callees
                    // otherwise require conservative effects before type checking.
                    Expr::Member { object, .. } if matches!(object.unlocated(), Expr::Name(n) if self.enums.contains(n) && !self.local.contains(n)) =>
                        {}
                    _ => self.mark_runtime_effect(EffectKind::Opaque),
                }
            }
            Expr::Index { object, index } => {
                self.expr(object);
                self.expr(index);
            }
            Expr::Member { object, .. } => self.expr(object),
            Expr::Unary { value, .. } | Expr::Async(value) | Expr::Try(value) => self.expr(value),
            Expr::Await(value) => {
                self.mark_runtime_effect(EffectKind::Await);
                self.expr(value);
            }
            Expr::Binary { left, op, right } => {
                self.expr(left);
                let reachable = !matches!(
                    (op, crate::flow::constant_bool(left)),
                    (BinaryOp::And, Some(false)) | (BinaryOp::Or, Some(true))
                );
                self.with_runtime_effects(reachable, |scan| scan.expr(right));
            }
            Expr::If { subject, arms } => self.conditional(subject.as_deref(), arms),
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

#[cfg(test)]
mod tests {
    use super::{EffectKind, Effects};

    #[test]
    fn merging_preserves_independent_effects_and_dependencies() {
        let mut combined = Effects::default();
        combined.mark(EffectKind::Assignment);
        combined.mark(EffectKind::Await);
        combined.uses.insert("input".into());
        let mut incoming = Effects::default();
        incoming.mark(EffectKind::Opaque);
        incoming.mark(EffectKind::CapturedWrite);
        incoming.writes.insert("state".into());
        incoming.calls.insert("helper".into());
        incoming.uses.extend(["dead".into(), "live".into()]);
        incoming.runtime_uses.insert("live".into());

        combined.merge(&incoming);
        for kind in [
            EffectKind::Assignment,
            EffectKind::Await,
            EffectKind::Opaque,
            EffectKind::CapturedWrite,
        ] {
            assert!(combined.contains(kind));
        }
        assert!(combined.uses.contains("input"));
        assert!(combined.uses.contains("dead"));
        assert!(combined.uses.contains("live"));
        assert!(combined.runtime_uses.contains("live"));
        assert!(!combined.runtime_uses.contains("dead"));
        assert!(!combined.runtime_uses.contains("input"));
        assert!(combined.writes.contains("state"));
        assert!(combined.calls.contains("helper"));
        assert!(!incoming.contains(EffectKind::Assignment));
        assert!(!incoming.contains(EffectKind::Await));

        let merged = combined.clone();
        combined.merge(&incoming);
        assert!(combined == merged);
    }
}
